//! Repository trait for definition storage operations.
//!
//! The service depends on this trait; concrete implementations
//! (currently PostgreSQL) are injected at construction time.

use sqlx::Postgres;
use sqlx::Transaction;
use uuid::Uuid;

use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::{
    NodeDefinition, TransitionDefinition, WorkflowDefinition, WorkflowDefinitionVersion,
};
use crate::domain::enums::DefinitionVersionStatus;

/// Data returned from repository queries, combining definition + version info.
#[derive(Debug, Clone)]
pub struct DefinitionData {
    pub definition: WorkflowDefinition,
    pub version: Option<WorkflowDefinitionVersion>,
    pub nodes: Vec<NodeDefinition>,
    pub transitions: Vec<TransitionDefinition>,
}

/// Repository trait for the definition domain.
///
/// All methods are fallible and return [`DefinitionError`].
#[allow(async_fn_in_trait)]
pub trait DefinitionRepository {
    // -----------------------------------------------------------------------
    // Principals & Domains (read-only checks)
    // -----------------------------------------------------------------------

    /// Check that a principal exists and is enabled.
    async fn check_principal_enabled(&self, principal_id: Uuid) -> Result<bool, DefinitionError>;

    /// Check that a domain exists and is enabled.
    async fn check_domain_enabled(&self, domain_id: Uuid) -> Result<bool, DefinitionError>;

    /// Check that a principal has a given role (e.g., DOMAIN_OWNER) for a domain.
    async fn check_domain_role(
        &self,
        principal_id: Uuid,
        domain_id: Uuid,
        role_key: &str,
    ) -> Result<bool, DefinitionError>;

    // -----------------------------------------------------------------------
    // Definition CRUD
    // -----------------------------------------------------------------------

    /// Create a new workflow definition.
    async fn create_definition(
        &self,
        id: Uuid,
        domain_id: Uuid,
        definition_key: &str,
        display_name: &str,
        description: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<WorkflowDefinition, DefinitionError>;

    /// Check if a definition_key already exists within a domain.
    async fn definition_key_exists(
        &self,
        domain_id: Uuid,
        definition_key: &str,
    ) -> Result<bool, DefinitionError>;

    /// Get a workflow definition by ID.
    async fn get_definition(&self, id: Uuid) -> Result<WorkflowDefinition, DefinitionError>;

    /// Get a definition version by ID.
    async fn get_version(
        &self,
        version_id: Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;

    /// Get the domain_id for a workflow definition.
    async fn get_definition_domain(&self, definition_id: Uuid) -> Result<Uuid, DefinitionError>;

    /// Get the workflow_definition_id for a version.
    async fn get_version_definition_id(&self, version_id: Uuid) -> Result<Uuid, DefinitionError>;

    // -----------------------------------------------------------------------
    // Version CRUD
    // -----------------------------------------------------------------------

    /// Create a new DRAFT version with the next version_number.
    async fn create_draft_version(
        &self,
        id: Uuid,
        workflow_definition_id: Uuid,
        version_number: i32,
        context_schema: Option<&serde_json::Value>,
        json_schema_dialect: Option<&str>,
        validator_version: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;

    /// Get the next version number for a definition.
    async fn next_version_number(
        &self,
        workflow_definition_id: Uuid,
    ) -> Result<i32, DefinitionError>;

    /// List all versions of a definition, ordered by version_number descending.
    async fn list_versions(
        &self,
        workflow_definition_id: Uuid,
    ) -> Result<Vec<WorkflowDefinitionVersion>, DefinitionError>;

    // -----------------------------------------------------------------------
    // Graph operations (within a transaction for ReplaceDraftGraph)
    // -----------------------------------------------------------------------

    /// Atomically replace the entire graph of a DRAFT version.
    /// This must happen in a transaction:
    ///   1. Lock the version row
    ///   2. Verify it's still DRAFT
    ///   3. Delete old nodes/transitions
    ///   4. Insert new nodes/transitions
    ///   5. Update context schema
    async fn replace_draft_graph(
        &self,
        version_id: Uuid,
        context_schema: Option<&serde_json::Value>,
        nodes: &[NodeDefinition],
        transitions: &[TransitionDefinition],
    ) -> Result<(), DefinitionError>;

    /// Get the complete graph for a version (nodes + transitions).
    async fn get_complete_graph(
        &self,
        version_id: Uuid,
    ) -> Result<(Vec<NodeDefinition>, Vec<TransitionDefinition>), DefinitionError>;

    // -----------------------------------------------------------------------
    // Lifecycle operations (within a transaction)
    // -----------------------------------------------------------------------

    /// Lock a version row for update (within a transaction).
    async fn lock_version(
        &self,
        version_id: Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;

    /// Update version status and digest, recording the actor.
    async fn publish_version(
        &self,
        version_id: Uuid,
        digest: &str,
        actor_principal_id: Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;

    /// Transition version status, recording the actor.
    async fn update_version_status(
        &self,
        version_id: Uuid,
        new_status: DefinitionVersionStatus,
        actor_principal_id: Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;

    /// Get nodes and transitions that reference specific principals (for validity checks).
    async fn get_nodes_by_version(
        &self,
        version_id: Uuid,
    ) -> Result<Vec<NodeDefinition>, DefinitionError>;
    async fn get_transitions_by_version(
        &self,
        version_id: Uuid,
    ) -> Result<Vec<TransitionDefinition>, DefinitionError>;

    /// Check if a principal exists and is enabled (by ID).
    async fn check_principal_exists(&self, principal_id: Uuid) -> Result<bool, DefinitionError>;

    // -----------------------------------------------------------------------
    // B-1: Atomic lifecycle operations (single transaction with row lock)
    // -----------------------------------------------------------------------

    /// Open a new database transaction.
    async fn begin_tx(&self) -> Result<Transaction<'_, Postgres>, DefinitionError>;

    /// Execute a complete publish inside a single transaction.
    ///
    /// Within the transaction:
    /// 1. Lock the version row (FOR UPDATE)
    /// 2. Verify DRAFT status
    /// 3. Verify domain enabled + domain owner
    /// 4. Re-read the complete graph inside the tx
    /// 5. Re-compute digest and verify it matches `precomputed_digest`
    /// 6. Update status to PUBLISHED, set digest + actor
    /// 7. Commit
    ///
    /// If a concurrent ReplaceDraftGraph changed the graph between when
    /// the service computed `precomputed_digest` and when this method
    /// re-reads the graph inside the transaction, the digest mismatch
    /// causes a `ConcurrentModification` error (caller retries).
    async fn atomic_publish(
        &self,
        version_id: Uuid,
        actor_principal_id: Uuid,
        precomputed_digest: &str,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;

    /// Execute a complete deprecation inside a single transaction.
    async fn atomic_deprecate(
        &self,
        version_id: Uuid,
        actor_principal_id: Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;

    /// Execute a complete revocation inside a single transaction.
    async fn atomic_revoke(
        &self,
        version_id: Uuid,
        actor_principal_id: Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError>;
}
