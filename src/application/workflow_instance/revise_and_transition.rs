//! Application service for the atomic context revision + transition command.

use sqlx::PgPool;

use crate::domain::workflow_instance::combined_errors::ReviseContextAndTransitionError;
use crate::domain::workflow_instance::commands::ReviseContextAndTransitionCommand;
use crate::store::postgres::workflow_instance_repository::combined_transaction;

use super::idempotency::compute_combined_request_hash;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReviseContextAndTransitionResult {
    pub workflow_instance_id: uuid::Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: uuid::Uuid,
    pub source_node_visit_id: uuid::Uuid,
    pub current_node_visit_id: uuid::Uuid,
    pub submission_id: uuid::Uuid,
    pub event_sequence: i32,
}

impl From<combined_transaction::CombinedResult> for ReviseContextAndTransitionResult {
    fn from(result: combined_transaction::CombinedResult) -> Self {
        Self {
            workflow_instance_id: result.workflow_instance_id,
            workflow_state_version: result.workflow_state_version,
            current_context_revision_id: result.current_context_revision_id,
            source_node_visit_id: result.source_node_visit_id,
            current_node_visit_id: result.current_node_visit_id,
            submission_id: result.submission_id,
            event_sequence: result.event_sequence,
        }
    }
}

/// Revise DRAFT context and execute its primary ADVANCE in one transaction.
pub async fn revise_context_and_transition(
    pool: &PgPool,
    command: ReviseContextAndTransitionCommand,
) -> Result<ReviseContextAndTransitionResult, ReviseContextAndTransitionError> {
    pre_validate_principal_exists(pool, command.principal_id.into_uuid()).await?;

    let request_hash = compute_combined_request_hash(
        &command.command_schema_version,
        &command.principal_id,
        &command.workflow_instance_id,
        command.expected_workflow_state_version,
        &command.transition_definition_id,
        &command.context_payload,
        &command.submission_payload,
    )?;

    let outcome = combined_transaction::revise_context_and_transition_atomically(
        pool,
        command,
        &request_hash,
    )
    .await?;

    match outcome {
        combined_transaction::CombinedOutcome::Executed(result)
        | combined_transaction::CombinedOutcome::Replayed(result) => Ok(result.into()),
        combined_transaction::CombinedOutcome::ReplayedFailure(status, body) => {
            Err(replayed_error(status, &body))
        }
    }
}

async fn pre_validate_principal_exists(
    pool: &PgPool,
    principal_id: uuid::Uuid,
) -> Result<(), ReviseContextAndTransitionError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM principals WHERE principal_id = $1)")
            .bind(principal_id)
            .fetch_one(pool)
            .await
            .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;

    if exists {
        Ok(())
    } else {
        Err(ReviseContextAndTransitionError::PrincipalNotFound)
    }
}

fn replayed_error(status: i32, body: &serde_json::Value) -> ReviseContextAndTransitionError {
    let detail = || body["detail"].as_str().unwrap_or("unknown").to_string();
    match (status, body["error"].as_str().unwrap_or("unknown")) {
        (404, "instance_not_found") => ReviseContextAndTransitionError::InstanceNotFound,
        (404, "current_visit_not_found") => ReviseContextAndTransitionError::CurrentVisitNotFound,
        (403, "principal_disabled") => ReviseContextAndTransitionError::PrincipalDisabled,
        (403, "principal_not_creator") => ReviseContextAndTransitionError::PrincipalNotCreator,
        (403, "principal_not_assignee") => ReviseContextAndTransitionError::PrincipalNotAssignee,
        (409, "current_node_not_draft") => ReviseContextAndTransitionError::CurrentNodeNotDraft,
        (409, "definition_version_revoked") => {
            ReviseContextAndTransitionError::DefinitionVersionRevoked
        }
        (500, "definition_version_draft") => {
            ReviseContextAndTransitionError::DefinitionVersionDraft
        }
        (409, "workflow_state_version_conflict") => {
            ReviseContextAndTransitionError::WorkflowStateVersionConflict {
                expected: body["expected"].as_i64().unwrap_or_default() as i32,
                actual: body["actual"].as_i64().unwrap_or_default() as i32,
            }
        }
        (409, "transition_not_applicable") => {
            ReviseContextAndTransitionError::TransitionNotApplicable(detail())
        }
        (422, "context_validation_failed") => {
            ReviseContextAndTransitionError::ContextValidationFailed(detail())
        }
        (422, "submission_validation_failed") => {
            ReviseContextAndTransitionError::SubmissionValidationFailed(detail())
        }
        (413, "size_limit_exceeded") => {
            ReviseContextAndTransitionError::SizeLimitExceeded(detail())
        }
        (422, "assignee_resolution_failed") => {
            ReviseContextAndTransitionError::AssigneeResolutionFailed(detail())
        }
        (500, "internal_consistency_error") => {
            ReviseContextAndTransitionError::InternalConsistency(detail())
        }
        (_, error) => ReviseContextAndTransitionError::StorageError(format!(
            "replayed deterministic failure: status={}, error={}",
            status, error
        )),
    }
}
