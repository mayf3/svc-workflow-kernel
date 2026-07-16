//! Atomic workflow transition execution transaction.
//!
//! Implements the core atomic transaction that:
//! 1. Handles idempotency (CommandReceipt)
//! 2. Locks the WorkflowInstance
//! 3. Validates assignee, state version, definition version, transition validity
//! 4. Handles optional/required submission (schema validation, size limits, RETURN refs)
//! 5. Creates target NodeVisit with resolved assignee
//! 6. Updates instance projection (current_node_visit_id, state_version)
//! 7. Creates WORKFLOW_TRANSITION_COMMITTED WorkflowEvent
//! 8. Completes the CommandReceipt

use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::enums::NodeType;
use crate::domain::workflow_instance::commands::ExecuteWorkflowTransitionCommand;
use crate::domain::workflow_instance::errors::ExecuteWorkflowTransitionError;

use super::transition_helpers::{self};
pub(crate) use super::transition_helpers::{TransitionOutcome, TransitionResult};
use super::transition_receipt::{
    self, complete_transition_receipt, try_insert_transition_receipt,
    write_transition_attempt_audit, TransitionReplayResult,
};
use super::transition_validation;
use super::transition_validation::{
    lock_instance, read_current_visit, read_source_node, read_target_node, read_transition,
    resolve_assignee, validate_definition_version_status, validate_principal_enabled,
    validate_return_references, validate_submission_schema, validate_submission_size,
};

