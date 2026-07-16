//! Golden test for the legacy import requestHash computation.
//!
//! `compute_legacy_import_request_hash()` builds a canonical JCS envelope and
//! produces SHA-256(JCS(envelope)). Unlike the create/transition/revise
//! envelopes, the legacy import envelope uses camelCase field names.
//!
//! This test fixes the expected canonical JSON structure and SHA-256 hex for a
//! known command input. Any change to the field names, null handling, or JSON
//! structure will cause the test to fail, providing a contract-level guard
//! against silent envelope drift that could invalidate deployed retries.

use svc_workflow::application::workflow_instance::import::compute_legacy_import_request_hash;
use svc_workflow::domain::ids::{DefinitionVersionId, DomainId, NodeId, PrincipalId};
use svc_workflow::domain::workflow_instance::import::{
    ImportLegacyWorkflowInstanceCommand, LegacyAdcImportSnapshotV1, COMMAND_SCHEMA_VERSION,
    COMMAND_TYPE, SNAPSHOT_SCHEMA_VERSION,
};

/// Fixed UUIDs for deterministic hash computation.
const FIXED_PRINCIPAL_ID: &str = "11111111-1111-1111-1111-111111111111";
const FIXED_DOMAIN_ID: &str = "22222222-2222-2222-2222-222222222222";
const FIXED_DEF_VERSION_ID: &str = "33333333-3333-3333-3333-333333333333";
const FIXED_NODE_ID: &str = "44444444-4444-4444-4444-444444444444";
const FIXED_LEGACY_RECORD_ID: &str = "55555555-5555-5555-5555-555555555555";
const FIXED_CREATOR_ID: &str = "66666666-6666-6666-6666-666666666666";

/// Expected canonical JCS JSON for the fixed command.
///
/// Rules observed:
/// - camelCase field names (matching the Rust `#[serde(rename_all = "camelCase")]`)
/// - the envelope excludes the derived idempotency key
/// - route parameters carry domain / definition / node IDs
/// - request body carries the actor, full snapshot, digest, creator, URL, metadata
/// - JCS sorts all object keys alphabetically
const EXPECTED_CANONICAL_JSON: &str = r#"{"commandSchemaVersion":"v1","commandType":"IMPORT_LEGACY_WORKFLOW_INSTANCE","requestBody":{"expectedLegacySnapshotDigest":"5ad699c70ff84564a3df969602833f77e0f783d3ebd3637c89439dad5fec50ce","externalUrl":null,"legacyCreatorPrincipalId":null,"legacyRecordId":"55555555-5555-5555-5555-555555555555","legacySnapshot":{"assigneeId":null,"contextPayload":{"description":"golden requirement","domainKey":"dev","title":"golden"},"currentStep":"draft","domainKey":"dev","requesterId":"66666666-6666-6666-6666-666666666666","requirementId":"55555555-5555-5555-5555-555555555555","schemaVersion":"ADC_WORKFLOW_IMPORT_SNAPSHOT_V1","stateVersion":3,"updatedAt":"2026-01-01T00:00:00Z","workflowId":"wf-1","workflowSnapshot":{"template":"hotfix"}},"metadata":{"source":"adc-import"},"principalId":"11111111-1111-1111-1111-111111111111"},"routeParameters":{"definitionVersionId":"33333333-3333-3333-3333-333333333333","domainId":"22222222-2222-2222-2222-222222222222","importedNodeId":"44444444-4444-4444-4444-444444444444"}}"#;

/// Expected SHA-256 hex digest of the canonical JSON above.
const EXPECTED_SHA256_HEX: &str =
    "110313a0ac7aa18c2d4db09ee2e879351437ec2bc2a717fddc80b7a67b304088";

fn fixed_snapshot() -> LegacyAdcImportSnapshotV1 {
    LegacyAdcImportSnapshotV1 {
        schema_version: SNAPSHOT_SCHEMA_VERSION.to_string(),
        requirement_id: uuid::Uuid::parse_str(FIXED_LEGACY_RECORD_ID).unwrap(),
        domain_key: "dev".to_string(),
        workflow_id: "wf-1".to_string(),
        workflow_snapshot: serde_json::json!({"template": "hotfix"}),
        current_step: "draft".to_string(),
        assignee_id: None,
        requester_id: Some(uuid::Uuid::parse_str(FIXED_CREATOR_ID).unwrap()),
        state_version: 3,
        updated_at: "2026-01-01T00:00:00Z".to_string(),
        context_payload: serde_json::json!({"title": "golden", "description": "golden requirement", "domainKey": "dev"}),
    }
}

