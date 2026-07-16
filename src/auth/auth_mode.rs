//! Dual auth mode selection and configuration validation.

use std::net::IpAddr;

/// Supported authentication modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// HS256 shared-secret mode for local development and isolated smoke tests.
    /// Binds to loopback only. Requires `WORKFLOW_JWT_SECRET`.
    TestHs256,
    /// RS256 JWKS mode for staging, canary, shadow, and production environments.
    /// Requires `WORKFLOW_JWKS_URL`, `WORKFLOW_JWT_ISSUER`, `WORKFLOW_JWT_AUDIENCE`.
    Jwks,
}

impl AuthMode {
    /// Read `WORKFLOW_AUTH_MODE` from the environment and return the matching variant.
    ///
    /// Returns an error if the variable is missing or has an unrecognized value.
    pub fn from_env() -> Result<Self, String> {
        let raw = std::env::var("WORKFLOW_AUTH_MODE")
            .map_err(|_| "WORKFLOW_AUTH_MODE is required (test_hs256 or jwks)".to_string())?;
        match raw.as_str() {
            "test_hs256" => Ok(Self::TestHs256),
            "jwks" => Ok(Self::Jwks),
            other => Err(format!(
                "invalid WORKFLOW_AUTH_MODE '{other}': expected 'test_hs256' or 'jwks'"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-mode config
// ---------------------------------------------------------------------------

/// HS256-specific configuration.
#[derive(Debug, Clone)]
pub struct Hs256Config {
    pub secret: String,
    pub issuer: String,
    pub audience: String,
    pub clock_skew_seconds: u64,
}

impl Hs256Config {
    pub fn from_env() -> Result<Self, String> {
        let secret = std::env::var("WORKFLOW_JWT_SECRET")
            .map_err(|_| "WORKFLOW_JWT_SECRET is required in test_hs256 mode".to_string())?;
        if secret.is_empty() {
            return Err("WORKFLOW_JWT_SECRET must not be empty".to_string());
        }
        let clock_skew_seconds = std::env::var("WORKFLOW_JWT_CLOCK_SKEW")
            .unwrap_or_else(|_| "60".to_string())
            .parse::<u64>()
            .map_err(|_| "WORKFLOW_JWT_CLOCK_SKEW must be an unsigned integer".to_string())?;
        Ok(Self {
            secret,
            issuer: std::env::var("WORKFLOW_JWT_ISSUER")
                .unwrap_or_else(|_| "auth-service".to_string()),
            audience: std::env::var("WORKFLOW_JWT_AUDIENCE")
                .unwrap_or_else(|_| "svc-workflow".to_string()),
            clock_skew_seconds,
        })
    }
}

/// JWKS-mode configuration.
#[derive(Debug, Clone)]
pub struct JwksConfig {
    pub jwks_url: String,
    pub issuer: String,
    pub audience: String,
    pub cache_ttl_secs: u64,
    pub http_timeout_secs: u64,
    pub max_stale_secs: u64,
    pub clock_skew_seconds: u64,
}

impl JwksConfig {
    pub fn from_env() -> Result<Self, String> {
        let jwks_url = std::env::var("WORKFLOW_JWKS_URL")
            .map_err(|_| "WORKFLOW_JWKS_URL is required in jwks mode".to_string())?;
        if jwks_url.is_empty() {
            return Err("WORKFLOW_JWKS_URL must not be empty".to_string());
        }
        let issuer = std::env::var("WORKFLOW_JWT_ISSUER")
            .map_err(|_| "WORKFLOW_JWT_ISSUER is required in jwks mode".to_string())?;
        if issuer.is_empty() {
            return Err("WORKFLOW_JWT_ISSUER must not be empty".to_string());
        }
        let audience = std::env::var("WORKFLOW_JWT_AUDIENCE")
            .map_err(|_| "WORKFLOW_JWT_AUDIENCE is required in jwks mode".to_string())?;
        if audience.is_empty() {
            return Err("WORKFLOW_JWT_AUDIENCE must not be empty".to_string());
        }
        let cache_ttl_secs = std::env::var("WORKFLOW_JWKS_CACHE_TTL")
            .unwrap_or_else(|_| "300".to_string())
            .parse::<u64>()
            .map_err(|_| "WORKFLOW_JWKS_CACHE_TTL must be an unsigned integer".to_string())?;
        let http_timeout_secs = std::env::var("WORKFLOW_JWKS_HTTP_TIMEOUT")
            .unwrap_or_else(|_| "5".to_string())
            .parse::<u64>()
            .map_err(|_| "WORKFLOW_JWKS_HTTP_TIMEOUT must be an unsigned integer".to_string())?;
        let max_stale_secs = std::env::var("WORKFLOW_JWKS_MAX_STALE")
            .unwrap_or_else(|_| "600".to_string())
            .parse::<u64>()
            .map_err(|_| "WORKFLOW_JWKS_MAX_STALE must be an unsigned integer".to_string())?;
        let clock_skew_seconds = std::env::var("WORKFLOW_JWT_CLOCK_SKEW")
            .unwrap_or_else(|_| "60".to_string())
            .parse::<u64>()
            .map_err(|_| "WORKFLOW_JWT_CLOCK_SKEW must be an unsigned integer".to_string())?;
        Ok(Self {
            jwks_url,
            issuer,
            audience,
            cache_ttl_secs,
            http_timeout_secs,
            max_stale_secs,
            clock_skew_seconds,
        })
    }
}

// ---------------------------------------------------------------------------
// Gate validation
// ---------------------------------------------------------------------------

/// Validate that the environment matches the chosen auth mode's security gates.
///
/// `test_hs256`:
///   - Must not have `WORKFLOW_JWKS_URL` set.
///   - Must bind to loopback (127.0.0.1).
///
/// `jwks`:
///   - Must not have `WORKFLOW_JWT_SECRET` set (no fallback).
pub fn validate_mode_gates(mode: AuthMode, bind_addr: IpAddr) -> Result<(), String> {
    match mode {
        AuthMode::TestHs256 => {
            if std::env::var("WORKFLOW_JWKS_URL").is_ok() {
                return Err("WORKFLOW_JWKS_URL must not be set in test_hs256 mode".to_string());
            }
            if !bind_addr.is_loopback() {
                return Err(
                    "test_hs256 mode requires binding to a loopback address (127.0.0.1)"
                        .to_string(),
                );
            }
        }
        AuthMode::Jwks => {
            if std::env::var("WORKFLOW_JWT_SECRET").is_ok() {
                return Err(
                    "WORKFLOW_JWT_SECRET must not be set in jwks mode (use JWKS keys instead)"
                        .to_string(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hs256_from_env() {
        assert!(AuthMode::from_env().is_err());
        unsafe {
            std::env::set_var("WORKFLOW_AUTH_MODE", "test_hs256");
        }
        assert_eq!(AuthMode::from_env().unwrap(), AuthMode::TestHs256);
        unsafe {
            std::env::set_var("WORKFLOW_AUTH_MODE", "jwks");
        }
        assert_eq!(AuthMode::from_env().unwrap(), AuthMode::Jwks);
        unsafe {
            std::env::set_var("WORKFLOW_AUTH_MODE", "invalid");
        }
        assert!(AuthMode::from_env().is_err());
    }

    #[test]
    fn test_hs256_gate_rejects_jwks_url() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        unsafe {
            std::env::set_var("WORKFLOW_JWKS_URL", "http://example.com");
        }
        let result = validate_mode_gates(AuthMode::TestHs256, loopback);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("WORKFLOW_JWKS_URL must not be set"));
    }

    #[test]
    fn test_hs256_gate_rejects_non_loopback() {
        let non_loopback: IpAddr = "0.0.0.0".parse().unwrap();
        unsafe {
            std::env::remove_var("WORKFLOW_JWKS_URL");
        }
        let result = validate_mode_gates(AuthMode::TestHs256, non_loopback);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("loopback"));
    }

    #[test]
    fn jwks_gate_rejects_hs256_secret() {
        let any_addr: IpAddr = "0.0.0.0".parse().unwrap();
        unsafe {
            std::env::set_var("WORKFLOW_JWT_SECRET", "some-secret");
            std::env::remove_var("WORKFLOW_JWKS_URL");
        }
        let result = validate_mode_gates(AuthMode::Jwks, any_addr);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("WORKFLOW_JWT_SECRET must not be set"));
    }

    #[test]
    fn test_hs256_gate_accepts_valid() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        unsafe {
            std::env::remove_var("WORKFLOW_JWKS_URL");
            std::env::remove_var("WORKFLOW_JWT_SECRET");
        }
        assert!(validate_mode_gates(AuthMode::TestHs256, loopback).is_ok());
    }

    #[test]
    fn jwks_gate_accepts_valid() {
        let any_addr: IpAddr = "0.0.0.0".parse().unwrap();
        unsafe {
            std::env::remove_var("WORKFLOW_JWT_SECRET");
        }
        assert!(validate_mode_gates(AuthMode::Jwks, any_addr).is_ok());
    }
}
