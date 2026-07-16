//! PostgreSQL implementation of the [`DefinitionRepository`] trait.
#![allow(clippy::needless_borrow)]

pub use super::repository_rows;

mod authorization_queries;
mod definition_crud;
mod error_mapping;
mod graph_read;
mod graph_write;
mod lifecycle_transactions;

use crate::application::definition::DefinitionRepository;
use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::{
    NodeDefinition, TransitionDefinition, WorkflowDefinition, WorkflowDefinitionVersion,
};
use crate::domain::enums::DefinitionVersionStatus;
use sqlx::{PgPool, Postgres, Transaction};

/// PostgreSQL-backed implementation of [`DefinitionRepository`].
pub struct PgDefinitionRepository {
    pool: PgPool,
}

impl PgDefinitionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[allow(async_fn_in_trait)]
impl DefinitionRepository for PgDefinitionRepository {
    // -- Principals & Domains ---------------------------------------------------

    async fn check_principal_enabled(
        &self,
        principal_id: uuid::Uuid,
    ) -> Result<bool, DefinitionError> {
        self.check_principal_enabled_inner(principal_id).await
    }

    async fn check_domain_enabled(&self, domain_id: uuid::Uuid) -> Result<bool, DefinitionError> {
        self.check_domain_enabled_inner(domain_id).await
    }

    async fn check_domain_role(
        &self,
        principal_id: uuid::Uuid,
        domain_id: uuid::Uuid,
        role_key: &str,
    ) -> Result<bool, DefinitionError> {
        self.check_domain_role_inner(principal_id, domain_id, role_key)
            .await
    }

    // -- Definition CRUD -------------------------------------------------------

    async fn create_definition(
        &self,
        id: uuid::Uuid,
        domain_id: uuid::Uuid,
        definition_key: &str,
        display_name: &str,
        description: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<WorkflowDefinition, DefinitionError> {
        self.create_definition_inner(
            id,
            domain_id,
            definition_key,
            display_name,
            description,
            metadata,
        )
        .await
    }

    async fn definition_key_exists(
        &self,
        domain_id: uuid::Uuid,
        definition_key: &str,
    ) -> Result<bool, DefinitionError> {
        self.definition_key_exists_inner(domain_id, definition_key)
            .await
    }

    async fn get_definition(&self, id: uuid::Uuid) -> Result<WorkflowDefinition, DefinitionError> {
        self.get_definition_inner(id).await
    }

    async fn get_definition_domain(
        &self,
        definition_id: uuid::Uuid,
    ) -> Result<uuid::Uuid, DefinitionError> {
        self.get_definition_domain_inner(definition_id).await
    }

    async fn get_version_definition_id(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<uuid::Uuid, DefinitionError> {
        self.get_version_definition_id_inner(version_id).await
    }

    async fn get_version(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.get_version_inner(version_id).await
    }

    // -- Version CRUD ----------------------------------------------------------

    async fn create_draft_version(
        &self,
        id: uuid::Uuid,
        workflow_definition_id: uuid::Uuid,
        version_number: i32,
        context_schema: Option<&serde_json::Value>,
        json_schema_dialect: Option<&str>,
        validator_version: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.create_draft_version_inner(
            id,
            workflow_definition_id,
            version_number,
            context_schema,
            json_schema_dialect,
            validator_version,
            metadata,
        )
        .await
    }

    async fn next_version_number(
        &self,
        workflow_definition_id: uuid::Uuid,
    ) -> Result<i32, DefinitionError> {
        self.next_version_number_inner(workflow_definition_id).await
    }

    async fn list_versions(
        &self,
        workflow_definition_id: uuid::Uuid,
    ) -> Result<Vec<WorkflowDefinitionVersion>, DefinitionError> {
        self.list_versions_inner(workflow_definition_id).await
    }

    // -- Graph operations ------------------------------------------------------

    async fn get_nodes_by_version(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<Vec<NodeDefinition>, DefinitionError> {
        self.get_nodes_by_version_inner(version_id).await
    }

    async fn get_transitions_by_version(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<Vec<TransitionDefinition>, DefinitionError> {
        self.get_transitions_by_version_inner(version_id).await
    }

    async fn get_complete_graph(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<(Vec<NodeDefinition>, Vec<TransitionDefinition>), DefinitionError> {
        self.get_complete_graph_inner(version_id).await
    }

    async fn replace_draft_graph(
        &self,
        version_id: uuid::Uuid,
        context_schema: Option<&serde_json::Value>,
        nodes: &[NodeDefinition],
        transitions: &[TransitionDefinition],
    ) -> Result<(), DefinitionError> {
        self.replace_draft_graph_inner(version_id, context_schema, nodes, transitions)
            .await
    }

    // -- Lifecycle operations --------------------------------------------------

    async fn lock_version(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.lock_version_inner(version_id).await
    }

    async fn publish_version(
        &self,
        version_id: uuid::Uuid,
        digest: &str,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.publish_version_inner(version_id, digest, actor_principal_id)
            .await
    }

    async fn update_version_status(
        &self,
        version_id: uuid::Uuid,
        new_status: DefinitionVersionStatus,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.update_version_status_inner(version_id, new_status, actor_principal_id)
            .await
    }

    async fn check_principal_exists(
        &self,
        principal_id: uuid::Uuid,
    ) -> Result<bool, DefinitionError> {
        self.check_principal_exists_inner(principal_id).await
    }

    async fn begin_tx(&self) -> Result<Transaction<'_, Postgres>, DefinitionError> {
        self.begin_tx_inner().await
    }

    async fn atomic_publish(
        &self,
        version_id: uuid::Uuid,
        actor_principal_id: uuid::Uuid,
        precomputed_digest: &str,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.atomic_publish_inner(version_id, actor_principal_id, precomputed_digest)
            .await
    }

    async fn atomic_deprecate(
        &self,
        version_id: uuid::Uuid,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.atomic_deprecate_inner(version_id, actor_principal_id)
            .await
    }

    async fn atomic_revoke(
        &self,
        version_id: uuid::Uuid,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        self.atomic_revoke_inner(version_id, actor_principal_id)
            .await
    }
}
