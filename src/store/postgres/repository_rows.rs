//! Row types for SQLx queries used by [`super::definition_repository`].

use uuid::Uuid;

use crate::domain::definition::model::{
    AssigneeRef, NodeDefinition, TransitionDefinition, WorkflowDefinition,
    WorkflowDefinitionVersion,
};
use crate::domain::enums::{AssigneeRefType, DefinitionVersionStatus, NodeType, TransitionEffect};
use crate::domain::ids::{
    DefinitionVersionId, DomainId, NodeId, PrincipalId, TransitionId, WorkflowDefinitionId,
};

// ---------------------------------------------------------------------------
// Row types for SQLx query_as
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub(super) struct WorkflowDefinitionRow {
    workflow_definition_id: Uuid,
    domain_id: Uuid,
    definition_key: String,
    display_name: String,
    description: Option<String>,
    metadata: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl WorkflowDefinitionRow {
    pub(super) fn into_domain(self) -> WorkflowDefinition {
        WorkflowDefinition {
            id: WorkflowDefinitionId::from_uuid(self.workflow_definition_id),
            domain_id: DomainId::from_uuid(self.domain_id),
            definition_key: self.definition_key,
            display_name: self.display_name,
            description: self.description,
            metadata: self.metadata,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct WorkflowDefinitionVersionRow {
    definition_version_id: Uuid,
    workflow_definition_id: Uuid,
    version_number: i32,
    version_status: String,
    definition_digest: Option<String>,
    json_schema_dialect: Option<String>,
    validator_version: Option<String>,
    context_schema: Option<serde_json::Value>,
    submission_schema: Option<serde_json::Value>,
    metadata: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    published_at: Option<chrono::DateTime<chrono::Utc>>,
    deprecated_at: Option<chrono::DateTime<chrono::Utc>>,
    revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    published_by_principal_id: Option<Uuid>,
    deprecated_by_principal_id: Option<Uuid>,
    revoked_by_principal_id: Option<Uuid>,
}

impl WorkflowDefinitionVersionRow {
    pub(super) fn into_domain(self) -> WorkflowDefinitionVersion {
        let status = self
            .version_status
            .parse::<DefinitionVersionStatus>()
            .unwrap_or(DefinitionVersionStatus::DRAFT);
        WorkflowDefinitionVersion {
            id: DefinitionVersionId::from_uuid(self.definition_version_id),
            workflow_definition_id: WorkflowDefinitionId::from_uuid(self.workflow_definition_id),
            version_number: self.version_number,
            version_status: status,
            definition_digest: self.definition_digest,
            json_schema_dialect: self.json_schema_dialect,
            validator_version: self.validator_version,
            context_schema: self.context_schema,
            submission_schema: self.submission_schema,
            metadata: self.metadata,
            created_at: self.created_at,
            updated_at: self.updated_at,
            published_at: self.published_at,
            deprecated_at: self.deprecated_at,
            revoked_at: self.revoked_at,
            published_by_principal_id: self.published_by_principal_id.map(PrincipalId::from_uuid),
            deprecated_by_principal_id: self.deprecated_by_principal_id.map(PrincipalId::from_uuid),
            revoked_by_principal_id: self.revoked_by_principal_id.map(PrincipalId::from_uuid),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct NodeDefinitionRow {
    node_id: Uuid,
    definition_version_id: Uuid,
    node_key: String,
    display_name: String,
    order_index: i32,
    node_type: String,
    assignee_ref_type: Option<String>,
    fixed_principal_id: Option<Uuid>,
    instructions: Option<String>,
    primary_advance_transition_id: Option<Uuid>,
    metadata: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl NodeDefinitionRow {
    pub(super) fn into_domain(self) -> NodeDefinition {
        let node_type = self
            .node_type
            .parse::<NodeType>()
            .unwrap_or(NodeType::NORMAL);
        let assignee_ref = self.assignee_ref_type.map(|value| AssigneeRef {
            ref_type: value
                .parse::<AssigneeRefType>()
                .unwrap_or(AssigneeRefType::WorkflowCreator),
            fixed_principal_id: self.fixed_principal_id.map(PrincipalId::from_uuid),
        });

        NodeDefinition {
            node_id: NodeId::from_uuid(self.node_id),
            definition_version_id: DefinitionVersionId::from_uuid(self.definition_version_id),
            node_key: self.node_key,
            display_name: self.display_name,
            order_index: self.order_index,
            node_type,
            assignee_ref,
            instructions: self.instructions,
            primary_advance_transition_id: self
                .primary_advance_transition_id
                .map(TransitionId::from_uuid),
            metadata: self.metadata,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct TransitionDefinitionRow {
    transition_id: Uuid,
    definition_version_id: Uuid,
    transition_key: String,
    display_name: String,
    source_node_id: Uuid,
    target_node_id: Uuid,
    transition_effect: String,
    submission_schema: Option<serde_json::Value>,
    metadata: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl TransitionDefinitionRow {
    pub(super) fn into_domain(self) -> TransitionDefinition {
        let effect = self
            .transition_effect
            .parse::<TransitionEffect>()
            .unwrap_or(TransitionEffect::Advance);
        TransitionDefinition {
            transition_id: TransitionId::from_uuid(self.transition_id),
            definition_version_id: DefinitionVersionId::from_uuid(self.definition_version_id),
            transition_key: self.transition_key,
            display_name: self.display_name,
            source_node_id: NodeId::from_uuid(self.source_node_id),
            target_node_id: NodeId::from_uuid(self.target_node_id),
            transition_effect: effect,
            submission_schema: self.submission_schema,
            metadata: self.metadata,
            created_at: self.created_at,
        }
    }
}
