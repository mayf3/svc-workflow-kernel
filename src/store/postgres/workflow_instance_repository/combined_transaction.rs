//! Atomic ReviseContextAndTransition transaction.

use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::enums::NodeType;
use crate::domain::workflow_instance::combined_errors::{
    error_code, error_label, ReviseContextAndTransitionError,
};
use crate::domain::workflow_instance::commands::ReviseContextAndTransitionCommand;

use super::combined_helpers::{self};
pub(crate) use super::combined_helpers::{CombinedOutcome, CombinedResult};
use super::combined_receipt::{self, CombinedReplayResult};
use super::{revise_validation, transition_helpers, transition_validation};

/// Execute a context revision and the DRAFT primary ADVANCE as one transaction.
pub(crate) async fn revise_context_and_transition_atomically(
    pool: &PgPool,
    command: ReviseContextAndTransitionCommand,
    request_hash: &str,
) -> Result<CombinedOutcome, ReviseContextAndTransitionError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;

    let principal_id = command.principal_id.into_uuid();
    let instance_id = command.workflow_instance_id.into_uuid();
    let transition_id = command.transition_definition_id.into_uuid();
    let command_id = Uuid::new_v4();

    let actual_command_id = match combined_receipt::try_insert_receipt(
        &mut tx,
        command_id,
        principal_id,
        &command.idempotency_key,
        request_hash,
    )
    .await?
    {
        Some(inserted_id) => inserted_id,
        None => {
            let replay = combined_receipt::replay_receipt(
                &mut tx,
                principal_id,
                &command.idempotency_key,
                request_hash,
            )
            .await?;
            match replay {
                CombinedReplayResult::CompletedMatch {
                    response_status: 200,
                    response_body,
                } => {
                    let result = combined_helpers::parse_replayed_response(&response_body)?;
                    tx.commit().await.map_err(|error| {
                        ReviseContextAndTransitionError::StorageError(error.to_string())
                    })?;
                    return Ok(CombinedOutcome::Replayed(result));
                }
                CombinedReplayResult::CompletedMatch {
                    response_status,
                    response_body,
                } => {
                    tx.commit().await.map_err(|error| {
                        ReviseContextAndTransitionError::StorageError(error.to_string())
                    })?;
                    return Ok(CombinedOutcome::ReplayedFailure(
                        response_status,
                        response_body,
                    ));
                }
                CombinedReplayResult::CompletedConflict {
                    command_id: original_command_id,
                    original_request_hash,
                } => {
                    combined_receipt::write_attempt_audit(
                        &mut tx,
                        original_command_id,
                        principal_id,
                        &command.idempotency_key,
                        request_hash,
                        &original_request_hash,
                    )
                    .await?;
                    tx.commit().await.map_err(|error| {
                        ReviseContextAndTransitionError::StorageError(error.to_string())
                    })?;
                    return Err(ReviseContextAndTransitionError::IdempotencyConflict {
                        original_command_id,
                        original_request_hash,
                    });
                }
                CombinedReplayResult::StillProcessing => {
                    tx.commit().await.map_err(|error| {
                        ReviseContextAndTransitionError::StorageError(error.to_string())
                    })?;
                    return Err(ReviseContextAndTransitionError::CommandStillProcessing);
                }
            }
        }
    };

    macro_rules! deterministic_failure {
        ($error:expr) => {{
            let error = $error;
            let response_body = error_response_body(&error);
            let response_digest = digest::compute_json_digest(&response_body)
                .map_err(ReviseContextAndTransitionError::StorageError)?;
            combined_receipt::complete_receipt(
                &mut tx,
                actual_command_id,
                error_code(&error),
                &response_body,
                &response_digest,
            )
            .await?;
            tx.commit().await.map_err(|commit_error| {
                ReviseContextAndTransitionError::StorageError(commit_error.to_string())
            })?;
            return Err(error);
        }};
    }

    macro_rules! domain_result {
        ($result:expr) => {{
            match $result {
                Ok(value) => value,
                Err(ReviseContextAndTransitionError::StorageError(detail)) => {
                    return Err(ReviseContextAndTransitionError::StorageError(detail));
                }
                Err(error) => deterministic_failure!(error),
            }
        }};
    }

    // Fixed lock order: Receipt -> Instance -> DefinitionVersion.
    let instance = domain_result!(transition_validation::lock_instance(&mut tx, instance_id)
        .await
        .map_err(ReviseContextAndTransitionError::from));
    domain_result!(transition_validation::validate_definition_version_status(
        &mut tx,
        instance.definition_version_id,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from));

    if command.expected_workflow_state_version != instance.workflow_state_version {
        deterministic_failure!(
            ReviseContextAndTransitionError::WorkflowStateVersionConflict {
                expected: command.expected_workflow_state_version,
                actual: instance.workflow_state_version,
            }
        );
    }

    let current_visit = domain_result!(transition_validation::read_current_visit(
        &mut tx,
        instance_id,
        instance.current_node_visit_id,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from));
    if instance.created_by_principal_id != principal_id {
        deterministic_failure!(ReviseContextAndTransitionError::PrincipalNotCreator);
    }
    if current_visit.assignee_principal_id != Some(principal_id) {
        deterministic_failure!(ReviseContextAndTransitionError::PrincipalNotAssignee);
    }
    if current_visit.node_type_enum() != NodeType::DRAFT {
        deterministic_failure!(ReviseContextAndTransitionError::CurrentNodeNotDraft);
    }

    if let Some(error) = domain_result!(transition_validation::validate_principal_enabled(
        &mut tx,
        principal_id
    )
    .await
    .map_err(ReviseContextAndTransitionError::from))
    {
        deterministic_failure!(ReviseContextAndTransitionError::from(error));
    }

    let current_context = domain_result!(revise_validation::read_current_context(
        &mut tx,
        instance_id,
        instance.current_context_revision_id,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from));
    let source_node = domain_result!(transition_validation::read_source_node(
        &mut tx,
        current_visit.node_id,
        instance.definition_version_id,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from));
    let transition = domain_result!(transition_validation::read_transition(
        &mut tx,
        transition_id,
        instance.definition_version_id,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from));

    if transition.source_node_id != current_visit.node_id {
        deterministic_failure!(ReviseContextAndTransitionError::TransitionNotApplicable(
            "transition source node does not match the current DRAFT".to_string(),
        ));
    }
    if transition.transition_effect != "ADVANCE"
        || source_node.primary_advance_transition_id != Some(transition.transition_id)
    {
        deterministic_failure!(ReviseContextAndTransitionError::TransitionNotApplicable(
            "combined command requires the current DRAFT primary ADVANCE transition".to_string(),
        ));
    }

    let target_node = domain_result!(transition_validation::read_target_node(
        &mut tx,
        transition.target_node_id,
        instance.definition_version_id,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from));

    if let Err(error) = validate_payload_size("context_payload", &command.context_payload) {
        deterministic_failure!(error);
    }
    if let Err(error) = validate_payload_size("submission_payload", &command.submission_payload) {
        deterministic_failure!(error);
    }

    let context_schema: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT context_schema FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(instance.definition_version_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;
    if let Err(error) = validate_schema(&context_schema, &command.context_payload, "context") {
        deterministic_failure!(error);
    }
    if let Err(error) = validate_schema(
        &transition.submission_schema,
        &command.submission_payload,
        "submission",
    ) {
        deterministic_failure!(error);
    }

    let domain_id: Uuid = sqlx::query_scalar(
        "SELECT domain_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;
    let target_assignee = domain_result!(transition_validation::resolve_assignee(
        &mut tx,
        &target_node,
        &instance,
        domain_id
    )
    .await
    .map_err(ReviseContextAndTransitionError::from));
    let target_visit_number: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(visit_number), 0) + 1 FROM workflow_node_visits \
         WHERE workflow_instance_id = $1 AND node_id = $2",
    )
    .bind(instance_id)
    .bind(target_node.node_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;

    let new_context_revision_id = Uuid::new_v4();
    let submission_id = Uuid::new_v4();
    let target_visit_id = Uuid::new_v4();
    let new_context_digest = digest::compute_json_digest(&command.context_payload)
        .map_err(ReviseContextAndTransitionError::StorageError)?;
    let submission_digest = digest::compute_json_digest(&command.submission_payload)
        .map_err(ReviseContextAndTransitionError::StorageError)?;
    let new_state_version = instance.workflow_state_version + 1;

    combined_helpers::insert_context_revision(
        &mut tx,
        new_context_revision_id,
        instance_id,
        current_context.revision_number + 1,
        current_context.context_revision_id,
        &command.context_payload,
        &new_context_digest,
        principal_id,
    )
    .await?;
    transition_helpers::insert_submission(
        &mut tx,
        submission_id,
        instance_id,
        instance.current_node_visit_id,
        new_context_revision_id,
        principal_id,
        transition.transition_id,
        &command.submission_payload,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from)?;
    transition_helpers::insert_node_visit(
        &mut tx,
        target_visit_id,
        instance_id,
        target_node.node_id,
        target_visit_number,
        target_assignee,
        transition.transition_id,
    )
    .await
    .map_err(ReviseContextAndTransitionError::from)?;
    combined_helpers::update_instance(
        &mut tx,
        instance_id,
        new_context_revision_id,
        target_visit_id,
        new_state_version,
        instance.workflow_state_version,
        instance.current_context_revision_id,
        instance.current_node_visit_id,
    )
    .await?;
    combined_helpers::insert_event(
        &mut tx,
        Uuid::new_v4(),
        instance_id,
        actual_command_id,
        instance.current_node_visit_id,
        target_visit_id,
        new_context_revision_id,
        submission_id,
        principal_id,
        source_node.node_id,
        target_node.node_id,
        instance.workflow_state_version,
        new_state_version,
        current_context.context_revision_id,
        &current_context.payload_digest,
        &new_context_digest,
        &transition,
        &submission_digest,
    )
    .await?;

    let response_body = serde_json::json!({
        "workflowInstanceId": instance_id,
        "workflowStateVersion": new_state_version,
        "currentContextRevisionId": new_context_revision_id,
        "sourceNodeVisitId": instance.current_node_visit_id,
        "currentNodeVisitId": target_visit_id,
        "submissionId": submission_id,
        "eventSequence": new_state_version,
    });
    let response_digest = digest::compute_json_digest(&response_body)
        .map_err(ReviseContextAndTransitionError::StorageError)?;
    combined_receipt::complete_receipt(
        &mut tx,
        actual_command_id,
        200,
        &response_body,
        &response_digest,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?;

    Ok(CombinedOutcome::Executed(CombinedResult {
        workflow_instance_id: instance_id,
        workflow_state_version: new_state_version,
        current_context_revision_id: new_context_revision_id,
        source_node_visit_id: instance.current_node_visit_id,
        current_node_visit_id: target_visit_id,
        submission_id,
        event_sequence: new_state_version,
    }))
}

fn validate_payload_size(
    field: &str,
    payload: &serde_json::Value,
) -> Result<(), ReviseContextAndTransitionError> {
    let size = serde_json::to_vec(payload)
        .map_err(|error| ReviseContextAndTransitionError::StorageError(error.to_string()))?
        .len();
    if size > 1024 * 1024 {
        Err(ReviseContextAndTransitionError::SizeLimitExceeded(format!(
            "{} exceeds 1 MiB",
            field
        )))
    } else {
        Ok(())
    }
}

fn validate_schema(
    schema: &Option<serde_json::Value>,
    payload: &serde_json::Value,
    kind: &str,
) -> Result<(), ReviseContextAndTransitionError> {
    let Some(schema) = schema else {
        return Ok(());
    };
    let validator = jsonschema::validator_for(schema).map_err(|error| match kind {
        "context" => ReviseContextAndTransitionError::ContextValidationFailed(format!(
            "context schema compilation failed: {}",
            error
        )),
        _ => ReviseContextAndTransitionError::SubmissionValidationFailed(format!(
            "submission schema compilation failed: {}",
            error
        )),
    })?;
    validator.validate(payload).map_err(|error| match kind {
        "context" => ReviseContextAndTransitionError::ContextValidationFailed(format!(
            "context payload failed schema validation: {}",
            error
        )),
        _ => ReviseContextAndTransitionError::SubmissionValidationFailed(format!(
            "submission payload failed schema validation: {}",
            error
        )),
    })
}

fn error_response_body(error: &ReviseContextAndTransitionError) -> serde_json::Value {
    match error {
        ReviseContextAndTransitionError::WorkflowStateVersionConflict { expected, actual } => {
            serde_json::json!({
                "error": error_label(error),
                "expected": expected,
                "actual": actual,
            })
        }
        ReviseContextAndTransitionError::TransitionNotApplicable(detail)
        | ReviseContextAndTransitionError::ContextValidationFailed(detail)
        | ReviseContextAndTransitionError::SubmissionValidationFailed(detail)
        | ReviseContextAndTransitionError::SizeLimitExceeded(detail)
        | ReviseContextAndTransitionError::AssigneeResolutionFailed(detail)
        | ReviseContextAndTransitionError::InternalConsistency(detail) => {
            serde_json::json!({"error": error_label(error), "detail": detail})
        }
        _ => serde_json::json!({"error": error_label(error)}),
    }
}
