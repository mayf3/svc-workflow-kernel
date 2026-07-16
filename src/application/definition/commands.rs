//! Command input types for the Definition Application Service.
//!
//! These are the "what" — the service receives these and produces results.

use uuid::Uuid;

/// Create a new Workflow Definition.
#[derive(Debug, Clone)]
pub struct CreateDefinition {
    pub actor_principal_id: Uuid,
    pub owner_domain_id: Uuid,
    pub definition_key: String,
    pub display_name: String,
    pub description: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Create a new DRAFT version of a workflow definition.
#[derive(Debug, Clone)]
pub struct CreateDraftVersion {
    pub actor_principal_id: Uuid,
    pub workflow_definition_id: Uuid,
    pub context_schema: Option<serde_json::Value>,
    pub json_schema_dialect: Option<String>,
    pub validator_version: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Atomically replace the graph (nodes + transitions) of a DRAFT version.
#[derive(Debug, Clone)]
pub struct ReplaceDraftGraph {
    pub actor_principal_id: Uuid,
    pub definition_version_id: Uuid,
    pub context_schema: Option<serde_json::Value>,
    pub nodes: Vec<RawNodeDefinition>,
    pub transitions: Vec<RawTransitionDefinition>,
}

/// A node in the graph replacement command.
#[derive(Debug, Clone)]
pub struct RawNodeDefinition {
    pub node_key: String,
    pub display_name: String,
    pub order_index: i32,
    pub node_type: String,
    /// Must be `None` for TERMINAL and `Some` for every non-terminal node.
    pub assignee_ref_type: Option<String>,
    pub fixed_principal_id: Option<Uuid>,
    pub instructions: Option<String>,
    pub primary_advance_transition_key: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// A transition in the graph replacement command.
#[derive(Debug, Clone)]
pub struct RawTransitionDefinition {
    pub transition_key: String,
    pub display_name: String,
    pub source_node_key: String,
    pub target_node_key: String,
    pub transition_effect: String,
    pub submission_schema: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

/// Validate a DRAFT version without changing state.
#[derive(Debug, Clone)]
pub struct ValidateDraftVersion {
    pub actor_principal_id: Uuid,
    pub definition_version_id: Uuid,
}

/// Publish a DRAFT version -> PUBLISHED.
#[derive(Debug, Clone)]
pub struct PublishVersion {
    pub actor_principal_id: Uuid,
    pub definition_version_id: Uuid,
}

/// Deprecate a PUBLISHED version -> DEPRECATED.
#[derive(Debug, Clone)]
pub struct DeprecateVersion {
    pub actor_principal_id: Uuid,
    pub definition_version_id: Uuid,
}

/// Revoke a PUBLISHED or DEPRECATED version -> REVOKED.
#[derive(Debug, Clone)]
pub struct RevokeVersion {
    pub actor_principal_id: Uuid,
    pub definition_version_id: Uuid,
}
