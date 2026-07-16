use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::application::workflow_instance::admin_recovery::AdminEmergencyOverrideResult;
use crate::domain::definition::digest;
use crate::domain::workflow_instance::events::{
    ADMIN_EMERGENCY_OVERRIDE_COMMITTED_EVENT_TYPE, EVENT_SCHEMA_VERSION,
};
use crate::domain::workflow_instance::recovery::{
    AdminEmergencyOperation, AdminEmergencyOverrideCommand, RecoveryError,
    COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE,
};

use super::{authorization, receipt, snapshot};

async fn record_failure(
    tx: &mut Transaction<'_, Postgres>,
    command_id: Uuid,
    actor: Uuid,
    command: &AdminEmergencyOverrideCommand,
    request_hash: &str,
    error: &RecoveryError,
) -> Result<(), RecoveryError> {
    receipt::complete(
        tx,
        command_id,
        error.status_code(),
        &receipt::error_body(error),
    )
    .await?;
    receipt::write_attempt(
        tx,
        command_id,
        actor,
        &command.idempotency_key,
        "DETERMINISTIC_FAILURE",
        Some(error.label()),
        request_hash,
        &serde_json::json!({"error": error.label()}),
    )
    .await?;
    receipt::write_security(
        tx,
        actor,
        "ADMIN_EMERGENCY_OVERRIDE_REJECTED",
        command.workflow_instance_id.into_uuid(),
        &serde_json::json!({"commandId": command_id, "reason": error.label()}),
    )
    .await
}

