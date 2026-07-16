//! SQLx row types for the workflow transition execution.
//!
//! Additional row types beyond those in `row_types.rs` for transition-specific queries.

use crate::domain::enums::{AssigneeRefType, NodeType};

/// Row type for reading a transition definition with key info.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct TransitionDefinitionRow {
    pub(super) transition_id: uuid::Uuid,
    pub(super) transition_key: String,
    pub(super) definition_version_id: uuid::Uuid,
    pub(super) source_node_id: uuid::Uuid,
    pub(super) target_node_id: uuid::Uuid,
    pub(super) transition_effect: String,
    pub(super) submission_schema: Option<serde_json::Value>,
}

/// Row type for reading a target node definition (for assignee resolution + type check).
#[derive(Debug, sqlx::FromRow)]
pub(super) struct TargetNodeRow {
    pub(super) node_id: uuid::Uuid,
    pub(super) node_type: String,
    pub(super) assignee_ref_type: Option<String>,
    pub(super) fixed_principal_id: Option<uuid::Uuid>,
    pub(super) order_index: i32,
}

impl TargetNodeRow {
    pub(super) fn node_type_enum(&self) -> NodeType {
        self.node_type
            .parse::<NodeType>()
            .unwrap_or(NodeType::NORMAL)
    }

    pub(super) fn assignee_ref_type_enum(&self) -> Option<AssigneeRefType> {
        self.assignee_ref_type
            .as_deref()
            .and_then(|value| value.parse::<AssigneeRefType>().ok())
    }
}

/// Row type for the current node visit with full details (for transition validation).
#[derive(Debug, sqlx::FromRow)]
pub(super) struct CurrentVisitFullRow {
    pub(super) node_visit_id: uuid::Uuid,
    pub(super) node_id: uuid::Uuid,
    pub(super) assignee_principal_id: Option<uuid::Uuid>,
    pub(super) node_type: String,
    pub(super) primary_advance_transition_id: Option<uuid::Uuid>,
    pub(super) order_index: i32,
}

impl CurrentVisitFullRow {
    pub(super) fn node_type_enum(&self) -> NodeType {
        self.node_type
            .parse::<NodeType>()
            .unwrap_or(NodeType::NORMAL)
    }
}

/// Row type for source node definition (primary_advance_transition_id).
#[derive(Debug, sqlx::FromRow)]
pub(super) struct SourceNodeRow {
    pub(super) node_id: uuid::Uuid,
    pub(super) node_type: String,
    pub(super) primary_advance_transition_id: Option<uuid::Uuid>,
    pub(super) order_index: i32,
}

impl SourceNodeRow {
    pub(super) fn node_type_enum(&self) -> NodeType {
        self.node_type
            .parse::<NodeType>()
            .unwrap_or(NodeType::NORMAL)
    }
}
