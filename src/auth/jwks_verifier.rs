//! RS256 JWKS verifier with caching, controlled refresh, and fail-closed semantics.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::domain::ids::PrincipalId;
use crate::http::error::ApiError;

use super::auth_context::AuthContext;
use super::auth_mode::JwksConfig;
use super::claims::{self, WorkflowClaims};
use super::AuthenticatedPrincipal;

/// Maximum response body size for JWKS fetch (1 MB).
const MAX_JWKS_BODY_BYTES: usize = 1_048_576;

/// A raw JWK entry from the JWKS endpoint.
#[derive(Debug, Deserialize)]
struct RawJwk {
    kid: Option<String>,
    #[serde(rename = "kty")]
    key_type: Option<String>,
    #[serde(rename = "use")]
    key_use: Option<String>,
    alg: Option<String>,
    /// RSA modulus (base64url-encoded).
    n: Option<String>,
    /// RSA exponent (base64url-encoded).
    e: Option<String>,
}

/// JWKS response body.
#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<RawJwk>,
}

/// A cached JWK with its `DecodingKey`.
struct JwkKey {
    kid: String,
    decoding_key: DecodingKey,
}

/// State of the JWKS cache.
struct JwksCacheState {
    keys: Vec<JwkKey>,
    fetched_at: Instant,
}

/// RS256 JWKS verifier with caching and controlled refresh.
pub struct JwksVerifier {
    cache: Arc<tokio::sync::RwLock<Option<JwksCacheState>>>,
    http_client: reqwest::Client,
    jwks_url: String,
    cache_ttl: Duration,
    max_stale: Duration,
    /// Concurrency suppression for JWKS refresh.
    refresh_lock: Arc<Mutex<()>>,
    issuer: String,
    audience: String,
    clock_skew_seconds: u64,
}

