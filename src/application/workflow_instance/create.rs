//! CreateWorkflowInstance application service.
//!
//! Orchestrates the full creation workflow:
//! 1. Compute request hash for idempotency
//! 2. Validate that the authenticated principal has a database identity
//! 3. Delegate to the atomic creation transaction
//! 4. Map result to the public response type

use sqlx::PgPool;

use crate::domain::workflow_instance::commands::CreateWorkflowInstanceCommand;
use crate::domain::workflow_instance::errors::CreateWorkflowInstanceError;
use crate::store::postgres::workflow_instance_repository::create_transaction;

use super::idempotency::compute_request_hash;

/// Result of a successful CreateWorkflowInstance command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateWorkflowInstanceResult {
    pub workflow_instance_id: uuid::Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: uuid::Uuid,
    pub current_node_visit_id: uuid::Uuid,
    pub event_sequence: i32,
}

impl From<create_transaction::CreateResult> for CreateWorkflowInstanceResult {
    fn from(r: create_transaction::CreateResult) -> Self {
        Self {
            workflow_instance_id: r.workflow_instance_id,
            workflow_state_version: r.workflow_state_version,
            current_context_revision_id: r.current_context_revision_id,
            current_node_visit_id: r.current_node_visit_id,
            event_sequence: r.event_sequence,
        }
    }
}

/// Create a new workflow instance atomically.
///
/// # Errors
///
/// Returns `CreateWorkflowInstanceError` for all validation, authorization,
/// and infrastructure failures.
pub async fn create_workflow_instance(
    pool: &PgPool,
    command: CreateWorkflowInstanceCommand,
) -> Result<CreateWorkflowInstanceResult, CreateWorkflowInstanceError> {
    // 1. Compute the identity-independent request hash before business validation.
    let request_hash = compute_request_hash(
        &command.command_schema_version,
        &command.idempotency_key,
        &command.principal_id,
        &command.domain_id,
        &command.definition_version_id,
        &command.context_payload,
        &command.metadata,
        &command.external_reference,
        &command.external_url,
    )?;

    // 2. A missing principal cannot own a receipt because receipts have a principal FK.
    // Disabled principals do own stable failure receipts and are checked in-transaction.
    let principal_uuid = command.principal_id.into_uuid();
    pre_validate_principal_exists(pool, principal_uuid).await?;

    // 3. Execute atomic creation. All deterministic business and size checks happen
    // after receipt ownership and before the first runtime fact is written.
    let outcome =
        create_transaction::create_workflow_instance_atomically(pool, command, &request_hash)
            .await?;

    // 4. Map outcome to public result
    match outcome {
        create_transaction::CreateOutcome::Created(result) => Ok(result.into()),
        create_transaction::CreateOutcome::Replayed(result) => Ok(result.into()),
        create_transaction::CreateOutcome::ReplayedFailure(status, body) => {
            Err(replayed_failure_error(status, &body))
        }
    }
}

pub(crate) fn replayed_failure_error(
    status: i32,
    body: &serde_json::Value,
) -> CreateWorkflowInstanceError {
    let error_code = body["error"].as_str().unwrap_or("unknown");
    match (status, error_code) {
        (404, "domain_not_found") => CreateWorkflowInstanceError::DomainNotFound,
        (403, "domain_disabled") => CreateWorkflowInstanceError::DomainDisabled,
        (404, "principal_not_found") => CreateWorkflowInstanceError::PrincipalNotFound,
        (403, "principal_disabled") => CreateWorkflowInstanceError::PrincipalDisabled,
        (403, "domain_membership_required") => {
            CreateWorkflowInstanceError::DomainMembershipRequired
        }
        (403, "cross_domain_violation") => CreateWorkflowInstanceError::CrossDomainViolation,
        (404, "definition_version_not_found") => {
            CreateWorkflowInstanceError::DefinitionVersionNotFound
        }
        (409, "version_not_published") => CreateWorkflowInstanceError::VersionNotPublished,
        (422, "context_validation_failed") => CreateWorkflowInstanceError::ContextValidationFailed(
            body["detail"]
                .as_str()
                .unwrap_or("validation failed")
                .to_string(),
        ),
        (413, "size_limit_exceeded") => CreateWorkflowInstanceError::SizeLimitExceeded(
            body["detail"]
                .as_str()
                .unwrap_or("size limit exceeded")
                .to_string(),
        ),
        (422, "assignee_resolution_failed") => {
            CreateWorkflowInstanceError::AssigneeResolutionFailed(
                body["detail"]
                    .as_str()
                    .unwrap_or("resolution failed")
                    .to_string(),
            )
        }
        _ => CreateWorkflowInstanceError::StorageError(format!(
            "replayed deterministic failure: status={}, error={}",
            status, error_code
        )),
    }
}

/// Resolve the authenticated principal to a persisted workflow identity.
async fn pre_validate_principal_exists(
    pool: &PgPool,
    principal_uuid: uuid::Uuid,
) -> Result<(), CreateWorkflowInstanceError> {
    let row: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT principal_id FROM principals WHERE principal_id = $1")
            .bind(principal_uuid)
            .fetch_optional(pool)
            .await
            .map_err(|e| CreateWorkflowInstanceError::StorageError(e.to_string()))?;

    match row {
        None => Err(CreateWorkflowInstanceError::PrincipalNotFound),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::postgres::workflow_instance_repository::validation_helpers::{
        deterministic_error_body, deterministic_error_code,
    };

    fn same_error(left: &CreateWorkflowInstanceError, right: &CreateWorkflowInstanceError) {
        assert_eq!(std::mem::discriminant(left), std::mem::discriminant(right));
        match (left, right) {
            (
                CreateWorkflowInstanceError::ContextValidationFailed(left),
                CreateWorkflowInstanceError::ContextValidationFailed(right),
            )
            | (
                CreateWorkflowInstanceError::SizeLimitExceeded(left),
                CreateWorkflowInstanceError::SizeLimitExceeded(right),
            )
            | (
                CreateWorkflowInstanceError::AssigneeResolutionFailed(left),
                CreateWorkflowInstanceError::AssigneeResolutionFailed(right),
            ) => assert_eq!(left, right),
            _ => {}
        }
    }

    #[test]
    fn every_deterministic_failure_body_round_trips_through_replay_mapping() {
        use CreateWorkflowInstanceError as E;
        let errors = vec![
            E::PrincipalNotFound,
            E::PrincipalDisabled,
            E::DomainNotFound,
            E::DomainDisabled,
            E::DomainMembershipRequired,
            E::DefinitionVersionNotFound,
            E::VersionNotPublished,
            E::CrossDomainViolation,
            E::ContextValidationFailed("context detail".to_string()),
            E::SizeLimitExceeded("metadata exceeds 64 KiB".to_string()),
            E::AssigneeResolutionFailed("assignee detail".to_string()),
        ];
        for original in errors {
            let body = deterministic_error_body(&original);
            let replayed = replayed_failure_error(deterministic_error_code(&original), &body);
            same_error(&original, &replayed);
        }
    }
}
