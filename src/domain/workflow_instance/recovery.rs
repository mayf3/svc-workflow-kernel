//! Frozen PR5 administrative recovery command and value types.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::ids::{NodeId, PrincipalId, WorkflowInstanceId};

pub const BEFORE_SNAPSHOT_SCHEMA_VERSION: &str = "WORKFLOW_INSTANCE_BEFORE_SNAPSHOT_V1";
pub const COMMAND_TYPE_REBUILD_PROJECTION: &str = "REBUILD_PROJECTION";
pub const COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE: &str = "ADMIN_EMERGENCY_OVERRIDE";

#[derive(Debug, Clone)]
pub struct RebuildProjectionCommand {
    pub principal_id: PrincipalId,
    pub idempotency_key: String,
    pub command_schema_version: String,
    pub workflow_instance_id: WorkflowInstanceId,
    pub expected_before_snapshot_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AdminEmergencyOperation {
    MoveToNode,
    TerminateInstance,
}

impl AdminEmergencyOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MoveToNode => "MOVE_TO_NODE",
            Self::TerminateInstance => "TERMINATE_INSTANCE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminRelatedReference {
    pub resource_type: String,
    pub resource_id: String,
}

#[derive(Debug, Clone)]
pub struct AdminEmergencyOverrideCommand {
    pub principal_id: PrincipalId,
    pub idempotency_key: String,
    pub command_schema_version: String,
    pub workflow_instance_id: WorkflowInstanceId,
    pub expected_workflow_state_version: i32,
    pub operation: AdminEmergencyOperation,
    pub target_node_id: NodeId,
    pub reason: String,
    pub related_references: Vec<AdminRelatedReference>,
    pub expected_before_snapshot_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowProjection {
    pub current_context_revision_id: Option<Uuid>,
    pub current_node_visit_id: Option<Uuid>,
    pub workflow_state_version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeSnapshotV1 {
    pub schema_version: &'static str,
    pub workflow_instance_id: Uuid,
    pub domain_id: Uuid,
    pub definition_version_id: Uuid,
    pub created_by_principal_id: Uuid,
    pub current_context_revision_id: Option<Uuid>,
    pub current_node_visit_id: Option<Uuid>,
    pub workflow_state_version: i32,
}

impl BeforeSnapshotV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workflow_instance_id: Uuid,
        domain_id: Uuid,
        definition_version_id: Uuid,
        created_by_principal_id: Uuid,
        projection: &WorkflowProjection,
    ) -> Self {
        Self {
            schema_version: BEFORE_SNAPSHOT_SCHEMA_VERSION,
            workflow_instance_id,
            domain_id,
            definition_version_id,
            created_by_principal_id,
            current_context_revision_id: projection.current_context_revision_id,
            current_node_visit_id: projection.current_node_visit_id,
            workflow_state_version: projection.workflow_state_version,
        }
    }

    pub fn canonical_json(&self) -> Result<String, RecoveryError> {
        let json = serde_json::to_string(self)
            .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
        jcs_canonicalize::canonicalize(&json)
            .map_err(|error| RecoveryError::StorageError(error.to_string()))
    }

    pub fn digest(&self) -> Result<String, RecoveryError> {
        jcs_canonicalize::sha256_jcs_hex(self)
            .map_err(|error| RecoveryError::StorageError(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    PrincipalNotFound,
    PrincipalDisabled,
    PrincipalTypeNotAllowed,
    PermissionDenied,
    InstanceNotFound,
    InvalidInput(String),
    BeforeSnapshotDigestMismatch { expected: String, actual: String },
    WorkflowStateVersionConflict { expected: i32, actual: i32 },
    InvalidImmutableFacts(String),
    InvalidTarget(String),
    AssigneeResolutionFailed(String),
    IdempotencyConflict,
    CommandStillProcessing,
    InternalConsistency(String),
    StorageError(String),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.label())
    }
}

impl std::error::Error for RecoveryError {}

impl RecoveryError {
    pub fn status_code(&self) -> i32 {
        match self {
            Self::PrincipalNotFound | Self::InstanceNotFound => 404,
            Self::PrincipalDisabled | Self::PrincipalTypeNotAllowed | Self::PermissionDenied => 403,
            Self::InvalidInput(_) | Self::InvalidTarget(_) | Self::AssigneeResolutionFailed(_) => {
                422
            }
            Self::BeforeSnapshotDigestMismatch { .. }
            | Self::WorkflowStateVersionConflict { .. }
            | Self::IdempotencyConflict => 409,
            Self::CommandStillProcessing => 425,
            Self::InvalidImmutableFacts(_)
            | Self::InternalConsistency(_)
            | Self::StorageError(_) => 500,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::PrincipalNotFound => "principal_not_found",
            Self::PrincipalDisabled => "principal_disabled",
            Self::PrincipalTypeNotAllowed => "principal_type_not_allowed",
            Self::PermissionDenied => "permission_denied",
            Self::InstanceNotFound => "instance_not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::BeforeSnapshotDigestMismatch { .. } => "before_snapshot_digest_mismatch",
            Self::WorkflowStateVersionConflict { .. } => "workflow_state_version_conflict",
            Self::InvalidImmutableFacts(_) => "invalid_immutable_facts",
            Self::InvalidTarget(_) => "invalid_target",
            Self::AssigneeResolutionFailed(_) => "assignee_resolution_failed",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::CommandStillProcessing => "command_still_processing",
            Self::InternalConsistency(_) => "internal_consistency_error",
            Self::StorageError(_) => "storage_error",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::InvalidInput(value)
            | Self::InvalidImmutableFacts(value)
            | Self::InvalidTarget(value)
            | Self::AssigneeResolutionFailed(value)
            | Self::InternalConsistency(value)
            | Self::StorageError(value) => Some(value),
            _ => None,
        }
    }
}
