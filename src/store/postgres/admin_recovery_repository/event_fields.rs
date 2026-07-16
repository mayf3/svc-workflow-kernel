use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::workflow_instance::recovery::RecoveryError;

use super::rows::EventFact;

fn invalid(detail: impl Into<String>) -> RecoveryError {
    RecoveryError::InvalidImmutableFacts(detail.into())
}

pub(super) fn exact_keys(value: &serde_json::Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
    })
}

pub(super) fn uuid_field(value: &serde_json::Value, key: &str) -> Option<Uuid> {
    value.get(key)?.as_str()?.parse().ok()
}

pub(super) fn string_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

pub(super) fn optional_string_field<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Option<Option<&'a str>> {
    match value.get(key)? {
        serde_json::Value::Null => Some(None),
        serde_json::Value::String(value) => Some(Some(value)),
        _ => None,
    }
}

pub(super) fn event_data(event: &EventFact) -> Result<&serde_json::Value, RecoveryError> {
    let data = event
        .event_data
        .as_ref()
        .ok_or_else(|| invalid("event data is missing"))?;
    let actual = digest::compute_json_digest(data).map_err(RecoveryError::StorageError)?;
    if event.event_data_digest.as_deref() != Some(actual.as_str()) {
        return Err(invalid("event data digest mismatch"));
    }
    Ok(data)
}

pub(super) fn admin_payload_is_bounded(data: &serde_json::Value) -> bool {
    let Some(reason) = string_field(data, "reason") else {
        return false;
    };
    if reason != reason.trim()
        || reason.is_empty()
        || reason.chars().count() > 2000
        || reason.chars().any(char::is_control)
    {
        return false;
    }
    let Some(references) = data["relatedReferences"].as_array() else {
        return false;
    };
    references.len() <= 20
        && references.iter().all(|reference| {
            if !exact_keys(reference, &["resourceType", "resourceId"]) {
                return false;
            }
            let (Some(resource_type), Some(resource_id)) = (
                string_field(reference, "resourceType"),
                string_field(reference, "resourceId"),
            ) else {
                return false;
            };
            !resource_type.is_empty()
                && resource_type.len() <= 128
                && !resource_id.is_empty()
                && resource_id.len() <= 256
                && !resource_type.chars().any(char::is_control)
                && !resource_id.chars().any(char::is_control)
        })
}
