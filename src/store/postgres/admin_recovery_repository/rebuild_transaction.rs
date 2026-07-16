use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::application::workflow_instance::admin_recovery::RebuildProjectionResult;
use crate::domain::workflow_instance::recovery::{
    RebuildProjectionCommand, RecoveryError, COMMAND_TYPE_REBUILD_PROJECTION,
};

use super::{authorization, receipt, snapshot};

async fn record_failure(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    actor: Uuid,
    idempotency_key: &str,
    request_hash: &str,
    instance_id: Uuid,
    error: &RecoveryError,
) -> Result<(), RecoveryError> {
    let body = receipt::error_body(error);
    receipt::complete(tx, command_id, error.status_code(), &body).await?;
    receipt::write_attempt(
        tx,
        command_id,
        actor,
        idempotency_key,
        "DETERMINISTIC_FAILURE",
        Some(error.label()),
        request_hash,
        &serde_json::json!({"error": error.label()}),
    )
    .await?;
    receipt::write_security(
        tx,
        actor,
        "REBUILD_PROJECTION_REJECTED",
        instance_id,
        &serde_json::json!({"commandId": command_id, "reason": error.label()}),
    )
    .await
}

async fn commit_failure(
    mut tx: Transaction<'_, Postgres>,
    command_id: Uuid,
    command: &RebuildProjectionCommand,
    request_hash: &str,
    error: RecoveryError,
) -> Result<RebuildProjectionResult, RecoveryError> {
    record_failure(
        &mut tx,
        command_id,
        command.principal_id.into_uuid(),
        &command.idempotency_key,
        request_hash,
        command.workflow_instance_id.into_uuid(),
        &error,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|cause| RecoveryError::StorageError(cause.to_string()))?;
    Err(error)
}

async fn deny_access(
    mut tx: Transaction<'_, Postgres>,
    acquired: &receipt::AcquireReceipt,
    command: &RebuildProjectionCommand,
    request_hash: &str,
    error: RecoveryError,
) -> Result<RebuildProjectionResult, RecoveryError> {
    if acquired.is_owned() {
        return commit_failure(tx, acquired.command_id(), command, request_hash, error).await;
    }
    receipt::record_existing_denial(
        &mut tx,
        acquired,
        command.principal_id.into_uuid(),
        &command.idempotency_key,
        request_hash,
        command.workflow_instance_id.into_uuid(),
        "REBUILD_PROJECTION_REJECTED",
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    Err(error)
}

pub async fn rebuild_projection(
    pool: &PgPool,
    command: RebuildProjectionCommand,
    request_hash: &str,
) -> Result<RebuildProjectionResult, RecoveryError> {
    let actor = command.principal_id.into_uuid();
    let instance_id = command.workflow_instance_id.into_uuid();
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    let acquired = receipt::acquire(
        &mut tx,
        actor,
        &command.idempotency_key,
        COMMAND_TYPE_REBUILD_PROJECTION,
        request_hash,
    )
    .await?;

    let instance = match snapshot::lock_instance(&mut tx, instance_id).await {
        Ok(instance) => instance,
        Err(RecoveryError::InstanceNotFound) => {
            return deny_access(
                tx,
                &acquired,
                &command,
                request_hash,
                RecoveryError::PermissionDenied,
            )
            .await
        }
        Err(error) => return Err(error),
    };
    authorization::lock_definition_version_any(&mut tx, instance.definition_version_id).await?;
    let access = match authorization::validate_actor(&mut tx, actor).await {
        Ok(()) => authorization::validate_workflow_admin(&mut tx, actor, instance.domain_id).await,
        Err(error) => Err(error),
    };
    if let Err(error) = access {
        if matches!(error, RecoveryError::StorageError(_)) {
            return Err(error);
        }
        return deny_access(tx, &acquired, &command, request_hash, error).await;
    }

    let command_id = match acquired {
        receipt::AcquireReceipt::Owned(command_id) => command_id,
        receipt::AcquireReceipt::Replay {
            command_id,
            response_status,
            response_body,
        } => {
            tx.commit()
                .await
                .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
            if response_status != 200 {
                return Err(receipt::error_from_body(&response_body));
            }
            let mut result: RebuildProjectionResult = serde_json::from_value(response_body)
                .map_err(|error| RecoveryError::InternalConsistency(error.to_string()))?;
            if result.command_id != command_id {
                return Err(RecoveryError::InternalConsistency(
                    "receipt response command id mismatch".to_string(),
                ));
            }
            result.replayed = true;
            return Ok(result);
        }
        acquired @ (receipt::AcquireReceipt::Conflict { .. }
        | receipt::AcquireReceipt::Processing { .. }) => {
            let error = receipt::record_conflict_or_processing(
                &mut tx,
                &acquired,
                actor,
                &command.idempotency_key,
                request_hash,
            )
            .await?;
            tx.commit()
                .await
                .map_err(|cause| RecoveryError::StorageError(cause.to_string()))?;
            return Err(error);
        }
    };
    let before_projection = instance.projection();
    let before_snapshot_digest = snapshot::before_snapshot(&instance).digest()?;
    if let Err(error) = snapshot::verify_expected_digest(
        command.expected_before_snapshot_digest.as_deref(),
        &before_snapshot_digest,
    ) {
        return commit_failure(tx, command_id, &command, request_hash, error).await;
    }
    let after_projection = match snapshot::reconstruct_projection(&mut tx, &instance).await {
        Ok(projection) => projection,
        Err(error @ RecoveryError::InvalidImmutableFacts(_)) => {
            return commit_failure(tx, command_id, &command, request_hash, error).await
        }
        Err(error) => return Err(error),
    };
    let changed = before_projection != after_projection;
    if changed {
        let affected = sqlx::query(
            "UPDATE workflow_instances
             SET current_context_revision_id = $2, current_node_visit_id = $3,
                 workflow_state_version = $4, updated_at = now()
             WHERE workflow_instance_id = $1",
        )
        .bind(instance_id)
        .bind(after_projection.current_context_revision_id)
        .bind(after_projection.current_node_visit_id)
        .bind(after_projection.workflow_state_version)
        .execute(&mut *tx)
        .await
        .map_err(|error| RecoveryError::StorageError(error.to_string()))?
        .rows_affected();
        if affected != 1 {
            return Err(RecoveryError::InternalConsistency(
                "projection rebuild updated an unexpected row count".to_string(),
            ));
        }
    }
    let result = RebuildProjectionResult {
        command_id,
        workflow_instance_id: instance_id,
        before_projection,
        after_projection,
        before_snapshot_digest: before_snapshot_digest.clone(),
        changed,
        replayed: false,
    };
    let body = serde_json::to_value(&result)
        .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    receipt::complete(&mut tx, command_id, 200, &body).await?;
    receipt::write_security(
        &mut tx,
        actor,
        "REBUILD_PROJECTION_COMMITTED",
        instance_id,
        &serde_json::json!({
            "commandId": command_id,
            "beforeSnapshotDigest": before_snapshot_digest,
            "changed": changed,
        }),
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    Ok(result)
}
