//! Atomic workflow context revision transaction.
//!
//! Implements the core atomic transaction that:
//! 1. Handles idempotency (CommandReceipt)
//! 2. Locks the WorkflowInstance
//! 3. Validates Creator-only + DRAFT-only + state version
//! 4. Creates a new WorkflowContextRevision #N+1
//! 5. Updates instance projection
//! 6. Creates a CONTEXT_REVISED WorkflowEvent
//! 7. Completes the CommandReceipt

use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::workflow_instance::commands::ReviseWorkflowContextCommand;
use crate::domain::workflow_instance::errors::CreateWorkflowInstanceError;
use crate::domain::workflow_instance::errors::ReviseWorkflowContextError;
use crate::domain::workflow_instance::events::{
    ContextRevisedEventData, COMMAND_TYPE_REVISE_CONTEXT, CONTEXT_REVISED_EVENT_TYPE,
    EVENT_SCHEMA_VERSION,
};

use super::command_receipt::{
    self, complete_receipt, try_insert_receipt, write_attempt_audit, ReceiptReplayResult,
};
use super::revise_validation;

/// Convert a CreateWorkflowInstanceError to ReviseWorkflowContextError.
fn map_create_err(e: CreateWorkflowInstanceError) -> ReviseWorkflowContextError {
    match &e {
        CreateWorkflowInstanceError::StorageError(s) => {
            ReviseWorkflowContextError::StorageError(s.clone())
        }
        _ => ReviseWorkflowContextError::StorageError(e.to_string()),
    }
}

/// Outcome of an atomic revision attempt.
pub(crate) enum ReviseOutcome {
    /// Fresh successful revision.
    Revised(ReviseResult),
    /// Idempotent replay of a successful request.
    Replayed(ReviseResult),
    /// Idempotent replay of a failed request.
    ReplayedFailure(i32, serde_json::Value),
}

/// Result of a successful atomic revision.
pub(crate) struct ReviseResult {
    pub workflow_instance_id: Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub event_sequence: i32,
}

