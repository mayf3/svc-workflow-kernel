//! Definition CRUD operations for the PostgreSQL definition repository.
//!
//! Handles creation and retrieval of definitions and versions
//! (excluding graph nodes/transitions and lifecycle operations).

use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::{WorkflowDefinition, WorkflowDefinitionVersion};

use super::error_mapping::map_db_error;
use super::repository_rows::*;
use super::PgDefinitionRepository;

impl PgDefinitionRepository {
    pub(super) async fn create_definition_inner(
        &self,
        id: uuid::Uuid,
        domain_id: uuid::Uuid,
        definition_key: &str,
        display_name: &str,
        description: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<WorkflowDefinition, DefinitionError> {
        sqlx::query(
            r#"
            INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name, description, metadata)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(id)
        .bind(domain_id)
        .bind(definition_key)
        .bind(display_name)
        .bind(description)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if let Some(code) = db_err.code() {
                    if code == "23505" {
                        return DefinitionError::DefinitionKeyConflict;
                    }
                }
            }
            map_db_error(e)
        })?;

        self.get_definition_inner(id).await
    }

    #[allow(dead_code)]
    pub(super) async fn definition_key_exists_inner(
        &self,
        domain_id: uuid::Uuid,
        definition_key: &str,
    ) -> Result<bool, DefinitionError> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM workflow_definitions WHERE domain_id = $1 AND definition_key = $2",
        )
        .bind(domain_id)
        .bind(definition_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?;

        Ok(row.map(|r| r.0 > 0).unwrap_or(false))
    }

    pub(super) async fn get_definition_inner(
        &self,
        id: uuid::Uuid,
    ) -> Result<WorkflowDefinition, DefinitionError> {
        let row: Option<WorkflowDefinition> = sqlx::query_as::<_, WorkflowDefinitionRow>(
            "SELECT * FROM workflow_definitions WHERE workflow_definition_id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?
        .map(|r| r.into_domain());

        row.ok_or(DefinitionError::DefinitionNotFound)
    }

    pub(super) async fn get_definition_domain_inner(
        &self,
        definition_id: uuid::Uuid,
    ) -> Result<uuid::Uuid, DefinitionError> {
        let row: Option<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT domain_id FROM workflow_definitions WHERE workflow_definition_id = $1",
        )
        .bind(definition_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?;

        row.map(|r| r.0).ok_or(DefinitionError::DefinitionNotFound)
    }

    pub(super) async fn get_version_definition_id_inner(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<uuid::Uuid, DefinitionError> {
        let row: Option<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT workflow_definition_id FROM workflow_definition_versions WHERE definition_version_id = $1",
        )
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?;

        row.map(|r| r.0)
            .ok_or(DefinitionError::DefinitionVersionNotFound)
    }

    pub(super) async fn get_version_inner(
        &self,
        version_id: uuid::Uuid,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        let row: Option<WorkflowDefinitionVersion> = sqlx::query_as::<_, WorkflowDefinitionVersionRow>(
            "SELECT definition_version_id, workflow_definition_id, version_number, version_status::TEXT AS version_status, definition_digest, json_schema_dialect, validator_version, context_schema, submission_schema, metadata, created_at, updated_at, published_at, deprecated_at, revoked_at, published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1",
        )
        .bind(version_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?
        .map(|r| r.into_domain());

        row.ok_or(DefinitionError::DefinitionVersionNotFound)
    }

    pub(super) async fn create_draft_version_inner(
        &self,
        id: uuid::Uuid,
        workflow_definition_id: uuid::Uuid,
        version_number: i32,
        context_schema: Option<&serde_json::Value>,
        json_schema_dialect: Option<&str>,
        validator_version: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        sqlx::query(
            r#"
            INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema, json_schema_dialect, validator_version, metadata)
            VALUES ($1, $2, $3, 'DRAFT', $4, $5, $6, $7)
            "#,
        )
        .bind(id)
        .bind(workflow_definition_id)
        .bind(version_number)
        .bind(context_schema)
        .bind(json_schema_dialect)
        .bind(validator_version)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if let Some(code) = db_err.code() {
                    if code == "23505" {
                        return DefinitionError::ConcurrentModification(
                            "duplicate version number".to_string(),
                        );
                    }
                }
            }
            map_db_error(e)
        })?;

        self.get_version_inner(id).await
    }

    pub(super) async fn next_version_number_inner(
        &self,
        workflow_definition_id: uuid::Uuid,
    ) -> Result<i32, DefinitionError> {
        let row: Option<(Option<i32>,)> = sqlx::query_as(
            "SELECT MAX(version_number) FROM workflow_definition_versions WHERE workflow_definition_id = $1",
        )
        .bind(workflow_definition_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?;

        Ok(row.and_then(|r| r.0).unwrap_or(0) + 1)
    }

    pub(super) async fn list_versions_inner(
        &self,
        workflow_definition_id: uuid::Uuid,
    ) -> Result<Vec<WorkflowDefinitionVersion>, DefinitionError> {
        let rows: Vec<WorkflowDefinitionVersion> = sqlx::query_as::<_, WorkflowDefinitionVersionRow>(
            "SELECT definition_version_id, workflow_definition_id, version_number, version_status::TEXT AS version_status, definition_digest, json_schema_dialect, validator_version, context_schema, submission_schema, metadata, created_at, updated_at, published_at, deprecated_at, revoked_at, published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE workflow_definition_id = $1 ORDER BY version_number DESC",
        )
        .bind(workflow_definition_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_db_error)?
        .into_iter()
        .map(|r| r.into_domain())
        .collect();

        Ok(rows)
    }
}
