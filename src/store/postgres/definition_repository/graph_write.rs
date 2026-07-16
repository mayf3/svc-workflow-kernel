//! Graph write operations for the PostgreSQL definition repository.
//!
//! Handles atomic replacement of a DRAFT version's graph
//! (nodes and transitions) inside a single transaction.

use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::{NodeDefinition, TransitionDefinition};

use super::error_mapping::map_db_error;
use super::PgDefinitionRepository;

impl PgDefinitionRepository {
    /// Atomically replace the entire graph of a DRAFT version.
    ///
    /// The operation runs in a transaction:
    /// 1. Lock the version row (FOR UPDATE)
    /// 2. Verify it's still DRAFT
    /// 3. Delete old transitions (FK to nodes) then old nodes
    /// 4. Insert new nodes and transitions
    /// 5. Update context schema (patch semantics)
    pub(super) async fn replace_draft_graph_inner(
        &self,
        version_id: uuid::Uuid,
        context_schema: Option<&serde_json::Value>,
        nodes: &[NodeDefinition],
        transitions: &[TransitionDefinition],
    ) -> Result<(), DefinitionError> {
        let mut tx = self.pool.begin().await.map_err(map_db_error)?;

        // Lock version and verify DRAFT
        let version: Option<(String,)> = sqlx::query_as(
            "SELECT version_status::TEXT FROM workflow_definition_versions WHERE definition_version_id = $1 FOR UPDATE",
        )
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_db_error)?;

        match version {
            None => return Err(DefinitionError::DefinitionVersionNotFound),
            Some((status,)) if status != "DRAFT" => return Err(DefinitionError::VersionNotDraft),
            _ => {}
        }

        // Delete old transitions first (FK to nodes)
        sqlx::query("DELETE FROM workflow_transition_definitions WHERE definition_version_id = $1")
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(map_db_error)?;

        // Delete old nodes
        sqlx::query("DELETE FROM workflow_node_definitions WHERE definition_version_id = $1")
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(map_db_error)?;

        // Insert new nodes
        for node in nodes {
            sqlx::query(
                r#"
                INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type, fixed_principal_id, instructions, primary_advance_transition_id, metadata)
                VALUES ($1, $2, $3, $4, $5, $6::node_type, $7::assignee_ref_type, $8, $9, $10, $11)
                "#,
            )
            .bind(node.node_id.into_uuid())
            .bind(version_id)
            .bind(&node.node_key)
            .bind(&node.display_name)
            .bind(node.order_index)
            .bind(node.node_type.to_string())
            .bind(
                node.assignee_ref
                    .as_ref()
                    .map(|reference| reference.ref_type.to_string()),
            )
            .bind(
                node.assignee_ref
                    .as_ref()
                    .and_then(|reference| reference.fixed_principal_id)
                    .map(|id| id.into_uuid()),
            )
            .bind(&node.instructions)
            .bind(node.primary_advance_transition_id.map(|id| id.into_uuid()))
            .bind(&node.metadata)
            .execute(&mut *tx)
            .await
            .map_err(map_db_error)?;
        }

        // Insert new transitions
        for trans in transitions {
            sqlx::query(
                r#"
                INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect, submission_schema, metadata)
                VALUES ($1, $2, $3, $4, $5, $6, $7::transition_effect, $8, $9)
                "#,
            )
            .bind(trans.transition_id.into_uuid())
            .bind(version_id)
            .bind(&trans.transition_key)
            .bind(&trans.display_name)
            .bind(trans.source_node_id.into_uuid())
            .bind(trans.target_node_id.into_uuid())
            .bind(trans.transition_effect.to_string())
            .bind(&trans.submission_schema)
            .bind(&trans.metadata)
            .execute(&mut *tx)
            .await
            .map_err(map_db_error)?;
        }

        // M-1: Update context schema with three-state patch semantics:
        // None keeps the current value, JSON null clears the SQL column, and
        // any other JSON value replaces the current schema.
        if let Some(schema) = context_schema {
            if schema.is_null() {
                sqlx::query(
                    "UPDATE workflow_definition_versions SET context_schema = NULL, updated_at = now() WHERE definition_version_id = $1",
                )
                .bind(version_id)
                .execute(&mut *tx)
                .await
                .map_err(map_db_error)?;
            } else {
                sqlx::query(
                    "UPDATE workflow_definition_versions SET context_schema = $1, updated_at = now() WHERE definition_version_id = $2",
                )
                .bind(schema)
                .bind(version_id)
                .execute(&mut *tx)
                .await
                .map_err(map_db_error)?;
            }
        }

        tx.commit().await.map_err(map_db_error)?;

        Ok(())
    }
}
