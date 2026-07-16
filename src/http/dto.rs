//! Strict transport DTOs for the internal API.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::application::workflow_instance::create::CreateWorkflowInstanceResult;
use crate::application::workflow_instance::execute_transition::ExecuteWorkflowTransitionResult;
use crate::application::workflow_instance::query_types::{
    WorkflowEventItem, WorkflowInstanceDetail,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateWorkflowInstanceRequest {
    pub domain_id: Uuid,
    pub definition_version_id: Uuid,
    pub external_reference: Option<String>,
    pub external_url: Option<String>,
    pub metadata: serde_json::Value,
    pub context_payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkflowInstanceResponse {
    pub workflow_instance_id: Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub event_sequence: i32,
}

impl From<CreateWorkflowInstanceResult> for CreateWorkflowInstanceResponse {
    fn from(value: CreateWorkflowInstanceResult) -> Self {
        Self {
            workflow_instance_id: value.workflow_instance_id,
            workflow_state_version: value.workflow_state_version,
            current_context_revision_id: value.current_context_revision_id,
            current_node_visit_id: value.current_node_visit_id,
            event_sequence: value.event_sequence,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecuteWorkflowTransitionRequest {
    pub transition_definition_id: Uuid,
    pub expected_workflow_state_version: i32,
    pub submission_payload: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteWorkflowTransitionResponse {
    pub workflow_instance_id: Uuid,
    pub workflow_state_version: i32,
    pub current_context_revision_id: Uuid,
    pub source_node_visit_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub submission_id: Option<Uuid>,
    pub event_sequence: i32,
}

impl From<ExecuteWorkflowTransitionResult> for ExecuteWorkflowTransitionResponse {
    fn from(value: ExecuteWorkflowTransitionResult) -> Self {
        Self {
            workflow_instance_id: value.workflow_instance_id,
            workflow_state_version: value.workflow_state_version,
            current_context_revision_id: value.current_context_revision_id,
            source_node_visit_id: value.source_node_visit_id,
            current_node_visit_id: value.current_node_visit_id,
            submission_id: value.submission_id,
            event_sequence: value.event_sequence,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineQuery {
    pub after: Option<i32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineResponse {
    pub items: Vec<WorkflowEventItem>,
    pub next_cursor: Option<i32>,
}

pub fn detail_response(detail: WorkflowInstanceDetail) -> serde_json::Value {
    match detail {
        WorkflowInstanceDetail::Full(detail) => {
            serde_json::json!({ "visibility": "full", "detail": detail })
        }
        WorkflowInstanceDetail::HistoricalParticipant(detail) => serde_json::json!({
            "visibility": "historical_participant",
            "detail": detail
        }),
    }
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResponse {
    pub service: &'static str,
    pub version: &'static str,
    pub git_sha: &'static str,
    pub schema_version: &'static str,
    pub api_contract_version: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_fields_are_rejected() {
        let value = serde_json::json!({
            "domainId": Uuid::new_v4(),
            "definitionVersionId": Uuid::new_v4(),
            "metadata": {},
            "contextPayload": {},
            "principalId": Uuid::new_v4()
        });
        assert!(serde_json::from_value::<CreateWorkflowInstanceRequest>(value).is_err());
    }
}
