//! Authenticated workflow principal with auth context.

use std::collections::HashSet;

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::domain::ids::PrincipalId;
use crate::http::error::ApiError;
use crate::http::AppState;

use super::auth_context::AuthContext;

/// Authenticated principal extracted from a verified JWT.
///
/// Carries both the domain identity (`principal_id`) and the structured
/// authentication context for audit logging.
#[derive(Debug, Clone)]
pub struct AuthenticatedPrincipal {
    pub principal_id: PrincipalId,
    scopes: HashSet<String>,
    pub auth_context: AuthContext,
}

impl AuthenticatedPrincipal {
    /// Internal constructor used by verifiers.
    pub(crate) fn new_with_context(
        principal_id: PrincipalId,
        scopes: HashSet<String>,
        auth_context: AuthContext,
    ) -> Self {
        Self {
            principal_id,
            scopes,
            auth_context,
        }
    }

    pub fn has_scope(&self, required: &str) -> bool {
        self.scopes.contains(required)
    }
}

impl FromRequestParts<AppState> for AuthenticatedPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let value = parts
            .headers
            .get(AUTHORIZATION)
            .ok_or_else(|| ApiError::unauthorized("unauthenticated", "bearer token is required"))?;
        let value = value.to_str().map_err(|_| {
            ApiError::unauthorized("unauthenticated", "authorization header is invalid")
        })?;
        let token = value.strip_prefix("Bearer ").ok_or_else(|| {
            ApiError::unauthorized("unauthenticated", "authorization scheme must be Bearer")
        })?;
        if token.is_empty() {
            return Err(ApiError::unauthorized(
                "unauthenticated",
                "bearer token is required",
            ));
        }

        let request_id = parts
            .headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();

        let endpoint = parts
            .uri
            .path_and_query()
            .map(|pq| {
                format!(
                    "{} {} {}",
                    parts.method.as_str(),
                    pq.path(),
                    pq.query().unwrap_or("")
                )
            })
            .unwrap_or_else(|| parts.method.to_string());

        let principal = state.auth_verifier.verify(token).await?;

        // Structured audit logging.
        principal.auth_context.log_audit(&endpoint, &request_id);

        Ok(principal)
    }
}
