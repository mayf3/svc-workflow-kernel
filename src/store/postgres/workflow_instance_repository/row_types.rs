//! SQLx row types for the workflow instance repository.
//!
//! These types map PostgreSQL query results to domain types.

use crate::domain::enums::{AssigneeRefType, DefinitionVersionStatus, NodeType};

/// Row type for reading a workflow definition version (subset of columns).
#[derive(Debug, sqlx::FromRow)]
pub(super) struct DefinitionVersionStatusRow {
    pub(super) definition_version_id: uuid::Uuid,
    pub(super) workflow_definition_id: uuid::Uuid,
    pub(super) version_number: i32,
    pub(super) version_status: String,
    pub(super) definition_digest: Option<String>,
    pub(super) json_schema_dialect: Option<String>,
    pub(super) validator_version: Option<String>,
    pub(super) context_schema: Option<serde_json::Value>,
}

impl DefinitionVersionStatusRow {
    pub(super) fn version_status_enum(&self) -> DefinitionVersionStatus {
        self.version_status
            .parse::<DefinitionVersionStatus>()
            .unwrap_or(DefinitionVersionStatus::DRAFT)
    }
}

/// Row type for reading a minimal workflow definition (domain_id).
#[derive(Debug, sqlx::FromRow)]
pub(super) struct WorkflowDefinitionDomainRow {
    pub(super) workflow_definition_id: uuid::Uuid,
    pub(super) domain_id: uuid::Uuid,
}

/// Row type for reading a DRAFT node definition.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct DraftNodeRow {
    pub(super) node_id: uuid::Uuid,
    pub(super) node_type: String,
    pub(super) assignee_ref_type: String,
    pub(super) fixed_principal_id: Option<uuid::Uuid>,
}

/// Row type for locking and reading a workflow instance.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct InstanceLockRow {
    pub(super) workflow_instance_id: uuid::Uuid,
    pub(super) created_by_principal_id: uuid::Uuid,
    pub(super) definition_version_id: uuid::Uuid,
    pub(super) current_context_revision_id: uuid::Uuid,
    pub(super) current_node_visit_id: uuid::Uuid,
    pub(super) workflow_state_version: i32,
}

/// Row type for reading a node visit with its node definition.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct CurrentVisitRow {
    pub(super) node_visit_id: uuid::Uuid,
    pub(super) node_id: uuid::Uuid,
    pub(super) node_type: String,
}

impl CurrentVisitRow {
    pub(super) fn node_type_enum(&self) -> NodeType {
        self.node_type
            .parse::<NodeType>()
            .unwrap_or(NodeType::NORMAL)
    }
}

/// Row type for reading current context revision metadata.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct CurrentContextRow {
    pub(super) context_revision_id: uuid::Uuid,
    pub(super) revision_number: i32,
    pub(super) payload_digest: String,
}

impl DraftNodeRow {
    pub(super) fn node_type_enum(&self) -> NodeType {
        self.node_type
            .parse::<NodeType>()
            .unwrap_or(NodeType::NORMAL)
    }

    pub(super) fn assignee_ref_type_enum(&self) -> AssigneeRefType {
        self.assignee_ref_type
            .parse::<AssigneeRefType>()
            .unwrap_or(AssigneeRefType::WorkflowCreator)
    }
}

/// Row type for reading a domain role binding.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct DomainRoleBindingRow {
    pub(super) binding_id: uuid::Uuid,
    pub(super) domain_id: uuid::Uuid,
    pub(super) principal_id: uuid::Uuid,
    pub(super) role_key: String,
    pub(super) enabled: bool,
}

/// Row type for principal existence / enabled check.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct PrincipalRow {
    pub(super) principal_id: uuid::Uuid,
    pub(super) enabled: bool,
}
