//! Public request and response types for PostgreSQL-backed workflow queries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowQueryError {
    PrincipalNotFound,
    PrincipalDisabled,
    WorkflowInstanceNotFoundOrNotVisible,
    RestrictedHistoryNotVisible,
    InvalidPagination(String),
    InternalConsistency(String),
    StorageError(String),
}

impl std::fmt::Display for WorkflowQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrincipalNotFound => write!(f, "principal not found"),
            Self::PrincipalDisabled => write!(f, "principal is disabled"),
            Self::WorkflowInstanceNotFoundOrNotVisible => {
                write!(f, "workflow instance not found or not visible")
            }
            Self::RestrictedHistoryNotVisible => write!(f, "restricted history not visible"),
            Self::InvalidPagination(detail) => write!(f, "invalid pagination: {detail}"),
            Self::InternalConsistency(detail) => {
                write!(f, "internal consistency error: {detail}")
            }
            Self::StorageError(detail) => write!(f, "storage error: {detail}"),
        }
    }
}

impl std::error::Error for WorkflowQueryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeUuidCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T, C = TimeUuidCursor> {
    pub items: Vec<T>,
    pub next_cursor: Option<C>,
}

#[derive(Debug, Clone, Copy)]
pub struct GetWorkflowInstanceDetail {
    pub actor_principal_id: Uuid,
    pub workflow_instance_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub struct ListWorkflowTimeline {
    pub actor_principal_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub after_event_sequence: Option<i32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct ListContextRevisions {
    pub actor_principal_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub after_revision_number: Option<i32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct ListNodeVisits {
    pub actor_principal_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub after: Option<TimeUuidCursor>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct ListSubmissionHistory {
    pub actor_principal_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub after: Option<TimeUuidCursor>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct ListAssignedToMe {
    pub actor_principal_id: Uuid,
    pub before: Option<TimeUuidCursor>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct ListCreatorOwnedDrafts {
    pub actor_principal_id: Uuid,
    pub before: Option<TimeUuidCursor>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicNodeSummary {
    pub node_id: Uuid,
    pub node_key: String,
    pub display_name: String,
    pub node_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowInstanceSummary {
    pub workflow_instance_id: Uuid,
    pub domain_id: Uuid,
    pub definition_version_id: Uuid,
    pub definition_version_status: String,
    pub created_by_principal_id: Uuid,
    pub workflow_state_version: i32,
    pub external_reference: Option<String>,
    pub external_url: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub domain_enabled: bool,
    pub is_terminal: bool,
    pub current_node: PublicNodeSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextRevisionItem {
    pub context_revision_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub revision_number: i32,
    pub previous_revision_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub payload_digest: String,
    pub created_by_principal_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeVisitItem {
    pub node_visit_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub node: PublicNodeSummary,
    pub visit_number: i32,
    /// `None` is the canonical representation for a Terminal visit.
    pub assignee_principal_id: Option<Uuid>,
    pub entered_by_transition_id: Option<Uuid>,
    pub instructions: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransitionBlockedReason {
    ActorNotCurrentAssignee,
    CurrentNodeTerminal,
    DefinitionVersionRevoked,
    DefinitionVersionDraft,
    AdvanceNotPrimary,
    TargetAssigneeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutgoingTransitionItem {
    pub transition_id: Uuid,
    pub transition_key: String,
    pub display_name: String,
    pub transition_effect: String,
    pub target_node: PublicNodeSummary,
    pub submission_schema: Option<serde_json::Value>,
    pub executable_for_actor: bool,
    pub blocked_reason: Option<TransitionBlockedReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FullWorkflowInstanceDetail {
    pub instance: WorkflowInstanceSummary,
    pub current_context_revision_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub current_context: ContextRevisionItem,
    pub current_visit: NodeVisitItem,
    pub outgoing_transitions: Vec<OutgoingTransitionItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantWorkflowInstanceDetail {
    pub instance: ParticipantWorkflowInstanceSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantWorkflowInstanceSummary {
    pub workflow_instance_id: Uuid,
    pub domain_id: Uuid,
    pub definition_version_id: Uuid,
    pub definition_version_status: String,
    pub workflow_state_version: i32,
    pub created_at: DateTime<Utc>,
    pub domain_enabled: bool,
    pub is_terminal: bool,
    pub current_node: PublicNodeSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "visibility", content = "detail")]
pub enum WorkflowInstanceDetail {
    Full(Box<FullWorkflowInstanceDetail>),
    HistoricalParticipant(ParticipantWorkflowInstanceDetail),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowEventItem {
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmissionHistoryItem {
    pub submission_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub source_node_visit_id: Uuid,
    pub source_node: PublicNodeSummary,
    pub context_revision_id: Uuid,
    pub author_principal_id: Uuid,
    pub transition_id: Uuid,
    pub transition_effect: String,
    pub payload: serde_json::Value,
    pub payload_digest: String,
    pub schema_version: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssignedWorkItem {
    pub detail: FullWorkflowInstanceDetail,
    pub upstream_submissions: Vec<SubmissionHistoryItem>,
    pub return_feedback_events: Vec<WorkflowEventItem>,
    pub submissions_truncated: bool,
    pub return_events_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatorDraftItem {
    pub detail: FullWorkflowInstanceDetail,
    pub context_editable: bool,
    pub combined_executable: bool,
}
