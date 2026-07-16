//! Workflow event timeline endpoint.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::Json;

use crate::application::workflow_instance::query_types::ListWorkflowTimeline;
use crate::auth::AuthenticatedPrincipal;
use crate::http::dto::{TimelineQuery, TimelineResponse};
use crate::http::error::ApiError;
use crate::http::AppState;

use super::{path_uuid, require_scope};

pub(crate) async fn list(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(workflow_instance_id): Path<String>,
    query: Result<Query<TimelineQuery>, QueryRejection>,
) -> Result<Json<TimelineResponse>, ApiError> {
    require_scope(&principal, "workflow.read")?;
    let workflow_instance_id = path_uuid(&workflow_instance_id)?;
    let Query(query) = query.map_err(ApiError::from_query_rejection)?;
    let page = state
        .query_service
        .list_workflow_timeline(ListWorkflowTimeline {
            actor_principal_id: principal.principal_id.into_uuid(),
            workflow_instance_id,
            after_event_sequence: query.after,
            limit: query.limit,
        })
        .await
        .map_err(ApiError::from_query)?;
    Ok(Json(TimelineResponse {
        items: page.items,
        next_cursor: page.next_cursor,
    }))
}