/// Execute the full atomic revision workflow inside a single transaction.
pub(crate) async fn revise_workflow_context_atomically(
    pool: &PgPool,
    cmd: ReviseWorkflowContextCommand,
    request_hash: &str,
) -> Result<ReviseOutcome, ReviseWorkflowContextError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    // Pre-generate all IDs
    let command_id = Uuid::new_v4();
    let new_context_revision_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();

    let principal_uuid = cmd.principal_id.into_uuid();
    let instance_uuid = cmd.workflow_instance_id.into_uuid();

    // ---------------------------------------------------------------
    // Step 1: Insert command receipt (idempotency gate)
    // ---------------------------------------------------------------
    let receipt_owned = try_insert_receipt(
        &mut tx,
        command_id,
        principal_uuid,
        &cmd.idempotency_key,
        COMMAND_TYPE_REVISE_CONTEXT,
        request_hash,
    )
    .await
    .map_err(map_create_err)?;

    let actual_command_id: Uuid = match receipt_owned {
        Some(cmd_id) => cmd_id,
        None => {
            let replay = command_receipt::replay_existing_receipt(
                &mut tx,
                principal_uuid,
                &cmd.idempotency_key,
                request_hash,
            )
            .await
            .map_err(map_create_err)?;

            match replay {
                ReceiptReplayResult::CompletedMatch {
                    command_id: _,
                    response_status,
                    response_body,
                } => {
                    if response_status != 200 {
                        tx.commit()
                            .await
                            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
                        return Ok(ReviseOutcome::ReplayedFailure(
                            response_status,
                            response_body,
                        ));
                    }

                    let wf_id = response_body["workflowInstanceId"]
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .ok_or_else(|| {
                            ReviseWorkflowContextError::StorageError(
                                "stored response missing workflowInstanceId".to_string(),
                            )
                        })?;
                    let ctx_rev_id = response_body["currentContextRevisionId"]
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .ok_or_else(|| {
                            ReviseWorkflowContextError::StorageError(
                                "stored response missing currentContextRevisionId".to_string(),
                            )
                        })?;
                    let visit_id = response_body["currentNodeVisitId"]
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .ok_or_else(|| {
                            ReviseWorkflowContextError::StorageError(
                                "stored response missing currentNodeVisitId".to_string(),
                            )
                        })?;
                    let state_ver =
                        response_body["workflowStateVersion"].as_i64().unwrap_or(1) as i32;
                    let ev_seq = response_body["eventSequence"].as_i64().unwrap_or(1) as i32;

                    tx.commit()
                        .await
                        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

                    return Ok(ReviseOutcome::Replayed(ReviseResult {
                        workflow_instance_id: wf_id,
                        workflow_state_version: state_ver,
                        current_context_revision_id: ctx_rev_id,
                        current_node_visit_id: visit_id,
                        event_sequence: ev_seq,
                    }));
                }
                ReceiptReplayResult::CompletedConflict {
                    command_id: cid,
                    original_request_hash: orig_hash,
                } => {
                    let audit_id = Uuid::new_v4();
                    let details = serde_json::json!({
                        "conflictType": "IDEMPOTENCY_KEY_MISMATCH",
                        "originalRequestHash": orig_hash,
                        "newRequestHash": request_hash,
                    });
                    write_attempt_audit(
                        &mut tx,
                        audit_id,
                        cid,
                        principal_uuid,
                        &cmd.idempotency_key,
                        "IDEMPOTENCY_CONFLICT",
                        Some("request hash mismatch"),
                        request_hash,
                        Some(&details),
                    )
                    .await
                    .map_err(map_create_err)?;

                    tx.commit()
                        .await
                        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

                    return Err(ReviseWorkflowContextError::IdempotencyConflict {
                        original_command_id: cid,
                        original_request_hash: orig_hash,
                    });
                }
                ReceiptReplayResult::StillProcessing => {
                    tx.commit()
                        .await
                        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
                    return Err(ReviseWorkflowContextError::CommandStillProcessing);
                }
            }
        }
    };

    // ---------------------------------------------------------------
    // Step 2: Lock the workflow instance
    // ---------------------------------------------------------------
    let instance = revise_validation::lock_instance(&mut tx, instance_uuid).await?;

    // Step 3: Validate caller is the Workflow Creator
    if instance.created_by_principal_id != principal_uuid {
        let response_body = serde_json::json!({"error": "principal_not_found"});
        let response_digest = digest::compute_sha256(b"principal_not_found");
        complete_receipt(
            &mut tx,
            actual_command_id,
            404,
            &response_body,
            &response_digest,
        )
        .await
        .map_err(map_create_err)?;
        tx.commit()
            .await
            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
        return Err(ReviseWorkflowContextError::PrincipalNotFound);
    }

    // Step 4: Validate principal enabled (inside tx for consistency)
    if let Some(err) =
        revise_validation::validate_principal_enabled(&mut tx, principal_uuid).await?
    {
        let status_code = crate::domain::workflow_instance::errors::revise_error_code(&err);
        let error_code = crate::domain::workflow_instance::errors::revise_error_label(&err);
        let response_body = serde_json::json!({"error": error_code});
        let response_digest = digest::compute_sha256(error_code.as_bytes());
        complete_receipt(
            &mut tx,
            actual_command_id,
            status_code,
            &response_body,
            &response_digest,
        )
        .await
        .map_err(map_create_err)?;
        tx.commit()
            .await
            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
        return Err(err);
    }

    // Step 5: Validate expected workflow state version
    if instance.workflow_state_version != cmd.expected_workflow_state_version {
        let response_body = serde_json::json!({
            "error": "workflow_state_version_conflict",
            "expected": cmd.expected_workflow_state_version,
            "actual": instance.workflow_state_version,
        });
        let response_digest = digest::compute_sha256(b"workflow_state_version_conflict");
        complete_receipt(
            &mut tx,
            actual_command_id,
            409,
            &response_body,
            &response_digest,
        )
        .await
        .map_err(map_create_err)?;
        tx.commit()
            .await
            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
        return Err(ReviseWorkflowContextError::WorkflowStateVersionConflict {
            expected: cmd.expected_workflow_state_version,
            actual: instance.workflow_state_version,
        });
    }

    // Step 6: Validate definition version status
    if let Err(err) = revise_validation::validate_definition_version_status(
        &mut tx,
        instance.definition_version_id,
    )
    .await
    {
        let status_code = crate::domain::workflow_instance::errors::revise_error_code(&err);
        let error_code = crate::domain::workflow_instance::errors::revise_error_label(&err);
        let response_body = serde_json::json!({"error": error_code});
        let response_digest = digest::compute_sha256(error_code.as_bytes());
        complete_receipt(
            &mut tx,
            actual_command_id,
            status_code,
            &response_body,
            &response_digest,
        )
        .await
        .map_err(map_create_err)?;
        tx.commit()
            .await
            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
        return Err(err);
    }

    // Step 7: Validate current node visit is DRAFT
    let current_visit = revise_validation::validate_current_visit(
        &mut tx,
        instance_uuid,
        instance.current_node_visit_id,
    )
    .await?;

    // Step 8: Read current context revision
    let current_context = revise_validation::read_current_context(
        &mut tx,
        instance_uuid,
        instance.current_context_revision_id,
    )
    .await?;

    // Step 9: Validate context payload against schema
    let schema_row: Option<(Option<serde_json::Value>,)> = sqlx::query_as(
        "SELECT context_schema FROM workflow_definition_versions \
         WHERE definition_version_id = $1",
    )
    .bind(instance.definition_version_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e: sqlx::Error| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    let context_schema = schema_row.and_then(|(s,)| s);

    if let Err(err) = revise_validation::validate_context_schema(&context_schema, &cmd) {
        let status_code = crate::domain::workflow_instance::errors::revise_error_code(&err);
        let error_code = crate::domain::workflow_instance::errors::revise_error_label(&err);
        let response_body = serde_json::json!({"error": error_code});
        let response_digest = digest::compute_sha256(error_code.as_bytes());
        complete_receipt(
            &mut tx,
            actual_command_id,
            status_code,
            &response_body,
            &response_digest,
        )
        .await
        .map_err(map_create_err)?;
        tx.commit()
            .await
            .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
        return Err(err);
    }

    // ---------------------------------------------------------------
    // Step 10: Compute digests
    // ---------------------------------------------------------------
    let new_payload_digest = digest::compute_json_digest(&cmd.context_payload)
        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
    let new_revision_number = current_context.revision_number + 1;
    let old_state_version = instance.workflow_state_version;
    let new_state_version = old_state_version + 1;
    let event_sequence = new_state_version;

    // ---------------------------------------------------------------
    // Step 11: Insert new WorkflowContextRevision
    // ---------------------------------------------------------------
    sqlx::query(
        r#"
        INSERT INTO workflow_context_revisions
            (context_revision_id, workflow_instance_id, revision_number,
             previous_revision_id, payload, payload_digest,
             created_by_principal_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(new_context_revision_id)
    .bind(instance_uuid)
    .bind(new_revision_number)
    .bind(current_context.context_revision_id)
    .bind(&cmd.context_payload)
    .bind(&new_payload_digest)
    .bind(principal_uuid)
    .execute(&mut *tx)
    .await
    .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    // ---------------------------------------------------------------
    // Step 12: Update instance projection
    // ---------------------------------------------------------------
    sqlx::query(
        "UPDATE workflow_instances \
         SET current_context_revision_id = $1, workflow_state_version = $2 \
         WHERE workflow_instance_id = $3",
    )
    .bind(new_context_revision_id)
    .bind(new_state_version)
    .bind(instance_uuid)
    .execute(&mut *tx)
    .await
    .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    // ---------------------------------------------------------------
    // Step 13: Insert CONTEXT_REVISED event
    // ---------------------------------------------------------------
    let event_data = ContextRevisedEventData {
        previous_context_revision_id: current_context.context_revision_id.to_string(),
        new_context_revision_id: new_context_revision_id.to_string(),
        previous_payload_digest: current_context.payload_digest.clone(),
        new_payload_digest: new_payload_digest.clone(),
        current_node_id: current_visit.node_id.to_string(),
    };

    let event_data_json = serde_json::to_value(&event_data)
        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;
    let event_data_digest = digest::compute_json_digest(&event_data_json)
        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             command_id, event_type, source_node_visit_id, target_node_visit_id,
             context_revision_id, event_data, event_data_digest,
             actor_principal_id, old_workflow_state_version, new_workflow_state_version)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(event_id)
    .bind(instance_uuid)
    .bind(event_sequence)
    .bind(EVENT_SCHEMA_VERSION)
    .bind(actual_command_id)
    .bind(CONTEXT_REVISED_EVENT_TYPE)
    .bind(instance.current_node_visit_id)
    .bind(instance.current_node_visit_id)
    .bind(new_context_revision_id)
    .bind(&event_data_json)
    .bind(&event_data_digest)
    .bind(principal_uuid)
    .bind(old_state_version)
    .bind(new_state_version)
    .execute(&mut *tx)
    .await
    .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    // ---------------------------------------------------------------
    // Step 14: Complete the command receipt
    // ---------------------------------------------------------------
    let response_body = serde_json::json!({
        "workflowInstanceId": instance_uuid,
        "workflowStateVersion": new_state_version,
        "currentContextRevisionId": new_context_revision_id,
        "currentNodeVisitId": instance.current_node_visit_id,
        "eventSequence": event_sequence,
    });

    let response_digest = digest::compute_json_digest(&response_body)
        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    complete_receipt(
        &mut tx,
        actual_command_id,
        200,
        &response_body,
        &response_digest,
    )
    .await
    .map_err(map_create_err)?;

    // ---------------------------------------------------------------
    // Step 15: Commit
    // ---------------------------------------------------------------
    tx.commit()
        .await
        .map_err(|e| ReviseWorkflowContextError::StorageError(e.to_string()))?;

    Ok(ReviseOutcome::Revised(ReviseResult {
        workflow_instance_id: instance_uuid,
        workflow_state_version: new_state_version,
        current_context_revision_id: new_context_revision_id,
        current_node_visit_id: instance.current_node_visit_id,
        event_sequence,
    }))
}