/// Execute the full atomic transition workflow inside a single transaction.
///
/// This implements the complete workflow transition per PR 3C:
/// ADVANCE, RETURN, or TERMINATE with optional submission handling.
pub(crate) async fn execute_workflow_transition_atomically(
    pool: &PgPool,
    cmd: ExecuteWorkflowTransitionCommand,
    request_hash: &str,
) -> Result<TransitionOutcome, ExecuteWorkflowTransitionError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    let principal_uuid = cmd.principal_id.into_uuid();
    let instance_uuid = cmd.workflow_instance_id.into_uuid();
    let transition_uuid = cmd.transition_definition_id.into_uuid();
    let command_id = Uuid::new_v4();
    let new_node_visit_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let submission_id = Uuid::new_v4();
    let audit_id = Uuid::new_v4();
    // ---------------------------------------------------------------
    // Step 1: Insert command receipt (idempotency gate)
    // ---------------------------------------------------------------
    let receipt_owned = try_insert_transition_receipt(
        &mut tx,
        command_id,
        principal_uuid,
        &cmd.idempotency_key,
        request_hash,
    )
    .await?;

    let _actual_command_id: Uuid = match receipt_owned {
        Some(cmd_id) => cmd_id,
        None => {
            // Must use explicit match (not ?) to commit tx on both Ok and Err
            let replay_result = transition_helpers::handle_receipt_replay(
                &mut tx,
                principal_uuid,
                &cmd.idempotency_key,
                request_hash,
                audit_id,
                command_id,
            )
            .await;

            match replay_result {
                Ok(Some(TransitionOutcome::Replayed(result))) => {
                    tx.commit()
                        .await
                        .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;
                    return Ok(TransitionOutcome::Replayed(result));
                }
                Ok(Some(TransitionOutcome::ReplayedFailure(status, body))) => {
                    tx.commit()
                        .await
                        .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;
                    return Ok(TransitionOutcome::ReplayedFailure(status, body));
                }
                Ok(_) => {
                    tx.commit()
                        .await
                        .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;
                    return Err(ExecuteWorkflowTransitionError::InternalConsistency(
                        "unexpected replay state".to_string(),
                    ));
                }
                Err(e) => {
                    // IdempotencyConflict or StillProcessing — commit so audit persists
                    tx.commit().await.map_err(|e2| {
                        ExecuteWorkflowTransitionError::StorageError(e2.to_string())
                    })?;
                    return Err(e);
                }
            }
        }
    };

    // Every deterministic check below runs before the first runtime fact write.
    macro_rules! deterministic_failure {
        ($err:expr) => {{
            let err = $err;
            transition_validation::persist_deterministic_failure(tx, _actual_command_id, &err)
                .await?;
            return Err(err);
        }};
    }
    macro_rules! validation_result {
        ($result:expr) => {{
            match $result {
                Ok(value) => value,
                Err(err) if transition_validation::is_deterministic_error(&err) => {
                    deterministic_failure!(err)
                }
                Err(err) => return Err(err),
            }
        }};
    }

    // ---------------------------------------------------------------
    // Step 2: Lock WorkflowInstance FOR UPDATE
    // ---------------------------------------------------------------
    let instance = validation_result!(lock_instance(&mut tx, instance_uuid).await);

    // Read domain_id for assignee resolution
    let domain_row: Option<(Uuid,)> =
        sqlx::query_as("SELECT domain_id FROM workflow_instances WHERE workflow_instance_id = $1")
            .bind(instance_uuid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    let domain_uuid = domain_row
        .ok_or_else(|| {
            ExecuteWorkflowTransitionError::InternalConsistency(
                "instance has no domain".to_string(),
            )
        })?
        .0;

    // ---------------------------------------------------------------
    // Step 3: Validate expectedWorkflowStateVersion
    // ---------------------------------------------------------------
    if cmd.expected_workflow_state_version != instance.workflow_state_version {
        deterministic_failure!(
            ExecuteWorkflowTransitionError::WorkflowStateVersionConflict {
                expected: cmd.expected_workflow_state_version,
                actual: instance.workflow_state_version,
            }
        );
    }

    // ---------------------------------------------------------------
    // Step 4: Read and validate current NodeVisit
    // ---------------------------------------------------------------
    let current_visit = validation_result!(
        read_current_visit(&mut tx, instance_uuid, instance.current_node_visit_id).await
    );
    // ---------------------------------------------------------------
    // Step 5: A Terminal visit is canonically unassigned and cannot transition.
    // ---------------------------------------------------------------
    if current_visit.node_type_enum() == NodeType::TERMINAL {
        deterministic_failure!(ExecuteWorkflowTransitionError::SourceNodeTerminal);
    }

    // ---------------------------------------------------------------
    // Step 6: Validate caller principal enabled
    // ---------------------------------------------------------------
    if let Some(err) = validation_result!(validate_principal_enabled(&mut tx, principal_uuid).await)
    {
        deterministic_failure!(err);
    }

    // ---------------------------------------------------------------
    // Step 7: Validate caller is current visit assignee
    // ---------------------------------------------------------------
    if current_visit.assignee_principal_id != Some(principal_uuid) {
        deterministic_failure!(ExecuteWorkflowTransitionError::PrincipalNotAssignee);
    }

    // ---------------------------------------------------------------
    // Step 8: Validate Definition Version status (with FOR UPDATE lock)
    // ---------------------------------------------------------------
    validation_result!(
        validate_definition_version_status(&mut tx, instance.definition_version_id).await
    );

    // ---------------------------------------------------------------
    // Step 9: Read source node definition (for primary check)
    // ---------------------------------------------------------------
    let source_node = validation_result!(
        read_source_node(
            &mut tx,
            current_visit.node_id,
            instance.definition_version_id,
        )
        .await
    );

    // ---------------------------------------------------------------
    // Step 10: Read and validate TransitionDefinition
    // ---------------------------------------------------------------
    let transition = validation_result!(
        read_transition(&mut tx, transition_uuid, instance.definition_version_id).await
    );

    // Validate transition source matches current node
    if transition.source_node_id != current_visit.node_id {
        deterministic_failure!(ExecuteWorkflowTransitionError::TransitionNotApplicable(
            "transition source node does not match current visit node".to_string(),
        ));
    }

    // Validate transition effect and primary constraint
    let effect = transition.transition_effect.clone();
    match effect.as_str() {
        "ADVANCE" => {
            // Verify this is the primary ADVANCE transition
            let primary_id = source_node.primary_advance_transition_id.ok_or_else(|| {
                ExecuteWorkflowTransitionError::InternalConsistency(
                    "source node has no primary advance transition".to_string(),
                )
            })?;
            if transition.transition_id != primary_id {
                deterministic_failure!(ExecuteWorkflowTransitionError::TransitionNotApplicable(
                    "ADVANCE must use the primary advance transition".to_string(),
                ));
            }
        }
        "RETURN" => {
            // Verify it's NOT the primary ADVANCE transition
            if let Some(primary_id) = source_node.primary_advance_transition_id {
                if transition.transition_id == primary_id {
                    deterministic_failure!(
                        ExecuteWorkflowTransitionError::TransitionNotApplicable(
                            "RETURN transition must not be the primary advance".to_string(),
                        )
                    );
                }
            }
        }
        "TERMINATE" => {
            // Verify it's NOT the primary ADVANCE transition
            if let Some(primary_id) = source_node.primary_advance_transition_id {
                if transition.transition_id == primary_id {
                    deterministic_failure!(
                        ExecuteWorkflowTransitionError::TransitionNotApplicable(
                            "TERMINATE transition must not be the primary advance".to_string(),
                        )
                    );
                }
            }
        }
        _ => {
            deterministic_failure!(ExecuteWorkflowTransitionError::TransitionNotApplicable(
                format!("unknown transition effect: {}", effect)
            ));
        }
    }

    // ---------------------------------------------------------------
    // Step 11: Read target node and validate node type vs effect
    // ---------------------------------------------------------------
    let target_node = validation_result!(
        read_target_node(
            &mut tx,
            transition.target_node_id,
            instance.definition_version_id,
        )
        .await
    );

    match effect.as_str() {
        "ADVANCE" => {
            // ADVANCE allows any non-TERMINAL target (including normal completion)
            // If target is TERMINAL, it's a normal completion via primary ADVANCE
            // Always allowed for ADVANCE
        }
        "RETURN" => {
            // Target must be non-TERMINAL and have order_index < source
            if target_node.node_type_enum() == NodeType::TERMINAL {
                deterministic_failure!(ExecuteWorkflowTransitionError::TransitionNotApplicable(
                    "RETURN target must not be a TERMINAL node".to_string(),
                ));
            }
            if target_node.order_index >= source_node.order_index {
                deterministic_failure!(ExecuteWorkflowTransitionError::TransitionNotApplicable(
                    "RETURN target must have lower order_index than source".to_string(),
                ));
            }
        }
        "TERMINATE" => {
            // Target must be TERMINAL
            if target_node.node_type_enum() != NodeType::TERMINAL {
                deterministic_failure!(ExecuteWorkflowTransitionError::TransitionNotApplicable(
                    "TERMINATE target must be a TERMINAL node".to_string(),
                ));
            }
        }
        _ => {
            deterministic_failure!(ExecuteWorkflowTransitionError::TransitionNotApplicable(
                format!("unknown transition effect: {}", effect)
            ));
        }
    }

    // ---------------------------------------------------------------
    // Step 12: Handle Submission
    // ---------------------------------------------------------------
    let has_submission_schema = transition.submission_schema.is_some();
    let final_submission_id: Option<Uuid>;

    match (&cmd.submission_payload, has_submission_schema) {
        (None, true) => {
            // Schema exists but no payload — submission required
            deterministic_failure!(ExecuteWorkflowTransitionError::SubmissionRequired);
        }
        (None, false) => {
            // No schema, no payload — no submission
            final_submission_id = None;
        }
        (Some(payload), _) => {
            // Payload provided — validate and create submission

            // Size check
            validation_result!(validate_submission_size(payload));

            // Schema validation
            validation_result!(validate_submission_schema(
                &transition.submission_schema,
                payload,
            ));

            // RETURN-specific reference validation
            if effect == "RETURN" {
                validation_result!(
                    validate_return_references(&mut tx, payload, instance_uuid,).await
                );
            }
            final_submission_id = Some(submission_id);
        }
    }

    // ---------------------------------------------------------------
    // Step 13: Resolve target assignee
    // ---------------------------------------------------------------
    let target_assignee_id =
        validation_result!(resolve_assignee(&mut tx, &target_node, &instance, domain_uuid).await);

    // ---------------------------------------------------------------
    // Step 14: Compute target visit_number
    // ---------------------------------------------------------------
    let visit_number: (Option<i32>,) = sqlx::query_as(
        "SELECT COALESCE(MAX(visit_number), 0) + 1 \
         FROM workflow_node_visits \
         WHERE workflow_instance_id = $1 AND node_id = $2",
    )
    .bind(instance_uuid)
    .bind(target_node.node_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    let target_visit_number = visit_number.0.unwrap_or(1);

    // All deterministic validation is complete. Runtime fact writes start here;
    // any following error rolls back the receipt and all partial facts.
    if let Some(payload) = &cmd.submission_payload {
        transition_helpers::insert_submission(
            &mut tx,
            submission_id,
            instance_uuid,
            instance.current_node_visit_id,
            instance.current_context_revision_id,
            principal_uuid,
            transition.transition_id,
            payload,
        )
        .await?;
    }

    transition_helpers::insert_node_visit(
        &mut tx,
        new_node_visit_id,
        instance_uuid,
        target_node.node_id,
        target_visit_number,
        target_assignee_id,
        transition.transition_id,
    )
    .await?;

    // ---------------------------------------------------------------
    // Step 16: Update WorkflowInstance projection
    // ---------------------------------------------------------------
    let old_state_version = instance.workflow_state_version;
    let new_state_version = old_state_version + 1;

    transition_helpers::update_instance(
        &mut tx,
        instance_uuid,
        new_node_visit_id,
        new_state_version,
        old_state_version,
        instance.current_node_visit_id,
    )
    .await?;

    // ---------------------------------------------------------------
    // Step 17: Insert WORKFLOW_TRANSITION_COMMITTED Event
    // ---------------------------------------------------------------
    let event_sequence = new_state_version;

    let submission_payload_digest = match &cmd.submission_payload {
        Some(payload) => {
            let dig = digest::compute_json_digest(payload)
                .map_err(ExecuteWorkflowTransitionError::StorageError)?;
            Some(dig)
        }
        None => None,
    };

    transition_helpers::insert_event(
        &mut tx,
        event_id,
        instance_uuid,
        event_sequence,
        _actual_command_id,
        &effect,
        instance.current_node_visit_id,
        new_node_visit_id,
        instance.current_context_revision_id,
        final_submission_id,
        principal_uuid,
        source_node.node_id,
        target_node.node_id,
        old_state_version,
        new_state_version,
        &transition,
        submission_payload_digest,
    )
    .await?;

    // ---------------------------------------------------------------
    // Step 18: Complete the command receipt
    // ---------------------------------------------------------------
    let response_body = serde_json::json!({
        "workflowInstanceId": instance_uuid,
        "workflowStateVersion": new_state_version,
        "currentContextRevisionId": instance.current_context_revision_id,
        "sourceNodeVisitId": instance.current_node_visit_id,
        "currentNodeVisitId": new_node_visit_id,
        "submissionId": final_submission_id,
        "eventSequence": event_sequence,
    });

    let response_digest = digest::compute_json_digest(&response_body)
        .map_err(ExecuteWorkflowTransitionError::StorageError)?;

    complete_transition_receipt(
        &mut tx,
        _actual_command_id,
        200,
        &response_body,
        &response_digest,
    )
    .await?;

    // ---------------------------------------------------------------
    // Step 19: Commit
    // ---------------------------------------------------------------
    tx.commit()
        .await
        .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    Ok(TransitionOutcome::Executed(TransitionResult {
        workflow_instance_id: instance_uuid,
        workflow_state_version: new_state_version,
        current_context_revision_id: instance.current_context_revision_id,
        source_node_visit_id: instance.current_node_visit_id,
        current_node_visit_id: new_node_visit_id,
        submission_id: final_submission_id,
        event_sequence,
    }))
}
