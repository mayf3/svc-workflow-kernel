//! JWT claims types for direct and OBO token verification.

use serde::Deserialize;
use uuid::Uuid;

use crate::domain::ids::PrincipalId;

/// Act claim for OBO delegation (RFC 8693 style).
#[derive(Debug, Deserialize)]
pub struct ActClaim {
    pub sub: Option<String>,
}

/// Full set of claims supported by svc-workflow authentication.
///
/// Supports both direct access tokens (`token_use: access`) and
/// on-behalf-of tokens (`token_use: workflow_obo`).
#[derive(Debug, Deserialize)]
pub struct WorkflowClaims {
    pub sub: Option<String>,
    pub iss: Option<String>,
    pub aud: Option<String>,
    pub exp: Option<usize>,
    pub iat: Option<usize>,
    pub nbf: Option<usize>,
    pub principal_type: Option<String>,
    /// Legacy `type` claim — required for backward compatibility.
    #[serde(rename = "type")]
    pub token_type: Option<String>,
    pub version: Option<String>,
    pub scope: Option<String>,
    /// Token use discriminator: `access` (direct) or `workflow_obo` (delegated).
    pub token_use: Option<String>,
    /// OBO: subject of the delegated authority (ADC service principal).
    pub act: Option<ActClaim>,
    /// OBO: authorized party (OAuth client ID).
    pub azp: Option<String>,
    /// OBO: unique token identifier for replay prevention.
    pub jti: Option<String>,
}

/// Result of parsing subject into a validated PrincipalId.
pub struct ParsedSubject {
    pub principal_id: PrincipalId,
    pub subject_uuid: Uuid,
}

/// Parse and validate the `sub` claim as a UUID.
pub fn parse_subject(sub: &Option<String>) -> Result<ParsedSubject, String> {
    let sub = sub.as_deref().ok_or("missing sub")?;
    let uuid = Uuid::parse_str(sub).map_err(|_| "sub must be a valid UUID".to_string())?;
    Ok(ParsedSubject {
        principal_id: PrincipalId::from_uuid(uuid),
        subject_uuid: uuid,
    })
}

/// Validate that a required non-empty string claim is present.
pub fn require_claim(value: &Option<String>, name: &str) -> Result<(), String> {
    match value.as_deref() {
        Some(v) if !v.is_empty() => Ok(()),
        _ => Err(format!("missing required claim: {name}")),
    }
}

/// Validate `principal_type` is `human` or `agent`.
pub fn validate_principal_type(principal_type: &Option<String>) -> Result<(), String> {
    match principal_type.as_deref() {
        Some("human") | Some("agent") => Ok(()),
        Some(other) => Err(format!(
            "invalid principal_type '{other}': expected 'human' or 'agent'"
        )),
        None => Err("missing principal_type".to_string()),
    }
}

/// Validate `token_use` is a known value.
pub fn validate_token_use(token_use: &Option<String>) -> Result<(), String> {
    match token_use.as_deref() {
        Some("access") | Some("workflow_obo") => Ok(()),
        Some(other) => Err(format!(
            "invalid token_use '{other}': expected 'access' or 'workflow_obo'"
        )),
        None => {
            // Default to "access" if missing (backward compat with existing tokens)
            Ok(())
        }
    }
}

/// Validate OBO-specific claims.
pub fn validate_obo(claims: &WorkflowClaims) -> Result<(), String> {
    let is_obo = matches!(claims.token_use.as_deref(), Some("workflow_obo"))
        || (claims.token_use.is_none() && claims.act.is_some());

    if !is_obo {
        return Ok(()); // Direct token: no OBO validation needed
    }

    // OBO token: act.sub must be present and a valid UUID
    let act = claims.act.as_ref().ok_or("OBO token missing act")?;
    let act_sub = act.sub.as_deref().ok_or("OBO token missing act.sub")?;
    Uuid::parse_str(act_sub).map_err(|_| "OBO act.sub must be a valid UUID".to_string())?;
    if claims.azp.as_deref().is_none_or(str::is_empty) {
        return Err("OBO token missing azp".to_string());
    }
    if claims.jti.as_deref().is_none_or(str::is_empty) {
        return Err("OBO token missing jti".to_string());
    }
    Ok(())
}

/// Check if the claims indicate an OBO token.
pub fn is_obo(claims: &WorkflowClaims) -> bool {
    claims.act.is_some() || matches!(claims.token_use.as_deref(), Some("workflow_obo"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_valid_subject() {
        let uuid = Uuid::new_v4();
        let result = parse_subject(&Some(uuid.to_string())).unwrap();
        assert_eq!(result.subject_uuid, uuid);
    }

    #[test]
    fn rejects_invalid_subject() {
        assert!(parse_subject(&Some("not-a-uuid".to_string())).is_err());
        assert!(parse_subject(&None).is_err());
    }

    #[test]
    fn accepts_human_and_agent() {
        assert!(validate_principal_type(&Some("human".to_string())).is_ok());
        assert!(validate_principal_type(&Some("agent".to_string())).is_ok());
        assert!(validate_principal_type(&Some("service".to_string())).is_err());
        assert!(validate_principal_type(&None).is_err());
    }

    #[test]
    fn token_use_defaults_to_access() {
        assert!(validate_token_use(&None).is_ok());
        assert!(validate_token_use(&Some("access".to_string())).is_ok());
        assert!(validate_token_use(&Some("workflow_obo".to_string())).is_ok());
        assert!(validate_token_use(&Some("invalid".to_string())).is_err());
    }

    #[test]
    fn obo_validation() {
        let mut claims = WorkflowClaims {
            sub: Some(Uuid::new_v4().to_string()),
            iss: Some("auth-service".to_string()),
            aud: Some("svc-workflow".to_string()),
            exp: Some(9999999999),
            iat: Some(1000000000),
            nbf: None,
            principal_type: Some("human".to_string()),
            token_type: Some("access".to_string()),
            version: Some("v1".to_string()),
            scope: Some("workflow.execute".to_string()),
            token_use: Some("workflow_obo".to_string()),
            act: Some(ActClaim {
                sub: Some(Uuid::new_v4().to_string()),
            }),
            azp: Some("test-client".to_string()),
            jti: Some("unique-token-id".to_string()),
        };
        assert!(validate_obo(&claims).is_ok());

        // Missing jti
        claims.jti = None;
        assert!(validate_obo(&claims).is_err());

        // Restore jti, missing azp
        claims.jti = Some("tid".to_string());
        claims.azp = None;
        assert!(validate_obo(&claims).is_err());

        // Invalid act.sub
        claims.azp = Some("client".to_string());
        claims.act = Some(ActClaim {
            sub: Some("not-uuid".to_string()),
        });
        assert!(validate_obo(&claims).is_err());
    }
}
