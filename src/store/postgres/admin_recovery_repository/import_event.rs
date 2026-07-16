use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::workflow_instance::recovery::RecoveryError;
use crate::store::postgres::import_receipt_validation::{
    validate_success, ImportReceiptFact, ImportedIdentity,
};

use super::event_fields::{exact_keys, string_field};
use super::rows::{ContextFact, EventFact, InstanceRow, VisitFact};

fn invalid(detail: impl Into<String>) -> RecoveryError {
    RecoveryError::InvalidImmutableFacts(detail.into())
}

fn canonical_uuid(value: &str) -> Option<Uuid> {
    let parsed = value.parse::<Uuid>().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn lowercase_digest(value: &str) -> bool {
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

pub(super) fn validate(
    data: &serde_json::Value,
    event: &EventFact,
    instance: &InstanceRow,
    context: &ContextFact,
    visit: &VisitFact,
) -> Result<(), RecoveryError> {
    let keys = [
        "legacySystem",
        "legacyRecordId",
        "legacySnapshotDigest",
        "importedNodeId",
        "importedAt",
        "creatorResolution",
    ];
    let record = string_field(data, "legacyRecordId").and_then(canonical_uuid);
    let imported_node = string_field(data, "importedNodeId").and_then(canonical_uuid);
    let digest = string_field(data, "legacySnapshotDigest");
    let imported_at = string_field(data, "importedAt");
    let resolution = string_field(data, "creatorResolution");
    let expected_reference = record.map(|id| format!("migration:adc:{id}:v1"));
    if !exact_keys(data, &keys)
        || string_field(data, "legacySystem") != Some("adc")
        || imported_node != Some(visit.node_id)
        || digest.is_none_or(|value| !lowercase_digest(value))
        || imported_at.is_none_or(|value| !whole_second_utc(value))
        || !matches!(resolution, Some("LEGACY_CREATOR" | "DOMAIN_OWNER_FALLBACK"))
        || instance.external_reference.as_ref() != expected_reference.as_ref()
        || instance.created_by_principal_type == "SERVICE"
        || event.actor_principal_type != "SERVICE"
        || event.actor_principal_id == instance.created_by_principal_id
        || context.created_by_principal_id != instance.created_by_principal_id
        || (visit.node_type == "TERMINAL" && visit.assignee_principal_id.is_some())
        || (visit.node_type != "TERMINAL" && visit.assignee_principal_id.is_none())
    {
        return Err(invalid("import event data or identity facts are invalid"));
    }
    Ok(())
}

/// Proves the imported event is anchored to exactly one completed, successful
/// `IMPORT_LEGACY_WORKFLOW_INSTANCE` receipt whose identity and stored response
/// match the immutable facts (audit High 2). Projection rebuild is the
/// corruption-repair gate, so a history that is not anchored to its import
/// command must be rejected rather than replayed.
pub(super) async fn validate_receipt_linkage(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventFact,
    instance: &InstanceRow,
    context: &ContextFact,
    visit: &VisitFact,
) -> Result<(), RecoveryError> {
    let command_id = event
        .command_id
        .ok_or_else(|| invalid("imported event is not anchored to an import command receipt"))?;
    let external_reference = instance
        .external_reference
        .as_deref()
        .ok_or_else(|| invalid("imported instance has no external reference"))?;
    let receipt: ImportReceiptFact = sqlx::query_as(
        "SELECT command_id, principal_id, idempotency_key, command_type,
                request_hash, receipt_status::text AS receipt_status,
                response_status, response_body, response_digest
         FROM workflow_command_receipts WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| RecoveryError::StorageError(error.to_string()))?
    .ok_or_else(|| invalid("imported event references a missing command receipt"))?;
    let data = event
        .event_data
        .as_ref()
        .ok_or_else(|| invalid("import event data is missing"))?;
    let snapshot_digest = string_field(data, "legacySnapshotDigest")
        .filter(|value| lowercase_digest(value))
        .ok_or_else(|| invalid("import event snapshot digest is invalid"))?;
    let resolution = string_field(data, "creatorResolution")
        .ok_or_else(|| invalid("import event creator resolution is invalid"))?;
    validate_success(
        &receipt,
        &ImportedIdentity {
            command_id,
            actor_principal_id: event.actor_principal_id,
            external_reference,
            workflow_instance_id: instance.workflow_instance_id,
            context_revision_id: context.context_revision_id,
            node_visit_id: visit.node_visit_id,
            event_id: event.event_id,
            legacy_snapshot_digest: snapshot_digest,
            creator_resolution: resolution,
        },
    )
    .map_err(|detail| invalid(format!("import receipt linkage is invalid: {detail}")))?;
    Ok(())
}
