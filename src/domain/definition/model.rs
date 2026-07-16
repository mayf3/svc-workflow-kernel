//! Domain model types for workflow definition entities.
//!
//! These types represent the business concepts of Workflow Definition,
//! Definition Version, Node Definitions, and Transition Definitions.
//! They are independent of any storage or serialization format.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::enums::{AssigneeRefType, DefinitionVersionStatus, NodeType, TransitionEffect};
use crate::domain::ids::{
    DefinitionVersionId, DomainId, NodeId, PrincipalId, TransitionId, WorkflowDefinitionId,
};

// ---------------------------------------------------------------------------
// Workflow Definition
// ---------------------------------------------------------------------------

/// A workflow definition (template) that belongs to a domain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowDefinition {
    pub id: WorkflowDefinitionId,
    pub domain_id: DomainId,
    pub definition_key: String,
    pub display_name: String,
    pub description: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Workflow Definition Version
// ---------------------------------------------------------------------------

/// A versioned snapshot of a workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowDefinitionVersion {
    pub id: DefinitionVersionId,
    pub workflow_definition_id: WorkflowDefinitionId,
    pub version_number: i32,
    pub version_status: DefinitionVersionStatus,
    pub definition_digest: Option<String>,
    pub json_schema_dialect: Option<String>,
    pub validator_version: Option<String>,
    pub context_schema: Option<serde_json::Value>,
    pub submission_schema: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub deprecated_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub published_by_principal_id: Option<PrincipalId>,
    pub deprecated_by_principal_id: Option<PrincipalId>,
    pub revoked_by_principal_id: Option<PrincipalId>,
}

// ---------------------------------------------------------------------------
// Node Definition
// ---------------------------------------------------------------------------

/// A node (step) within a workflow definition version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeDefinition {
    pub node_id: NodeId,
    pub definition_version_id: DefinitionVersionId,
    pub node_key: String,
    pub display_name: String,
    pub order_index: i32,
    pub node_type: NodeType,
    /// Terminal nodes have no assignee reference. Non-terminal nodes must have one.
    pub assignee_ref: Option<AssigneeRef>,
    pub instructions: Option<String>,
    pub primary_advance_transition_id: Option<TransitionId>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Represents how a node's assignee is resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssigneeRef {
    pub ref_type: AssigneeRefType,
    pub fixed_principal_id: Option<PrincipalId>,
}

// ---------------------------------------------------------------------------
// Transition Definition
// ---------------------------------------------------------------------------

/// A transition (edge) connecting two nodes in a workflow definition version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransitionDefinition {
    pub transition_id: TransitionId,
    pub definition_version_id: DefinitionVersionId,
    pub transition_key: String,
    pub display_name: String,
    pub source_node_id: NodeId,
    pub target_node_id: NodeId,
    pub transition_effect: TransitionEffect,
    pub submission_schema: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Helper types for graph representation
// ---------------------------------------------------------------------------

/// A complete workflow graph bundled with its context schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowGraph {
    pub nodes: Vec<NodeDefinition>,
    pub transitions: Vec<TransitionDefinition>,
    pub context_schema: Option<serde_json::Value>,
}

/// Result of graph validation.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<super::error::GraphValidationError>,
    pub warnings: Vec<String>,
    pub computed_digest: Option<String>,
}

/// Result of publishing a version.
#[derive(Debug, Clone)]
pub struct PublishResult {
    pub version: WorkflowDefinitionVersion,
    pub digest: String,
}

// ---------------------------------------------------------------------------
// Sort keys helpers (used in digest computation)
// ---------------------------------------------------------------------------

impl NodeDefinition {
    /// Stable sort key for canonical ordering.
    pub fn sort_key(&self) -> &str {
        &self.node_key
    }
}

impl TransitionDefinition {
    /// Stable sort key for canonical ordering.
    pub fn sort_key(&self) -> &str {
        &self.transition_key
    }
}
