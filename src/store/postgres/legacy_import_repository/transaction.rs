use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::application::workflow_instance::import::ImportLegacyWorkflowInstanceResult;
use crate::domain::definition::digest;
use crate::domain::workflow_instance::import::{
    ImportLegacyWorkflowInstanceCommand, LegacyImportError, COMMAND_TYPE, EVENT_TYPE,
};

use super::{receipt, replay, validation};

fn infrastructure(error: &LegacyImportError) -> bool {
    matches!(
        error,
        LegacyImportError::StorageError(_) | LegacyImportError::InternalConsistency(_)
    )
}

async fn record_failure(
    tx: &mut Transaction<'_, Postgres>,
    acquired: &receipt::Acquired,
    command: &ImportLegacyWorkflowInstanceCommand,
    request_hash: &str,
    error: &LegacyImportError,
) -> Result<(), LegacyImportError> {
    if acquired.is_owned() {
        receipt::complete(
            tx,
            acquired.command_id(),
            error.status_code(),
            &receipt::error_body(error),
        )
        .await?;
    }
    receipt::write_attempt(
        tx,
        acquired,
        command.principal_id.into_uuid(),
        &command.idempotency_key(),
        request_hash,
        "DETERMINISTIC_FAILURE",
        error,
    )
    .await
}

async fn commit_failure(
    mut tx: Transaction<'_, Postgres>,
    acquired: &receipt::Acquired,
    command: &ImportLegacyWorkflowInstanceCommand,
    request_hash: &str,
    error: LegacyImportError,
) -> Result<ImportLegacyWorkflowInstanceResult, LegacyImportError> {
    record_failure(&mut tx, acquired, command, request_hash, &error).await?;
    tx.commit()
        .await
        .map_err(|cause| LegacyImportError::StorageError(cause.to_string()))?;
    Err(error)
}

async fn insert_facts(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    command: &ImportLegacyWorkflowInstanceCommand,
    validated: validation::ValidatedImport,
) -> Result<ImportLegacyWorkflowInstanceResult, LegacyImportError> {
    let instance_id = Uuid::new_v4();
    let context_id = Uuid::new_v4();
    let visit_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let actor = command.principal_id.into_uuid();
    let imported_at: String = sqlx::query_scalar(
        "SELECT to_char(date_trunc('second', clock_timestamp()) AT TIME ZONE 'UTC',
                'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
    let payload_digest = digest::compute_json_digest(&command.legacy_snapshot.context_payload)
        .map_err(LegacyImportError::StorageError)?;
    sqlx::query(
        "INSERT INTO workflow_instances
         (workflow_instance_id, domain_id, definition_version_id,
          created_by_principal_id, workflow_state_version,
          current_context_revision_id, current_node_visit_id,
          external_reference, external_url, metadata)
         VALUES ($1, $2, $3, $4, 1, $5, $6, $7, $8, $9)",
    )
    .bind(instance_id)
    .bind(command.domain_id.into_uuid())
    .bind(command.definition_version_id.into_uuid())
    .bind(validated.creator_id)
    .bind(context_id)
    .bind(visit_id)
    .bind(command.external_reference())
    .bind(&command.external_url)
    .bind(&command.metadata)
    .execute(&mut **tx)
    .await
    .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
    sqlx::query(
        "INSERT INTO workflow_context_revisions
         (context_revision_id, workflow_instance_id, revision_number,
          previous_revision_id, payload, payload_digest, created_by_principal_id)
         VALUES ($1, $2, 1, NULL, $3, $4, $5)",
    )
    .bind(context_id)
    .bind(instance_id)
    .bind(&command.legacy_snapshot.context_payload)
    .bind(payload_digest)
    .bind(validated.creator_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number,
          assignee_principal_id, entered_by_transition_id)
         VALUES ($1, $2, $3, 1, $4, NULL)",
    )
    .bind(visit_id)
    .bind(instance_id)
    .bind(command.imported_node_id.into_uuid())
    .bind(validated.assignee_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
    let event_data = serde_json::json!({
        "legacySystem": "adc",
        "legacyRecordId": command.legacy_record_id.to_string(),
        "legacySnapshotDigest": validated.snapshot_digest,
        "importedNodeId": command.imported_node_id.to_string(),
        "importedAt": imported_at,
        "creatorResolution": validated.creator_resolution.as_str(),
    });
    let event_digest =
        digest::compute_json_digest(&event_data).map_err(LegacyImportError::StorageError)?;
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          command_id, event_type, transition_effect, source_node_visit_id,
          target_node_visit_id, context_revision_id, submission_id, event_data,
          event_data_digest, actor_principal_id, from_node_id, to_node_id,
          old_workflow_state_version, new_workflow_state_version)
         VALUES ($1, $2, 1, 'v1', $3, $4, NULL, NULL, $5, $6, NULL, $7, $8,
                 $9, NULL, NULL, 0, 1)",
    )
    .bind(event_id)
    .bind(instance_id)
    .bind(command_id)
    .bind(EVENT_TYPE)
    .bind(visit_id)
    .bind(context_id)
    .bind(&event_data)
    .bind(event_digest)
    .bind(actor)
    .execute(&mut **tx)
    .await
    .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
    Ok(ImportLegacyWorkflowInstanceResult {
        command_id,
        workflow_instance_id: instance_id,
        current_context_revision_id: context_id,
        current_node_visit_id: visit_id,
        event_id,
        workflow_state_version: 1,
        event_sequence: 1,
        legacy_snapshot_digest: command.expected_legacy_snapshot_digest.clone(),
        creator_resolution: validated.creator_resolution,
        replayed: false,
    })
}