fn make_fixed_command() -> ImportLegacyWorkflowInstanceCommand {
    let snapshot = fixed_snapshot();
    let expected_digest = snapshot.digest().expect("snapshot digest");
    ImportLegacyWorkflowInstanceCommand {
        principal_id: PrincipalId::from_uuid(uuid::Uuid::parse_str(FIXED_PRINCIPAL_ID).unwrap()),
        command_schema_version: COMMAND_SCHEMA_VERSION.to_string(),
        domain_id: DomainId::from_uuid(uuid::Uuid::parse_str(FIXED_DOMAIN_ID).unwrap()),
        definition_version_id: DefinitionVersionId::from_uuid(
            uuid::Uuid::parse_str(FIXED_DEF_VERSION_ID).unwrap(),
        ),
        imported_node_id: NodeId::from_uuid(uuid::Uuid::parse_str(FIXED_NODE_ID).unwrap()),
        legacy_record_id: uuid::Uuid::parse_str(FIXED_LEGACY_RECORD_ID).unwrap(),
        legacy_snapshot: snapshot,
        expected_legacy_snapshot_digest: expected_digest,
        legacy_creator_principal_id: None,
        external_url: None,
        metadata: serde_json::json!({"source": "adc-import"}),
    }
}

#[test]
fn legacy_import_request_hash_golden_canonical_json() {
    let cmd = make_fixed_command();

    // Rebuild the same camelCase envelope the production code builds, using the
    // same jcs_canonicalize crate, to verify the frozen canonical JSON shape.
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RequestEnvelope<'a> {
        command_schema_version: &'a str,
        command_type: &'static str,
        route_parameters: RouteParameters,
        request_body: RequestBody<'a>,
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RouteParameters {
        domain_id: String,
        definition_version_id: String,
        imported_node_id: String,
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RequestBody<'a> {
        principal_id: String,
        legacy_record_id: uuid::Uuid,
        legacy_snapshot: &'a LegacyAdcImportSnapshotV1,
        expected_legacy_snapshot_digest: &'a str,
        legacy_creator_principal_id: Option<String>,
        external_url: &'a Option<String>,
        metadata: &'a serde_json::Value,
    }

    let envelope = RequestEnvelope {
        command_schema_version: &cmd.command_schema_version,
        command_type: COMMAND_TYPE,
        route_parameters: RouteParameters {
            domain_id: cmd.domain_id.to_string(),
            definition_version_id: cmd.definition_version_id.to_string(),
            imported_node_id: cmd.imported_node_id.to_string(),
        },
        request_body: RequestBody {
            principal_id: cmd.principal_id.to_string(),
            legacy_record_id: cmd.legacy_record_id,
            legacy_snapshot: &cmd.legacy_snapshot,
            expected_legacy_snapshot_digest: &cmd.expected_legacy_snapshot_digest,
            legacy_creator_principal_id: cmd
                .legacy_creator_principal_id
                .map(|principal| principal.to_string()),
            external_url: &cmd.external_url,
            metadata: &cmd.metadata,
        },
    };

    let raw = serde_json::to_string(&envelope).expect("serialize envelope");
    let canonical = jcs_canonicalize::canonicalize(&raw).expect("canonicalize should succeed");
    assert_eq!(
        canonical, EXPECTED_CANONICAL_JSON,
        "canonical JSON mismatch — legacy import envelope fields may have changed"
    );
}

#[test]
fn legacy_import_request_hash_golden_sha256() {
    let cmd = make_fixed_command();
    let hash = compute_legacy_import_request_hash(&cmd).expect("hash should succeed");
    assert_eq!(
        hash, EXPECTED_SHA256_HEX,
        "request hash mismatch — legacy import envelope fields may have changed"
    );
}
