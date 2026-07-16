//! Event type constants and event data structures for the Workflow Instance domain.

use serde::{Deserialize, Serialize};

/// Stable schema version for all events created by this service.
pub const EVENT_SCHEMA_VERSION: &str = "v1";

/// Stable command type string for CreateWorkflowInstance.
pub const COMMAND_TYPE_CREATE_INSTANCE: &str = "CREATE_WORKFLOW_INSTANCE";

/// Stable command type string for ReviseWorkflowContext.
pub const COMMAND_TYPE_REVISE_CONTEXT: &str = "REVISE_WORKFLOW_CONTEXT";

/// Stable command type string for ExecuteWorkflowTransition.
pub const COMMAND_TYPE_EXECUTE_TRANSITION: &str = "EXECUTE_WORKFLOW_TRANSITION";

/// Stable command type string for ReviseContextAndTransition.
pub const COMMAND_TYPE_REVISE_CONTEXT_AND_TRANSITION: &str = "REVISE_CONTEXT_AND_TRANSITION";

/// Canonical command and event names for the legacy initial-import primitive.
pub const COMMAND_TYPE_IMPORT_LEGACY_INSTANCE: &str = "IMPORT_LEGACY_WORKFLOW_INSTANCE";
pub const WORKFLOW_INSTANCE_IMPORTED_EVENT_TYPE: &str = "WORKFLOW_INSTANCE_IMPORTED";

/// Event type for instance creation events.
pub const INSTANCE_CREATED_EVENT_TYPE: &str = "INSTANCE_CREATED";

/// Event type for context revision events.
pub const CONTEXT_REVISED_EVENT_TYPE: &str = "CONTEXT_REVISED";

/// Event type for transition execution events.
pub const TRANSITION_COMMITTED_EVENT_TYPE: &str = "WORKFLOW_TRANSITION_COMMITTED";

/// Event type for the atomic context revision + transition command.
pub const CONTEXT_REVISED_AND_TRANSITION_COMMITTED_EVENT_TYPE: &str =
    "WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED";

/// Canonical writer event type for PR5 emergency override.
pub const ADMIN_EMERGENCY_OVERRIDE_COMMITTED_EVENT_TYPE: &str =
    "ADMIN_EMERGENCY_OVERRIDE_COMMITTED";

/// Non-sensitive event data embedded in the INSTANCE_CREATED event.
///
/// This is the stable, serialized content of `event_data`. It must
/// never include the full context payload, credentials, or secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceCreatedEventData {
    /// The definition version ID that was instantiated.
    pub definition_version_id: String,
    /// The SHA-256 digest of the definition version at creation time.
    pub definition_digest: String,
    /// The node ID of the initial DRAFT node.
    pub initial_node_id: String,
    /// How the initial assignee was resolved (WORKFLOW_CREATOR, DOMAIN_OWNER, FIXED_PRINCIPAL).
    pub assignee_resolution_type: String,
}

/// Non-sensitive event data embedded in the CONTEXT_REVISED event.
///
/// Contains only stable identifiers and digests — never the full context payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRevisedEventData {
    /// The context revision ID that was the current revision before this command.
    pub previous_context_revision_id: String,
    /// The context revision ID created by this command.
    pub new_context_revision_id: String,
    /// SHA-256 digest of the previous context payload.
    pub previous_payload_digest: String,
    /// SHA-256 digest of the new context payload.
    pub new_payload_digest: String,
    /// The node ID of the current node visit (unchanged by this command).
    pub current_node_id: String,
}

/// Non-sensitive event data embedded in the WORKFLOW_TRANSITION_COMMITTED event.
///
/// Contains only stable identifiers and digests — never the full submission payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionCommittedEventData {
    /// The transition definition ID that was executed.
    pub transition_definition_id: String,
    /// The transition key (human-readable identifier).
    pub transition_key: String,
    /// The transition effect (ADVANCE, RETURN, TERMINATE).
    pub transition_effect: String,
    /// The node ID of the source (pre-transition) node.
    pub source_node_id: String,
    /// The node ID of the target (post-transition) node.
    pub target_node_id: String,
    /// The node visit ID of the source visit.
    pub source_node_visit_id: String,
    /// The node visit ID of the target visit.
    pub target_node_visit_id: String,
    /// The context revision ID used for this transition.
    pub context_revision_id: String,
    /// SHA-256 digest of the submission payload, or null if no submission.
    pub submission_payload_digest: Option<String>,
}

/// Non-sensitive event data for an atomic context revision + transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRevisedAndTransitionCommittedEventData {
    pub previous_context_revision_id: String,
    pub new_context_revision_id: String,
    pub previous_context_payload_digest: String,
    pub new_context_payload_digest: String,
    pub transition_definition_id: String,
    pub transition_key: String,
    pub transition_effect: String,
    pub source_node_id: String,
    pub target_node_id: String,
    pub source_node_visit_id: String,
    pub target_node_visit_id: String,
    pub submission_payload_digest: String,
}
