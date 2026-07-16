//! CommandReceipt operations for ReviseContextAndTransition.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::workflow_instance::combined_errors::ReviseContextAndTransitionError;
use crate::domain::workflow_instance::events::COMMAND_TYPE_REVISE_CONTEXT_AND_TRANSITION;

pub(super) enum CombinedReplayResult {
    CompletedMatch {
        response_status: i32,
        response_body: serde_json::Value,
    },
    CompletedConflict {
        command_id: Uuid,
        original_request_hash: String,
    },
    StillProcessing,
}

pub(super) async fn try_insert_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    principal_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<Uuid>, ReviseContextAndTransitionError> {
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type,
             request_hash, receipt_status)
        VALUES ($1, $2, $3, $4, $5, 'PROCESSING')
        ON CONFLICT (principal_id, idempotency_key) DO NOTHING
        RETURNING command_id
        "#,
    )
    .bind(command_id)
    .bind(principal_id)
    .bind(idempotency_key)
    .bind(COMMAND_TYPE_REVISE_CONTEXT_AND_TRANSITION)
    .bind(request_hash)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;

    Ok(inserted.map(|row| row.0))
}

pub(super) async fn replay_receipt(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<CombinedReplayResult, ReviseContextAndTransitionError> {
    let receipt: Option<(Uuid, String, String, i32, Option<serde_json::Value>)> = sqlx::query_as(
        r#"
            SELECT command_id, receipt_status::TEXT, request_hash,
                   COALESCE(response_status, 0), response_body
            FROM workflow_command_receipts
            WHERE principal_id = $1 AND idempotency_key = $2
            FOR UPDATE
            "#,
    )
    .bind(principal_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;

    match receipt {
        None => Err(ReviseContextAndTransitionError::StorageError(
            "idempotency key vanished between INSERT and SELECT".to_string(),
        )),
        Some((_, status, _, _, _)) if status == "PROCESSING" => {
            Ok(CombinedReplayResult::StillProcessing)
        }
        Some((_command_id, status, stored_hash, response_status, response_body))
            if status == "COMPLETED" && stored_hash == request_hash =>
        {
            Ok(CombinedReplayResult::CompletedMatch {
                response_status,
                response_body: response_body.unwrap_or(serde_json::Value::Null),
            })
        }
        Some((command_id, status, stored_hash, _, _)) if status == "COMPLETED" => {
            Ok(CombinedReplayResult::CompletedConflict {
                command_id,
                original_request_hash: stored_hash,
            })
        }
        Some((_, status, _, _, _)) => Err(ReviseContextAndTransitionError::StorageError(format!(
            "unknown receipt status: {}",
            status
        ))),
    }
}

pub(super) async fn write_attempt_audit(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    principal_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
    original_request_hash: &str,
) -> Result<(), ReviseContextAndTransitionError> {
    let details = serde_json::json!({
        "conflictType": "IDEMPOTENCY_KEY_MISMATCH",
        "originalRequestHash": original_request_hash,
        "newRequestHash": request_hash,
    });
    sqlx::query(
        r#"
        INSERT INTO workflow_command_attempt_audits
            (audit_id, command_id, principal_id, idempotency_key, attempt_type,
             failure_reason, request_hash, details)
        VALUES ($1, $2, $3, $4, 'IDEMPOTENCY_CONFLICT',
                'request hash mismatch', $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(command_id)
    .bind(principal_id)
    .bind(idempotency_key)
    .bind(request_hash)
    .bind(details)
    .execute(&mut **tx)
    .await
    .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;
    Ok(())
}

pub(super) async fn complete_receipt(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    response_status: i32,
    response_body: &serde_json::Value,
    response_digest: &str,
) -> Result<(), ReviseContextAndTransitionError> {
    let result = sqlx::query(
        r#"
        UPDATE workflow_command_receipts
        SET receipt_status = 'COMPLETED', response_status = $1,
            response_body = $2, response_digest = $3
        WHERE command_id = $4 AND receipt_status = 'PROCESSING'
        "#,
    )
    .bind(response_status)
    .bind(response_body)
    .bind(response_digest)
    .bind(command_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;

    if result.rows_affected() != 1 {
        return Err(ReviseContextAndTransitionError::InternalConsistency(
            "receipt completion affected unexpected number of rows".to_string(),
        ));
    }
    Ok(())
}
