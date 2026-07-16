//! Create and detail endpoints.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;

use crate::application::workflow_instance::create::create_workflow_instance;
use crate::application::workflow_instance::query_types::GetWorkflowInstanceDetail;
use crate::auth::AuthenticatedPrincipal;
use crate::domain::ids::{DefinitionVersionId, DomainId, WorkflowInstanceId};
use crate::domain::workflow_instance::commands::CreateWorkflowInstanceCommand;
use crate::http::dto::{
    detail_response, CreateWorkflowInstanceRequest, CreateWorkflowInstanceResponse,
};
use crate::http::error::ApiError;
use crate::http::AppState;

use super::{idempotency_key, path_uuid, require_scope};

pub(crate) async fn create(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    headers: HeaderMap,
    payload: Result<Json<CreateWorkflowInstanceRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    require_scope(&principal, "workflow.execute")?;
    let key = idempotency_key(&headers)?;
    let Json(payload) = payload.map_err(ApiError::from_json_rejection)?;
    if payload
        .external_reference
        .as_ref()
        .is_some_and(|value| value.chars().count() > 512)
    {
        return Err(ApiError::unprocessable(
            "invalid_input",
            "externalReference must not exceed 512 characters",
        ));
    }
    let command = CreateWorkflowInstanceCommand {
        principal_id: principal.principal_id,
        idempotency_key: key,
        command_schema_version: "v1".to_string(),
        domain_id: DomainId::from_uuid(payload.domain_id),
        definition_version_id: DefinitionVersionId::from_uuid(payload.definition_version_id),
        external_reference: payload.external_reference,
        external_url: payload.external_url,
        metadata: payload.metadata,
        context_payload: payload.context_payload,
    };
    let result = create_workflow_instance(&state.pool, command)
        .await
        .map_err(ApiError::from_create)?;
    let response = CreateWorkflowInstanceResponse::from(result);
    let location = format!(
        "/internal/v1/workflow-instances/{}",
        response.workflow_instance_id
    );
    let location = HeaderValue::from_str(&location).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_consistency_error",
            "failed to construct response location",
        )
    })?;
    Ok((
        StatusCode::CREATED,
        [("location", location)],
        Json(response),
    ))
}

pub(crate) async fn detail(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(workflow_instance_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    require_scope(&principal, "workflow.read")?;
    let workflow_instance_id = path_uuid(&workflow_instance_id)?;
    let detail = state
        .query_service
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: principal.principal_id.into_uuid(),
            workflow_instance_id,
        })
        .await
        .map_err(ApiError::from_query)?;
    Ok(Json(detail_response(detail)))
}
