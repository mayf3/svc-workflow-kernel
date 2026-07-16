//! Helper functions for the atomic transition transaction.
//!
//! Extracted from transition_transaction.rs to stay under 500 lines.

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::workflow_instance::errors::ExecuteWorkflowTransitionError;
use crate::domain::workflow_instance::events::{
    TransitionCommittedEventData, EVENT_SCHEMA_VERSION, TRANSITION_COMMITTED_EVENT_TYPE,
};

use super::transition_receipt::{self, TransitionReplayResult};
use super::transition_rows::*;

/// Outcome of an atomic transition attempt.
pub(crate) enum TransitionOutcome {
    /// Fresh successful transition.
    Executed(TransitionResult),
    /// Idempotent replay of a successful request.
    Replayed(TransitionResult),
    /// Idempotent replay of a failed request.
    ReplayedFailure(i32, serde_json::Value),
}

/// Result of a successful atomic transition.
pub(crate) struct TransitionResult {
    pub workflow_instance_id: Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: Uuid,
    pub source_node_visit_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub submission_id: Option<Uuid>,
    pub event_sequence: i32,
}

/// Parse the replayed response body into a TransitionResult.
fn parse_replayed_response(
    body: &serde_json::Value,
) -> Result<TransitionResult, ExecuteWorkflowTransitionError> {
    let wf_id = body["workflowInstanceId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            ExecuteWorkflowTransitionError::StorageError("missing workflowInstanceId".to_string())
        })?;
    let ctx_rev_id = body["currentContextRevisionId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            ExecuteWorkflowTransitionError::StorageError(
                "missing currentContextRevisionId".to_string(),
            )
        })?;
    let sv_id = body["sourceNodeVisitId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            ExecuteWorkflowTransitionError::StorageError("missing sourceNodeVisitId".to_string())
        })?;
    let tv_id = body["currentNodeVisitId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            ExecuteWorkflowTransitionError::StorageError("missing currentNodeVisitId".to_string())
        })?;
    let state_ver = body["workflowStateVersion"].as_i64().unwrap_or(1) as i32;
    let ev_seq = body["eventSequence"].as_i64().unwrap_or(1) as i32;
    let sub_id = body["submissionId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok());

    Ok(TransitionResult {
        workflow_instance_id: wf_id,
        workflow_state_version: state_ver,
        current_context_revision_id: ctx_rev_id,
        source_node_visit_id: sv_id,
        current_node_visit_id: tv_id,
        submission_id: sub_id,
        event_sequence: ev_seq,
    })
}

/// Handle the replay case when an existing receipt is found.
///
/// Returns:
/// - `Ok(Some(Handled(outcome)))` — replay handled, caller should commit and return outcome
/// - `Ok(None)` — should continue with the original command_id
/// - `Err(...)` — conflict/processing error, caller should commit and return err
pub(super) async fn handle_receipt_replay(
    tx: &mut Transaction<'_, Postgres>,
    principal_uuid: Uuid,
    idempotency_key: &str,
    request_hash: &str,
    audit_id: Uuid,
    _actual_command_id: Uuid,
) -> Result<Option<TransitionOutcome>, ExecuteWorkflowTransitionError> {
    let replay = transition_receipt::replay_transition_receipt(
        tx,
        principal_uuid,
        idempotency_key,
        request_hash,
    )
    .await?;

    match replay {
        TransitionReplayResult::CompletedMatch {
            response_status,
            response_body,
            ..
        } => {
            if response_status != 200 {
                // Deterministic failure replay — caller must commit
                return Ok(Some(TransitionOutcome::ReplayedFailure(
                    response_status,
                    response_body,
                )));
            }
            let result = parse_replayed_response(&response_body)?;
            Ok(Some(TransitionOutcome::Replayed(result)))
        }
        TransitionReplayResult::CompletedConflict {
            command_id: cid,
            original_request_hash: orig_hash,
        } => {
            let details = serde_json::json!({
                "conflictType": "IDEMPOTENCY_KEY_MISMATCH",
                "originalRequestHash": orig_hash,
                "newRequestHash": request_hash,
            });
            transition_receipt::write_transition_attempt_audit(
                tx,
                audit_id,
                cid,
                principal_uuid,
                idempotency_key,
                "IDEMPOTENCY_CONFLICT",
                Some("request hash mismatch"),
                request_hash,
                Some(&details),
            )
            .await?;
            Err(ExecuteWorkflowTransitionError::IdempotencyConflict {
                original_command_id: cid,
                original_request_hash: orig_hash,
            })
        }
        TransitionReplayResult::StillProcessing => {
            Err(ExecuteWorkflowTransitionError::CommandStillProcessing)
        }
    }
}

