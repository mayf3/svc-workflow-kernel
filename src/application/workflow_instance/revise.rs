//! ReviseWorkflowContext application service.
//!
//! Orchestrates the full context revision workflow:
//! 1. Pre-validate principal existence and enabled status
//! 2. Validate context_payload size
//! 3. Compute request hash for idempotency
//! 4. Delegate to the atomic revision transaction
//! 5. Map result to the public response type

use sqlx::PgPool;

use crate::domain::workflow_instance::commands::ReviseWorkflowContextCommand;
use crate::domain::workflow_instance::errors::ReviseWorkflowContextError;
use crate::store::postgres::workflow_instance_repository::revise_transaction;

use super::idempotency::compute_revise_request_hash;

/// Result of a successful ReviseWorkflowContext command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReviseWorkflowContextResult {
    pub workflow_instance_id: uuid::Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: uuid::Uuid,
    pub current_node_visit_id: uuid::Uuid,
    pub event_sequence: i32,
}

impl From<revise_transaction::ReviseResult> for ReviseWorkflowContextResult {
    fn from(r: revise_transaction::ReviseResult) -> Self {
        Self {
            workflow_instance_id: r.workflow_instance_id,
            workflow_state_version: r.workflow_state_version,
            current_context_revision_id: r.current_context_revision_id,
            current_node_visit_id: r.current_node_visit_id,
            event_sequence: r.event_sequence,
        }
    }
}

/// Revise the workflow context atomically.
///
/// # Errors
///
/// Returns `ReviseWorkflowContextError` for all validation, authorization,
/// version conflict, and infrastructure failures.
pub async fn revise_workflow_context(
    pool: &PgPool,
    command: ReviseWorkflowContextCommand,
) -> Result<ReviseWorkflowContextResult, ReviseWorkflowContextError> {
    // 1. Pre-validate principal existence and enabled status
    let principal_uuid = command.principal_id.into_uuid();
    pre_validate_principal(pool, principal_uuid).await?;

    // 2. Validate context payload size (pre-transaction fast-fail)
    validate_context_size(&command)?;

    // 3. Compute request hash for idempotency
    let request_hash = compute_revise_request_hash(
        &command.command_schema_version,
        &command.idempotency_key,
        &command.principal_id,
        &command.workflow_instance_id,
        command.expected_workflow_state_version,
        &command.context_payload,
    )?;

    // 4. Execute atomic revision
    let outcome =
        revise_transaction::revise_workflow_context_atomically(pool, command, &request_hash)
            .await?;

    // 5. Map outcome to public result
    match outcome {
        revise_transaction::ReviseOutcome::Revised(result) => Ok(result.into()),
        revise_transaction::ReviseOutcome::Replayed(result) => Ok(result.into()),
        revise_transaction::ReviseOutcome::ReplayedFailure(status, body) => {
            let error_code = body["error"].as_str().unwrap_or("unknown");
            Err(match (status, error_code) {
                (404, "instance_not_found") => ReviseWorkflowContextError::InstanceNotFound,
                (403, "principal_disabled") => ReviseWorkflowContextError::PrincipalDisabled,
                (404, "current_visit_not_found") => {
                    ReviseWorkflowContextError::CurrentVisitNotFound
                }
                (409, "current_node_not_draft") => ReviseWorkflowContextError::CurrentNodeNotDraft,
                (409, "definition_version_revoked") => {
                    ReviseWorkflowContextError::DefinitionVersionRevoked
                }
                (500, "definition_version_draft") => {
                    ReviseWorkflowContextError::DefinitionVersionDraft
                }
                (409, "workflow_state_version_conflict") => {
                    // Extract expected/actual from body if available
                    let expected = body["expected"].as_i64().unwrap_or(0) as i32;
                    let actual = body["actual"].as_i64().unwrap_or(0) as i32;
                    ReviseWorkflowContextError::WorkflowStateVersionConflict { expected, actual }
                }
                (422, "context_validation_failed") => {
                    ReviseWorkflowContextError::ContextValidationFailed(
                        body["error"].as_str().unwrap_or("unknown").to_string(),
                    )
                }
                (413, "size_limit_exceeded") => ReviseWorkflowContextError::SizeLimitExceeded(
                    body["error"].as_str().unwrap_or("unknown").to_string(),
                ),
                _ => ReviseWorkflowContextError::StorageError(format!(
                    "replayed deterministic failure: status={}, error={}",
                    status, error_code
                )),
            })
        }
    }
}

/// Fast-fail check that the principal exists and is enabled,
/// before entering the main transaction.
async fn pre_validate_principal(
    pool: &PgPool,
    principal_uuid: uuid::Uuid,
) -> Result<(), ReviseWorkflowContextError> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
            .bind(principal_uuid)
            .fetch_optional(pool)
            .await
            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    match row {
        None => Err(ReviseWorkflowContextError::PrincipalNotFound),
        Some((enabled,)) if !enabled => Err(ReviseWorkflowContextError::PrincipalDisabled),
        _ => Ok(()),
    }
}

/// Validate context payload size at the service layer (pre-transaction).
fn validate_context_size(
    cmd: &ReviseWorkflowContextCommand,
) -> Result<(), ReviseWorkflowContextError> {
    let context_bytes = serde_json::to_vec(&cmd.context_payload)
        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
    if context_bytes.len() > 1024 * 1024 {
        return Err(ReviseWorkflowContextError::SizeLimitExceeded(
            "context_payload exceeds 1 MiB".to_string(),
        ));
    }
    Ok(())
}
