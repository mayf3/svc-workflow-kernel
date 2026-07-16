//! Validation helpers for the atomic creation transaction.
//!
//! Extracted from create_transaction.rs to keep file sizes under 500 lines.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::enums::AssigneeRefType;
use crate::domain::workflow_instance::commands::CreateWorkflowInstanceCommand;
use crate::domain::workflow_instance::errors::CreateWorkflowInstanceError;

use super::command_receipt::complete_receipt;
use super::definition_lookup::DraftNodeInfo;

/// Validate all create payload limits after receipt ownership.
pub(super) fn validate_request_sizes(
    cmd: &CreateWorkflowInstanceCommand,
) -> Result<(), CreateWorkflowInstanceError> {
    let context_bytes = serde_json::to_vec(&cmd.context_payload)
        .map_err(|error| CreateWorkflowInstanceError::StorageError(error.to_string()))?;
    if context_bytes.len() > 1024 * 1024 {
        return Err(CreateWorkflowInstanceError::SizeLimitExceeded(
            "context_payload exceeds 1 MiB".to_string(),
        ));
    }
    let metadata_bytes = serde_json::to_vec(&cmd.metadata)
        .map_err(|error| CreateWorkflowInstanceError::StorageError(error.to_string()))?;
    if metadata_bytes.len() > 64 * 1024 {
        return Err(CreateWorkflowInstanceError::SizeLimitExceeded(
            "metadata exceeds 64 KiB".to_string(),
        ));
    }
    Ok(())
}

/// Validate domain exists and is enabled.
pub(super) async fn validate_domain_enabled(
    tx: &mut Transaction<'_, Postgres>,
    domain_uuid: Uuid,
) -> Result<Option<CreateWorkflowInstanceError>, CreateWorkflowInstanceError> {
    let domain: Option<(bool,)> =
        sqlx::query_as("SELECT enabled FROM domains WHERE domain_id = $1")
            .bind(domain_uuid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| CreateWorkflowInstanceError::StorageError(e.to_string()))?;

    match domain {
        None => Ok(Some(CreateWorkflowInstanceError::DomainNotFound)),
        Some((enabled,)) if !enabled => Ok(Some(CreateWorkflowInstanceError::DomainDisabled)),
        _ => Ok(None),
    }
}

/// Validate principal exists and is enabled.
pub(super) async fn validate_principal_enabled(
    tx: &mut Transaction<'_, Postgres>,
    principal_uuid: Uuid,
) -> Result<Option<CreateWorkflowInstanceError>, CreateWorkflowInstanceError> {
    let principal: Option<(bool,)> =
        sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
            .bind(principal_uuid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| CreateWorkflowInstanceError::StorageError(e.to_string()))?;

    match principal {
        None => Ok(Some(CreateWorkflowInstanceError::PrincipalNotFound)),
        Some((enabled,)) if !enabled => Ok(Some(CreateWorkflowInstanceError::PrincipalDisabled)),
        _ => Ok(None),
    }
}

/// Validate caller has an active domain membership binding.
pub(super) async fn validate_domain_membership(
    tx: &mut Transaction<'_, Postgres>,
    domain_uuid: Uuid,
    principal_uuid: Uuid,
) -> Result<Option<CreateWorkflowInstanceError>, CreateWorkflowInstanceError> {
    let membership: Option<(bool,)> = sqlx::query_as(
        "SELECT enabled FROM domain_role_bindings \
         WHERE domain_id = $1 AND principal_id = $2 AND enabled = TRUE \
         LIMIT 1",
    )
    .bind(domain_uuid)
    .bind(principal_uuid)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| CreateWorkflowInstanceError::StorageError(e.to_string()))?;

    if membership.is_none() {
        Ok(Some(CreateWorkflowInstanceError::DomainMembershipRequired))
    } else {
        Ok(None)
    }
}

