//! Frozen value types for the ADC legacy initial-import primitive.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::ids::{DefinitionVersionId, DomainId, NodeId, PrincipalId};

pub const SNAPSHOT_SCHEMA_VERSION: &str = "ADC_WORKFLOW_IMPORT_SNAPSHOT_V1";
pub const COMMAND_SCHEMA_VERSION: &str = "v1";
pub const COMMAND_TYPE: &str = "IMPORT_LEGACY_WORKFLOW_INSTANCE";
pub const EVENT_TYPE: &str = "WORKFLOW_INSTANCE_IMPORTED";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyAdcImportSnapshotV1 {
    pub schema_version: String,
    pub requirement_id: Uuid,
    pub domain_key: String,
    pub workflow_id: String,
    pub workflow_snapshot: serde_json::Value,
    pub current_step: String,
    pub assignee_id: Option<Uuid>,
    pub requester_id: Option<Uuid>,
    pub state_version: i64,
    pub updated_at: String,
    pub context_payload: serde_json::Value,
}

impl LegacyAdcImportSnapshotV1 {
    pub fn digest(&self) -> Result<String, LegacyImportError> {
        jcs_canonicalize::sha256_jcs_hex(self)
            .map_err(|error| LegacyImportError::StorageError(error.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct ImportLegacyWorkflowInstanceCommand {
    pub principal_id: PrincipalId,
    pub command_schema_version: String,
    pub domain_id: DomainId,
    pub definition_version_id: DefinitionVersionId,
    pub imported_node_id: NodeId,
    pub legacy_record_id: Uuid,
    pub legacy_snapshot: LegacyAdcImportSnapshotV1,
    pub expected_legacy_snapshot_digest: String,
    pub legacy_creator_principal_id: Option<PrincipalId>,
    pub external_url: Option<String>,
    pub metadata: serde_json::Value,
}

impl ImportLegacyWorkflowInstanceCommand {
    pub fn idempotency_key(&self) -> String {
        format!("migration:adc:{}:v1", self.legacy_record_id)
    }

    pub fn external_reference(&self) -> String {
        self.idempotency_key()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CreatorResolution {
    LegacyCreator,
    DomainOwnerFallback,
}

impl CreatorResolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LegacyCreator => "LEGACY_CREATOR",
            Self::DomainOwnerFallback => "DOMAIN_OWNER_FALLBACK",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyImportError {
    PrincipalNotFound,
    PrincipalDisabled,
    PrincipalTypeNotAllowed,
    MigrationBindingInvalid,
    PermissionDenied,
    DomainNotFound,
    DomainDisabled,
    DefinitionVersionNotFound,
    VersionNotPublished,
    ImportedNodeNotFound,
    InvalidInput(String),
    SnapshotDigestMismatch { expected: String, actual: String },
    CreatorResolutionFailed(String),
    AssigneeResolutionFailed(String),
    ContextValidationFailed(String),
    SizeLimitExceeded(String),
    ExternalReferenceConflict,
    IdempotencyConflict,
    CommandStillProcessing,
    InternalConsistency(String),
    StorageError(String),
}

impl LegacyImportError {
    pub fn status_code(&self) -> i32 {
        match self {
            Self::PrincipalNotFound
            | Self::DomainNotFound
            | Self::DefinitionVersionNotFound
            | Self::ImportedNodeNotFound => 404,
            Self::PrincipalDisabled
            | Self::PrincipalTypeNotAllowed
            | Self::MigrationBindingInvalid
            | Self::PermissionDenied
            | Self::DomainDisabled => 403,
            Self::SnapshotDigestMismatch { .. }
            | Self::ExternalReferenceConflict
            | Self::IdempotencyConflict => 409,
            Self::CommandStillProcessing => 425,
            Self::SizeLimitExceeded(_) => 413,
            Self::InvalidInput(_)
            | Self::CreatorResolutionFailed(_)
            | Self::AssigneeResolutionFailed(_)
            | Self::ContextValidationFailed(_) => 422,
            Self::VersionNotPublished => 409,
            Self::InternalConsistency(_) | Self::StorageError(_) => 500,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::PrincipalNotFound => "principal_not_found",
            Self::PrincipalDisabled => "principal_disabled",
            Self::PrincipalTypeNotAllowed => "principal_type_not_allowed",
            Self::MigrationBindingInvalid => "migration_binding_invalid",
            Self::PermissionDenied => "permission_denied",
            Self::DomainNotFound => "domain_not_found",
            Self::DomainDisabled => "domain_disabled",
            Self::DefinitionVersionNotFound => "definition_version_not_found",
            Self::VersionNotPublished => "version_not_published",
            Self::ImportedNodeNotFound => "imported_node_not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::SnapshotDigestMismatch { .. } => "snapshot_digest_mismatch",
            Self::CreatorResolutionFailed(_) => "creator_resolution_failed",
            Self::AssigneeResolutionFailed(_) => "assignee_resolution_failed",
            Self::ContextValidationFailed(_) => "context_validation_failed",
            Self::SizeLimitExceeded(_) => "size_limit_exceeded",
            Self::ExternalReferenceConflict => "external_reference_conflict",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::CommandStillProcessing => "command_still_processing",
            Self::InternalConsistency(_) => "internal_consistency_error",
            Self::StorageError(_) => "storage_error",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::InvalidInput(value)
            | Self::CreatorResolutionFailed(value)
            | Self::AssigneeResolutionFailed(value)
            | Self::ContextValidationFailed(value)
            | Self::SizeLimitExceeded(value)
            | Self::InternalConsistency(value)
            | Self::StorageError(value) => Some(value),
            _ => None,
        }
    }
}

impl std::fmt::Display for LegacyImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.detail() {
            Some(detail) => write!(formatter, "{}: {}", self.label(), detail),
            None => write!(formatter, "{}", self.label()),
        }
    }
}

impl std::error::Error for LegacyImportError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn golden_snapshot() -> LegacyAdcImportSnapshotV1 {
        LegacyAdcImportSnapshotV1 {
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_string(),
            requirement_id: "11111111-1111-1111-1111-111111111111".parse().unwrap(),
            domain_key: "adc".to_string(),
            workflow_id: "wf".to_string(),
            workflow_snapshot: serde_json::json!({"steps": ["draft", "review"]}),
            current_step: "review".to_string(),
            assignee_id: None,
            requester_id: Some("22222222-2222-2222-2222-222222222222".parse().unwrap()),
            state_version: 7,
            updated_at: "2026-07-15T01:02:03Z".to_string(),
            context_payload: serde_json::json!({"b": "x", "a": 1}),
        }
    }

    #[test]
    fn snapshot_digest_has_a_stable_jcs_golden_value() {
        assert_eq!(
            golden_snapshot().digest().unwrap(),
            "be2d22984b74102fda7bf34c62d3a8805084f1bde8a71d02b95d5300aba2c5bb"
        );
    }

    #[test]
    fn server_derived_identifiers_are_identical_and_canonical() {
        let snapshot = golden_snapshot();
        let command = ImportLegacyWorkflowInstanceCommand {
            principal_id: PrincipalId::new(),
            command_schema_version: COMMAND_SCHEMA_VERSION.to_string(),
            domain_id: DomainId::new(),
            definition_version_id: DefinitionVersionId::new(),
            imported_node_id: NodeId::new(),
            legacy_record_id: snapshot.requirement_id,
            expected_legacy_snapshot_digest: snapshot.digest().unwrap(),
            legacy_snapshot: snapshot,
            legacy_creator_principal_id: None,
            external_url: None,
            metadata: serde_json::json!({}),
        };
        assert_eq!(command.idempotency_key(), command.external_reference());
        assert_eq!(
            command.idempotency_key(),
            "migration:adc:11111111-1111-1111-1111-111111111111:v1"
        );
    }
}
