//! Structured authentication context for audit logging.
//!
//! This module captures the verified token claims into a read-only context
//! that can be passed through the request pipeline for audit logging.
//! Crucially, it never stores the full JWT, signature, or authorization header.

use crate::domain::ids::PrincipalId;

/// Read-only authentication context extracted from a verified JWT.
///
/// The domain actor is always `subject` (`JWT.sub`). OBO delegation fields
/// (`delegating_principal_id`, `authorized_party`, `token_id`) are present
/// only for `workflow_obo` tokens and are used exclusively for audit logging.
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// The `sub` claim — the domain actor for all authorization decisions.
    pub subject: PrincipalId,
    /// `principal_type`: `human` or `agent`.
    pub principal_type: String,
    /// `token_use`: `access` or `workflow_obo`.
    pub token_use: String,
    /// `act.sub` for OBO tokens — the ADC service principal that initiated the delegation.
    pub delegating_principal_id: Option<PrincipalId>,
    /// `azp` for OBO tokens — the OAuth client ID.
    pub authorized_party: Option<String>,
    /// `jti` for OBO tokens — unique token identifier.
    pub token_id: Option<String>,
    /// The `aud` claim — intended audience.
    pub audience: String,
    /// Raw scope string (space-separated).
    pub scope: String,
}

impl AuthContext {
    /// Log a structured audit record for the authenticated request.
    ///
    /// No full JWT, signature, authorization header, or sensitive material
    /// is written to the log.
    pub fn log_audit(&self, endpoint: &str, request_id: &str) {
        tracing::info!(
            request_id = request_id,
            jti = self.token_id.as_deref().unwrap_or("-"),
            sub = %self.subject,
            principal_type = self.principal_type,
            token_use = self.token_use,
            act_sub = self.delegating_principal_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            azp = self.authorized_party.as_deref().unwrap_or("-"),
            audience = self.audience,
            scope = self.scope,
            endpoint = endpoint,
            "authenticated request"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn auth_context_creation() {
        let ctx = AuthContext {
            subject: PrincipalId::from_uuid(Uuid::new_v4()),
            principal_type: "agent".to_string(),
            token_use: "access".to_string(),
            delegating_principal_id: None,
            authorized_party: None,
            token_id: None,
            audience: "svc-workflow".to_string(),
            scope: "workflow.execute".to_string(),
        };
        assert_eq!(ctx.token_use, "access");
        assert!(ctx.delegating_principal_id.is_none());
    }

    #[test]
    fn obo_auth_context() {
        let principal = Uuid::new_v4();
        let ctx = AuthContext {
            subject: PrincipalId::from_uuid(principal),
            principal_type: "human".to_string(),
            token_use: "workflow_obo".to_string(),
            delegating_principal_id: Some(PrincipalId::from_uuid(Uuid::new_v4())),
            authorized_party: Some("test-client".to_string()),
            token_id: Some("unique-jti".to_string()),
            audience: "svc-workflow".to_string(),
            scope: "workflow.execute".to_string(),
        };
        assert_eq!(ctx.token_use, "workflow_obo");
        assert!(ctx.delegating_principal_id.is_some());
        assert_eq!(ctx.authorized_party.as_deref(), Some("test-client"));
    }
}
