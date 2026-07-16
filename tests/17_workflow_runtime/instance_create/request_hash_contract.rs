//! Golden test for the requestHash computation.
//!
//! The request hash is computed by `compute_request_hash()` which builds a
//! canonical JCS envelope and produces SHA-256(JCS(envelope)).
//!
//! This test fixes the expected canonical JSON structure and SHA-256 hex for
//! a known command input. Any change to the field names, null handling, or
//! JSON structure will cause the test to fail, providing a contract-level guard.
//!
//! Per the contract, the envelope uses snake_case field names (matching the Rust
//! Serialize derive, not the old documentation's camelCase). The hash is
//! self-consistent — computed and compared within the same codebase.

use super::*;
use svc_workflow::application::workflow_instance::idempotency::compute_request_hash;

/// Fixed UUIDs for deterministic hash computation.
const FIXED_PRINCIPAL_ID: &str = "11111111-1111-1111-1111-111111111111";
const FIXED_DOMAIN_ID: &str = "22222222-2222-2222-2222-222222222222";
const FIXED_DEF_VERSION_ID: &str = "33333333-3333-3333-3333-333333333333";

/// Expected canonical JCS JSON for the fixed command.
///
/// Rules observed:
/// - snake_case field names (matching Rust `#[derive(Serialize)]` without rename)
/// - null fields serialized as `null` (Option::None → null)
/// - route_parameters is a stable empty object `{}`
/// - command_type is the constant "CREATE_WORKFLOW_INSTANCE"
/// - command_schema_version is "v1"
/// - idempotency_key is excluded from the hash
/// - JCS sorts all object keys alphabetically
const EXPECTED_CANONICAL_JSON: &str = r#"{"command_schema_version":"v1","command_type":"CREATE_WORKFLOW_INSTANCE","request_body":{"context_payload":{"hello":"world"},"definition_version_id":"33333333-3333-3333-3333-333333333333","domain_id":"22222222-2222-2222-2222-222222222222","external_reference":null,"external_url":null,"metadata":{"source":"test"},"principal_id":"11111111-1111-1111-1111-111111111111"},"route_parameters":{}}"#;

/// Expected SHA-256 hex digest of the canonical JSON above.
const EXPECTED_SHA256_HEX: &str =
    "ba40a90a5227ae7608f36e0bc2f0ca21092e1a3e56d5380f93655693b55a0d97";

fn make_fixed_command() -> CreateWorkflowInstanceCommand {
    CreateWorkflowInstanceCommand {
        principal_id: PrincipalId::from_uuid(uuid::Uuid::parse_str(FIXED_PRINCIPAL_ID).unwrap()),
        idempotency_key: "test-idem-key-not-in-hash".to_string(),
        command_schema_version: "v1".to_string(),
        domain_id: DomainId::from_uuid(uuid::Uuid::parse_str(FIXED_DOMAIN_ID).unwrap()),
        definition_version_id: DefinitionVersionId::from_uuid(
            uuid::Uuid::parse_str(FIXED_DEF_VERSION_ID).unwrap(),
        ),
        external_reference: None,
        external_url: None,
        metadata: serde_json::json!({"source": "test"}),
        context_payload: serde_json::json!({"hello": "world"}),
    }
}

/// Reconstruct the envelope struct and canonicalize it to verify the expected JSON.
/// This uses the same JCS crate as the production code.
fn compute_canonical_json(cmd: &CreateWorkflowInstanceCommand) -> String {
    // Build the same struct that compute_request_hash builds internally.
    // We duplicate it here to avoid exporting the private RequestEnvelope type,
    // but use the same field structure and the same jcs_canonicalize crate.
    #[derive(serde::Serialize)]
    struct RequestEnvelope {
        command_schema_version: String,
        command_type: String,
        route_parameters: serde_json::Value,
        request_body: RequestBody,
    }

    #[derive(serde::Serialize)]
    struct RequestBody {
        principal_id: String,
        domain_id: String,
        definition_version_id: String,
        context_payload: serde_json::Value,
        metadata: serde_json::Value,
        external_reference: Option<String>,
        external_url: Option<String>,
    }

    let envelope = RequestEnvelope {
        command_schema_version: cmd.command_schema_version.clone(),
        command_type: "CREATE_WORKFLOW_INSTANCE".to_string(),
        route_parameters: serde_json::json!({}),
        request_body: RequestBody {
            principal_id: cmd.principal_id.to_string(),
            domain_id: cmd.domain_id.to_string(),
            definition_version_id: cmd.definition_version_id.to_string(),
            context_payload: cmd.context_payload.clone(),
            metadata: cmd.metadata.clone(),
            external_reference: cmd.external_reference.clone(),
            external_url: cmd.external_url.clone(),
        },
    };

    // Serialize to JSON first, then canonicalize the JSON string
    let raw = serde_json::to_string(&envelope).expect("serialize envelope");
    jcs_canonicalize::canonicalize(&raw).expect("canonicalize should succeed")
}

#[test]
fn test_request_hash_golden_canonical_json() {
    let cmd = make_fixed_command();

    // 1. Verify the canonical JSON matches expected
    let canonical = compute_canonical_json(&cmd);
    assert_eq!(
        canonical, EXPECTED_CANONICAL_JSON,
        "canonical JSON mismatch — contract fields may have changed"
    );
}

#[test]
fn test_request_hash_golden_sha256() {
    let cmd = make_fixed_command();

    // 2. Verify the SHA-256 hex matches expected
    let hash = compute_request_hash(
        &cmd.command_schema_version,
        &cmd.idempotency_key,
        &cmd.principal_id,
        &cmd.domain_id,
        &cmd.definition_version_id,
        &cmd.context_payload,
        &cmd.metadata,
        &cmd.external_reference,
        &cmd.external_url,
    )
    .expect("compute_request_hash should succeed");

    assert_eq!(
        hash, EXPECTED_SHA256_HEX,
        "request hash mismatch — contract fields may have changed"
    );
}
