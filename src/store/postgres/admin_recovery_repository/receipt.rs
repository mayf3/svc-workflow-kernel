use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::workflow_instance::recovery::RecoveryError;

type StoredReceipt = (Uuid, String, String, Option<i32>, Option<serde_json::Value>);

pub(super) enum AcquireReceipt {
    Owned(Uuid),
    Replay {
        command_id: Uuid,
        response_status: i32,
        response_body: serde_json::Value,
    },
    Conflict {
        command_id: Uuid,
    },
    Processing {
        command_id: Uuid,
    },
}

impl AcquireReceipt {
    pub(super) fn command_id(&self) -> Uuid {
        match self {
            Self::Owned(command_id)
            | Self::Replay { command_id, .. }
            | Self::Conflict { command_id }
            | Self::Processing { command_id } => *command_id,
        }
    }

    pub(super) fn is_owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }
}

fn storage(error: sqlx::Error) -> RecoveryError {
    RecoveryError::StorageError(error.to_string())
}

pub(super) async fn acquire(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    idempotency_key: &str,
    command_type: &str,
    request_hash: &str,
) -> Result<AcquireReceipt, RecoveryError> {
    let proposed = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO workflow_command_receipts
         (command_id, principal_id, idempotency_key, command_type, request_hash, receipt_status)
         VALUES ($1, $2, $3, $4, $5, 'PROCESSING')
         ON CONFLICT (principal_id, idempotency_key) DO NOTHING
         RETURNING command_id",
    )
    .bind(proposed)
    .bind(principal_id)
    .bind(idempotency_key)
    .bind(command_type)
    .bind(request_hash)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    if let Some(command_id) = inserted {
        return Ok(AcquireReceipt::Owned(command_id));
    }

    let existing: Option<StoredReceipt> = sqlx::query_as(
        "SELECT command_id, receipt_status::text, request_hash,
                    response_status, response_body
             FROM workflow_command_receipts
             WHERE principal_id = $1 AND idempotency_key = $2
             FOR UPDATE",
    )
    .bind(principal_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let (command_id, status, original_hash, response_status, response_body) = existing
        .ok_or_else(|| RecoveryError::InternalConsistency("receipt disappeared".to_string()))?;

    if original_hash != request_hash {
        return Ok(AcquireReceipt::Conflict { command_id });
    }
    if status == "PROCESSING" {
        return Ok(AcquireReceipt::Processing { command_id });
    }
    let response_status = response_status.ok_or_else(|| {
        RecoveryError::InternalConsistency("completed receipt has no status".to_string())
    })?;
    let response_body = response_body.ok_or_else(|| {
        RecoveryError::InternalConsistency("completed receipt has no body".to_string())
    })?;
    Ok(AcquireReceipt::Replay {
        command_id,
        response_status,
        response_body,
    })
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    response_status: i32,
    response_body: &serde_json::Value,
) -> Result<(), RecoveryError> {
    let response_digest =
        digest::compute_json_digest(response_body).map_err(RecoveryError::StorageError)?;
    let affected = sqlx::query(
        "UPDATE workflow_command_receipts
         SET receipt_status = 'COMPLETED', response_status = $2,
             response_body = $3, response_digest = $4
         WHERE command_id = $1 AND receipt_status = 'PROCESSING'",
    )
    .bind(command_id)
    .bind(response_status)
    .bind(response_body)
    .bind(response_digest)
    .execute(&mut **tx)
    .await
    .map_err(storage)?
    .rows_affected();
    if affected != 1 {
        return Err(RecoveryError::InternalConsistency(
            "receipt completion affected an unexpected row count".to_string(),
        ));
    }
    Ok(())
}