/// Resolve the initial assignee principal ID based on the DRAFT node's config.
pub(super) async fn resolve_assignee(
    tx: &mut Transaction<'_, Postgres>,
    draft_node: &DraftNodeInfo,
    principal_uuid: Uuid,
    domain_uuid: Uuid,
) -> Result<Uuid, CreateWorkflowInstanceError> {
    match draft_node.assignee_ref_type {
        AssigneeRefType::WorkflowCreator => Ok(principal_uuid),
        AssigneeRefType::DomainOwner => {
            let owner: Option<(Uuid, bool)> = sqlx::query_as(
                "SELECT principal_id, enabled FROM domain_role_bindings \
                 WHERE domain_id = $1 AND role_key = 'DOMAIN_OWNER' AND enabled = TRUE \
                 LIMIT 1",
            )
            .bind(domain_uuid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| CreateWorkflowInstanceError::StorageError(e.to_string()))?;

            let (owner_id, _) = owner.ok_or_else(|| {
                CreateWorkflowInstanceError::AssigneeResolutionFailed(
                    "no enabled DOMAIN_OWNER found for domain".to_string(),
                )
            })?;

            let owner_enabled: Option<(bool,)> =
                sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
                    .bind(owner_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| CreateWorkflowInstanceError::StorageError(e.to_string()))?;

            match owner_enabled {
                None => Err(CreateWorkflowInstanceError::AssigneeResolutionFailed(
                    "DOMAIN_OWNER principal not found".to_string(),
                )),
                Some((enabled,)) if !enabled => {
                    Err(CreateWorkflowInstanceError::AssigneeResolutionFailed(
                        "DOMAIN_OWNER principal is disabled".to_string(),
                    ))
                }
                _ => Ok(owner_id),
            }
        }
        AssigneeRefType::FixedPrincipal => {
            let fixed_id = draft_node.fixed_principal_id.ok_or_else(|| {
                CreateWorkflowInstanceError::AssigneeResolutionFailed(
                    "FIXED_PRINCIPAL node has no principal_id configured".to_string(),
                )
            })?;

            let fixed_enabled: Option<(bool,)> =
                sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
                    .bind(fixed_id)
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(|e| CreateWorkflowInstanceError::StorageError(e.to_string()))?;

            match fixed_enabled {
                None => Err(CreateWorkflowInstanceError::AssigneeResolutionFailed(
                    "FIXED_PRINCIPAL not found".to_string(),
                )),
                Some((enabled,)) if !enabled => {
                    Err(CreateWorkflowInstanceError::AssigneeResolutionFailed(
                        "FIXED_PRINCIPAL is disabled".to_string(),
                    ))
                }
                _ => Ok(fixed_id),
            }
        }
    }
}

/// Validate context payload against the definition's context_schema.
pub(super) fn validate_context_schema(
    context_schema: &Option<serde_json::Value>,
    cmd: &CreateWorkflowInstanceCommand,
) -> Result<(), CreateWorkflowInstanceError> {
    if let Some(schema) = context_schema {
        jsonschema::validator_for(schema)
            .map_err(|e| {
                CreateWorkflowInstanceError::ContextValidationFailed(format!(
                    "context_schema compilation failed: {}",
                    e
                ))
            })?
            .validate(&cmd.context_payload)
            .map_err(|e| {
                CreateWorkflowInstanceError::ContextValidationFailed(format!(
                    "context_payload failed schema validation: {}",
                    e
                ))
            })?;
    }

    Ok(())
}

