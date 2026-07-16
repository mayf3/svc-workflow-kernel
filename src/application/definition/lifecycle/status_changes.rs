//! Lifecycle status-change operations.
//!
//! Handles DeprecateVersion and RevokeVersion, delegating to the
//! repository's atomic_deprecate / atomic_revoke (B-1).

use crate::domain::definition::error::DefinitionError;

use super::super::commands::{DeprecateVersion, RevokeVersion};
use super::super::repository::DefinitionRepository;
use super::super::service::DefinitionService;

impl<R: DefinitionRepository> DefinitionService<R> {
    /// Deprecate a PUBLISHED version -> DEPRECATED.
    ///
    /// B-1: Uses atomic_deprecate which locks the version row across
    /// all checks and writes in a single transaction.
    pub async fn deprecate_version(
        &self,
        cmd: DeprecateVersion,
    ) -> Result<crate::domain::definition::model::WorkflowDefinitionVersion, DefinitionError> {
        self.ensure_principal_enabled(cmd.actor_principal_id)
            .await?;

        let updated = self
            .repo
            .atomic_deprecate(cmd.definition_version_id, cmd.actor_principal_id)
            .await?;

        Ok(updated)
    }

    /// Revoke a PUBLISHED or DEPRECATED version -> REVOKED.
    ///
    /// B-1: Uses atomic_revoke which locks the version row across
    /// all checks and writes in a single transaction.
    pub async fn revoke_version(
        &self,
        cmd: RevokeVersion,
    ) -> Result<crate::domain::definition::model::WorkflowDefinitionVersion, DefinitionError> {
        self.ensure_principal_enabled(cmd.actor_principal_id)
            .await?;

        let updated = self
            .repo
            .atomic_revoke(cmd.definition_version_id, cmd.actor_principal_id)
            .await?;

        Ok(updated)
    }
}
