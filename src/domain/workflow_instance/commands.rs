//! Command input types for the Workflow Instance domain.

use crate::domain::ids::{
    DefinitionVersionId, DomainId, PrincipalId, TransitionId, WorkflowInstanceId,
};

/// Command to create a new workflow instance from a published definition version.
///
/// This is the sole command for PR 3A. All fields are required except
/// where explicitly marked as optional.
#[derive(Debug, Clone)]
pub struct CreateWorkflowInstanceCommand {
    /// The principal initiating the command.
    pub principal_id: PrincipalId,

    /// Client-supplied idempotency key, unique per principal.
    pub idempotency_key: String,

    /// Schema version of this command structure.
    pub command_schema_version: String,

    /// Target domain for the new instance.
    pub domain_id: DomainId,

    /// Published definition version to instantiate.
    pub definition_version_id: DefinitionVersionId,

    /// Optional caller-supplied external reference identifier.
    pub external_reference: Option<String>,

    /// Optional external URL associated with the instance.
    pub external_url: Option<String>,

    /// Arbitrary metadata attached to the instance.
    pub metadata: serde_json::Value,

    /// Initial context payload (validated against the definition's context_schema).
    pub context_payload: serde_json::Value,
}

/// Command to create a new revision of the workflow context for an existing instance.
///
/// This is the sole command for PR 3B. Only the Workflow Creator (the principal
/// whose ID equals `workflow_instance.created_by_principal_id`) may revise the
/// context, and only while the current node is of type DRAFT.
#[derive(Debug, Clone)]
pub struct ReviseWorkflowContextCommand {
    /// The principal initiating the command.
    pub principal_id: PrincipalId,

    /// Client-supplied idempotency key, unique per principal.
    pub idempotency_key: String,

    /// Schema version of this command structure.
    pub command_schema_version: String,

    /// The target workflow instance.
    pub workflow_instance_id: WorkflowInstanceId,

    /// The caller's expected current workflow state version (optimistic concurrency).
    pub expected_workflow_state_version: i32,

    /// The new context payload to store.
    pub context_payload: serde_json::Value,
}

/// Command to execute a workflow transition (ADVANCE, RETURN, or TERMINATE).
///
/// This is the sole command for PR 3C. The caller must be the current assignee
/// of the instance's current node visit. The transition is selected by
/// `transition_definition_id`, not by effect or target node.
///
/// PR 3C does NOT modify context — it only transitions the workflow state.
/// PR 3D will combine context revision + transition in a single command.
#[derive(Debug, Clone)]
pub struct ExecuteWorkflowTransitionCommand {
    /// The principal initiating the command (must be current node visit assignee).
    pub principal_id: PrincipalId,

    /// Client-supplied idempotency key, unique per principal.
    pub idempotency_key: String,

    /// Schema version of this command structure.
    pub command_schema_version: String,

    /// The target workflow instance.
    pub workflow_instance_id: WorkflowInstanceId,

    /// The caller's expected current workflow state version (optimistic concurrency).
    pub expected_workflow_state_version: i32,

    /// The transition definition to execute (UUID primary key).
    pub transition_definition_id: TransitionId,

    /// Optional submission payload. `None` means no submission is provided.
    /// `Some(Value::Null)` means an explicitly null payload is provided.
    pub submission_payload: Option<serde_json::Value>,
}

/// Atomically revise the DRAFT context and execute its primary ADVANCE transition.
///
/// The caller must be both the workflow creator and the current visit assignee.
/// Both payloads are required: the submission is always bound to the context
/// revision created by this command.
#[derive(Debug, Clone)]
pub struct ReviseContextAndTransitionCommand {
    pub principal_id: PrincipalId,
    pub idempotency_key: String,
    pub command_schema_version: String,
    pub workflow_instance_id: WorkflowInstanceId,
    pub expected_workflow_state_version: i32,
    pub transition_definition_id: TransitionId,
    pub context_payload: serde_json::Value,
    pub submission_payload: serde_json::Value,
}
