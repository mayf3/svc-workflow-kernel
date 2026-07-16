//! Query/read operations for definitions and versions.
//!
//! Handles get_definition, get_definition_version, list_definition_versions,
//! and get_complete_version_graph with domain authorization (H-5).

use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::WorkflowGraph;

use super::super::queries::{
    DefinitionQueryResult, GetCompleteVersionGraph, GetDefinition, GetDefinitionVersion,
    GraphQueryResult, ListDefinitionVersions, VersionListResult, VersionQueryResult,
};
use super::super::repository::DefinitionData;
use super::super::repository::DefinitionRepository;
use super::super::service::DefinitionService;

impl<R: DefinitionRepository> DefinitionService<R> {
    /// Get a definition by ID.
    pub async fn get_definition(
        &self,
        query: GetDefinition,
    ) -> Result<DefinitionQueryResult, DefinitionError> {
        self.ensure_principal_enabled(query.actor_principal_id)
            .await?;

        let definition = self
            .repo
            .get_definition(query.workflow_definition_id)
            .await?;

        // H-5: Domain authorization — caller must be domain owner
        self.ensure_domain_owner(query.actor_principal_id, definition.domain_id.into_uuid())
            .await?;

        Ok(DefinitionQueryResult {
            definition: DefinitionData {
                definition,
                version: None,
                nodes: vec![],
                transitions: vec![],
            },
        })
    }

    /// Get a specific version.
    pub async fn get_definition_version(
        &self,
        query: GetDefinitionVersion,
    ) -> Result<VersionQueryResult, DefinitionError> {
        self.ensure_principal_enabled(query.actor_principal_id)
            .await?;

        let version = self.repo.get_version(query.definition_version_id).await?;
        let def = self
            .repo
            .get_definition(version.workflow_definition_id.into_uuid())
            .await?;

        // H-5: Domain authorization
        self.ensure_domain_owner(query.actor_principal_id, def.domain_id.into_uuid())
            .await?;

        let (nodes, transitions) = self
            .repo
            .get_complete_graph(query.definition_version_id)
            .await?;

        let nodes_count = nodes.len();
        let transitions_count = transitions.len();

        Ok(VersionQueryResult {
            version: DefinitionData {
                definition: def,
                version: Some(version),
                nodes,
                transitions,
            },
            nodes_count,
            transitions_count,
        })
    }

    /// List all versions of a definition.
    pub async fn list_definition_versions(
        &self,
        query: ListDefinitionVersions,
    ) -> Result<VersionListResult, DefinitionError> {
        self.ensure_principal_enabled(query.actor_principal_id)
            .await?;

        let def = self
            .repo
            .get_definition(query.workflow_definition_id)
            .await?;

        // H-5: Domain authorization — check before listing versions
        self.ensure_domain_owner(query.actor_principal_id, def.domain_id.into_uuid())
            .await?;

        let versions = self
            .repo
            .list_versions(query.workflow_definition_id)
            .await?;

        let results = versions
            .into_iter()
            .map(|v| DefinitionData {
                definition: def.clone(),
                version: Some(v),
                nodes: vec![],
                transitions: vec![],
            })
            .collect();

        Ok(VersionListResult { versions: results })
    }

    /// Get a complete version graph.
    pub async fn get_complete_version_graph(
        &self,
        query: GetCompleteVersionGraph,
    ) -> Result<GraphQueryResult, DefinitionError> {
        self.ensure_principal_enabled(query.actor_principal_id)
            .await?;

        let version = self.repo.get_version(query.definition_version_id).await?;
        let def = self
            .repo
            .get_definition(version.workflow_definition_id.into_uuid())
            .await?;

        // H-5: Domain authorization
        self.ensure_domain_owner(query.actor_principal_id, def.domain_id.into_uuid())
            .await?;

        let (nodes, transitions) = self
            .repo
            .get_complete_graph(query.definition_version_id)
            .await?;

        Ok(GraphQueryResult {
            graph: WorkflowGraph {
                nodes,
                transitions,
                context_schema: version.context_schema,
            },
        })
    }
}
