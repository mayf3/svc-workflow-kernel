//! Definition Application Service.
//!
//! Orchestrates workflow definition and version lifecycle use cases.
//! The service depends on a [`DefinitionRepository`] trait for storage
//! and performs domain validation before delegating to the repository.

use uuid::Uuid;

use crate::domain::definition::error::DefinitionError;
use crate::domain::definition::model::{
    AssigneeRef, WorkflowDefinition, WorkflowDefinitionVersion,
};
use crate::domain::enums::AssigneeRefType;
use crate::domain::ids::PrincipalId;

use super::commands::{CreateDefinition, CreateDraftVersion};
use super::repository::DefinitionRepository;

/// The Definition Application Service.
///
/// All public methods correspond to a use case. Each method:
/// 1. Validates actor permissions
/// 2. Validates input against domain rules
/// 3. Delegates to the repository for storage
/// 4. Returns a result or error
pub struct DefinitionService<R: DefinitionRepository> {
    pub repo: R,
}

impl<R: DefinitionRepository> DefinitionService<R> {
    /// Create a new service with the given repository.
    pub fn new(repo: R) -> Self {
        Self { repo }
    }

    // -----------------------------------------------------------------------
    // 12.1 CreateDefinition
    // -----------------------------------------------------------------------

    /// Create a new workflow definition.
    pub async fn create_definition(
        &self,
        cmd: CreateDefinition,
    ) -> Result<WorkflowDefinition, DefinitionError> {
        // Validate actor
        self.ensure_principal_enabled(cmd.actor_principal_id)
            .await?;
        self.ensure_domain_enabled(cmd.owner_domain_id).await?;
        self.ensure_domain_owner(cmd.actor_principal_id, cmd.owner_domain_id)
            .await?;

        // M-6: No pre-check — we rely on the DB unique constraint to detect
        // duplicate keys atomically.  The repository maps 23505 to
        // DefinitionKeyConflict.
        //
        // Validate field length
        if cmd.definition_key.is_empty() || cmd.definition_key.len() > 128 {
            return Err(DefinitionError::SchemaValidationFailed(
                "definition_key must be 1-128 characters".to_string(),
            ));
        }
        if cmd.display_name.is_empty() || cmd.display_name.len() > 256 {
            return Err(DefinitionError::SchemaValidationFailed(
                "display_name must be 1-256 characters".to_string(),
            ));
        }

        // Create
        let id = Uuid::new_v4();
        let def = self
            .repo
            .create_definition(
                id,
                cmd.owner_domain_id,
                &cmd.definition_key,
                &cmd.display_name,
                cmd.description.as_deref(),
                cmd.metadata.as_ref(),
            )
            .await?;

        Ok(def)
    }

    // -----------------------------------------------------------------------
    // 12.2 CreateDraftVersion
    // -----------------------------------------------------------------------

    /// Create a new DRAFT version of a workflow definition.
    pub async fn create_draft_version(
        &self,
        cmd: CreateDraftVersion,
    ) -> Result<WorkflowDefinitionVersion, DefinitionError> {
        // Validate actor
        self.ensure_principal_enabled(cmd.actor_principal_id)
            .await?;

        // Get definition's domain
        let domain_id = self
            .repo
            .get_definition_domain(cmd.workflow_definition_id)
            .await?;

        // M-4: Domain must be enabled
        self.ensure_domain_enabled(domain_id).await?;

        // Actor must have manage permission for the domain
        self.ensure_domain_owner(cmd.actor_principal_id, domain_id)
            .await?;

        // Get next version number
        let next_ver = self
            .repo
            .next_version_number(cmd.workflow_definition_id)
            .await?;

        // Create the draft version
        let version_id = Uuid::new_v4();
        let version = self
            .repo
            .create_draft_version(
                version_id,
                cmd.workflow_definition_id,
                next_ver,
                cmd.context_schema.as_ref(),
                cmd.json_schema_dialect.as_deref(),
                cmd.validator_version.as_deref(),
                cmd.metadata.as_ref(),
            )
            .await?;

        Ok(version)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    pub(crate) async fn ensure_principal_enabled(
        &self,
        principal_id: Uuid,
    ) -> Result<(), DefinitionError> {
        let enabled = self.repo.check_principal_enabled(principal_id).await?;
        if !enabled {
            // Check if principal exists at all
            let exists = self.repo.check_principal_exists(principal_id).await?;
            if !exists {
                return Err(DefinitionError::PrincipalNotFound);
            }
            return Err(DefinitionError::PrincipalDisabled);
        }
        Ok(())
    }

    pub(crate) async fn ensure_domain_enabled(
        &self,
        domain_id: Uuid,
    ) -> Result<(), DefinitionError> {
        let enabled = self.repo.check_domain_enabled(domain_id).await?;
        if !enabled {
            return Err(DefinitionError::DomainDisabled);
        }
        Ok(())
    }

    pub(crate) async fn ensure_domain_owner(
        &self,
        principal_id: Uuid,
        domain_id: Uuid,
    ) -> Result<(), DefinitionError> {
        let is_owner = self
            .repo
            .check_domain_role(principal_id, domain_id, "DOMAIN_OWNER")
            .await?;
        if !is_owner {
            return Err(DefinitionError::PermissionDenied);
        }
        Ok(())
    }

    pub(crate) fn parse_assignee_ref(
        ref_type: &str,
        fixed_principal_id: Option<Uuid>,
    ) -> Result<AssigneeRef, DefinitionError> {
        let parsed = ref_type.parse::<AssigneeRefType>().map_err(|_| {
            DefinitionError::StorageError(format!("invalid assignee_ref_type: {}", ref_type))
        })?;

        match parsed {
            AssigneeRefType::FixedPrincipal => {
                if fixed_principal_id.is_none() {
                    return Err(DefinitionError::FixedPrincipalInvalid(
                        "FIXED_PRINCIPAL requires a principal_id".to_string(),
                    ));
                }
            }
            _ => {
                if fixed_principal_id.is_some() {
                    return Err(DefinitionError::FixedPrincipalInvalid(
                        "only FIXED_PRINCIPAL type should have fixed_principal_id".to_string(),
                    ));
                }
            }
        }

        Ok(AssigneeRef {
            ref_type: parsed,
            fixed_principal_id: fixed_principal_id.map(PrincipalId::from_uuid),
        })
    }
}
