//! Command receipt handling for workflow transition idempotency.
//!
//! Mirrors `command_receipt.rs` but returns `ExecuteWorkflowTransitionError`.
//! Pattern:
//! 1. INSERT ... ON CONFLICT DO NOTHING RETURNING
//! 2. If no row returned, SELECT ... FOR UPDATE and handle:
//!    - Same request_hash + COMPLETED → replay stored response
//!    - Different request_hash → IdempotencyConflict
//!    - Still PROCESSING → CommandStillProcessing

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::workflow_instance::errors::ExecuteWorkflowTransitionError;
use crate::domain::workflow_instance::events::COMMAND_TYPE_EXECUTE_TRANSITION;

/// Attempt to insert a new command receipt as the current owner.
///
/// Returns `Ok(Some(command_id))` if the insert succeeded (we own this request).
/// Returns `Ok(None)` if the key already exists (need to replay or conflict).
pub(super) async fn try_insert_transition_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    principal_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<Uuid>, ExecuteWorkflowTransitionError> {
    let result: Option<(Uuid,)> = sqlx::query_as(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash, receipt_status)
        VALUES ($1, $2, $3, $4, $5, 'PROCESSING')
        ON CONFLICT (principal_id, idempotency_key) DO NOTHING
        RETURNING command_id
        "#,
    )
    .bind(command_id)
    .bind(principal_id)
    .bind(idempotency_key)
    .bind(COMMAND_TYPE_EXECUTE_TRANSITION)
    .bind(request_hash)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    Ok(result.map(|r| r.0))
}

/// Result of replaying an existing receipt.
pub(super) enum TransitionReplayResult {
    /// The receipt is COMPLETED and the request hash matches → return stored response.
    CompletedMatch {
        command_id: Uuid,
        response_status: i32,
        response_body: serde_json::Value,
    },
    /// The receipt is COMPLETED but request hash differs → IdempotencyConflict.
    CompletedConflict {
        command_id: Uuid,
        original_request_hash: String,
    },
    /// The receipt is still PROCESSING.
    StillProcessing,
}

/// Read an existing receipt by idempotency key and handle replay/conflict.
pub(super) async fn replay_transition_receipt(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    idempotency_key: &str,
    new_request_hash: &str,
) -> Result<TransitionReplayResult, ExecuteWorkflowTransitionError> {
    let receipt: Option<(Uuid, String, String, i32, Option<serde_json::Value>)> = sqlx::query_as(
        r#"
        SELECT command_id, receipt_status::TEXT, request_hash,
               COALESCE(response_status, 0) AS response_status, response_body
        FROM workflow_command_receipts
        WHERE principal_id = $1 AND idempotency_key = $2
        FOR UPDATE
        "#,
    )
    .bind(principal_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    match receipt {
        None => Err(ExecuteWorkflowTransitionError::StorageError(
            "idempotency key vanished between INSERT and SELECT".to_string(),
        )),
        Some((cmd_id, status, stored_hash, resp_status, resp_body)) => match status.as_str() {
            "COMPLETED" => {
                if stored_hash == new_request_hash {
                    Ok(TransitionReplayResult::CompletedMatch {
                        command_id: cmd_id,
                        response_status: resp_status,
                        response_body: resp_body.unwrap_or(serde_json::Value::Null),
                    })
                } else {
                    Ok(TransitionReplayResult::CompletedConflict {
                        command_id: cmd_id,
                        original_request_hash: stored_hash,
                    })
                }
            }
            "PROCESSING" => Ok(TransitionReplayResult::StillProcessing),
            _ => Err(ExecuteWorkflowTransitionError::StorageError(format!(
                "unknown receipt status: {}",
                status
            ))),
        },
    }
}

/// Write a command attempt audit log entry (for idempotency conflicts).
pub(super) async fn write_transition_attempt_audit(
    tx: &mut Transaction<'_, Postgres>,
    audit_id: Uuid,
    command_id: Uuid,
    principal_id: Uuid,
    idempotency_key: &str,
    attempt_type: &str,
    failure_reason: Option<&str>,
    request_hash: &str,
    details: Option<&serde_json::Value>,
) -> Result<(), ExecuteWorkflowTransitionError> {
    sqlx::query(
        r#"
        INSERT INTO workflow_command_attempt_audits
            (audit_id, command_id, principal_id, idempotency_key, attempt_type,
             failure_reason, request_hash, details)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(audit_id)
    .bind(command_id)
    .bind(principal_id)
    .bind(idempotency_key)
    .bind(attempt_type)
    .bind(failure_reason)
    .bind(request_hash)
    .bind(details)
    .execute(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    Ok(())
}

/// Complete a command receipt (transition PROCESSING → COMPLETED).
pub(super) async fn complete_transition_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    response_status: i32,
    response_body: &serde_json::Value,
    response_digest: &str,
) -> Result<(), ExecuteWorkflowTransitionError> {
    sqlx::query(
        r#"
        UPDATE workflow_command_receipts
        SET receipt_status = 'COMPLETED',
            response_status = $1,
            response_body = $2,
            response_digest = $3
        WHERE command_id = $4 AND receipt_status = 'PROCESSING'
        "#,
    )
    .bind(response_status)
    .bind(response_body)
    .bind(response_digest)
    .bind(command_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    Ok(())
}
