//! ExecuteWorkflowTransition application service.
//!
//! Orchestrates the full workflow transition workflow:
//! 1. Compute request hash for idempotency
//! 2. Validate that the authenticated principal has a database identity
//! 3. Delegate to the atomic transition transaction
//! 4. Map result to the public response type

use sqlx::PgPool;

use crate::domain::workflow_instance::commands::ExecuteWorkflowTransitionCommand;
use crate::domain::workflow_instance::errors::ExecuteWorkflowTransitionError;
use crate::store::postgres::workflow_instance_repository::transition_transaction;

use super::idempotency::compute_transition_request_hash;

/// Result of a successful ExecuteWorkflowTransition command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecuteWorkflowTransitionResult {
    pub workflow_instance_id: uuid::Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: uuid::Uuid,
    pub source_node_visit_id: uuid::Uuid,
    pub current_node_visit_id: uuid::Uuid,
    pub submission_id: Option<uuid::Uuid>,
    pub event_sequence: i32,
}

impl From<transition_transaction::TransitionResult> for ExecuteWorkflowTransitionResult {
    fn from(r: transition_transaction::TransitionResult) -> Self {
        Self {
            workflow_instance_id: r.workflow_instance_id,
            workflow_state_version: r.workflow_state_version,
            current_context_revision_id: r.current_context_revision_id,
            source_node_visit_id: r.source_node_visit_id,
            current_node_visit_id: r.current_node_visit_id,
            submission_id: r.submission_id,
            event_sequence: r.event_sequence,
        }
    }
}

/// Execute a workflow transition atomically.
///
/// # Errors
///
/// Returns `ExecuteWorkflowTransitionError` for all validation, authorization,
/// version conflict, and infrastructure failures.
pub async fn execute_workflow_transition(
    pool: &PgPool,
    command: ExecuteWorkflowTransitionCommand,
) -> Result<ExecuteWorkflowTransitionResult, ExecuteWorkflowTransitionError> {
    // 1. Compute the identity-independent request hash before business validation.
    let request_hash = compute_transition_request_hash(
        &command.command_schema_version,
        &command.idempotency_key,
        &command.principal_id,
        &command.workflow_instance_id,
        command.expected_workflow_state_version,
        &command.transition_definition_id,
        &command.submission_payload,
    )?;

    // 2. A missing principal cannot own a receipt because receipts have a principal FK.
    // Disabled principals are checked after receipt ownership for stable replay.
    let principal_uuid = command.principal_id.into_uuid();
    pre_validate_principal_exists(pool, principal_uuid).await?;

    // 3. Execute atomic transition. Submission size is checked inside the receipt.
    let outcome = transition_transaction::execute_workflow_transition_atomically(
        pool,
        command,
        &request_hash,
    )
    .await?;

    // 5. Map outcome to public result
    match outcome {
        transition_transaction::TransitionOutcome::Executed(result) => Ok(result.into()),
        transition_transaction::TransitionOutcome::Replayed(result) => Ok(result.into()),
        transition_transaction::TransitionOutcome::ReplayedFailure(status, body) => {
            Err(replayed_failure_error(status, &body))
        }
    }
}

pub(crate) fn replayed_failure_error(
    status: i32,
    body: &serde_json::Value,
) -> ExecuteWorkflowTransitionError {
    let error_code = body["error"].as_str().unwrap_or("unknown");
    match (status, error_code) {
        (404, "principal_not_found") => ExecuteWorkflowTransitionError::PrincipalNotFound,
        (404, "instance_not_found") => ExecuteWorkflowTransitionError::InstanceNotFound,
        (403, "principal_disabled") => ExecuteWorkflowTransitionError::PrincipalDisabled,
        (404, "current_visit_not_found") => ExecuteWorkflowTransitionError::CurrentVisitNotFound,
        (403, "principal_not_assignee") => ExecuteWorkflowTransitionError::PrincipalNotAssignee,
        (409, "source_node_terminal") => ExecuteWorkflowTransitionError::SourceNodeTerminal,
        (409, "definition_version_revoked") => {
            ExecuteWorkflowTransitionError::DefinitionVersionRevoked
        }
        (500, "definition_version_draft") => ExecuteWorkflowTransitionError::DefinitionVersionDraft,
        (409, "workflow_state_version_conflict") => {
            let expected = body["expected"].as_i64().unwrap_or(0) as i32;
            let actual = body["actual"].as_i64().unwrap_or(0) as i32;
            ExecuteWorkflowTransitionError::WorkflowStateVersionConflict { expected, actual }
        }
        (409, "transition_not_applicable") => {
            ExecuteWorkflowTransitionError::TransitionNotApplicable(
                body["detail"].as_str().unwrap_or("unknown").to_string(),
            )
        }
        (422, "submission_required") => ExecuteWorkflowTransitionError::SubmissionRequired,
        (422, "submission_validation_failed") => {
            ExecuteWorkflowTransitionError::SubmissionValidationFailed(
                body["detail"]
                    .as_str()
                    .unwrap_or("validation failed")
                    .to_string(),
            )
        }
        (413, "size_limit_exceeded") => ExecuteWorkflowTransitionError::SizeLimitExceeded(
            body["detail"]
                .as_str()
                .unwrap_or("size limit exceeded")
                .to_string(),
        ),
        (422, "invalid_return_references") => {
            ExecuteWorkflowTransitionError::InvalidReturnReferences(
                body["detail"]
                    .as_str()
                    .unwrap_or("invalid references")
                    .to_string(),
            )
        }
        (422, "assignee_resolution_failed") => {
            ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
                body["detail"]
                    .as_str()
                    .unwrap_or("resolution failed")
                    .to_string(),
            )
        }
        _ => ExecuteWorkflowTransitionError::StorageError(format!(
            "replayed deterministic failure: status={}, error={}",
            status, error_code
        )),
    }
}