pub(super) async fn write_attempt(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    principal_id: Uuid,
    idempotency_key: &str,
    attempt_type: &str,
    failure_reason: Option<&str>,
    request_hash: &str,
    details: &serde_json::Value,
) -> Result<(), RecoveryError> {
    sqlx::query(
        "INSERT INTO workflow_command_attempt_audits
         (audit_id, command_id, principal_id, idempotency_key, attempt_type,
          failure_reason, request_hash, details)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(command_id)
    .bind(principal_id)
    .bind(idempotency_key)
    .bind(attempt_type)
    .bind(failure_reason)
    .bind(request_hash)
    .bind(details)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

pub(super) async fn write_security(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
    action: &str,
    instance_id: Uuid,
    details: &serde_json::Value,
) -> Result<(), RecoveryError> {
    sqlx::query(
        "INSERT INTO workflow_security_audits
         (audit_id, principal_id, action, resource_type, resource_id, details)
         VALUES ($1, $2, $3, 'WORKFLOW_INSTANCE', $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(principal_id)
    .bind(action)
    .bind(instance_id.to_string())
    .bind(details)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

pub(super) async fn record_existing_denial(
    tx: &mut Transaction<'_, Postgres>,
    acquired: &AcquireReceipt,
    principal_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
    instance_id: Uuid,
    action: &str,
) -> Result<(), RecoveryError> {
    write_attempt(
        tx,
        acquired.command_id(),
        principal_id,
        idempotency_key,
        "PERMISSION_DENIED",
        Some("current authorization denied"),
        request_hash,
        &serde_json::json!({"error": "permission_denied"}),
    )
    .await?;
    write_security(
        tx,
        principal_id,
        action,
        instance_id,
        &serde_json::json!({"reason": "permission_denied"}),
    )
    .await
}

pub(super) async fn record_conflict_or_processing(
    tx: &mut Transaction<'_, Postgres>,
    acquired: &AcquireReceipt,
    principal_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<RecoveryError, RecoveryError> {
    let (attempt_type, reason, error) = match acquired {
        AcquireReceipt::Conflict { .. } => (
            "IDEMPOTENCY_CONFLICT",
            "request hash mismatch",
            RecoveryError::IdempotencyConflict,
        ),
        AcquireReceipt::Processing { .. } => (
            "STILL_PROCESSING",
            "existing receipt is still processing",
            RecoveryError::CommandStillProcessing,
        ),
        _ => {
            return Err(RecoveryError::InternalConsistency(
                "receipt is neither conflict nor processing".to_string(),
            ))
        }
    };
    write_attempt(
        tx,
        acquired.command_id(),
        principal_id,
        idempotency_key,
        attempt_type,
        Some(reason),
        request_hash,
        &serde_json::json!({"error": error.label()}),
    )
    .await?;
    Ok(error)
}

pub(super) fn error_body(error: &RecoveryError) -> serde_json::Value {
    let mut value = serde_json::json!({"error": error.label()});
    match error {
        RecoveryError::BeforeSnapshotDigestMismatch { expected, actual } => {
            value["expected"] = expected.clone().into();
            value["actual"] = actual.clone().into();
        }
        RecoveryError::WorkflowStateVersionConflict { expected, actual } => {
            value["expected"] = (*expected).into();
            value["actual"] = (*actual).into();
        }
        _ => {
            if let Some(detail) = error.detail() {
                value["detail"] = detail.into();
            }
        }
    }
    value
}

pub(super) fn error_from_body(body: &serde_json::Value) -> RecoveryError {
    let detail = || {
        body["detail"]
            .as_str()
            .unwrap_or("replayed failure")
            .to_string()
    };
    match body["error"]
        .as_str()
        .unwrap_or("internal_consistency_error")
    {
        "principal_disabled" => RecoveryError::PrincipalDisabled,
        "principal_type_not_allowed" => RecoveryError::PrincipalTypeNotAllowed,
        "permission_denied" => RecoveryError::PermissionDenied,
        "instance_not_found" => RecoveryError::InstanceNotFound,
        "invalid_input" => RecoveryError::InvalidInput(detail()),
        "before_snapshot_digest_mismatch" => RecoveryError::BeforeSnapshotDigestMismatch {
            expected: body["expected"].as_str().unwrap_or_default().to_string(),
            actual: body["actual"].as_str().unwrap_or_default().to_string(),
        },
        "workflow_state_version_conflict" => RecoveryError::WorkflowStateVersionConflict {
            expected: body["expected"].as_i64().unwrap_or_default() as i32,
            actual: body["actual"].as_i64().unwrap_or_default() as i32,
        },
        "invalid_immutable_facts" => RecoveryError::InvalidImmutableFacts(detail()),
        "invalid_target" => RecoveryError::InvalidTarget(detail()),
        "assignee_resolution_failed" => RecoveryError::AssigneeResolutionFailed(detail()),
        _ => RecoveryError::InternalConsistency(detail()),
    }
}
