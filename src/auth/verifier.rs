//! HS256 shared-secret token verification (test_hs256 mode).
//!
//! Also exposes `require_legacy_claims` shared by both verifiers.

use std::collections::HashSet;

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::ids::PrincipalId;
use crate::http::error::ApiError;

use super::auth_context::AuthContext;
use super::auth_mode::Hs256Config;
use super::claims::{self, WorkflowClaims};
use super::AuthenticatedPrincipal;

/// Legacy claims struct for backward-compatible HS256 verification.
///
/// Kept as the internal decode target because it's simpler and matches
/// the existing token shape exactly. New fields like `token_use`, `act`,
/// and `azp` are silently ignored (no error), which is the desired
/// backward-compatible behavior.
#[derive(Debug, Deserialize)]
struct LegacyClaims {
    sub: Option<String>,
    iss: Option<String>,
    aud: Option<String>,
    #[allow(dead_code)]
    exp: Option<usize>,
    iat: Option<usize>,
    #[allow(dead_code)]
    nbf: Option<usize>,
    principal_type: Option<String>,
    #[serde(rename = "type")]
    token_type: Option<String>,
    version: Option<String>,
    scope: Option<String>,
}

/// Validate the legacy required claims (`type=access`, `version=v1`) that
/// both HS256 and JWKS verifiers must enforce for backward compatibility.
///
/// This is called after the modern `WorkflowClaims` decode, so we only
/// check the fields that `WorkflowClaims` doesn't natively enforce.
pub fn require_legacy_claims(claims: &WorkflowClaims) -> Result<(), ApiError> {
    if claims.token_type.as_deref() != Some("access") {
        return Err(ApiError::unauthorized(
            "invalid_token",
            "token type must be access",
        ));
    }
    if claims.version.as_deref() != Some("v1") {
        return Err(ApiError::unauthorized(
            "invalid_token",
            "token version must be v1",
        ));
    }
    Ok(())
}

/// HS256 shared-secret verifier (test_hs256 mode).
#[derive(Clone)]
pub struct Hs256Verifier {
    key: DecodingKey,
    validation: Validation,
    issuer: String,
    audience: String,
}

impl Hs256Verifier {
    pub fn new(config: &Hs256Config) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.algorithms = vec![Algorithm::HS256];
        validation.set_issuer(&[&config.issuer]);
        validation.set_audience(&[&config.audience]);
        for claim in ["exp", "iat", "iss", "aud", "sub"] {
            validation.required_spec_claims.insert(claim.to_string());
        }
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = config.clock_skew_seconds;
        Self {
            key: DecodingKey::from_secret(config.secret.as_bytes()),
            validation,
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
        }
    }

    pub fn verify(&self, token: &str) -> Result<AuthenticatedPrincipal, ApiError> {
        let data =
            decode::<LegacyClaims>(token, &self.key, &self.validation).map_err(
                |error| match error.kind() {
                    ErrorKind::ExpiredSignature => {
                        ApiError::unauthorized("token_expired", "access token has expired")
                    }
                    ErrorKind::MissingRequiredClaim(claim) => ApiError::unauthorized_with_details(
                        "missing_claim",
                        "access token is missing a required claim",
                        serde_json::json!({ "claim": claim }),
                    ),
                    _ => ApiError::unauthorized("unauthenticated", "invalid access token"),
                },
            )?;
        let claims = data.claims;
        require_claim(&claims.sub, "sub")?;
        require_claim(&claims.iss, "iss")?;
        require_claim(&claims.aud, "aud")?;
        if claims.iat.is_none() {
            return Err(missing_claim("iat"));
        }
        require_claim(&claims.principal_type, "principal_type")?;
        require_claim(&claims.token_type, "type")?;
        require_claim(&claims.version, "version")?;
        if claims.principal_type.as_deref() != Some("agent") {
            return Err(ApiError::unauthorized(
                "unauthenticated",
                "principal_type must be agent",
            ));
        }
        if claims.token_type.as_deref() != Some("access") {
            return Err(ApiError::unauthorized(
                "unauthenticated",
                "token type must be access",
            ));
        }
        if claims.version.as_deref() != Some("v1") {
            return Err(ApiError::unauthorized(
                "unauthenticated",
                "token version must be v1",
            ));
        }
        let subject = claims.sub.expect("sub checked above");
        let subject = Uuid::parse_str(&subject)
            .map_err(|_| ApiError::unauthorized("unauthenticated", "sub must be a UUID"))?;
        let scope_string = claims.scope.clone().unwrap_or_default();
        let scopes = scope_string
            .split_whitespace()
            .map(str::to_owned)
            .collect::<HashSet<_>>();

        let auth_context = AuthContext {
            subject: PrincipalId::from_uuid(subject),
            principal_type: "agent".to_string(),
            token_use: "access".to_string(),
            delegating_principal_id: None,
            authorized_party: None,
            token_id: None,
            audience: self.audience.clone(),
            scope: scope_string,
        };

        Ok(AuthenticatedPrincipal::new_with_context(
            PrincipalId::from_uuid(subject),
            scopes,
            auth_context,
        ))
    }
}

