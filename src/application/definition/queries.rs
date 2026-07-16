//! Query input and output types for the Definition Application Service.

use crate::domain::definition::model::WorkflowGraph;

use super::repository::DefinitionData;

/// Get a workflow definition by ID.
#[derive(Debug, Clone)]
pub struct GetDefinition {
    pub actor_principal_id: uuid::Uuid,
    pub workflow_definition_id: uuid::Uuid,
}

/// Get a specific version of a workflow definition.
#[derive(Debug, Clone)]
pub struct GetDefinitionVersion {
    pub actor_principal_id: uuid::Uuid,
    pub definition_version_id: uuid::Uuid,
}

/// List all versions of a workflow definition.
#[derive(Debug, Clone)]
pub struct ListDefinitionVersions {
    pub actor_principal_id: uuid::Uuid,
    pub workflow_definition_id: uuid::Uuid,
}

/// Get a complete version graph (nodes + transitions + schema).
#[derive(Debug, Clone)]
pub struct GetCompleteVersionGraph {
    pub actor_principal_id: uuid::Uuid,
    pub definition_version_id: uuid::Uuid,
}

/// Output of a definition query.
#[derive(Debug, Clone)]
pub struct DefinitionQueryResult {
    pub definition: DefinitionData,
}

/// Output of a version query.
#[derive(Debug, Clone)]
pub struct VersionQueryResult {
    pub version: DefinitionData,
    pub nodes_count: usize,
    pub transitions_count: usize,
}

/// Output of listing versions.
#[derive(Debug, Clone)]
pub struct VersionListResult {
    pub versions: Vec<DefinitionData>,
}

/// Output of a complete graph query.
#[derive(Debug, Clone)]
pub struct GraphQueryResult {
    pub graph: WorkflowGraph,
}
