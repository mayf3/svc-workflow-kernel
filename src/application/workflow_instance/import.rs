//! Application boundary for the ADC legacy initial-import primitive.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::workflow_instance::import::{
    CreatorResolution, ImportLegacyWorkflowInstanceCommand, LegacyImportError, COMMAND_TYPE,
};
use crate::store::postgres::legacy_import_repository;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportLegacyWorkflowInstanceResult {
    pub command_id: Uuid,
    pub workflow_instance_id: Uuid,
    pub current_context_revision_id: Uuid,
    pub current_node_visit_id: Uuid,
    pub event_id: Uuid,
    pub workflow_state_version: i32,
    pub event_sequence: i32,
    pub legacy_snapshot_digest: String,
    pub creator_resolution: CreatorResolution,
    pub replayed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestEnvelope<'a> {
    command_schema_version: &'a str,
    command_type: &'static str,
    route_parameters: RouteParameters,
    request_body: RequestBody<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteParameters {
    domain_id: String,
    definition_version_id: String,
    imported_node_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestBody<'a> {
    principal_id: String,
    legacy_record_id: Uuid,
    legacy_snapshot: &'a crate::domain::workflow_instance::import::LegacyAdcImportSnapshotV1,
    expected_legacy_snapshot_digest: &'a str,
    legacy_creator_principal_id: Option<String>,
    external_url: &'a Option<String>,
    metadata: &'a serde_json::Value,
}

pub fn compute_legacy_import_request_hash(
    command: &ImportLegacyWorkflowInstanceCommand,
) -> Result<String, LegacyImportError> {
    let envelope = RequestEnvelope {
        command_schema_version: &command.command_schema_version,
        command_type: COMMAND_TYPE,
        route_parameters: RouteParameters {
            domain_id: command.domain_id.to_string(),
            definition_version_id: command.definition_version_id.to_string(),
            imported_node_id: command.imported_node_id.to_string(),
        },
        request_body: RequestBody {
            principal_id: command.principal_id.to_string(),
            legacy_record_id: command.legacy_record_id,
            legacy_snapshot: &command.legacy_snapshot,
            expected_legacy_snapshot_digest: &command.expected_legacy_snapshot_digest,
            legacy_creator_principal_id: command
                .legacy_creator_principal_id
                .map(|principal| principal.to_string()),
            external_url: &command.external_url,
            metadata: &command.metadata,
        },
    };
    jcs_canonicalize::sha256_jcs_hex(&envelope)
        .map_err(|error| LegacyImportError::StorageError(error.to_string()))
}

async fn ensure_principal_exists(
    pool: &PgPool,
    principal_id: Uuid,
) -> Result<(), LegacyImportError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM principals WHERE principal_id = $1)")
            .bind(principal_id)
            .fetch_one(pool)
            .await
            .map_err(|error| LegacyImportError::StorageError(error.to_string()))?;
    exists
        .then_some(())
        .ok_or(LegacyImportError::PrincipalNotFound)
}

pub async fn import_legacy_workflow_instance(
    pool: &PgPool,
    command: ImportLegacyWorkflowInstanceCommand,
) -> Result<ImportLegacyWorkflowInstanceResult, LegacyImportError> {
    ensure_principal_exists(pool, command.principal_id.into_uuid()).await?;
    let request_hash = compute_legacy_import_request_hash(&command)?;
    legacy_import_repository::import(pool, command, &request_hash).await
}
