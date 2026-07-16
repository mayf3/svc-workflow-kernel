use uuid::Uuid;

use crate::domain::workflow_instance::recovery::WorkflowProjection;

#[derive(Debug, sqlx::FromRow)]
pub(super) struct InstanceRow {
    pub workflow_instance_id: Uuid,
    pub domain_id: Uuid,
    pub definition_version_id: Uuid,
    pub created_by_principal_id: Uuid,
    pub created_by_principal_type: String,
    pub external_reference: Option<String>,
    pub current_context_revision_id: Option<Uuid>,
    pub current_node_visit_id: Option<Uuid>,
    pub workflow_state_version: i32,
}

impl InstanceRow {
    pub(super) fn projection(&self) -> WorkflowProjection {
        WorkflowProjection {
            current_context_revision_id: self.current_context_revision_id,
            current_node_visit_id: self.current_node_visit_id,
            workflow_state_version: self.workflow_state_version,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct ContextFact {
    pub context_revision_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub revision_number: i32,
    pub previous_revision_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub payload_digest: String,
    pub created_by_principal_id: Uuid,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct VisitFact {
    pub node_visit_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub node_id: Uuid,
    pub visit_number: i32,
    pub assignee_principal_id: Option<Uuid>,
    pub entered_by_transition_id: Option<Uuid>,
    pub definition_version_id: Uuid,
    pub node_type: String,
    pub assignee_ref_type: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct SubmissionFact {
    pub submission_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub source_node_visit_id: Uuid,
    pub context_revision_id: Uuid,
    pub transition_id: Uuid,
    pub payload: serde_json::Value,
    pub payload_digest: String,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct TransitionFact {
    pub transition_id: Uuid,
    pub definition_version_id: Uuid,
    pub transition_key: String,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub transition_effect: String,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct EventFact {
    pub event_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub event_sequence: i32,
    pub event_schema_version: String,
    pub event_type: String,
    pub transition_effect: Option<String>,
    pub source_node_visit_id: Option<Uuid>,
    pub target_node_visit_id: Option<Uuid>,
    pub context_revision_id: Option<Uuid>,
    pub submission_id: Option<Uuid>,
    pub event_data: Option<serde_json::Value>,
    pub event_data_digest: Option<String>,
    pub command_id: Option<Uuid>,
    pub actor_principal_id: Uuid,
    pub actor_principal_type: String,
    pub from_node_id: Option<Uuid>,
    pub to_node_id: Option<Uuid>,
    pub old_workflow_state_version: i32,
    pub new_workflow_state_version: i32,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct TargetNodeRow {
    pub node_id: Uuid,
    pub definition_version_id: Uuid,
    pub node_type: String,
    pub assignee_ref_type: Option<String>,
    pub fixed_principal_id: Option<Uuid>,
}
