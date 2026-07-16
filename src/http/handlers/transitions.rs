//! Workflow transition endpoint.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;

use crate::application::workflow_instance::execute_transition::execute_workflow_transition;
use crate::auth::AuthenticatedPrincipal;
use crate::domain::ids::{TransitionId, WorkflowInstanceId};
use crate::domain::workflow_instance::commands::ExecuteWorkflowTransitionCommand;
use crate::http::dto::{ExecuteWorkflowTransitionRequest, ExecuteWorkflowTransitionResponse};
use crate::http::error::ApiError;
use crate::http::AppState;

use super::{idempotency_key, path_uuid, require_scope};

pub(crate) async fn execute(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(workflow_instance_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<ExecuteWorkflowTransitionRequest>, JsonRejection>,
) -> Result<Json<ExecuteWorkflowTransitionResponse>, ApiError> {
    require_scope(&principal, "workflow.execute")?;
    let key = idempotency_key(&headers)?;
    let workflow_instance_id = path_uuid(&workflow_instance_id)?;
    let Json(payload) = payload.map_err(ApiError::from_json_rejection)?;
    let result = execute_workflow_transition(
        &state.pool,
        ExecuteWorkflowTransitionCommand {
            principal_id: principal.principal_id,
            idempotency_key: key,
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(workflow_instance_id),
            expected_workflow_state_version: payload.expected_workflow_state_version,
            transition_definition_id: TransitionId::from_uuid(payload.transition_definition_id),
            submission_payload: payload.submission_payload,
        },
    )
    .await
    .map_err(ApiError::from_transition)?;
    Ok(Json(ExecuteWorkflowTransitionResponse::from(result)))
}