impl JwksVerifier {
    /// Create a new `JwksVerifier` from configuration.
    ///
    /// An initial JWKS fetch is attempted eagerly in the background.
    pub fn new(config: &JwksConfig) -> Self {
        let cache: Arc<tokio::sync::RwLock<Option<JwksCacheState>>> =
            Arc::new(tokio::sync::RwLock::new(None));
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.http_timeout_secs))
            .build()
            .expect("reqwest::Client builder with valid config");
        let verifier = Self {
            cache,
            http_client,
            jwks_url: config.jwks_url.clone(),
            cache_ttl: Duration::from_secs(config.cache_ttl_secs),
            max_stale: Duration::from_secs(config.max_stale_secs),
            refresh_lock: Arc::new(Mutex::new(())),
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            clock_skew_seconds: config.clock_skew_seconds,
        };
        // Eagerly attempt initial fetch.
        let eager = verifier.clone();
        tokio::spawn(async move {
            let _ = eager.fetch_jwks().await;
        });
        verifier
    }

    /// Check whether the verifier has at least one cached key within the max-stale window.
    pub async fn is_ready(&self) -> bool {
        let guard = self.cache.read().await;
        match guard.as_ref() {
            Some(state) => state.fetched_at.elapsed() <= self.max_stale,
            None => false,
        }
    }

    /// Verify a JWT in JWKS mode.
    ///
    /// Algorithm must be RS256. Token must include a `kid` that maps to
    /// a cached JWKS key. Unknown kids trigger a controlled refresh.
    pub async fn verify(&self, token: &str) -> Result<AuthenticatedPrincipal, ApiError> {
        // 1. Decode and validate the JWT header.
        let header = decode_header(token)
            .map_err(|_| ApiError::unauthorized("invalid_token", "malformed JWT header"))?;

        // Algorithm must be RS256.
        if header.alg != Algorithm::RS256 {
            return Err(ApiError::unauthorized(
                "invalid_token",
                "only RS256 algorithm is accepted",
            ));
        }

        // Kid is required.
        let kid = header.kid.as_deref().ok_or_else(|| {
            ApiError::unauthorized("invalid_token", "JWT must include a kid header")
        })?;

        // 2. Look up the key, optionally refreshing cache.
        let key = self.lookup_key(kid).await?;

        // 3. Build validation.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.algorithms = vec![Algorithm::RS256];
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        for claim in ["exp", "iat", "iss", "aud", "sub"] {
            validation.required_spec_claims.insert(claim.to_string());
        }
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = self.clock_skew_seconds;

        // 4. Decode and verify.
        let data =
            decode::<WorkflowClaims>(token, &key, &validation).map_err(|error| {
                match error.kind() {
                    ErrorKind::ExpiredSignature => {
                        ApiError::unauthorized("token_expired", "access token has expired")
                    }
                    ErrorKind::MissingRequiredClaim(claim) => ApiError::unauthorized_with_details(
                        "invalid_token",
                        "access token is missing a required claim",
                        serde_json::json!({ "claim": claim }),
                    ),
                    ErrorKind::InvalidAlgorithm => {
                        ApiError::unauthorized("invalid_token", "invalid JWT algorithm")
                    }
                    ErrorKind::InvalidIssuer | ErrorKind::InvalidAudience => {
                        ApiError::unauthorized("invalid_token", "token issuer or audience mismatch")
                    }
                    _ => ApiError::unauthorized("invalid_token", "invalid access token"),
                }
            })?;

        let claims = data.claims;

        // 5. Validate required custom claims (legacy backward compat).
        super::verifier::require_legacy_claims(&claims)?;

        // 6. Parse and validate subject.
        let parsed = claims::parse_subject(&claims.sub)
            .map_err(|_| ApiError::unauthorized("invalid_token", "invalid subject claim"))?;

        // 7. Validate principal_type (human or agent).
        claims::validate_principal_type(&claims.principal_type)
            .map_err(|_| ApiError::unauthorized("invalid_token", "invalid principal type"))?;

        // 8. Validate token_use.
        claims::validate_token_use(&claims.token_use)
            .map_err(|_| ApiError::unauthorized("invalid_token", "invalid token use"))?;

        // 9. Validate OBO claims if applicable.
        if claims.token_use.as_deref() == Some("workflow_obo") || claims.act.is_some() {
            claims::validate_obo(&claims).map_err(|_| {
                ApiError::unauthorized("invalid_token", "OBO claims validation failed")
            })?;
        }

        // 10. Parse scopes (clone before move).
        let scope_string = claims.scope.clone().unwrap_or_default();
        let scopes = scope_string
            .split_whitespace()
            .map(str::to_owned)
            .collect::<HashSet<_>>();

        // 11. Build auth context.
        let delegating = match &claims.act {
            Some(act) => act
                .sub
                .as_ref()
                .and_then(|s| Uuid::parse_str(s).ok())
                .map(PrincipalId::from_uuid),
            None => None,
        };
        let auth_context = AuthContext {
            subject: parsed.principal_id,
            principal_type: claims
                .principal_type
                .clone()
                .unwrap_or_else(|| "agent".to_string()),
            token_use: claims
                .token_use
                .clone()
                .unwrap_or_else(|| "access".to_string()),
            delegating_principal_id: delegating,
            authorized_party: claims.azp.clone(),
            token_id: claims.jti.clone(),
            audience: claims.aud.clone().unwrap_or_default(),
            scope: scope_string,
        };

        Ok(AuthenticatedPrincipal::new_with_context(
            parsed.principal_id,
            scopes,
            auth_context,
        ))
    }

    /// Look up a kid in cache, triggering a refresh if needed.
    async fn lookup_key(&self, kid: &str) -> Result<DecodingKey, ApiError> {
        // Fast path: check cache.
        {
            let guard = self.cache.read().await;
            if let Some(state) = guard.as_ref() {
                let elapsed = state.fetched_at.elapsed();
                // If within TTL, use cache directly.
                if elapsed <= self.cache_ttl {
                    if let Some(key) = find_key(&state.keys, kid) {
                        return Ok(key);
                    }
                }
                // If within max_stale but beyond TTL, use cache for known kid,
                // but trigger refresh for unknown kid.
                if elapsed <= self.max_stale {
                    if let Some(key) = find_key(&state.keys, kid) {
                        return Ok(key);
                    }
                }
            }
        }

        // Cache miss or stale. Try refreshing.
        let result = self.refresh_and_find(kid).await;
        match result {
            Ok(key) => Ok(key),
            Err(_) => {
                // Last resort: if cache is within max_stale, use it even for unknown kid
                // (it won't match but we tried). This is the "fail closed" path.
                let guard = self.cache.read().await;
                match guard.as_ref() {
                    Some(state) if state.fetched_at.elapsed() <= self.max_stale => Err(
                        ApiError::unauthorized("invalid_token", "unknown key ID after refresh"),
                    ),
                    _ => Err(ApiError::service_unavailable(
                        "auth_verifier_unavailable",
                        "authentication verifier is currently unavailable",
                    )),
                }
            }
        }
    }

    /// Refresh JWKS cache and look up the kid. Uses a mutex to prevent concurrent fetches.
    async fn refresh_and_find(&self, kid: &str) -> Result<DecodingKey, ()> {
        let _lock = self.refresh_lock.lock().await;

        // Double-check: maybe another thread already refreshed.
        {
            let guard = self.cache.read().await;
            if let Some(state) = guard.as_ref() {
                if let Some(key) = find_key(&state.keys, kid) {
                    return Ok(key);
                }
            }
        }

        // Fetch fresh JWKS.
        self.fetch_jwks().await.map_err(|_| ())?;

        // Look up the kid in the new set.
        let guard = self.cache.read().await;
        match guard.as_ref() {
            Some(state) => match find_key(&state.keys, kid) {
                Some(key) => Ok(key),
                None => Err(()),
            },
            None => Err(()),
        }
    }

    /// Fetch JWKS from the configured URL and update the cache.
    async fn fetch_jwks(&self) -> Result<(), ()> {
        let response = self
            .http_client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(url = %self.jwks_url, error = %error, "JWKS fetch failed");
            })?;

        let status = response.status();
        if !status.is_success() {
            tracing::warn!(url = %self.jwks_url, http_status = %status, "JWKS endpoint returned non-success");
            return Err(());
        }

        let body = response.bytes().await.map_err(|error| {
            tracing::warn!(url = %self.jwks_url, error = %error, "JWKS response body read failed");
        })?;

        if body.len() > MAX_JWKS_BODY_BYTES {
            tracing::warn!(url = %self.jwks_url, size = body.len(), "JWKS response exceeds size limit");
            return Err(());
        }

        let jwks: JwksResponse = serde_json::from_slice(&body).map_err(|error| {
            tracing::warn!(url = %self.jwks_url, error = %error, "JWKS JSON parse failed");
        })?;

        let mut keys: Vec<JwkKey> = Vec::new();
        for raw in jwks.keys {
            // Only accept RSA keys with sig use.
            if raw.key_type.as_deref() != Some("RSA") {
                continue;
            }
            if !matches!(raw.key_use.as_deref(), None | Some("sig")) {
                continue;
            }
            // Require algorithm RS256 or absent (will be checked during verify).
            if !matches!(raw.alg.as_deref(), None | Some("RS256")) {
                continue;
            }
            let kid = match raw.kid {
                Some(ref k) if !k.is_empty() => k.clone(),
                _ => continue,
            };
            let n = match raw.n {
                Some(ref n) if !n.is_empty() => n.clone(),
                _ => continue,
            };
            let e = match raw.e {
                Some(ref e) if !e.is_empty() => e.clone(),
                _ => continue,
            };
            // Build DecodingKey from RSA components (base64url-encoded n and e).
            let decoding_key = match DecodingKey::from_rsa_components(&n, &e) {
                Ok(key) => key,
                Err(error) => {
                    tracing::warn!(kid = %kid, error = %error, "failed to build RSA decoding key from JWK");
                    continue;
                }
            };
            keys.push(JwkKey { kid, decoding_key });
        }

        if keys.is_empty() {
            tracing::warn!(url = %self.jwks_url, "JWKS response contained no usable RSA keys");
            return Err(());
        }

        let mut guard = self.cache.write().await;
        *guard = Some(JwksCacheState {
            keys,
            fetched_at: Instant::now(),
        });
        tracing::info!(url = %self.jwks_url, "JWKS cache updated successfully");
        Ok(())
    }
}

