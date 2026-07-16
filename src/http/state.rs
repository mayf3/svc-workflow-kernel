//! HTTP service configuration and shared state.
//!
//! Supports dual auth mode: test_hs256 and jwks.

use std::net::{IpAddr, SocketAddr};

use sqlx::PgPool;

use crate::application::workflow_instance::query_service::WorkflowQueryService;
use crate::auth::{
    AuthMode, AuthenticatedPrincipal, Hs256Config, Hs256Verifier, JwksConfig, JwksVerifier,
};
use crate::http::error::ApiError;

/// Enum over the two supported auth verifiers.
///
/// Used in `AppState` to dispatch verification to the active mode.
#[derive(Clone)]
pub enum AuthVerifier {
    TestHs256(Hs256Verifier),
    Jwks(JwksVerifier),
}

impl AuthVerifier {
    /// Verify a JWT using the active verifier.
    pub async fn verify(&self, token: &str) -> Result<AuthenticatedPrincipal, ApiError> {
        match self {
            AuthVerifier::TestHs256(v) => v.verify(token),
            AuthVerifier::Jwks(v) => v.verify(token).await,
        }
    }

    /// Check whether the verifier is ready to handle requests.
    pub async fn is_ready(&self) -> bool {
        match self {
            AuthVerifier::TestHs256(_) => true,
            AuthVerifier::Jwks(v) => v.is_ready().await,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub bind_addr: SocketAddr,
    pub request_body_max_bytes: usize,
    pub request_timeout_seconds: u64,
    pub auth_mode: AuthMode,
    pub hs256_config: Option<Hs256Config>,
    pub jwks_config: Option<JwksConfig>,
}

impl HttpConfig {
    pub fn from_env() -> Result<Self, String> {
        let auth_mode = AuthMode::from_env()?;
        let ip = std::env::var("WORKFLOW_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1".to_string())
            .parse::<IpAddr>()
            .map_err(|_| "WORKFLOW_BIND_ADDR must be an IP address".to_string())?;

        // Validate mode-specific gates (bind addr etc.).
        crate::auth::validate_mode_gates(auth_mode, ip)?;

        let port = parse_env("WORKFLOW_PORT", 8989u16)?;
        let request_body_max_bytes = parse_env("WORKFLOW_REQUEST_BODY_MAX_BYTES", 2_097_152usize)?;
        if request_body_max_bytes == 0 {
            return Err("WORKFLOW_REQUEST_BODY_MAX_BYTES must be positive".to_string());
        }
        let request_timeout_seconds = parse_env("WORKFLOW_REQUEST_TIMEOUT_SECS", 30u64)?;
        if request_timeout_seconds == 0 {
            return Err("WORKFLOW_REQUEST_TIMEOUT_SECS must be positive".to_string());
        }

        let (hs256_config, jwks_config) = match auth_mode {
            AuthMode::TestHs256 => (Some(Hs256Config::from_env()?), None),
            AuthMode::Jwks => (None, Some(JwksConfig::from_env()?)),
        };

        Ok(Self {
            bind_addr: SocketAddr::new(ip, port),
            request_body_max_bytes,
            request_timeout_seconds,
            auth_mode,
            hs256_config,
            jwks_config,
        })
    }
}

fn parse_env<T>(name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr + ToString,
{
    std::env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<T>()
        .map_err(|_| format!("{name} has an invalid value"))
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) query_service: WorkflowQueryService,
    pub auth_verifier: AuthVerifier,
}

impl AppState {
    pub fn new(pool: PgPool, config: &HttpConfig) -> Self {
        let auth_verifier = match config.auth_mode {
            AuthMode::TestHs256 => {
                let hs256 = config.hs256_config.as_ref().expect("HS256 config loaded");
                AuthVerifier::TestHs256(Hs256Verifier::new(hs256))
            }
            AuthMode::Jwks => {
                let jwks = config.jwks_config.as_ref().expect("JWKS config loaded");
                AuthVerifier::Jwks(JwksVerifier::new(jwks))
            }
        };
        Self {
            query_service: WorkflowQueryService::new(pool.clone()),
            auth_verifier,
            pool,
        }
    }
}