/// Map an error to a deterministic HTTP-style status code.
pub(crate) fn deterministic_error_code(err: &CreateWorkflowInstanceError) -> i32 {
    match err {
        CreateWorkflowInstanceError::DomainNotFound => 404,
        CreateWorkflowInstanceError::PrincipalNotFound => 404,
        CreateWorkflowInstanceError::DefinitionVersionNotFound => 404,
        CreateWorkflowInstanceError::DomainDisabled => 403,
        CreateWorkflowInstanceError::PrincipalDisabled => 403,
        CreateWorkflowInstanceError::DomainMembershipRequired => 403,
        CreateWorkflowInstanceError::CrossDomainViolation => 403,
        CreateWorkflowInstanceError::VersionNotPublished => 409,
        CreateWorkflowInstanceError::ContextValidationFailed(_) => 422,
        CreateWorkflowInstanceError::SizeLimitExceeded(_) => 413,
        CreateWorkflowInstanceError::AssigneeResolutionFailed(_) => 422,
        _ => 400,
    }
}

/// Map an error to a deterministic string label for the response body.
pub(crate) fn deterministic_error_label(err: &CreateWorkflowInstanceError) -> &'static str {
    match err {
        CreateWorkflowInstanceError::DomainNotFound => "domain_not_found",
        CreateWorkflowInstanceError::DomainDisabled => "domain_disabled",
        CreateWorkflowInstanceError::PrincipalNotFound => "principal_not_found",
        CreateWorkflowInstanceError::PrincipalDisabled => "principal_disabled",
        CreateWorkflowInstanceError::DomainMembershipRequired => "domain_membership_required",
        CreateWorkflowInstanceError::CrossDomainViolation => "cross_domain_violation",
        CreateWorkflowInstanceError::DefinitionVersionNotFound => "definition_version_not_found",
        CreateWorkflowInstanceError::VersionNotPublished => "version_not_published",
        CreateWorkflowInstanceError::ContextValidationFailed(_) => "context_validation_failed",
        CreateWorkflowInstanceError::SizeLimitExceeded(_) => "size_limit_exceeded",
        CreateWorkflowInstanceError::AssigneeResolutionFailed(_) => "assignee_resolution_failed",
        _ => "validation_error",
    }
}

pub(crate) fn deterministic_error_body(err: &CreateWorkflowInstanceError) -> serde_json::Value {
    let label = deterministic_error_label(err);
    match err {
        CreateWorkflowInstanceError::SizeLimitExceeded(detail)
        | CreateWorkflowInstanceError::ContextValidationFailed(detail)
        | CreateWorkflowInstanceError::AssigneeResolutionFailed(detail) => {
            serde_json::json!({"error": label, "detail": detail})
        }
        _ => serde_json::json!({"error": label}),
    }
}

pub(super) fn is_deterministic_error(err: &CreateWorkflowInstanceError) -> bool {
    matches!(
        err,
        CreateWorkflowInstanceError::PrincipalNotFound
            | CreateWorkflowInstanceError::PrincipalDisabled
            | CreateWorkflowInstanceError::DomainNotFound
            | CreateWorkflowInstanceError::DomainDisabled
            | CreateWorkflowInstanceError::DomainMembershipRequired
            | CreateWorkflowInstanceError::DefinitionVersionNotFound
            | CreateWorkflowInstanceError::VersionNotPublished
            | CreateWorkflowInstanceError::CrossDomainViolation
            | CreateWorkflowInstanceError::ContextValidationFailed(_)
            | CreateWorkflowInstanceError::SizeLimitExceeded(_)
            | CreateWorkflowInstanceError::AssigneeResolutionFailed(_)
    )
}

/// Persist a deterministic failure receipt. Call only before runtime facts are written.
pub(super) async fn persist_deterministic_failure(
    mut tx: Transaction<'_, Postgres>,
    command_id: Uuid,
    err: &CreateWorkflowInstanceError,
) -> Result<(), CreateWorkflowInstanceError> {
    let status = deterministic_error_code(err);
    let body = deterministic_error_body(err);
    let response_digest =
        digest::compute_json_digest(&body).map_err(CreateWorkflowInstanceError::StorageError)?;
    complete_receipt(&mut tx, command_id, status, &body, &response_digest).await?;
    tx.commit()
        .await
        .map_err(|error| CreateWorkflowInstanceError::StorageError(error.to_string()))
}