/// Helper: clone for background eager fetch.
impl Clone for JwksVerifier {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            http_client: self.http_client.clone(),
            jwks_url: self.jwks_url.clone(),
            cache_ttl: self.cache_ttl,
            max_stale: self.max_stale,
            refresh_lock: self.refresh_lock.clone(),
            issuer: self.issuer.clone(),
            audience: self.audience.clone(),
            clock_skew_seconds: self.clock_skew_seconds,
        }
    }
}

/// Find a key by kid in the cached key list.
fn find_key(keys: &[JwkKey], kid: &str) -> Option<DecodingKey> {
    keys.iter()
        .find(|k| k.kid == kid)
        .map(|k| k.decoding_key.clone())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_key_matches_kid() {
        let key1 = JwkKey {
            kid: "key-1".to_string(),
            decoding_key: DecodingKey::from_rsa_components("dGVzdA", "AQAB").unwrap(),
        };
        let key2 = JwkKey {
            kid: "key-2".to_string(),
            decoding_key: DecodingKey::from_rsa_components("dGVzdA", "AQAB").unwrap(),
        };
        let keys = vec![key1, key2];
        assert!(find_key(&keys, "key-1").is_some());
        assert!(find_key(&keys, "key-2").is_some());
        assert!(find_key(&keys, "key-3").is_none());
    }
}
