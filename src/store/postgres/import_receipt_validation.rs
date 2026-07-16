use uuid::Uuid;

use crate::application::workflow_instance::import::ImportLegacyWorkflowInstanceResult;
use crate::domain::definition::digest;

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ImportReceiptFact {
    pub command_id: Uuid,
    pub principal_id: Uuid,
    pub idempotency_key: String,
    pub command_type: String,
    pub request_hash: String,
    pub receipt_status: String,
    pub response_status: Option<i32>,
    pub response_body: Option<serde_json::Value>,
    pub response_digest: Option<String>,
}

pub(crate) struct ImportedIdentity<'a> {
    pub command_id: Uuid,
    pub actor_principal_id: Uuid,
    pub external_reference: &'a str,
    pub workflow_instance_id: Uuid,
    pub context_revision_id: Uuid,
    pub node_visit_id: Uuid,
    pub event_id: Uuid,
    pub legacy_snapshot_digest: &'a str,
    pub creator_resolution: &'a str,
}

fn digest_equal(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn lowercase_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn validate_stored_digest(receipt: &ImportReceiptFact) -> Result<(), String> {
    let body = receipt
        .response_body
        .as_ref()
        .ok_or_else(|| "completed import receipt has no response body".to_string())?;
    let stored = receipt
        .response_digest
        .as_deref()
        .filter(|value| lowercase_digest(value))
        .ok_or_else(|| "completed import receipt has no valid response digest".to_string())?;
    let actual = digest::compute_json_digest(body)?;
    if !digest_equal(stored, &actual) {
        return Err("import receipt response digest mismatch".to_string());
    }
    Ok(())
}

pub(crate) fn parse_success(
    receipt: &ImportReceiptFact,
) -> Result<ImportLegacyWorkflowInstanceResult, String> {
    validate_stored_digest(receipt)?;
    if receipt.receipt_status != "COMPLETED" || receipt.response_status != Some(200) {
        return Err("import receipt is not a completed success".to_string());
    }
    let result: ImportLegacyWorkflowInstanceResult = serde_json::from_value(
        receipt
            .response_body
            .clone()
            .ok_or_else(|| "completed import receipt has no response body".to_string())?,
    )
    .map_err(|error| format!("invalid import success response: {error}"))?;
    if result.replayed
        || result.workflow_state_version != 1
        || result.event_sequence != 1
        || !lowercase_digest(&result.legacy_snapshot_digest)
    {
        return Err("import success response has invalid fixed fields".to_string());
    }
    Ok(result)
}

pub(crate) fn validate_success(
    receipt: &ImportReceiptFact,
    identity: &ImportedIdentity<'_>,
) -> Result<ImportLegacyWorkflowInstanceResult, String> {
    if receipt.command_id != identity.command_id
        || receipt.command_type != "IMPORT_LEGACY_WORKFLOW_INSTANCE"
        || receipt.principal_id != identity.actor_principal_id
        || receipt.idempotency_key != identity.external_reference
    {
        return Err("import receipt identity does not match immutable facts".to_string());
    }
    let result = parse_success(receipt)?;
    if result.command_id != identity.command_id
        || result.workflow_instance_id != identity.workflow_instance_id
        || result.current_context_revision_id != identity.context_revision_id
        || result.current_node_visit_id != identity.node_visit_id
        || result.event_id != identity.event_id
        || result.legacy_snapshot_digest != identity.legacy_snapshot_digest
        || result.creator_resolution.as_str() != identity.creator_resolution
    {
        return Err("import receipt response does not match immutable facts".to_string());
    }
    Ok(result)
}