/// Resolve the authenticated principal to a persisted workflow identity.
async fn pre_validate_principal_exists(
    pool: &PgPool,
    principal_uuid: uuid::Uuid,
) -> Result<(), ExecuteWorkflowTransitionError> {
    let row: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT principal_id FROM principals WHERE principal_id = $1")
            .bind(principal_uuid)
            .fetch_optional(pool)
            .await
            .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    match row {
        None => Err(ExecuteWorkflowTransitionError::PrincipalNotFound),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::workflow_instance::errors::transition_error_code;
    use crate::store::postgres::workflow_instance_repository::transition_validation::error_response_body;

    fn same_error(left: &ExecuteWorkflowTransitionError, right: &ExecuteWorkflowTransitionError) {
        assert_eq!(std::mem::discriminant(left), std::mem::discriminant(right));
        match (left, right) {
            (
                ExecuteWorkflowTransitionError::WorkflowStateVersionConflict {
                    expected: left_expected,
                    actual: left_actual,
                },
                ExecuteWorkflowTransitionError::WorkflowStateVersionConflict {
                    expected: right_expected,
                    actual: right_actual,
                },
            ) => assert_eq!((left_expected, left_actual), (right_expected, right_actual)),
            (
                ExecuteWorkflowTransitionError::TransitionNotApplicable(left),
                ExecuteWorkflowTransitionError::TransitionNotApplicable(right),
            )
            | (
                ExecuteWorkflowTransitionError::SubmissionValidationFailed(left),
                ExecuteWorkflowTransitionError::SubmissionValidationFailed(right),
            )
            | (
                ExecuteWorkflowTransitionError::SizeLimitExceeded(left),
                ExecuteWorkflowTransitionError::SizeLimitExceeded(right),
            )
            | (
                ExecuteWorkflowTransitionError::InvalidReturnReferences(left),
                ExecuteWorkflowTransitionError::InvalidReturnReferences(right),
            )
            | (
                ExecuteWorkflowTransitionError::AssigneeResolutionFailed(left),
                ExecuteWorkflowTransitionError::AssigneeResolutionFailed(right),
            ) => assert_eq!(left, right),
            _ => {}
        }
    }

    #[test]
    fn every_deterministic_failure_body_round_trips_through_replay_mapping() {
        use ExecuteWorkflowTransitionError as E;
        let errors = vec![
            E::PrincipalNotFound,
            E::PrincipalDisabled,
            E::InstanceNotFound,
            E::CurrentVisitNotFound,
            E::PrincipalNotAssignee,
            E::SourceNodeTerminal,
            E::DefinitionVersionRevoked,
            E::DefinitionVersionDraft,
            E::WorkflowStateVersionConflict {
                expected: 3,
                actual: 2,
            },
            E::TransitionNotApplicable("transition detail".to_string()),
            E::SubmissionRequired,
            E::SubmissionValidationFailed("submission detail".to_string()),
            E::SizeLimitExceeded("submission payload exceeds 1 MiB".to_string()),
            E::InvalidReturnReferences("return detail".to_string()),
            E::AssigneeResolutionFailed("assignee detail".to_string()),
        ];
        for original in errors {
            let body = error_response_body(&original);
            let replayed = replayed_failure_error(transition_error_code(&original), &body);
            same_error(&original, &replayed);
        }
    }
}