async fn commit_failure(
    mut tx: Transaction<'_, Postgres>,
    command_id: Uuid,
    command: &AdminEmergencyOverrideCommand,
    request_hash: &str,
    error: RecoveryError,
) -> Result<AdminEmergencyOverrideResult, RecoveryError> {
    record_failure(
        &mut tx,
        command_id,
        command.principal_id.into_uuid(),
        command,
        request_hash,
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
    command: &AdminEmergencyOverrideCommand,
    request_hash: &str,
    error: RecoveryError,
) -> Result<AdminEmergencyOverrideResult, RecoveryError> {
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
        "ADMIN_EMERGENCY_OVERRIDE_REJECTED",
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    Err(error)
}

fn validate_input(command: &AdminEmergencyOverrideCommand) -> Result<(), RecoveryError> {
    if command.reason != command.reason.trim()
        || command.reason.is_empty()
        || command.reason.chars().count() > 2000
        || command.reason.chars().any(char::is_control)
    {
        return Err(RecoveryError::InvalidInput(
            "reason must contain 1..2000 printable characters without surrounding whitespace"
                .to_string(),
        ));
    }
    if command.related_references.len() > 20 {
        return Err(RecoveryError::InvalidInput(
            "related_references must contain at most 20 entries".to_string(),
        ));
    }
    for reference in &command.related_references {
        if reference.resource_type.is_empty()
            || reference.resource_type.len() > 128
            || reference.resource_id.is_empty()
            || reference.resource_id.len() > 256
            || reference.resource_type.chars().any(char::is_control)
            || reference.resource_id.chars().any(char::is_control)
        {
            return Err(RecoveryError::InvalidInput(
                "related reference fields are empty, oversized, or contain controls".to_string(),
            ));
        }
    }
    Ok(())
}

async fn read_source_node(
    tx: &mut Transaction<'_, Postgres>,
    visit_id: Uuid,
) -> Result<Uuid, RecoveryError> {
    let source_node_id: Option<Uuid> =
        sqlx::query_scalar("SELECT node_id FROM workflow_node_visits WHERE node_visit_id = $1")
            .bind(visit_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    source_node_id.ok_or_else(|| {
        RecoveryError::InternalConsistency("replayed source visit disappeared".to_string())
    })
}

pub async fn admin_emergency_override(
    pool: &PgPool,
    command: AdminEmergencyOverrideCommand,
    request_hash: &str,
) -> Result<AdminEmergencyOverrideResult, RecoveryError> {
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
        COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE,
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
    let definition_status =
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
        receipt::AcquireReceipt::Owned(value) => value,
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
            let mut result: AdminEmergencyOverrideResult = serde_json::from_value(response_body)
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

    if let Err(error) = validate_input(&command) {
        return commit_failure(tx, command_id, &command, request_hash, error).await;
    }
    if let Err(error) = authorization::validate_override_definition_status(&definition_status) {
        return commit_failure(tx, command_id, &command, request_hash, error).await;
    }
    let reconstructed = match snapshot::reconstruct_projection(&mut tx, &instance).await {
        Ok(projection) => projection,
        Err(error @ RecoveryError::InvalidImmutableFacts(_)) => {
            return commit_failure(tx, command_id, &command, request_hash, error).await
        }
        Err(error) => return Err(error),
    };
    if reconstructed != instance.projection() {
        return commit_failure(
            tx,
            command_id,
            &command,
            request_hash,
            RecoveryError::InvalidImmutableFacts(
                "instance projection does not match immutable event replay".to_string(),
            ),
        )
        .await;
    }
    if command.expected_workflow_state_version != instance.workflow_state_version {
        let error = RecoveryError::WorkflowStateVersionConflict {
            expected: command.expected_workflow_state_version,
            actual: instance.workflow_state_version,
        };
        return commit_failure(tx, command_id, &command, request_hash, error).await;
    }
    let before_snapshot_digest = snapshot::before_snapshot(&instance).digest()?;
    if let Err(error) = snapshot::verify_expected_digest(
        command.expected_before_snapshot_digest.as_deref(),
        &before_snapshot_digest,
    ) {
        return commit_failure(tx, command_id, &command, request_hash, error).await;
    }
    let Some(current_context_id) = instance.current_context_revision_id else {
        let error =
            RecoveryError::InternalConsistency("current context projection is null".to_string());
        return commit_failure(tx, command_id, &command, request_hash, error).await;
    };
    let Some(source_visit_id) = instance.current_node_visit_id else {
        let error =
            RecoveryError::InternalConsistency("current visit projection is null".to_string());
        return commit_failure(tx, command_id, &command, request_hash, error).await;
    };
    let source_node_id = match read_source_node(&mut tx, source_visit_id).await {
        Ok(value) => value,
        Err(error @ RecoveryError::InternalConsistency(_)) => {
            return commit_failure(tx, command_id, &command, request_hash, error).await
        }
        Err(error) => return Err(error),
    };
    let target = match authorization::read_target_node(
        &mut tx,
        command.target_node_id.into_uuid(),
        instance.definition_version_id,
    )
    .await
    {
        Ok(value) => value,
        Err(error @ RecoveryError::InvalidTarget(_)) => {
            return commit_failure(tx, command_id, &command, request_hash, error).await
        }
        Err(error) => return Err(error),
    };
    let (assignee, effect) = match command.operation {
        AdminEmergencyOperation::MoveToNode => {
            let assignee = match authorization::resolve_non_terminal_assignee(
                &mut tx,
                &target,
                instance.created_by_principal_id,
                instance.domain_id,
            )
            .await
            {
                Ok(value) => value,
                Err(
                    error @ (RecoveryError::InvalidTarget(_)
                    | RecoveryError::AssigneeResolutionFailed(_)),
                ) => return commit_failure(tx, command_id, &command, request_hash, error).await,
                Err(error) => return Err(error),
            };
            (Some(assignee), "ADVANCE")
        }
        AdminEmergencyOperation::TerminateInstance => {
            if target.node_type != "TERMINAL" {
                let error = RecoveryError::InvalidTarget(
                    "TERMINATE_INSTANCE target must be terminal".to_string(),
                );
                return commit_failure(tx, command_id, &command, request_hash, error).await;
            }
            (None, "TERMINATE")
        }
    };
    let maximum_visit_number: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(visit_number) FROM workflow_node_visits
         WHERE workflow_instance_id = $1 AND node_id = $2",
    )
    .bind(instance_id)
    .bind(target.node_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    let visit_number = match maximum_visit_number.unwrap_or(0).checked_add(1) {
        Some(value) => value,
        None => {
            return commit_failure(
                tx,
                command_id,
                &command,
                request_hash,
                RecoveryError::InternalConsistency("node visit number overflow".to_string()),
            )
            .await
        }
    };
    let new_state_version = match instance.workflow_state_version.checked_add(1) {
        Some(value) => value,
        None => {
            return commit_failure(
                tx,
                command_id,
                &command,
                request_hash,
                RecoveryError::InternalConsistency("workflow state version overflow".to_string()),
            )
            .await
        }
    };
    let target_visit_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number,
          assignee_principal_id, entered_by_transition_id)
         VALUES ($1, $2, $3, $4, $5, NULL)",
    )
    .bind(target_visit_id)
    .bind(instance_id)
    .bind(target.node_id)
    .bind(visit_number)
    .bind(assignee)
    .execute(&mut *tx)
    .await
    .map_err(|error| RecoveryError::StorageError(error.to_string()))?;

    let affected = sqlx::query(
        "UPDATE workflow_instances SET current_node_visit_id = $2,
             workflow_state_version = $3, updated_at = now()
         WHERE workflow_instance_id = $1 AND workflow_state_version = $4
           AND current_node_visit_id = $5",
    )
    .bind(instance_id)
    .bind(target_visit_id)
    .bind(new_state_version)
    .bind(instance.workflow_state_version)
    .bind(source_visit_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| RecoveryError::StorageError(error.to_string()))?
    .rows_affected();
    if affected != 1 {
        return Err(RecoveryError::InternalConsistency(
            "override projection update affected an unexpected row count".to_string(),
        ));
    }
    let event_data = serde_json::json!({
        "operation": command.operation.as_str(),
        "reason": command.reason,
        "relatedReferences": command.related_references,
        "beforeSnapshotDigest": before_snapshot_digest,
    });
    let event_data_digest =
        digest::compute_json_digest(&event_data).map_err(RecoveryError::StorageError)?;
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          command_id, event_type, transition_effect, source_node_visit_id,
          target_node_visit_id, context_revision_id, submission_id, event_data,
          event_data_digest, actor_principal_id, from_node_id, to_node_id,
          old_workflow_state_version, new_workflow_state_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7::transition_effect, $8, $9,
                 $10, NULL, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(Uuid::new_v4())
    .bind(instance_id)
    .bind(new_state_version)
    .bind(EVENT_SCHEMA_VERSION)
    .bind(command_id)
    .bind(ADMIN_EMERGENCY_OVERRIDE_COMMITTED_EVENT_TYPE)
    .bind(effect)
    .bind(source_visit_id)
    .bind(target_visit_id)
    .bind(current_context_id)
    .bind(&event_data)
    .bind(event_data_digest)
    .bind(actor)
    .bind(source_node_id)
    .bind(target.node_id)
    .bind(instance.workflow_state_version)
    .bind(new_state_version)
    .execute(&mut *tx)
    .await
    .map_err(|error| RecoveryError::StorageError(error.to_string()))?;

    let result = AdminEmergencyOverrideResult {
        command_id,
        workflow_instance_id: instance_id,
        source_node_visit_id: source_visit_id,
        current_node_visit_id: target_visit_id,
        workflow_state_version: new_state_version,
        event_sequence: new_state_version,
        before_snapshot_digest: snapshot::before_snapshot(&instance).digest()?,
        replayed: false,
    };
    receipt::complete(
        &mut tx,
        command_id,
        200,
        &serde_json::to_value(&result)
            .map_err(|error| RecoveryError::StorageError(error.to_string()))?,
    )
    .await?;
    receipt::write_security(
        &mut tx,
        actor,
        "ADMIN_EMERGENCY_OVERRIDE_COMMITTED",
        instance_id,
        &serde_json::json!({
            "commandId": command_id,
            "operation": command.operation.as_str(),
            "beforeSnapshotDigest": result.before_snapshot_digest,
        }),
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    Ok(result)
}
