use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::workflow_instance::import::LegacyImportError;
use crate::store::postgres::import_receipt_validation::{
    validate_stored_digest, ImportReceiptFact,
};

pub(super) enum Acquired {
    Owned(Uuid),
    Replay(ImportReceiptFact),
    Conflict(Uuid),
    Processing(Uuid),
}

impl Acquired {
    pub(super) fn command_id(&self) -> Uuid {
        match self {
            Self::Owned(id) | Self::Conflict(id) | Self::Processing(id) => *id,
            Self::Replay(receipt) => receipt.command_id,
        }
    }

    pub(super) fn is_owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }
}

fn storage(error: sqlx::Error) -> LegacyImportError {
    LegacyImportError::StorageError(error.to_string())
}

pub(super) async fn acquire(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    key: &str,
    command_type: &str,
    request_hash: &str,
) -> Result<Acquired, LegacyImportError> {
    let proposed = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO workflow_command_receipts
         (command_id, principal_id, idempotency_key, command_type, request_hash, receipt_status)
         VALUES ($1, $2, $3, $4, $5, 'PROCESSING')
         ON CONFLICT (principal_id, idempotency_key) DO NOTHING RETURNING command_id",
    )
    .bind(proposed)
    .bind(actor)
    .bind(key)
    .bind(command_type)
    .bind(request_hash)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    if let Some(id) = inserted {
        return Ok(Acquired::Owned(id));
    }
    let receipt: ImportReceiptFact = sqlx::query_as(
        "SELECT command_id, principal_id, idempotency_key, command_type,
                request_hash, receipt_status::text AS receipt_status,
                response_status, response_body, response_digest
         FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2
         FOR UPDATE",
    )
    .bind(actor)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or_else(|| LegacyImportError::InternalConsistency("receipt disappeared".to_string()))?;
    if receipt.principal_id != actor
        || receipt.idempotency_key != key
        || receipt.command_type != command_type
    {
        return Err(LegacyImportError::InternalConsistency(
            "stored receipt identity or command type is invalid".to_string(),
        ));
    }
    if receipt.request_hash != request_hash {
        return Ok(Acquired::Conflict(receipt.command_id));
    }
    if receipt.receipt_status == "PROCESSING" {
        return Ok(Acquired::Processing(receipt.command_id));
    }
    validate_stored_digest(&receipt).map_err(LegacyImportError::InternalConsistency)?;
    Ok(Acquired::Replay(receipt))
}

pub(super) async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    status: i32,
    body: &serde_json::Value,
) -> Result<(), LegacyImportError> {
    let response_digest =
        digest::compute_json_digest(body).map_err(LegacyImportError::StorageError)?;
    let affected = sqlx::query(
        "UPDATE workflow_command_receipts SET receipt_status = 'COMPLETED',
         response_status = $2, response_body = $3, response_digest = $4
         WHERE command_id = $1 AND receipt_status = 'PROCESSING'",
    )
    .bind(command_id)
    .bind(status)
    .bind(body)
    .bind(response_digest)
    .execute(&mut **tx)
    .await
    .map_err(storage)?
    .rows_affected();
    if affected != 1 {
        return Err(LegacyImportError::InternalConsistency(
            "receipt completion affected an unexpected row count".to_string(),
        ));
    }
    Ok(())
}

