use super::*;
use svc_workflow::application::workflow_instance::idempotency::compute_combined_request_hash;

const PRINCIPAL_ID: &str = "11111111-1111-1111-1111-111111111111";
const INSTANCE_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const TRANSITION_ID: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
const GOLDEN_CANONICAL: &str = r#"{"command_schema_version":"v1","command_type":"REVISE_CONTEXT_AND_TRANSITION","request_body":{"context_payload":{"title":"golden"},"expected_workflow_state_version":1,"principal_id":"11111111-1111-1111-1111-111111111111","submission_payload":{"summary":"ready"},"transition_definition_id":"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb","workflow_instance_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"},"route_parameters":{}}"#;
const GOLDEN_SHA256: &str = "b2dee7389ffcba0a14bccde2fdf9033d5dd17cc54c1874baf3ab66b9662026cd";

#[test]
fn combined_request_hash_canonical_shape_is_frozen() {
    let envelope = serde_json::json!({
        "command_schema_version": "v1",
        "command_type": "REVISE_CONTEXT_AND_TRANSITION",
        "route_parameters": {},
        "request_body": {
            "principal_id": PRINCIPAL_ID,
            "workflow_instance_id": INSTANCE_ID,
            "expected_workflow_state_version": 1,
            "transition_definition_id": TRANSITION_ID,
            "context_payload": {"title": "golden"},
            "submission_payload": {"summary": "ready"},
        }
    });
    let canonical = jcs_canonicalize::canonicalize(&envelope.to_string()).unwrap();
    assert_eq!(canonical, GOLDEN_CANONICAL);
}

#[test]
fn combined_request_hash_sha256_is_frozen() {
    let hash = compute_combined_request_hash(
        "v1",
        &PrincipalId::from_uuid(Uuid::parse_str(PRINCIPAL_ID).unwrap()),
        &WorkflowInstanceId::from_uuid(Uuid::parse_str(INSTANCE_ID).unwrap()),
        1,
        &TransitionId::from_uuid(Uuid::parse_str(TRANSITION_ID).unwrap()),
        &serde_json::json!({"title": "golden"}),
        &serde_json::json!({"summary": "ready"}),
    )
    .unwrap();
    assert_eq!(hash, GOLDEN_SHA256);
}

#[test]
fn combined_request_hash_covers_both_payloads() {
    let base = |context: serde_json::Value, submission: serde_json::Value| {
        compute_combined_request_hash(
            "v1",
            &PrincipalId::from_uuid(Uuid::parse_str(PRINCIPAL_ID).unwrap()),
            &WorkflowInstanceId::from_uuid(Uuid::parse_str(INSTANCE_ID).unwrap()),
            1,
            &TransitionId::from_uuid(Uuid::parse_str(TRANSITION_ID).unwrap()),
            &context,
            &submission,
        )
        .unwrap()
    };
    let golden = base(
        serde_json::json!({"title": "golden"}),
        serde_json::json!({"summary": "ready"}),
    );
    assert_ne!(
        golden,
        base(
            serde_json::json!({"title": "changed"}),
            serde_json::json!({"summary": "ready"})
        )
    );
    assert_ne!(
        golden,
        base(
            serde_json::json!({"title": "golden"}),
            serde_json::json!({"summary": "changed"})
        )
    );
}
