use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::application::workflow_instance::import::ImportLegacyWorkflowInstanceResult;
use crate::domain::definition::digest;
use crate::domain::workflow_instance::import::LegacyImportError;
use crate::store::postgres::import_receipt_validation::{
    parse_success, validate_success, ImportReceiptFact, ImportedIdentity,
};

#[derive(sqlx::FromRow)]
struct ReplayFactRow {
    workflow_instance_id: Uuid,
    external_reference: Option<String>,
    context_revision_id: Uuid,
    revision_number: i32,
    previous_revision_id: Option<Uuid>,
    node_visit_id: Uuid,
    node_id: Uuid,
    visit_number: i32,
    entered_by_transition_id: Option<Uuid>,
    event_id: Uuid,
    command_id: Option<Uuid>,
    event_type: String,
    event_sequence: i32,
    event_schema_version: String,
    transition_effect: Option<String>,
    source_node_visit_id: Option<Uuid>,
    target_node_visit_id: Option<Uuid>,
    event_context_revision_id: Option<Uuid>,
    submission_id: Option<Uuid>,
    actor_principal_id: Uuid,
    from_node_id: Option<Uuid>,
    to_node_id: Option<Uuid>,
    old_workflow_state_version: i32,
    new_workflow_state_version: i32,
    event_data: Option<serde_json::Value>,
    event_data_digest: Option<String>,
}

fn internal(detail: impl Into<String>) -> LegacyImportError {
    LegacyImportError::InternalConsistency(detail.into())
}

fn exact_keys(value: &serde_json::Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
    })
}

fn canonical_uuid(value: &str) -> Option<Uuid> {
    let parsed = value.parse::<Uuid>().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn lower_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn whole_second_utc(value: &str) -> bool {
    value.len() == 20
        && chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%SZ")
            .is_ok_and(|parsed| parsed.format("%Y-%m-%dT%H:%M:%SZ").to_string() == value)
}

async fn load_facts(
    tx: &mut Transaction<'_, Postgres>,
    response: &ImportLegacyWorkflowInstanceResult,
) -> Result<ReplayFactRow, LegacyImportError> {
    sqlx::query_as(
        "SELECT i.workflow_instance_id, i.external_reference,
                c.context_revision_id, c.revision_number, c.previous_revision_id,
                v.node_visit_id, v.node_id, v.visit_number,
                v.entered_by_transition_id,
                e.event_id, e.command_id, e.event_type, e.event_sequence,
                e.event_schema_version, e.transition_effect::text,
                e.source_node_visit_id, e.target_node_visit_id,
                e.context_revision_id AS event_context_revision_id, e.submission_id,
                e.actor_principal_id, e.from_node_id, e.to_node_id,
                e.old_workflow_state_version, e.new_workflow_state_version,
                e.event_data, e.event_data_digest
         FROM workflow_instances i
         JOIN workflow_context_revisions c
           ON c.context_revision_id = $2 AND c.workflow_instance_id = i.workflow_instance_id
         JOIN workflow_node_visits v
           ON v.node_visit_id = $3 AND v.workflow_instance_id = i.workflow_instance_id
         JOIN workflow_events e
           ON e.event_id = $4 AND e.workflow_instance_id = i.workflow_instance_id
         WHERE i.workflow_instance_id = $1",
    )
    .bind(response.workflow_instance_id)
    .bind(response.current_context_revision_id)
    .bind(response.current_node_visit_id)
    .bind(response.event_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| LegacyImportError::StorageError(error.to_string()))?
    .ok_or_else(|| internal("import receipt response references missing immutable facts"))
}

fn validate_event_data(facts: &ReplayFactRow) -> Result<(&str, &str), LegacyImportError> {
    let data = facts
        .event_data
        .as_ref()
        .ok_or_else(|| internal("import event data is missing"))?;
    let actual = digest::compute_json_digest(data).map_err(LegacyImportError::StorageError)?;
    if facts.event_data_digest.as_deref() != Some(actual.as_str())
        || !exact_keys(
            data,
            &[
                "legacySystem",
                "legacyRecordId",
                "legacySnapshotDigest",
                "importedNodeId",
                "importedAt",
                "creatorResolution",
            ],
        )
    {
        return Err(internal("import event data digest or shape is invalid"));
    }
    let string = |key| data.get(key).and_then(serde_json::Value::as_str);
    let record = string("legacyRecordId").and_then(canonical_uuid);
    let snapshot_digest = string("legacySnapshotDigest")
        .filter(|value| lower_digest(value))
        .ok_or_else(|| internal("import event snapshot digest is invalid"))?;
    let resolution = string("creatorResolution")
        .filter(|value| matches!(*value, "LEGACY_CREATOR" | "DOMAIN_OWNER_FALLBACK"))
        .ok_or_else(|| internal("import event creator resolution is invalid"))?;
    if string("legacySystem") != Some("adc")
        || string("importedNodeId").and_then(canonical_uuid) != Some(facts.node_id)
        || string("importedAt").is_none_or(|value| !whole_second_utc(value))
        || facts.external_reference.as_deref()
            != record
                .map(|value| format!("migration:adc:{value}:v1"))
                .as_deref()
    {
        return Err(internal(
            "import event values disagree with immutable facts",
        ));
    }
    Ok((snapshot_digest, resolution))
}

pub(super) async fn replay_success(
    tx: &mut Transaction<'_, Postgres>,
    receipt: &ImportReceiptFact,
    expected_snapshot_digest: &str,
) -> Result<ImportLegacyWorkflowInstanceResult, LegacyImportError> {
    let response = parse_success(receipt).map_err(internal)?;
    let facts = load_facts(tx, &response).await?;
    if facts.revision_number != 1
        || facts.previous_revision_id.is_some()
        || facts.visit_number != 1
        || facts.entered_by_transition_id.is_some()
        || facts.command_id != Some(receipt.command_id)
        || facts.event_type != "WORKFLOW_INSTANCE_IMPORTED"
        || facts.event_sequence != 1
        || facts.event_schema_version != "v1"
        || facts.transition_effect.is_some()
        || facts.source_node_visit_id.is_some()
        || facts.target_node_visit_id != Some(facts.node_visit_id)
        || facts.event_context_revision_id != Some(facts.context_revision_id)
        || facts.submission_id.is_some()
        || facts.from_node_id.is_some()
        || facts.to_node_id.is_some()
        || facts.old_workflow_state_version != 0
        || facts.new_workflow_state_version != 1
    {
        return Err(internal("import receipt event matrix is invalid"));
    }
    let external_reference = facts
        .external_reference
        .as_deref()
        .ok_or_else(|| internal("imported instance has no external reference"))?;
    let (snapshot_digest, resolution) = validate_event_data(&facts)?;
    if snapshot_digest != expected_snapshot_digest {
        return Err(internal(
            "replayed import snapshot digest does not match the current request",
        ));
    }
    validate_success(
        receipt,
        &ImportedIdentity {
            command_id: receipt.command_id,
            actor_principal_id: facts.actor_principal_id,
            external_reference,
            workflow_instance_id: facts.workflow_instance_id,
            context_revision_id: facts.context_revision_id,
            node_visit_id: facts.node_visit_id,
            event_id: facts.event_id,
            legacy_snapshot_digest: snapshot_digest,
            creator_resolution: resolution,
        },
    )
    .map_err(internal)
}
