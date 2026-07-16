//! Validation helpers for the atomic workflow transition transaction.
//!
//! Provides lock_instance, validate_principal, validate_definition_version,
//! read_transition, read_source_node, read_target_node, resolve_assignee,
//! and submission validation for ADVANCE / RETURN / TERMINATE transitions.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::enums::{AssigneeRefType, DefinitionVersionStatus, NodeType};
use crate::domain::workflow_instance::errors::ExecuteWorkflowTransitionError;

use super::row_types::*;
use super::transition_receipt::complete_transition_receipt;
use super::transition_rows::*;

/// Lock and read the workflow instance for transition.
pub(super) async fn lock_instance(
    tx: &mut Transaction<'_, Postgres>,
    instance_uuid: Uuid,
) -> Result<InstanceLockRow, ExecuteWorkflowTransitionError> {
    let instance: Option<InstanceLockRow> = sqlx::query_as(
        "SELECT workflow_instance_id, created_by_principal_id, \
         definition_version_id, current_context_revision_id, \
         current_node_visit_id, workflow_state_version \
         FROM workflow_instances WHERE workflow_instance_id = $1 FOR UPDATE",
    )
    .bind(instance_uuid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    instance.ok_or(ExecuteWorkflowTransitionError::InstanceNotFound)
}

/// Validate principal exists and is enabled inside the transaction.
pub(super) async fn validate_principal_enabled(
    tx: &mut Transaction<'_, Postgres>,
    principal_uuid: Uuid,
) -> Result<Option<ExecuteWorkflowTransitionError>, ExecuteWorkflowTransitionError> {
    let principal: Option<(bool,)> =
        sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
            .bind(principal_uuid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    match principal {
        None => Ok(Some(ExecuteWorkflowTransitionError::PrincipalNotFound)),
        Some((enabled,)) if !enabled => Ok(Some(ExecuteWorkflowTransitionError::PrincipalDisabled)),
        _ => Ok(None),
    }
}

/// Validate definition version status for transition.
/// PUBLISHED and DEPRECATED are allowed; REVOKED and DRAFT are blocked.
pub(super) async fn validate_definition_version_status(
    tx: &mut Transaction<'_, Postgres>,
    definition_version_id: Uuid,
) -> Result<DefinitionVersionStatus, ExecuteWorkflowTransitionError> {
    let status: Option<(String,)> = sqlx::query_as(
        "SELECT version_status::TEXT FROM workflow_definition_versions \
         WHERE definition_version_id = $1 FOR UPDATE",
    )
    .bind(definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    match status {
        None => Err(ExecuteWorkflowTransitionError::InternalConsistency(
            "definition version not found for instance".to_string(),
        )),
        Some((s,)) if s == "REVOKED" => {
            Err(ExecuteWorkflowTransitionError::DefinitionVersionRevoked)
        }
        Some((s,)) if s == "DRAFT" => Err(ExecuteWorkflowTransitionError::DefinitionVersionDraft),
        Some((s,)) => {
            // Parse the status for the caller
            let parsed = s
                .parse::<DefinitionVersionStatus>()
                .unwrap_or(DefinitionVersionStatus::PUBLISHED);
            Ok(parsed)
        }
    }
}

/// Read the current node visit with node definition details.
pub(super) async fn read_current_visit(
    tx: &mut Transaction<'_, Postgres>,
    instance_uuid: Uuid,
    current_node_visit_id: Uuid,
) -> Result<CurrentVisitFullRow, ExecuteWorkflowTransitionError> {
    let visit: Option<CurrentVisitFullRow> = sqlx::query_as(
        "SELECT nv.node_visit_id, nv.node_id, nv.assignee_principal_id, \
                nd.node_type::TEXT AS node_type, \
                nd.primary_advance_transition_id, nd.order_index \
         FROM workflow_node_visits nv \
         JOIN workflow_node_definitions nd ON nd.node_id = nv.node_id \
         WHERE nv.node_visit_id = $1 AND nv.workflow_instance_id = $2",
    )
    .bind(current_node_visit_id)
    .bind(instance_uuid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    visit.ok_or(ExecuteWorkflowTransitionError::CurrentVisitNotFound)
}

/// Read the source node definition (from current visit's node_id).
pub(super) async fn read_source_node(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    definition_version_id: Uuid,
) -> Result<SourceNodeRow, ExecuteWorkflowTransitionError> {
    let node: Option<SourceNodeRow> = sqlx::query_as(
        "SELECT node_id, node_type::TEXT AS node_type, \
                primary_advance_transition_id, order_index \
         FROM workflow_node_definitions \
         WHERE node_id = $1 AND definition_version_id = $2",
    )
    .bind(node_id)
    .bind(definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    node.ok_or(ExecuteWorkflowTransitionError::InternalConsistency(
        "source node not found in instance definition version".to_string(),
    ))
}

/// Read a transition definition and validate it exists.
pub(super) async fn read_transition(
    tx: &mut Transaction<'_, Postgres>,
    transition_id: Uuid,
    definition_version_id: Uuid,
) -> Result<TransitionDefinitionRow, ExecuteWorkflowTransitionError> {
    let trans: Option<TransitionDefinitionRow> = sqlx::query_as(
        "SELECT transition_id, transition_key, definition_version_id, \
                source_node_id, target_node_id, transition_effect::TEXT AS transition_effect, \
                submission_schema \
         FROM workflow_transition_definitions \
         WHERE transition_id = $1 AND definition_version_id = $2",
    )
    .bind(transition_id)
    .bind(definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    trans.ok_or(ExecuteWorkflowTransitionError::TransitionNotApplicable(
        "transition definition not found for this version".to_string(),
    ))
}

/// Read the target node definition.
pub(super) async fn read_target_node(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    definition_version_id: Uuid,
) -> Result<TargetNodeRow, ExecuteWorkflowTransitionError> {
    let node: Option<TargetNodeRow> = sqlx::query_as(
        "SELECT node_id, node_type::TEXT AS node_type, \
                assignee_ref_type::TEXT AS assignee_ref_type, \
                fixed_principal_id, order_index \
         FROM workflow_node_definitions \
         WHERE node_id = $1 AND definition_version_id = $2",
    )
    .bind(node_id)
    .bind(definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    node.ok_or(ExecuteWorkflowTransitionError::InternalConsistency(
        "target node not found in definition version".to_string(),
    ))
}

/// Resolve target assignee for the target node.
pub(super) async fn resolve_assignee(
    tx: &mut Transaction<'_, Postgres>,
    target_node: &TargetNodeRow,
    instance: &InstanceLockRow,
    domain_uuid: Uuid,
) -> Result<Option<Uuid>, ExecuteWorkflowTransitionError> {
    if target_node.node_type_enum() == NodeType::TERMINAL {
        // Published legacy Terminal definitions can still carry an obsolete
        // reference. It never grants authority and every new Terminal visit is unassigned.
        return Ok(None);
    }
    match target_node.assignee_ref_type_enum() {
        Some(AssigneeRefType::WorkflowCreator) => Ok(Some(instance.created_by_principal_id)),
        Some(AssigneeRefType::DomainOwner) => {
            let owner: Option<(Uuid, bool)> = sqlx::query_as(
                "SELECT principal_id, enabled FROM domain_role_bindings \
                 WHERE domain_id = $1 AND role_key = 'DOMAIN_OWNER' AND enabled = TRUE \
                 LIMIT 1",
            )
            .bind(domain_uuid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

            let (owner_id, _) = owner.ok_or_else(|| {
                ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
                    "no enabled DOMAIN_OWNER found for domain".to_string(),
                )
            })?;

            let owner_enabled: Option<(bool,)> =
                sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
                    .bind(owner_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

            match owner_enabled {
                None => Err(ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
                    "DOMAIN_OWNER principal not found".to_string(),
                )),
                Some((enabled,)) if !enabled => {
                    Err(ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
                        "DOMAIN_OWNER principal is disabled".to_string(),
                    ))
                }
                _ => Ok(Some(owner_id)),
            }
        }
        Some(AssigneeRefType::FixedPrincipal) => {
            let fixed_id = target_node.fixed_principal_id.ok_or_else(|| {
                ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
                    "FIXED_PRINCIPAL node has no principal_id configured".to_string(),
                )
            })?;

            let fixed_enabled: Option<(bool,)> =
                sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
                    .bind(fixed_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

            match fixed_enabled {
                None => Err(ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
                    "FIXED_PRINCIPAL not found".to_string(),
                )),
                Some((enabled,)) if !enabled => {
                    Err(ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
                        "FIXED_PRINCIPAL is disabled".to_string(),
                    ))
                }
                _ => Ok(Some(fixed_id)),
            }
        }
        None => Err(ExecuteWorkflowTransitionError::AssigneeResolutionFailed(
            "non-terminal node has no valid assignee reference".to_string(),
        )),
    }
}

/// Validate submission payload size (≤ 1 MiB).
pub(super) fn validate_submission_size(
    payload: &serde_json::Value,
) -> Result<(), ExecuteWorkflowTransitionError> {
    let serialized = serde_json::to_vec(payload).map_err(|e| {
        ExecuteWorkflowTransitionError::SizeLimitExceeded(format!(
            "submission serialization failed: {}",
            e
        ))
    })?;

    if serialized.len() > 1048576 {
        return Err(ExecuteWorkflowTransitionError::SizeLimitExceeded(
            "submission payload exceeds 1 MiB".to_string(),
        ));
    }

    Ok(())
}

/// Validate submission payload against a JSON schema.
pub(super) fn validate_submission_schema(
    schema: &Option<serde_json::Value>,
    payload: &serde_json::Value,
) -> Result<(), ExecuteWorkflowTransitionError> {
    if let Some(schema_value) = schema {
        let validator = jsonschema::validator_for(schema_value).map_err(|e| {
            ExecuteWorkflowTransitionError::SubmissionValidationFailed(format!(
                "submission schema compilation failed: {}",
                e
            ))
        })?;

        validator.validate(payload).map_err(|e| {
            ExecuteWorkflowTransitionError::SubmissionValidationFailed(format!(
                "submission payload failed schema validation: {}",
                e
            ))
        })?;
    }

    Ok(())
}

/// Validate RETURN submission references.
pub(super) async fn validate_return_references(
    tx: &mut Transaction<'_, Postgres>,
    payload: &serde_json::Value,
    instance_uuid: Uuid,
) -> Result<(), ExecuteWorkflowTransitionError> {
    // Extract rootCauseNodeVisitId
    let root_cause = payload
        .get("rootCauseNodeVisitId")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            ExecuteWorkflowTransitionError::InvalidReturnReferences(
                "rootCauseNodeVisitId is required and must be a valid UUID".to_string(),
            )
        })?;

    // Verify rootCauseNodeVisitId exists and belongs to this instance
    let root_visit: Option<(Uuid,)> = sqlx::query_as(
        "SELECT node_visit_id FROM workflow_node_visits \
         WHERE node_visit_id = $1 AND workflow_instance_id = $2",
    )
    .bind(root_cause)
    .bind(instance_uuid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

    if root_visit.is_none() {
        return Err(ExecuteWorkflowTransitionError::InvalidReturnReferences(
            "rootCauseNodeVisitId does not exist or belongs to a different instance".to_string(),
        ));
    }

    // Extract and validate relatedSubmissionIds
    if let Some(related) = payload
        .get("relatedSubmissionIds")
        .and_then(|v| v.as_array())
    {
        for entry in related {
            let sub_id_str = entry.as_str().ok_or_else(|| {
                ExecuteWorkflowTransitionError::InvalidReturnReferences(
                    "relatedSubmissionIds entries must be strings".to_string(),
                )
            })?;

            let sub_id = Uuid::parse_str(sub_id_str).map_err(|_| {
                ExecuteWorkflowTransitionError::InvalidReturnReferences(
                    "relatedSubmissionIds entry is not a valid UUID".to_string(),
                )
            })?;

            let sub: Option<(Uuid,)> = sqlx::query_as(
                "SELECT submission_id FROM workflow_submissions \
                 WHERE submission_id = $1 AND workflow_instance_id = $2",
            )
            .bind(sub_id)
            .bind(instance_uuid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| ExecuteWorkflowTransitionError::StorageError(e.to_string()))?;

            if sub.is_none() {
                return Err(ExecuteWorkflowTransitionError::InvalidReturnReferences(
                    format!(
                        "relatedSubmissionId {} does not exist or belongs to a different instance",
                        sub_id
                    ),
                ));
            }
        }
    }

    // Validate required fields exist
    if payload.get("reasonCode").is_none() {
        return Err(ExecuteWorkflowTransitionError::InvalidReturnReferences(
            "reasonCode is required for RETURN submissions".to_string(),
        ));
    }

    if payload.get("reason").is_none() {
        return Err(ExecuteWorkflowTransitionError::InvalidReturnReferences(
            "reason is required for RETURN submissions".to_string(),
        ));
    }

    Ok(())
}

/// Map an ExecuteWorkflowTransitionError to a response body for deterministic failure receipts.
pub(crate) fn error_response_body(err: &ExecuteWorkflowTransitionError) -> serde_json::Value {
    match err {
        ExecuteWorkflowTransitionError::WorkflowStateVersionConflict { expected, actual } => {
            serde_json::json!({
                "error": "workflow_state_version_conflict",
                "expected": expected,
                "actual": actual,
            })
        }
        ExecuteWorkflowTransitionError::SubmissionValidationFailed(detail) => {
            serde_json::json!({
                "error": "submission_validation_failed",
                "detail": detail,
            })
        }
        ExecuteWorkflowTransitionError::TransitionNotApplicable(detail) => {
            serde_json::json!({
                "error": "transition_not_applicable",
                "detail": detail,
            })
        }
        ExecuteWorkflowTransitionError::SizeLimitExceeded(detail)
        | ExecuteWorkflowTransitionError::InvalidReturnReferences(detail)
        | ExecuteWorkflowTransitionError::AssigneeResolutionFailed(detail) => {
            let label = crate::domain::workflow_instance::errors::transition_error_label(err);
            serde_json::json!({"error": label, "detail": detail})
        }
        _ => {
            let label = crate::domain::workflow_instance::errors::transition_error_label(err);
            serde_json::json!({"error": label})
        }
    }
}

pub(super) fn is_deterministic_error(err: &ExecuteWorkflowTransitionError) -> bool {
    matches!(
        err,
        ExecuteWorkflowTransitionError::PrincipalNotFound
            | ExecuteWorkflowTransitionError::PrincipalDisabled
            | ExecuteWorkflowTransitionError::InstanceNotFound
            | ExecuteWorkflowTransitionError::CurrentVisitNotFound
            | ExecuteWorkflowTransitionError::PrincipalNotAssignee
            | ExecuteWorkflowTransitionError::SourceNodeTerminal
            | ExecuteWorkflowTransitionError::DefinitionVersionRevoked
            | ExecuteWorkflowTransitionError::DefinitionVersionDraft
            | ExecuteWorkflowTransitionError::WorkflowStateVersionConflict { .. }
            | ExecuteWorkflowTransitionError::TransitionNotApplicable(_)
            | ExecuteWorkflowTransitionError::SubmissionRequired
            | ExecuteWorkflowTransitionError::SubmissionValidationFailed(_)
            | ExecuteWorkflowTransitionError::SizeLimitExceeded(_)
            | ExecuteWorkflowTransitionError::InvalidReturnReferences(_)
            | ExecuteWorkflowTransitionError::AssigneeResolutionFailed(_)
    )
}

/// Persist a deterministic failure receipt. Call only before runtime facts are written.
pub(super) async fn persist_deterministic_failure(
    mut tx: Transaction<'_, Postgres>,
    command_id: Uuid,
    err: &ExecuteWorkflowTransitionError,
) -> Result<(), ExecuteWorkflowTransitionError> {
    let status = crate::domain::workflow_instance::errors::transition_error_code(err);
    let body = error_response_body(err);
    let response_digest =
        digest::compute_json_digest(&body).map_err(ExecuteWorkflowTransitionError::StorageError)?;
    complete_transition_receipt(&mut tx, command_id, status, &body, &response_digest).await?;
    tx.commit()
        .await
        .map_err(|error| ExecuteWorkflowTransitionError::StorageError(error.to_string()))
}
