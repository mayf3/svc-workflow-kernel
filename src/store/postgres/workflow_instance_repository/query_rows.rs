use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::application::workflow_instance::query_types::{
    ContextRevisionItem, NodeVisitItem, ParticipantWorkflowInstanceSummary, PublicNodeSummary,
    SubmissionHistoryItem, WorkflowEventItem, WorkflowInstanceSummary,
};

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct QueryBaseRow {
    pub workflow_instance_id: Uuid,
    pub domain_id: Uuid,
    pub definition_domain_id: Uuid,
    pub definition_version_id: Uuid,
    pub definition_version_status: String,
    pub created_by_principal_id: Uuid,
    pub current_context_revision_id: Option<Uuid>,
    pub current_node_visit_id: Option<Uuid>,
    pub workflow_state_version: i32,
    pub external_reference: Option<String>,
    pub external_url: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub instance_created_at: DateTime<Utc>,
    pub domain_enabled: bool,
    pub context_instance_id: Option<Uuid>,
    pub context_revision_number: Option<i32>,
    pub context_previous_revision_id: Option<Uuid>,
    pub context_payload: Option<serde_json::Value>,
    pub context_payload_digest: Option<String>,
    pub context_created_by_principal_id: Option<Uuid>,
    pub context_created_at: Option<DateTime<Utc>>,
    pub visit_instance_id: Option<Uuid>,
    pub current_node_id: Option<Uuid>,
    pub visit_number: Option<i32>,
    pub current_assignee_principal_id: Option<Uuid>,
    pub entered_by_transition_id: Option<Uuid>,
    pub visit_created_at: Option<DateTime<Utc>>,
    pub node_definition_version_id: Option<Uuid>,
    pub current_node_key: Option<String>,
    pub current_node_display_name: Option<String>,
    pub current_node_type: Option<String>,
    pub current_node_instructions: Option<String>,
    pub current_primary_advance_transition_id: Option<Uuid>,
    pub event_count: i64,
    pub min_event_sequence: Option<i32>,
    pub max_event_sequence: Option<i32>,
    pub event_references_consistent: Option<bool>,
}

impl QueryBaseRow {
    pub(crate) fn summary(&self) -> Option<WorkflowInstanceSummary> {
        Some(WorkflowInstanceSummary {
            workflow_instance_id: self.workflow_instance_id,
            domain_id: self.domain_id,
            definition_version_id: self.definition_version_id,
            definition_version_status: self.definition_version_status.clone(),
            created_by_principal_id: self.created_by_principal_id,
            workflow_state_version: self.workflow_state_version,
            external_reference: self.external_reference.clone(),
            external_url: self.external_url.clone(),
            metadata: self.metadata.clone(),
            created_at: self.instance_created_at,
            domain_enabled: self.domain_enabled,
            is_terminal: self.current_node_type.as_deref()? == "TERMINAL",
            current_node: PublicNodeSummary {
                node_id: self.current_node_id?,
                node_key: self.current_node_key.clone()?,
                display_name: self.current_node_display_name.clone()?,
                node_type: self.current_node_type.clone()?,
            },
        })
    }

    pub(crate) fn participant_summary(&self) -> Option<ParticipantWorkflowInstanceSummary> {
        let current_node = PublicNodeSummary {
            node_id: self.current_node_id?,
            node_key: self.current_node_key.clone()?,
            display_name: self.current_node_display_name.clone()?,
            node_type: self.current_node_type.clone()?,
        };
        Some(ParticipantWorkflowInstanceSummary {
            workflow_instance_id: self.workflow_instance_id,
            domain_id: self.domain_id,
            definition_version_id: self.definition_version_id,
            definition_version_status: self.definition_version_status.clone(),
            workflow_state_version: self.workflow_state_version,
            created_at: self.instance_created_at,
            domain_enabled: self.domain_enabled,
            is_terminal: current_node.node_type == "TERMINAL",
            current_node,
        })
    }

    pub(crate) fn current_context(&self) -> Option<ContextRevisionItem> {
        Some(ContextRevisionItem {
            context_revision_id: self.current_context_revision_id?,
            workflow_instance_id: self.context_instance_id?,
            revision_number: self.context_revision_number?,
            previous_revision_id: self.context_previous_revision_id,
            payload: self.context_payload.clone()?,
            payload_digest: self.context_payload_digest.clone()?,
            created_by_principal_id: self.context_created_by_principal_id?,
            created_at: self.context_created_at?,
        })
    }

