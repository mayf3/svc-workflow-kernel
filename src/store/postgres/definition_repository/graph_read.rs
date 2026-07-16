//! Graph read operations for the PostgreSQL definition repository.
//!
//! Handles reading graph structure (nodes and transitions) for a version.

use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::{NodeDefinition, TransitionDefinition};

use super::error_mapping::map_db_error;
use super::repository_rows::*;
use super::PgDefinitionRepository;

impl PgDefinitionRepository {
    pub(super) async fn get_complete_graph_inner(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<(Vec<NodeDefinition>, Vec<TransitionDefinition>), DefinitionError> {
        let nodes: Vec<NodeDefinition> = sqlx::query_as::<_, NodeDefinitionRow>(
            "SELECT node_id, definition_version_id, node_key, display_name, order_index, node_type::TEXT AS node_type, assignee_ref_type::TEXT AS assignee_ref_type, fixed_principal_id, instructions, primary_advance_transition_id, metadata, created_at FROM workflow_node_definitions WHERE definition_version_id = $1 ORDER BY order_index",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(|r| r.into_domain())
        .collect();

        let transitions: Vec<TransitionDefinition> = sqlx::query_as::<_, TransitionDefinitionRow>(
            "SELECT transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect::TEXT AS transition_effect, submission_schema, metadata, created_at FROM workflow_transition_definitions WHERE definition_version_id = $1 ORDER BY transition_key",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(|r| r.into_domain())
        .collect();

        Ok((nodes, transitions))
    }

    pub(super) async fn get_nodes_by_version_inner(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<Vec<NodeDefinition>, DefinitionError> {
        let nodes: Vec<NodeDefinition> = sqlx::query_as::<_, NodeDefinitionRow>(
            "SELECT node_id, definition_version_id, node_key, display_name, order_index, node_type::TEXT AS node_type, assignee_ref_type::TEXT AS assignee_ref_type, fixed_principal_id, instructions, primary_advance_transition_id, metadata, created_at FROM workflow_node_definitions WHERE definition_version_id = $1 ORDER BY order_index",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(|r| r.into_domain())
        .collect();

        Ok(nodes)
    }

    pub(super) async fn get_transitions_by_version_inner(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<Vec<TransitionDefinition>, DefinitionError> {
        let transitions: Vec<TransitionDefinition> = sqlx::query_as::<_, TransitionDefinitionRow>(
            "SELECT transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect::TEXT AS transition_effect, submission_schema, metadata, created_at FROM workflow_transition_definitions WHERE definition_version_id = $1 ORDER BY transition_key",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(|r| r.into_domain())
        .collect();

        Ok(transitions)
    }
}
