//! Atomic lifecycle operations for the PostgreSQL definition repository.
//!
//! B-1: Each lifecycle method runs inside a single transaction with
//! FOR UPDATE row locking:
//! - `atomic_publish`: lock, verify DRAFT, domain checks, re-read graph,
//!   digest consistency check, status update, commit.
//! - `atomic_deprecate`: lock, verify PUBLISHED, domain checks, status update.
//! - `atomic_revoke`: lock, verify PUBLISHED|DEPRECATED, domain checks, status update.
//!
//! Also includes the simple `lock_version`, `publish_version`, and
//! `update_version_status` methods used by non-transactional callers.

use std::collections::HashMap;

use crate::domain::definition::digest;
use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::{
    NodeDefinition, TransitionDefinition, WorkflowDefinition, WorkflowDefinitionVersion,
};
use crate::domain::enums::DefinitionVersionStatus;

use super::error_mapping::map_db_error;
use super::repository_rows::*;
use super::PgDefinitionRepository;

use sqlx::{Postgres, Transaction};

impl PgDefinitionRepository {
    /// Lock a version row for update (FOR UPDATE).
    pub(super) async fn lock_version_inner(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        let row: Option<WorkflowDefinitionVersion> = sqlx::query_as::<_, WorkflowDefinitionVersionRow>(
            "SELECT definition_version_id, workflow_definition_id, version_number, version_status::TEXT AS version_status, definition_digest, json_schema_dialect, validator_version, context_schema, submission_schema, metadata, created_at, updated_at, published_at, deprecated_at, revoked_at, published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1 FOR UPDATE",
        )
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?
        .map(|r| r.into_domain());

        row.ok_or(DefinitionError::DefinitionVersionNotFound)
    }

    /// Simple publish update (non-atomic, used by non-transactional callers).
    pub(super) async fn publish_version_inner(
        &self,
        version_id: uuid::Uuid,
        digest_str: &str,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        sqlx::query(
            r#"
            UPDATE workflow_definition_versions
            SET version_status = 'PUBLISHED', definition_digest = $1, published_at = now(),
                published_by_principal_id = $2, updated_at = now()
            WHERE definition_version_id = $3 AND version_status = 'DRAFT'
            "#,
        )
        .bind(digest_str)
        .bind(actor_principal_id)
        .bind(version_id)
        .execute(&self.pool)
        .await
        .map_err(map_db_error)?;

        self.get_version_inner(version_id).await
    }

    /// Simple status update (non-atomic, used by non-transactional callers).
    pub(super) async fn update_version_status_inner(
        &self,
        version_id: uuid::Uuid,
        new_status: DefinitionVersionStatus,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        let (status_col, principal_col) = match new_status {
            DefinitionVersionStatus::DEPRECATED => ("deprecated_at", "deprecated_by_principal_id"),
            DefinitionVersionStatus::REVOKED => ("revoked_at", "revoked_by_principal_id"),
            _ => {
                return Err(DefinitionError::InvalidLifecycleTransition);
            }
        };

        let query = format!(
            "UPDATE workflow_definition_versions SET version_status = $1::definition_version_status, {} = now(), {} = $2, updated_at = now() WHERE definition_version_id = $3",
            status_col, principal_col
        );

        sqlx::query(&query)
            .bind(new_status.to_string())
            .bind(actor_principal_id)
            .bind(version_id)
            .execute(&self.pool)
            .await
            .map_err(map_db_error)?;

        self.get_version_inner(version_id).await
    }

