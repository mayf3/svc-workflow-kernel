use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::workflow_instance::recovery::RecoveryError;

use super::rows::TargetNodeRow;

fn storage(error: sqlx::Error) -> RecoveryError {
    RecoveryError::StorageError(error.to_string())
}

pub(super) async fn validate_actor(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
) -> Result<(), RecoveryError> {
    let principal: Option<(bool, String)> = sqlx::query_as(
        "SELECT enabled, principal_type::text FROM principals
         WHERE principal_id = $1 FOR SHARE",
    )
    .bind(principal_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let (enabled, principal_type) = principal.ok_or(RecoveryError::PrincipalNotFound)?;
    if !enabled {
        return Err(RecoveryError::PrincipalDisabled);
    }
    if principal_type == "SERVICE" {
        return Err(RecoveryError::PrincipalTypeNotAllowed);
    }
    Ok(())
}

pub(super) async fn validate_workflow_admin(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    domain_id: Uuid,
) -> Result<(), RecoveryError> {
    let principal: Option<(bool, String)> = sqlx::query_as(
        "SELECT enabled, principal_type::text FROM principals WHERE principal_id = $1",
    )
    .bind(principal_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let (enabled, principal_type) = principal.ok_or(RecoveryError::PrincipalNotFound)?;
    if !enabled {
        return Err(RecoveryError::PrincipalDisabled);
    }
    if principal_type == "SERVICE" {
        return Err(RecoveryError::PrincipalTypeNotAllowed);
    }
    let authorized: Option<Uuid> = sqlx::query_scalar(
        "SELECT b.binding_id FROM domain_role_bindings b
         JOIN principals p ON p.principal_id = b.principal_id
         WHERE b.domain_id = $1 AND b.principal_id = $2
           AND b.role_key = 'WORKFLOW_ADMIN' AND b.enabled = TRUE
           AND p.enabled = TRUE AND p.principal_type <> 'SERVICE'
         FOR SHARE OF b, p",
    )
    .bind(domain_id)
    .bind(principal_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    if authorized.is_none() {
        return Err(RecoveryError::PermissionDenied);
    }
    Ok(())
}

pub(super) async fn lock_definition_version_any(
    tx: &mut Transaction<'_, Postgres>,
    definition_version_id: Uuid,
) -> Result<String, RecoveryError> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT version_status::text FROM workflow_definition_versions
         WHERE definition_version_id = $1 FOR UPDATE",
    )
    .bind(definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    status
        .ok_or_else(|| RecoveryError::InternalConsistency("definition version missing".to_string()))
}

pub(super) fn validate_override_definition_status(status: &str) -> Result<(), RecoveryError> {
    match status {
        "PUBLISHED" | "DEPRECATED" | "REVOKED" => Ok(()),
        "DRAFT" => Err(RecoveryError::InvalidTarget(
            "emergency override is not available for a DRAFT definition version".to_string(),
        )),
        _ => Err(RecoveryError::InternalConsistency(
            "definition version has an unknown status".to_string(),
        )),
    }
}

pub(super) async fn read_target_node(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    definition_version_id: Uuid,
) -> Result<TargetNodeRow, RecoveryError> {
    sqlx::query_as(
        "SELECT node_id, definition_version_id, node_type::text,
                assignee_ref_type::text, fixed_principal_id
         FROM workflow_node_definitions
         WHERE node_id = $1 AND definition_version_id = $2",
    )
    .bind(node_id)
    .bind(definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or_else(|| {
        RecoveryError::InvalidTarget(
            "target node is not in the fixed definition version".to_string(),
        )
    })
}

pub(super) async fn resolve_non_terminal_assignee(
    tx: &mut Transaction<'_, Postgres>,
    target: &TargetNodeRow,
    creator: Uuid,
    domain_id: Uuid,
) -> Result<Uuid, RecoveryError> {
    if target.node_type == "TERMINAL" {
        return Err(RecoveryError::InvalidTarget(
            "MOVE_TO_NODE target must be non-terminal".to_string(),
        ));
    }
    let candidate = match target.assignee_ref_type.as_deref() {
        Some("WORKFLOW_CREATOR") => creator,
        Some("DOMAIN_OWNER") => sqlx::query_scalar(
            "SELECT b.principal_id FROM domain_role_bindings b
             JOIN principals p ON p.principal_id = b.principal_id
             WHERE b.domain_id = $1 AND b.role_key = 'DOMAIN_OWNER'
               AND b.enabled = TRUE AND p.enabled = TRUE
             FOR SHARE OF b, p",
        )
        .bind(domain_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| {
            RecoveryError::AssigneeResolutionFailed("enabled DOMAIN_OWNER unavailable".to_string())
        })?,
        Some("FIXED_PRINCIPAL") => target.fixed_principal_id.ok_or_else(|| {
            RecoveryError::AssigneeResolutionFailed("fixed principal is missing".to_string())
        })?,
        _ => {
            return Err(RecoveryError::AssigneeResolutionFailed(
                "non-terminal target has no valid assignee reference".to_string(),
            ))
        }
    };
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM principals WHERE principal_id = $1 FOR SHARE")
            .bind(candidate)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?;
    enabled
        .filter(|value| *value)
        .map(|_| candidate)
        .ok_or_else(|| {
            RecoveryError::AssigneeResolutionFailed("target assignee is unavailable".to_string())
        })
}
