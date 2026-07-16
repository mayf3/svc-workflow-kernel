//! Persistence helpers for ReviseContextAndTransition.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::workflow_instance::combined_errors::ReviseContextAndTransitionError;
use crate::domain::workflow_instance::events::{
    ContextRevisedAndTransitionCommittedEventData,
    CONTEXT_REVISED_AND_TRANSITION_COMMITTED_EVENT_TYPE, EVENT_SCHEMA_VERSION,
};

use super::transition_rows::TransitionDefinitionRow;

pub(crate) enum CombinedOutcome {
    Executed(CombinedResult),
    Replayed(CombinedResult),
    ReplayedFailure(i32, serde_json::Value),
}

pub struct CombinedResult {
    pub workflow_instance_id: Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: Uuid,
    pub source_node_visit_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub submission_id: Uuid,
    pub event_sequence: i32,
}

pub(super) fn parse_replayed_response(
    body: &serde_json::Value,
) -> Result<CombinedResult, ReviseContextAndTransitionError> {
    fn uuid_field(
        body: &serde_json::Value,
        name: &str,
    ) -> Result<Uuid, ReviseContextAndTransitionError> {
        body[name]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| {
                ReviseContextAndTransitionError::StorageError(format!(
                    "stored response missing {}",
                    name
                ))
            })
    }

    Ok(CombinedResult {
        workflow_instance_id: uuid_field(body, "workflowInstanceId")?,
        workflow_state_version: body["workflowStateVersion"].as_i64().ok_or_else(|| {
            ReviseContextAndTransitionError::StorageError(
                "stored response missing workflowStateVersion".to_string(),
            )
        })? as i32,
        current_context_revision_id: uuid_field(body, "currentContextRevisionId")?,
        source_node_visit_id: uuid_field(body, "sourceNodeVisitId")?,
        current_node_visit_id: uuid_field(body, "currentNodeVisitId")?,
        submission_id: uuid_field(body, "submissionId")?,
        event_sequence: body["eventSequence"].as_i64().ok_or_else(|| {
            ReviseContextAndTransitionError::StorageError(
                "stored response missing eventSequence".to_string(),
            )
        })? as i32,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_context_revision(
    tx: &mut Transaction<'_, Postgres>,
    context_revision_id: Uuid,
    instance_id: Uuid,
    revision_number: i32,
    previous_revision_id: Uuid,
    payload: &serde_json::Value,
    payload_digest: &str,
    principal_id: Uuid,
) -> Result<(), ReviseContextAndTransitionError> {
    sqlx::query(
        r#"
        INSERT INTO workflow_context_revisions
            (context_revision_id, workflow_instance_id, revision_number,
             previous_revision_id, payload, payload_digest, created_by_principal_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(context_revision_id)
    .bind(instance_id)
    .bind(revision_number)
    .bind(previous_revision_id)
    .bind(payload)
    .bind(payload_digest)
    .bind(principal_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| {
        ReviseContextAndTransitionError::StorageError(format!(
            "context revision insert failed: {}",
            error
        ))
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_instance(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: Uuid,
    new_context_revision_id: Uuid,
    new_node_visit_id: Uuid,
    new_state_version: i32,
    old_state_version: i32,
    old_context_revision_id: Uuid,
    old_node_visit_id: Uuid,
) -> Result<(), ReviseContextAndTransitionError> {
    let result = sqlx::query(
        r#"
        UPDATE workflow_instances
        SET current_context_revision_id = $1,
            current_node_visit_id = $2,
            workflow_state_version = $3
        WHERE workflow_instance_id = $4
          AND workflow_state_version = $5
          AND current_context_revision_id = $6
          AND current_node_visit_id = $7
        "#,
    )
    .bind(new_context_revision_id)
    .bind(new_node_visit_id)
    .bind(new_state_version)
    .bind(instance_id)
    .bind(old_state_version)
    .bind(old_context_revision_id)
    .bind(old_node_visit_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| {
        ReviseContextAndTransitionError::StorageError(format!("instance update failed: {}", error))
    })?;

    if result.rows_affected() != 1 {
        return Err(ReviseContextAndTransitionError::InternalConsistency(
            "instance update affected unexpected number of rows".to_string(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    instance_id: Uuid,
    command_id: Uuid,
    source_visit_id: Uuid,
    target_visit_id: Uuid,
    context_revision_id: Uuid,
    submission_id: Uuid,
    principal_id: Uuid,
    source_node_id: Uuid,
    target_node_id: Uuid,
    old_state_version: i32,
    new_state_version: i32,
    previous_context_revision_id: Uuid,
    previous_context_digest: &str,
    new_context_digest: &str,
    transition: &TransitionDefinitionRow,
    submission_digest: &str,
) -> Result<(), ReviseContextAndTransitionError> {
    let event_data = ContextRevisedAndTransitionCommittedEventData {
        previous_context_revision_id: previous_context_revision_id.to_string(),
        new_context_revision_id: context_revision_id.to_string(),
        previous_context_payload_digest: previous_context_digest.to_string(),
        new_context_payload_digest: new_context_digest.to_string(),
        transition_definition_id: transition.transition_id.to_string(),
        transition_key: transition.transition_key.clone(),
        transition_effect: transition.transition_effect.clone(),
        source_node_id: source_node_id.to_string(),
        target_node_id: target_node_id.to_string(),
        source_node_visit_id: source_visit_id.to_string(),
        target_node_visit_id: target_visit_id.to_string(),
        submission_payload_digest: submission_digest.to_string(),
    };
    let event_data = serde_json::to_value(event_data)
        .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;
    let event_digest = digest::compute_json_digest(&event_data)
        .map_err(ReviseContextAndTransitionError::StorageError)?;

    sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             command_id, event_type, transition_effect,
             source_node_visit_id, target_node_visit_id,
             context_revision_id, submission_id, event_data, event_data_digest,
             actor_principal_id, from_node_id, to_node_id,
             old_workflow_state_version, new_workflow_state_version)
        VALUES ($1, $2, $3, $4, $5, $6, 'ADVANCE', $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17)
        "#,
    )
    .bind(event_id)
    .bind(instance_id)
    .bind(new_state_version)
    .bind(EVENT_SCHEMA_VERSION)
    .bind(command_id)
    .bind(CONTEXT_REVISED_AND_TRANSITION_COMMITTED_EVENT_TYPE)
    .bind(source_visit_id)
    .bind(target_visit_id)
    .bind(context_revision_id)
    .bind(submission_id)
    .bind(event_data)
    .bind(event_digest)
    .bind(principal_id)
    .bind(source_node_id)
    .bind(target_node_id)
    .bind(old_state_version)
    .bind(new_state_version)
    .execute(&mut **tx)
    .await
    .map_err(|error| {
        ReviseContextAndTransitionError::StorageError(format!(
            "combined event insert failed: {}",
            error
        ))
    })?;
    Ok(())
}