    /// Open a new database transaction.
    pub(super) async fn begin_tx_inner(
        &self,
    ) -> Result<Transaction<'_, Postgres>, DefinitionError> {
        self.pool.begin().await.map_err(map_db_error)
    }

    // -----------------------------------------------------------------------
    // B-1: Atomic lifecycle operations (single transaction with row lock)
    // -----------------------------------------------------------------------

    /// Execute a complete publish inside a single transaction.
    pub(super) async fn atomic_publish_inner(
        &self,
        version_id: uuid::Uuid,
        actor_principal_id: uuid::Uuid,
        precomputed_digest: &str,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        let mut tx = self.pool.begin().await.map_err(map_db_error)?;

        // 1. Lock version with FOR UPDATE and verify DRAFT
        let version: Option<WorkflowDefinitionVersion> =
            sqlx::query_as::<_, WorkflowDefinitionVersionRow>(
                "SELECT definition_version_id, workflow_definition_id, version_number, version_status::TEXT AS version_status, definition_digest, json_schema_dialect, validator_version, context_schema, submission_schema, metadata, created_at, updated_at, published_at, deprecated_at, revoked_at, published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1 FOR UPDATE",
            )
            .bind(version_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_db_error)?
            .map(|r| r.into_domain());

        let version = match version {
            None => return Err(DefinitionError::DefinitionVersionNotFound),
            Some(v) if v.version_status != DefinitionVersionStatus::DRAFT => {
                return Err(DefinitionError::VersionNotDraft);
            }
            Some(v) => v,
        };

        // 2. Read definition inside tx
        let def: Option<WorkflowDefinition> = sqlx::query_as::<_, WorkflowDefinitionRow>(
            "SELECT * FROM workflow_definitions WHERE workflow_definition_id = $1",
        )
        .bind(version.workflow_definition_id.into_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_db_error)?
        .map(|r| r.into_domain());

        let def = def.ok_or(DefinitionError::DefinitionNotFound)?;
        let domain_id = def.domain_id.into_uuid();

        // 3. Verify domain enabled
        let domain: Option<(bool,)> =
            sqlx::query_as("SELECT enabled FROM domains WHERE domain_id = $1")
                .bind(domain_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_db_error)?;

        match domain {
            None => return Err(DefinitionError::DomainNotFound),
            Some((enabled,)) if !enabled => return Err(DefinitionError::DomainDisabled),
            _ => {}
        }

        // 4. Verify domain owner
        let is_owner: Option<(bool,)> = sqlx::query_as(
            "SELECT enabled FROM domain_role_bindings WHERE domain_id = $1 AND principal_id = $2 AND role_key = 'DOMAIN_OWNER'",
        )
        .bind(domain_id)
        .bind(actor_principal_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_db_error)?;

        match is_owner {
            None => return Err(DefinitionError::PermissionDenied),
            Some((enabled,)) if !enabled => return Err(DefinitionError::PermissionDenied),
            _ => {}
        }

        // 5. Re-read complete graph inside tx
        let nodes: Vec<NodeDefinition> = sqlx::query_as::<_, NodeDefinitionRow>(
            "SELECT node_id, definition_version_id, node_key, display_name, order_index, node_type::TEXT AS node_type, assignee_ref_type::TEXT AS assignee_ref_type, fixed_principal_id, instructions, primary_advance_transition_id, metadata, created_at FROM workflow_node_definitions WHERE definition_version_id = $1 ORDER BY order_index",
        )
        .bind(version_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(|r| r.into_domain())
        .collect();

        let transitions: Vec<TransitionDefinition> = sqlx::query_as::<_, TransitionDefinitionRow>(
            "SELECT transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect::TEXT AS transition_effect, submission_schema, metadata, created_at FROM workflow_transition_definitions WHERE definition_version_id = $1 ORDER BY transition_key",
        )
        .bind(version_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(|r| r.into_domain())
        .collect();

        // 6. Re-compute digest from data read inside tx to verify consistency
        let node_key_map: HashMap<_, _> = nodes
            .iter()
            .map(|n| (n.node_id, n.node_key.clone()))
            .collect();
        let transition_key_map: HashMap<_, _> = transitions
            .iter()
            .map(|t| (t.transition_id, t.transition_key.clone()))
            .collect();

        let actual_digest = digest::compute_digest(
            &def.definition_key,
            version.version_number,
            version.json_schema_dialect.as_deref(),
            version.validator_version.as_deref(),
            version.context_schema.as_ref(),
            &nodes,
            &transitions,
            &node_key_map,
            &transition_key_map,
        )?;

        if actual_digest != precomputed_digest {
            return Err(DefinitionError::ConcurrentModification(
                "definition graph changed during publish; retry with fresh data".to_string(),
            ));
        }

        // 7. Write publish status inside tx
        sqlx::query(
            r#"
            UPDATE workflow_definition_versions
            SET version_status = 'PUBLISHED', definition_digest = $1, published_at = now(),
                published_by_principal_id = $2, updated_at = now()
            WHERE definition_version_id = $3 AND version_status = 'DRAFT'
            "#,
        )
        .bind(precomputed_digest)
        .bind(actor_principal_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(map_db_error)?;

        // 8. Commit
        tx.commit().await.map_err(map_db_error)?;

        // Re-read and return
        self.get_version_inner(version_id).await
    }

    /// Execute a complete deprecation inside a single transaction.
    pub(super) async fn atomic_deprecate_inner(
        &self,
        version_id: uuid::Uuid,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        let mut tx = self.pool.begin().await.map_err(map_db_error)?;

        // Lock + verify PUBLISHED status
        let version: Option<WorkflowDefinitionVersion> =
            sqlx::query_as::<_, WorkflowDefinitionVersionRow>(
                "SELECT definition_version_id, workflow_definition_id, version_number, version_status::TEXT AS version_status, definition_digest, json_schema_dialect, validator_version, context_schema, submission_schema, metadata, created_at, updated_at, published_at, deprecated_at, revoked_at, published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1 FOR UPDATE",
            )
            .bind(version_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_db_error)?
            .map(|r| r.into_domain());

        let version = match version {
            None => return Err(DefinitionError::DefinitionVersionNotFound),
            Some(v) if v.version_status != DefinitionVersionStatus::PUBLISHED => {
                return Err(DefinitionError::InvalidLifecycleTransition);
            }
            Some(v) => v,
        };

        // Check domain enabled + domain owner inside tx
        let domain_id = {
            let def_row: Option<(uuid::Uuid,)> = sqlx::query_as(
                "SELECT domain_id FROM workflow_definitions WHERE workflow_definition_id = $1",
            )
            .bind(version.workflow_definition_id.into_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_db_error)?;
            match def_row {
                None => return Err(DefinitionError::DefinitionNotFound),
                Some((id,)) => id,
            }
        };

        let domain: Option<(bool,)> =
            sqlx::query_as("SELECT enabled FROM domains WHERE domain_id = $1")
                .bind(domain_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_db_error)?;

        match domain {
            None => return Err(DefinitionError::DomainNotFound),
            Some((enabled,)) if !enabled => return Err(DefinitionError::DomainDisabled),
            _ => {}
        }

        let is_owner: Option<(bool,)> = sqlx::query_as(
            "SELECT enabled FROM domain_role_bindings WHERE domain_id = $1 AND principal_id = $2 AND role_key = 'DOMAIN_OWNER'",
        )
        .bind(domain_id)
        .bind(actor_principal_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_db_error)?;

        match is_owner {
            None => return Err(DefinitionError::PermissionDenied),
            Some((enabled,)) if !enabled => return Err(DefinitionError::PermissionDenied),
            _ => {}
        }

        // Write deprecate status inside tx
        sqlx::query(
            r#"
            UPDATE workflow_definition_versions
            SET version_status = 'DEPRECATED', deprecated_at = now(),
                deprecated_by_principal_id = $1, updated_at = now()
            WHERE definition_version_id = $2
            "#,
        )
        .bind(actor_principal_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(map_db_error)?;

        tx.commit().await.map_err(map_db_error)?;

        self.get_version_inner(version_id).await
    }

    /// Execute a complete revocation inside a single transaction.
    pub(super) async fn atomic_revoke_inner(
        &self,
        version_id: uuid::Uuid,
        actor_principal_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        let mut tx = self.pool.begin().await.map_err(map_db_error)?;

        // Lock + verify PUBLISHED or DEPRECATED status
        let version: Option<WorkflowDefinitionVersion> =
            sqlx::query_as::<_, WorkflowDefinitionVersionRow>(
                "SELECT definition_version_id, workflow_definition_id, version_number, version_status::TEXT AS version_status, definition_digest, json_schema_dialect, validator_version, context_schema, submission_schema, metadata, created_at, updated_at, published_at, deprecated_at, revoked_at, published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1 FOR UPDATE",
            )
            .bind(version_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_db_error)?
            .map(|r| r.into_domain());

        let version = match version {
            None => return Err(DefinitionError::DefinitionVersionNotFound),
            Some(v)
                if v.version_status != DefinitionVersionStatus::PUBLISHED
                    && v.version_status != DefinitionVersionStatus::DEPRECATED =>
            {
                return Err(DefinitionError::InvalidLifecycleTransition);
            }
            Some(v) => v,
        };

        // Check domain enabled + domain owner inside tx
        let domain_id = {
            let def_row: Option<(uuid::Uuid,)> = sqlx::query_as(
                "SELECT domain_id FROM workflow_definitions WHERE workflow_definition_id = $1",
            )
            .bind(version.workflow_definition_id.into_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_db_error)?;
            match def_row {
                None => return Err(DefinitionError::DefinitionNotFound),
                Some((id,)) => id,
            }
        };

        let domain: Option<(bool,)> =
            sqlx::query_as("SELECT enabled FROM domains WHERE domain_id = $1")
                .bind(domain_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_db_error)?;

        match domain {
            None => return Err(DefinitionError::DomainNotFound),
            Some((enabled,)) if !enabled => return Err(DefinitionError::DomainDisabled),
            _ => {}
        }

        let is_owner: Option<(bool,)> = sqlx::query_as(
            "SELECT enabled FROM domain_role_bindings WHERE domain_id = $1 AND principal_id = $2 AND role_key = 'DOMAIN_OWNER'",
        )
        .bind(domain_id)
        .bind(actor_principal_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_db_error)?;

        match is_owner {
            None => return Err(DefinitionError::PermissionDenied),
            Some((enabled,)) if !enabled => return Err(DefinitionError::PermissionDenied),
            _ => {}
        }

        // Write revoke status inside tx
        sqlx::query(
            r#"
            UPDATE workflow_definition_versions
            SET version_status = 'REVOKED', revoked_at = now(),
                revoked_by_principal_id = $1, updated_at = now()
            WHERE definition_version_id = $2
            "#,
        )
        .bind(actor_principal_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(map_db_error)?;

        tx.commit().await.map_err(map_db_error)?;

        self.get_version_inner(version_id).await
    }
}