    pub(crate) fn current_visit(&self, include_instructions: bool) -> Option<NodeVisitItem> {
        Some(NodeVisitItem {
            node_visit_id: self.current_node_visit_id?,
            workflow_instance_id: self.visit_instance_id?,
            node: PublicNodeSummary {
                node_id: self.current_node_id?,
                node_key: self.current_node_key.clone()?,
                display_name: self.current_node_display_name.clone()?,
                node_type: self.current_node_type.clone()?,
            },
            visit_number: self.visit_number?,
            assignee_principal_id: self.current_assignee_principal_id,
            entered_by_transition_id: self.entered_by_transition_id,
            instructions: include_instructions
                .then(|| self.current_node_instructions.clone())
                .flatten(),
            created_at: self.visit_created_at?,
        })
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct OutgoingRow {
    pub transition_id: Uuid,
    pub transition_key: String,
    pub display_name: String,
    pub transition_effect: String,
    pub submission_schema: Option<serde_json::Value>,
    pub transition_definition_version_id: Uuid,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub target_definition_version_id: Uuid,
    pub target_node_key: String,
    pub target_display_name: String,
    pub target_node_type: String,
    pub target_assignee_ref_type: Option<String>,
    pub target_fixed_principal_id: Option<Uuid>,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct EventRow {
    pub event_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub event_sequence: i32,
    pub event_schema_version: String,
    pub command_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub correlation_id: Option<Uuid>,
    pub event_type: String,
    pub transition_effect: Option<String>,
    pub source_node_visit_id: Option<Uuid>,
    pub target_node_visit_id: Option<Uuid>,
    pub context_revision_id: Option<Uuid>,
    pub submission_id: Option<Uuid>,
    pub event_data: Option<serde_json::Value>,
    pub event_data_digest: Option<String>,
    pub actor_principal_id: Uuid,
    pub from_node_id: Option<Uuid>,
    pub to_node_id: Option<Uuid>,
    pub old_workflow_state_version: i32,
    pub new_workflow_state_version: i32,
    pub created_at: DateTime<Utc>,
    pub references_consistent: bool,
}

impl EventRow {
    pub(crate) fn into_item(self) -> WorkflowEventItem {
        WorkflowEventItem {
            event_id: self.event_id,
            workflow_instance_id: self.workflow_instance_id,
            event_sequence: self.event_sequence,
            event_schema_version: self.event_schema_version,
            command_id: self.command_id,
            causation_id: self.causation_id,
            correlation_id: self.correlation_id,
            event_type: self.event_type,
            transition_effect: self.transition_effect,
            source_node_visit_id: self.source_node_visit_id,
            target_node_visit_id: self.target_node_visit_id,
            context_revision_id: self.context_revision_id,
            submission_id: self.submission_id,
            event_data: self.event_data,
            event_data_digest: self.event_data_digest,
            actor_principal_id: self.actor_principal_id,
            from_node_id: self.from_node_id,
            to_node_id: self.to_node_id,
            old_workflow_state_version: self.old_workflow_state_version,
            new_workflow_state_version: self.new_workflow_state_version,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct ContextRow {
    pub context_revision_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub revision_number: i32,
    pub previous_revision_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub payload_digest: String,
    pub created_by_principal_id: Uuid,
    pub created_at: DateTime<Utc>,
}

impl ContextRow {
    pub(crate) fn into_item(self) -> ContextRevisionItem {
        ContextRevisionItem {
            context_revision_id: self.context_revision_id,
            workflow_instance_id: self.workflow_instance_id,
            revision_number: self.revision_number,
            previous_revision_id: self.previous_revision_id,
            payload: self.payload,
            payload_digest: self.payload_digest,
            created_by_principal_id: self.created_by_principal_id,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct VisitRow {
    pub node_visit_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub node_id: Uuid,
    pub node_definition_version_id: Uuid,
    pub node_key: String,
    pub display_name: String,
    pub node_type: String,
    pub visit_number: i32,
    pub assignee_principal_id: Option<Uuid>,
    pub entered_by_transition_id: Option<Uuid>,
    pub instructions: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl VisitRow {
    pub(crate) fn into_item(self, include_instructions: bool) -> NodeVisitItem {
        NodeVisitItem {
            node_visit_id: self.node_visit_id,
            workflow_instance_id: self.workflow_instance_id,
            node: PublicNodeSummary {
                node_id: self.node_id,
                node_key: self.node_key,
                display_name: self.display_name,
                node_type: self.node_type,
            },
            visit_number: self.visit_number,
            assignee_principal_id: self.assignee_principal_id,
            entered_by_transition_id: self.entered_by_transition_id,
            instructions: include_instructions.then_some(self.instructions).flatten(),
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct SubmissionRow {
    pub submission_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub source_node_visit_id: Uuid,
    pub source_visit_instance_id: Uuid,
    pub source_node_id: Uuid,
    pub source_node_definition_version_id: Uuid,
    pub source_node_key: String,
    pub source_node_display_name: String,
    pub source_node_type: String,
    pub context_revision_id: Uuid,
    pub context_instance_id: Uuid,
    pub author_principal_id: Uuid,
    pub transition_id: Uuid,
    pub transition_definition_version_id: Uuid,
    pub transition_effect: String,
    pub payload: serde_json::Value,
    pub payload_digest: String,
    pub schema_version: String,
    pub created_at: DateTime<Utc>,
}

impl SubmissionRow {
    pub(crate) fn into_item(self) -> SubmissionHistoryItem {
        SubmissionHistoryItem {
            submission_id: self.submission_id,
            workflow_instance_id: self.workflow_instance_id,
            source_node_visit_id: self.source_node_visit_id,
            source_node: PublicNodeSummary {
                node_id: self.source_node_id,
                node_key: self.source_node_key,
                display_name: self.source_node_display_name,
                node_type: self.source_node_type,
            },
            context_revision_id: self.context_revision_id,
            author_principal_id: self.author_principal_id,
            transition_id: self.transition_id,
            transition_effect: self.transition_effect,
            payload: self.payload,
            payload_digest: self.payload_digest,
            schema_version: self.schema_version,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct InstanceCursorRow {
    pub workflow_instance_id: Uuid,
    pub created_at: DateTime<Utc>,
}