pub async fn import(
    pool: &PgPool,
    command: ImportLegacyWorkflowInstanceCommand,
    request_hash: &str,
) -> Result<ImportLegacyWorkflowInstanceResult, LegacyImportError> {
    let actor = command.principal_id.into_uuid();
    let key = command.idempotency_key();
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
    let acquired = receipt::acquire(&mut tx, actor, &key, COMMAND_TYPE, request_hash).await?;
    let access = match validation::validate_access(&mut tx, &command).await {
        Ok(value) => value,
        Err(error) if !infrastructure(&error) => {
            return commit_failure(tx, &acquired, &command, request_hash, error).await
        }
        Err(error) => return Err(error),
    };
    match acquired {
        receipt::Acquired::Replay(receipt) => {
            let mut result = if receipt.response_status == Some(200) {
                replay::replay_success(&mut tx, &receipt, &command.expected_legacy_snapshot_digest)
                    .await?
            } else {
                return Err(receipt::error_from_receipt(&receipt)?);
            };
            tx.commit()
                .await
                .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
            result.replayed = true;
            Ok(result)
        }
        acquired @ receipt::Acquired::Conflict(_) => {
            let error = LegacyImportError::IdempotencyConflict;
            receipt::write_attempt(
                &mut tx,
                &acquired,
                actor,
                &key,
                request_hash,
                "IDEMPOTENCY_CONFLICT",
                &error,
            )
            .await?;
            tx.commit()
                .await
                .map_err(|cause| LegacyImportError::StorageError(cause.to_string()))?;
            Err(error)
        }
        acquired @ receipt::Acquired::Processing(_) => {
            let error = LegacyImportError::CommandStillProcessing;
            receipt::write_attempt(
                &mut tx,
                &acquired,
                actor,
                &key,
                request_hash,
                "STILL_PROCESSING",
                &error,
            )
            .await?;
            tx.commit()
                .await
                .map_err(|cause| LegacyImportError::StorageError(cause.to_string()))?;
            Err(error)
        }
        acquired @ receipt::Acquired::Owned(command_id) => {
            let validated = match validation::validate_owned(&mut tx, &command, access).await {
                Ok(value) => value,
                Err(error) if !infrastructure(&error) => {
                    return commit_failure(tx, &acquired, &command, request_hash, error).await
                }
                Err(error) => return Err(error),
            };
            if let Err(error) = validation::validate_external_reference_absent(
                &mut tx,
                &command.external_reference(),
            )
            .await
            {
                return commit_failure(tx, &acquired, &command, request_hash, error).await;
            }
            let result = insert_facts(&mut tx, command_id, &command, validated).await?;
            let body = serde_json::to_value(&result)
                .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
            receipt::complete(&mut tx, command_id, 200, &body).await?;
            receipt::write_security(
                &mut tx,
                actor,
                result.workflow_instance_id,
                &serde_json::json!({
                    "commandId": command_id,
                    "legacySystem": "adc",
                    "legacyRecordId": command.legacy_record_id,
                    "legacySnapshotDigest": result.legacy_snapshot_digest,
                }),
            )
            .await?;
            tx.commit()
                .await
                .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
            Ok(result)
        }
    }
}
