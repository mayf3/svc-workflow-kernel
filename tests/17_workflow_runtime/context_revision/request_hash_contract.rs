//! Golden test for ReviseWorkflowContext request hash.
//!
//! Uses the production `compute_revise_request_hash` function with fixed inputs.

use super::*;
use svc_workflow::application::workflow_instance::idempotency::compute_revise_request_hash;

const FIXED_PRINCIPAL_ID: &str = "11111111-1111-1111-1111-111111111111";
const FIXED_INSTANCE_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

#[allow(dead_code)]
const EXPECTED_CANONICAL_JSON: &str = r#"{"command_schema_version":"v1","command_type":"REVISE_WORKFLOW_CONTEXT","request_body":{"context_payload":{"title":"golden"},"expected_workflow_state_version":1,"principal_id":"11111111-1111-1111-1111-111111111111","workflow_instance_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"},"route_parameters":{}}"#;

const EXPECTED_SHA256_HEX: &str =
    "08575a676da7538f5b7a9c167ea10beae9c592ed3246d857bde04e949ff3490f";

fn make_fixed_command() -> ReviseWorkflowContextCommand {
    ReviseWorkflowContextCommand {
        principal_id: PrincipalId::from_uuid(uuid::Uuid::parse_str(FIXED_PRINCIPAL_ID).unwrap()),
        idempotency_key: "test-idem-key-not-in-hash".to_string(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(
            uuid::Uuid::parse_str(FIXED_INSTANCE_ID).unwrap(),
        ),
        expected_workflow_state_version: 1,
        context_payload: serde_json::json!({"title": "golden"}),
    }
}

#[test]
fn test_revise_request_hash_golden_sha256() {
    let cmd = make_fixed_command();
    let hash = compute_revise_request_hash(
        &cmd.command_schema_version,
        &cmd.idempotency_key,
        &cmd.principal_id,
        &cmd.workflow_instance_id,
        cmd.expected_workflow_state_version,
        &cmd.context_payload,
    )
    .expect("compute_revise_request_hash");
    assert_eq!(hash, EXPECTED_SHA256_HEX, "request hash mismatch");
}