fn require_claim(value: &Option<String>, name: &'static str) -> Result<(), ApiError> {
    if value.as_deref().is_none_or(str::is_empty) {
        Err(missing_claim(name))
    } else {
        Ok(())
    }
}

fn missing_claim(name: &'static str) -> ApiError {
    ApiError::unauthorized_with_details(
        "missing_claim",
        "access token is missing a required claim",
        serde_json::json!({ "claim": name }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;

    #[derive(Serialize)]
    struct TestClaims<'a> {
        sub: String,
        iss: &'a str,
        aud: &'a str,
        exp: usize,
        iat: usize,
        principal_type: &'a str,
        #[serde(rename = "type")]
        token_type: &'a str,
        version: &'a str,
        scope: &'a str,
    }

    fn config() -> Hs256Config {
        Hs256Config {
            secret: "test-secret-at-least-32-bytes-long".to_string(),
            issuer: "auth-service".to_string(),
            audience: "svc-workflow".to_string(),
            clock_skew_seconds: 0,
        }
    }

    fn token(algorithm: Algorithm, audience: &str, exp_offset: i64) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = TestClaims {
            sub: Uuid::new_v4().to_string(),
            iss: "auth-service",
            aud: audience,
            exp: (now + exp_offset) as usize,
            iat: now as usize,
            principal_type: "agent",
            token_type: "access",
            version: "v1",
            scope: "workflow.read workflow.execute",
        };
        encode(
            &Header::new(algorithm),
            &claims,
            &EncodingKey::from_secret(config().secret.as_bytes()),
        )
        .unwrap()
    }

    fn token_without(claim: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        let mut claims = serde_json::to_value(TestClaims {
            sub: Uuid::new_v4().to_string(),
            iss: "auth-service",
            aud: "svc-workflow",
            exp: (now + 300) as usize,
            iat: now as usize,
            principal_type: "agent",
            token_type: "access",
            version: "v1",
            scope: "workflow.read",
        })
        .unwrap();
        claims.as_object_mut().unwrap().remove(claim);
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(config().secret.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn verifies_hs256_machine_token() {
        let principal = Hs256Verifier::new(&config())
            .verify(&token(Algorithm::HS256, "svc-workflow", 300))
            .unwrap();
        assert!(principal.has_scope("workflow.execute"));
    }

    #[test]
    fn rejects_wrong_algorithm_audience_and_expiry() {
        let verifier = Hs256Verifier::new(&config());
        assert!(verifier
            .verify(&token(Algorithm::HS512, "svc-workflow", 300))
            .is_err());
        assert!(verifier
            .verify(&token(Algorithm::HS256, "other", 300))
            .is_err());
        let expired = verifier
            .verify(&token(Algorithm::HS256, "svc-workflow", -300))
            .unwrap_err();
        assert_eq!(expired.code(), "token_expired");
    }

    #[test]
    fn missing_registered_and_custom_claims_are_stable() {
        let verifier = Hs256Verifier::new(&config());
        for claim in [
            "iss",
            "aud",
            "sub",
            "iat",
            "exp",
            "principal_type",
            "type",
            "version",
        ] {
            assert_eq!(
                verifier.verify(&token_without(claim)).unwrap_err().code(),
                "missing_claim"
            );
        }
    }
}
