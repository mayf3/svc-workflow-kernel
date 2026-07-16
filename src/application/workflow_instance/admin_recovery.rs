//! Administrative emergency recovery application service.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::workflow_instance::recovery::{
    AdminEmergencyOverrideCommand, RebuildProjectionCommand, RecoveryError, WorkflowProjection,
    COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE, COMMAND_TYPE_REBUILD_PROJECTION,
};
use crate::store::postgres::admin_recovery_repository;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildProjectionResult {
    pub command_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub before_projection: WorkflowProjection,
    pub after_projection: WorkflowProjection,
    pub before_snapshot_digest: String,
    pub changed: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminEmergencyOverrideResult {
    pub command_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub source_node_visit_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub workflow_state_version: i32,
    pub event_sequence: i32,
    pub before_snapshot_digest: String,
    pub replayed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestEnvelope<T> {
    command_schema_version: String,
    command_type: &'static str,
    route_parameters: serde_json::Value,
    request_body: T,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RebuildBody {
    principal_id: String,
    workflow_instance_id: String,
    expected_before_snapshot_digest: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OverrideBody {
    principal_id: String,
    workflow_instance_id: String,
    expected_workflow_state_version: i32,
    operation: &'static str,
    target_node_id: String,
    reason: String,
    related_references: Vec<crate::domain::workflow_instance::recovery::AdminRelatedReference>,
    expected_before_snapshot_digest: Option<String>,
}

fn hash<T: Serialize>(envelope: &RequestEnvelope<T>) -> Result<String, RecoveryError> {
    jcs_canonicalize::sha256_jcs_hex(envelope)
        .map_err(|error| RecoveryError::StorageError(error.to_string()))
}

fn rebuild_hash(command: &RebuildProjectionCommand) -> Result<String, RecoveryError> {
    hash(&RequestEnvelope {
        command_schema_version: command.command_schema_version.clone(),
        command_type: COMMAND_TYPE_REBUILD_PROJECTION,
        route_parameters: serde_json::json!({}),
        request_body: RebuildBody {
            principal_id: command.principal_id.to_string(),
            workflow_instance_id: command.workflow_instance_id.to_string(),
            expected_before_snapshot_digest: command.expected_before_snapshot_digest.clone(),
        },
    })
}

fn override_hash(command: &AdminEmergencyOverrideCommand) -> Result<String, RecoveryError> {
    hash(&RequestEnvelope {
        command_schema_version: command.command_schema_version.clone(),
        command_type: COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE,
        route_parameters: serde_json::json!({}),
        request_body: OverrideBody {
            principal_id: command.principal_id.to_string(),
            workflow_instance_id: command.workflow_instance_id.to_string(),
            expected_workflow_state_version: command.expected_workflow_state_version,
            operation: command.operation.as_str(),
            target_node_id: command.target_node_id.to_string(),
            reason: command.reason.clone(),
            related_references: command.related_references.clone(),
            expected_before_snapshot_digest: command.expected_before_snapshot_digest.clone(),
        },
    })
}

fn validate_key(command_schema_version: &str, idempotency_key: &str) -> Result<(), RecoveryError> {
    if command_schema_version.is_empty() || command_schema_version.len() > 64 {
        return Err(RecoveryError::InvalidInput(
            "command_schema_version must contain 1..64 bytes".to_string(),
        ));
    }
    if idempotency_key.is_empty() || idempotency_key.len() > 512 {
        return Err(RecoveryError::InvalidInput(
            "idempotency_key must contain 1..512 bytes".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_principal_exists(pool: &PgPool, principal_id: Uuid) -> Result<(), RecoveryError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM principals WHERE principal_id = $1)")
            .bind(principal_id)
            .fetch_one(pool)
            .await
            .map_err(|error| RecoveryError::StorageError(error.to_string()))?;
    exists.then_some(()).ok_or(RecoveryError::PrincipalNotFound)
}

pub async fn rebuild_projection(
    pool: &PgPool,
    command: RebuildProjectionCommand,
) -> Result<RebuildProjectionResult, RecoveryError> {
    validate_key(&command.command_schema_version, &command.idempotency_key)?;
    ensure_principal_exists(pool, command.principal_id.into_uuid()).await?;
    let request_hash = rebuild_hash(&command)?;
    admin_recovery_repository::rebuild_projection(pool, command, &request_hash).await
}

pub async fn admin_emergency_override(
    pool: &PgPool,
    command: AdminEmergencyOverrideCommand,
) -> Result<AdminEmergencyOverrideResult, RecoveryError> {
    validate_key(&command.command_schema_version, &command.idempotency_key)?;
    ensure_principal_exists(pool, command.principal_id.into_uuid()).await?;
    let request_hash = override_hash(&command)?;
    admin_recovery_repository::admin_emergency_override(pool, command, &request_hash).await
}