pub(super) async fn write_attempt(
    tx: &mut Transaction<'_, Postgres>,
    acquired: &Acquired,
    actor: Uuid,
    key: &str,
    request_hash: &str,
    attempt_type: &str,
    error: &LegacyImportError,
) -> Result<(), LegacyImportError> {
    sqlx::query(
        "INSERT INTO workflow_command_attempt_audits
         (audit_id, command_id, principal_id, idempotency_key, attempt_type,
          failure_reason, request_hash, details)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::new_v4())
    .bind(acquired.command_id())
    .bind(actor)
    .bind(key)
    .bind(attempt_type)
    .bind(error.label())
    .bind(request_hash)
    .bind(error_body(error))
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

pub(super) async fn write_security(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    instance_id: Uuid,
    details: &serde_json::Value,
) -> Result<(), LegacyImportError> {
    sqlx::query(
        "INSERT INTO workflow_security_audits
         (audit_id, principal_id, action, resource_type, resource_id, details)
         VALUES ($1, $2, 'LEGACY_WORKFLOW_IMPORT_COMMITTED', 'WORKFLOW_INSTANCE', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(actor)
    .bind(instance_id.to_string())
    .bind(details)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

pub(super) fn error_body(error: &LegacyImportError) -> serde_json::Value {
    let mut body = serde_json::json!({"error": error.label()});
    if let LegacyImportError::SnapshotDigestMismatch { expected, actual } = error {
        body["expected"] = expected.clone().into();
        body["actual"] = actual.clone().into();
    }
    body
}

fn lower_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn error_from_receipt(
    receipt: &ImportReceiptFact,
) -> Result<LegacyImportError, LegacyImportError> {
    validate_stored_digest(receipt).map_err(LegacyImportError::InternalConsistency)?;
    if receipt.receipt_status != "COMPLETED" {
        return Err(LegacyImportError::InternalConsistency(
            "failure receipt is not completed".to_string(),
        ));
    }
    let status = receipt.response_status.ok_or_else(|| {
        LegacyImportError::InternalConsistency("completed receipt has no status".to_string())
    })?;
    let body = receipt.response_body.as_ref().ok_or_else(|| {
        LegacyImportError::InternalConsistency("completed receipt has no body".to_string())
    })?;
    let object = body.as_object().ok_or_else(|| {
        LegacyImportError::InternalConsistency("failure response is not an object".to_string())
    })?;
    let label = object
        .get("error")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            LegacyImportError::InternalConsistency(
                "failure response has no error label".to_string(),
            )
        })?;
    let detail = "replayed deterministic failure".to_string();
    let error = match label {
        "principal_not_found" if status == 404 => LegacyImportError::PrincipalNotFound,
        "principal_disabled" if status == 403 => LegacyImportError::PrincipalDisabled,
        "principal_type_not_allowed" if status == 403 => LegacyImportError::PrincipalTypeNotAllowed,
        "migration_binding_invalid" if status == 403 => LegacyImportError::MigrationBindingInvalid,
        "permission_denied" if status == 403 => LegacyImportError::PermissionDenied,
        "domain_not_found" if status == 404 => LegacyImportError::DomainNotFound,
        "domain_disabled" if status == 403 => LegacyImportError::DomainDisabled,
        "definition_version_not_found" if status == 404 => {
            LegacyImportError::DefinitionVersionNotFound
        }
        "version_not_published" if status == 409 => LegacyImportError::VersionNotPublished,
        "imported_node_not_found" if status == 404 => LegacyImportError::ImportedNodeNotFound,
        "invalid_input" if status == 422 => LegacyImportError::InvalidInput(detail),
        "creator_resolution_failed" if status == 422 => {
            LegacyImportError::CreatorResolutionFailed(detail)
        }
        "assignee_resolution_failed" if status == 422 => {
            LegacyImportError::AssigneeResolutionFailed(detail)
        }
        "context_validation_failed" if status == 422 => {
            LegacyImportError::ContextValidationFailed(detail)
        }
        "size_limit_exceeded" if status == 413 => LegacyImportError::SizeLimitExceeded(detail),
        "external_reference_conflict" if status == 409 => {
            LegacyImportError::ExternalReferenceConflict
        }
        "snapshot_digest_mismatch" if status == 409 => {
            let expected = object.get("expected").and_then(serde_json::Value::as_str);
            let actual = object.get("actual").and_then(serde_json::Value::as_str);
            if object.len() != 3
                || expected.is_none_or(|value| !lower_digest(value))
                || actual.is_none_or(|value| !lower_digest(value))
            {
                return Err(LegacyImportError::InternalConsistency(
                    "snapshot digest failure response is malformed".to_string(),
                ));
            }
            LegacyImportError::SnapshotDigestMismatch {
                expected: expected.unwrap().to_string(),
                actual: actual.unwrap().to_string(),
            }
        }
        _ => {
            return Err(LegacyImportError::InternalConsistency(
                "failure response status or label is invalid".to_string(),
            ))
        }
    };
    if label != "snapshot_digest_mismatch" && object.len() != 1 {
        return Err(LegacyImportError::InternalConsistency(
            "failure response has unexpected fields".to_string(),
        ));
    }
    Ok(error)
}
