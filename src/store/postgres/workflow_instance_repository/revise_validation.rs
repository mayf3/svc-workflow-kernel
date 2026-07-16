//! Validation helpers for the atomic context revision transaction.
//!
//! Mirrors the pattern from `validation_helpers.rs` for CreateWorkflowInstance,
//! but specific to ReviseWorkflowContext semantics.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::enums::NodeType;
use crate::domain::workflow_instance::commands::ReviseWorkflowContextCommand;
use crate::domain::workflow_instance::errors::ReviseWorkflowContextError;

use super::row_types::*;

/// Validate the principal exists and is enabled inside the transaction.
pub(super) async fn validate_principal_enabled(
    tx: &mut Transaction<'_, Postgres>,
    principal_uuid: Uuid,
) -> Result<Option<ReviseWorkflowContextError>, ReviseWorkflowContextError> {
    let principal: Option<(bool,)> =
        sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
            .bind(principal_uuid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    match principal {
        None => Ok(Some(ReviseWorkflowContextError::PrincipalNotFound)),
        Some((enabled,)) if !enabled => Ok(Some(ReviseWorkflowContextError::PrincipalDisabled)),
        _ => Ok(None),
    }
}

/// Validate context payload against the definition's context_schema.
pub(super) fn validate_context_schema(
    context_schema: &Option<serde_json::Value>,
    cmd: &ReviseWorkflowContextCommand,
) -> Result<(), ReviseWorkflowContextError> {
    if let Some(schema) = context_schema {
        jsonschema::validator_for(schema)
            .map_err(|e| {
                ReviseWorkflowContextError::ContextValidationFailed(format!(
                    "context_schema compilation failed: {}",
                    e
                ))
            })?
            .validate(&cmd.context_payload)
            .map_err(|e| {
                ReviseWorkflowContextError::ContextValidationFailed(format!(
                    "context_payload failed schema validation: {}",
                    e
                ))
            })?;
    }

    Ok(())
}

/// Lock and read the workflow instance for revision.
/// Returns (instance, definition_version_id) or an error.
pub(super) async fn lock_instance(
    tx: &mut Transaction<'_, Postgres>,
    instance_uuid: Uuid,
) -> Result<InstanceLockRow, ReviseWorkflowContextError> {
    let instance: Option<InstanceLockRow> = sqlx::query_as(
        "SELECT workflow_instance_id, created_by_principal_id, \
         definition_version_id, current_context_revision_id, \
         current_node_visit_id, workflow_state_version \
         FROM workflow_instances WHERE workflow_instance_id = $1 FOR UPDATE",
    )
    .bind(instance_uuid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    instance.ok_or(ReviseWorkflowContextError::InstanceNotFound)
}

/// Read and validate the current node visit.
/// Must be of type DRAFT.
pub(super) async fn validate_current_visit(
    tx: &mut Transaction<'_, Postgres>,
    instance_uuid: Uuid,
    current_node_visit_id: Uuid,
) -> Result<CurrentVisitRow, ReviseWorkflowContextError> {
    let visit: Option<CurrentVisitRow> = sqlx::query_as(
        "SELECT nv.node_visit_id, nv.node_id, nd.node_type::TEXT \
         FROM workflow_node_visits nv \
         JOIN workflow_node_definitions nd ON nd.node_id = nv.node_id \
         WHERE nv.node_visit_id = $1 AND nv.workflow_instance_id = $2",
    )
    .bind(current_node_visit_id)
    .bind(instance_uuid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    let visit = visit.ok_or(ReviseWorkflowContextError::CurrentVisitNotFound)?;

    if visit.node_type_enum() != NodeType::DRAFT {
        return Err(ReviseWorkflowContextError::CurrentNodeNotDraft);
    }

    Ok(visit)
}

/// Read the current context revision metadata.
pub(super) async fn read_current_context(
    tx: &mut Transaction<'_, Postgres>,
    instance_uuid: Uuid,
    context_revision_id: Uuid,
) -> Result<CurrentContextRow, ReviseWorkflowContextError> {
    let ctx: Option<CurrentContextRow> = sqlx::query_as(
        "SELECT context_revision_id, revision_number, COALESCE(payload_digest, '') AS payload_digest \
         FROM workflow_context_revisions \
         WHERE context_revision_id = $1 AND workflow_instance_id = $2",
    )
    .bind(context_revision_id)
    .bind(instance_uuid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    ctx.ok_or(ReviseWorkflowContextError::InternalConsistency(
        "current context revision not found or belongs to different instance".to_string(),
    ))
}

/// Validate definition version status for revision.
/// PUBLISHED and DEPRECATED are allowed; REVOKED and DRAFT are blocked.
pub(super) async fn validate_definition_version_status(
    tx: &mut Transaction<'_, Postgres>,
    definition_version_id: Uuid,
) -> Result<(), ReviseWorkflowContextError> {
    let status: Option<(String,)> = sqlx::query_as(
        "SELECT version_status::TEXT FROM workflow_definition_versions \
         WHERE definition_version_id = $1 FOR UPDATE",
    )
    .bind(definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    match status {
        None => Err(ReviseWorkflowContextError::InternalConsistency(
            "definition version not found for instance".to_string(),
        )),
        Some((s,)) if s == "REVOKED" => Err(ReviseWorkflowContextError::DefinitionVersionRevoked),
        Some((s,)) if s == "DRAFT" => Err(ReviseWorkflowContextError::DefinitionVersionDraft),
        _ => Ok(()), // PUBLISHED or DEPRECATED allowed
    }
}

/// Map a ReviseWorkflowContextError to a response body for deterministic failure receipts.
pub(super) fn error_response_body(err: &ReviseWorkflowContextError) -> serde_json::Value {
    match err {
        ReviseWorkflowContextError::WorkflowStateVersionConflict { expected, actual } => {
            serde_json::json!({
                "error": "workflow_state_version_conflict",
                "expected": expected,
                "actual": actual,
            })
        }
        _ => {
            let label = crate::domain::workflow_instance::errors::revise_error_label(err);
            serde_json::json!({"error": label})
        }
    }
}
