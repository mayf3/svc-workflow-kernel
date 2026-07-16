//! Authorization-related database queries for the definition repository.
//!
//! Handles:
//! - Principal existence and enabled checks
//! - Domain enabled check
//! - Domain role binding checks (DOMAIN_OWNER)

use crate::domain::definition::error::DefinitionError;

use super::error_mapping::map_db_error;
use super::PgDefinitionRepository;

impl PgDefinitionRepository {
    pub(super) async fn check_principal_enabled_inner(
        &self,
        principal_id: uuid::Uuid,
    ) -> Result<bool, DefinitionError> {
        let row: Option<(bool,)> =
            sqlx::query_as("SELECT enabled FROM principals WHERE principal_id = $1")
                .bind(principal_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_db_error)?;

        Ok(row.map(|r| r.0).unwrap_or(false))
    }

    pub(super) async fn check_domain_enabled_inner(
        &self,
        domain_id: uuid::Uuid,
    ) -> Result<bool, DefinitionError> {
        let row: Option<(bool,)> =
            sqlx::query_as("SELECT enabled FROM domains WHERE domain_id = $1")
                .bind(domain_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_db_error)?;

        Ok(row.map(|r| r.0).unwrap_or(false))
    }

    pub(super) async fn check_domain_role_inner(
        &self,
        principal_id: uuid::Uuid,
        domain_id: uuid::Uuid,
        role_key: &str,
    ) -> Result<bool, DefinitionError> {
        let row: Option<(bool,)> = sqlx::query_as(
            "SELECT enabled FROM domain_role_bindings WHERE domain_id = $1 AND principal_id = $2 AND role_key = $3",
        )
        .bind(domain_id)
        .bind(principal_id)
        .bind(role_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_db_error)?;

        Ok(row.map(|r| r.0).unwrap_or(false))
    }

    pub(super) async fn check_principal_exists_inner(
        &self,
        principal_id: uuid::Uuid,
    ) -> Result<bool, DefinitionError> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM principals WHERE principal_id = $1")
                .bind(principal_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_db_error)?;

        Ok(row.map(|r| r.0 > 0).unwrap_or(false))
    }
}