/// Insert a submission record.
pub(super) async fn insert_submission(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    instance_uuid: Uuid,
    source_visit_id: Uuid,
    context_revision_id: Uuid,
    principal_uuid: Uuid,
    transition_id: Uuid,
    payload: &Value,
) -> Result<(), ExecuteWorkflowTransitionError> {
    let payload_digest = digest::compute_json_digest(payload)
        .map_err(ExecuteWorkflowTransitionError::StorageError)?;

    sqlx::query(
        r#"
        INSERT INTO workflow_submissions
            (submission_id, workflow_instance_id, source_node_visit_id,
             context_revision_id, author_principal_id, transition_id,
             payload, payload_digest, schema_version)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(submission_id)
    .bind(instance_uuid)
    .bind(source_visit_id)
    .bind(context_revision_id)
    .bind(principal_uuid)
    .bind(transition_id)
    .bind(payload)
    .bind(&payload_digest)
    .bind("v1")
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        ExecuteWorkflowTransitionError::StorageError(format!("submission insert failed: {}", e))
    })?;

    Ok(())
}

/// Insert the target node visit.
pub(super) async fn insert_node_visit(
    tx: &mut Transaction<'_, Postgres>,
    node_visit_id: Uuid,
    instance_uuid: Uuid,
    node_id: Uuid,
    visit_number: i32,
    assignee_id: Option<Uuid>,
    transition_id: Uuid,
) -> Result<(), ExecuteWorkflowTransitionError> {
    sqlx::query(
        r#"
        INSERT INTO workflow_node_visits
            (node_visit_id, workflow_instance_id, node_id, visit_number,
             assignee_principal_id, entered_by_transition_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(node_visit_id)
    .bind(instance_uuid)
    .bind(node_id)
    .bind(visit_number)
    .bind(assignee_id)
    .bind(transition_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        ExecuteWorkflowTransitionError::StorageError(format!("node visit insert failed: {}", e))
    })?;

    Ok(())
}

/// Update the workflow instance projection.
pub(super) async fn update_instance(
    tx: &mut Transaction<'_, Postgres>,
    instance_uuid: Uuid,
    new_node_visit_id: Uuid,
    new_state_version: i32,
    old_state_version: i32,
    old_node_visit_id: Uuid,
) -> Result<(), ExecuteWorkflowTransitionError> {
    let result = sqlx::query(
        r#"
        UPDATE workflow_instances
        SET current_node_visit_id = $1,
            workflow_state_version = $2
        WHERE workflow_instance_id = $3
          AND workflow_state_version = $4
          AND current_node_visit_id = $5
        "#,
    )
    .bind(new_node_visit_id)
    .bind(new_state_version)
    .bind(instance_uuid)
    .bind(old_state_version)
    .bind(old_node_visit_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        ExecuteWorkflowTransitionError::StorageError(format!("instance update failed: {}", e))
    })?;

    if result.rows_affected() != 1 {
        return Err(ExecuteWorkflowTransitionError::InternalConsistency(
            "instance update affected unexpected number of rows".to_string(),
        ));
    }

    Ok(())
}

/// Insert the WORKFLOW_TRANSITION_COMMITTED event.
pub(super) async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    instance_uuid: Uuid,
    event_sequence: i32,
    actual_command_id: Uuid,
    transition_effect: &str,
    source_visit_id: Uuid,
    target_visit_id: Uuid,
    context_revision_id: Uuid,
    submission_id: Option<Uuid>,
    actor_principal_id: Uuid,
    source_node_id: Uuid,
    target_node_id: Uuid,
    old_state_version: i32,
    new_state_version: i32,
    transition: &TransitionDefinitionRow,
    submission_payload_digest: Option<String>,
) -> Result<(), ExecuteWorkflowTransitionError> {
    let event_data = TransitionCommittedEventData {
        transition_definition_id: transition.transition_id.to_string(),
        transition_key: transition.transition_key.clone(),
        transition_effect: transition.transition_effect.clone(),
        source_node_id: source_node_id.to_string(),
        target_node_id: target_node_id.to_string(),
        source_node_visit_id: source_visit_id.to_string(),
        target_node_visit_id: target_visit_id.to_string(),
        context_revision_id: context_revision_id.to_string(),
        submission_payload_digest,
    };

    let event_data_json = serde_json::to_value(&event_data)
        .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;
    let event_data_digest = digest::compute_json_digest(&event_data_json)
        .map_err(ExecuteWorkflowTransitionError::StorageError)?;

    sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             command_id, event_type, transition_effect,
             source_node_visit_id, target_node_visit_id,
             context_revision_id, submission_id,
             event_data, event_data_digest,
             actor_principal_id, from_node_id, to_node_id,
             old_workflow_state_version, new_workflow_state_version)
        VALUES ($1, $2, $3, $4, $5, $6, $7::transition_effect,
                $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
        "#,
    )
    .bind(event_id)
    .bind(instance_uuid)
    .bind(event_sequence)
    .bind(EVENT_SCHEMA_VERSION)
    .bind(actual_command_id)
    .bind(TRANSITION_COMMITTED_EVENT_TYPE)
    .bind(transition_effect)
    .bind(source_visit_id)
    .bind(target_visit_id)
    .bind(context_revision_id)
    .bind(submission_id)
    .bind(&event_data_json)
    .bind(&event_data_digest)
    .bind(actor_principal_id)
    .bind(source_node_id)
    .bind(target_node_id)
    .bind(old_state_version)
    .bind(new_state_version)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        ExecuteWorkflowTransitionError::StorageError(format!("event insert failed: {}", e))
    })?;

    Ok(())
}
