//! Context validation tests.
//!
//! Covers: valid context, size limits, pre-transaction rejection,
//! and non-null context_schema validation (both valid and invalid payloads).

use super::*;

/// JSON Schema that requires title (string, minLength 1) and priority (integer >= 0),
/// and forbids additional properties.
fn required_fields_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["title", "priority"],
        "properties": {
            "title": {"type": "string", "minLength": 1},
            "priority": {"type": "integer", "minimum": 0}
        },
        "additionalProperties": false
    })
}

#[tokio::test]
async fn test_valid_context_accepted() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"any": "value"});
    let result = create_workflow_instance(&pool, cmd)
        .await
        .expect("should succeed");
    verify_creation(&pool, &result, principal_id, domain_id, ver_id).await;
}

#[tokio::test]
async fn test_context_payload_too_large_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let big_str = "x".repeat(1024 * 1024 + 1);
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"data": big_str});
    let err = create_workflow_instance(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::SizeLimitExceeded(_)
    ));
}

#[tokio::test]
async fn test_metadata_too_large_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let big_str = "x".repeat(64 * 1024 + 1);
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.metadata = serde_json::json!({"data": big_str});
    let err = create_workflow_instance(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::SizeLimitExceeded(_)
    ));
}

#[tokio::test]
async fn test_size_failure_completes_receipt_without_runtime_artifacts() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let big_str = "x".repeat(64 * 1024 + 1);
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.metadata = serde_json::json!({"data": big_str});
    let idem_key = cmd.idempotency_key.clone();
    let err = create_workflow_instance(&pool, cmd).await;
    assert!(err.is_err());
    let receipt: (String, i32) = sqlx::query_as(
        "SELECT receipt_status::text, response_status FROM workflow_command_receipts \
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(&idem_key)
    .fetch_one(&pool)
    .await
    .expect("size failure receipt");
    assert_eq!(receipt, ("COMPLETED".to_string(), 413));
    let runtime_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_instances WHERE created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .expect("runtime count");
    assert_eq!(runtime_count, 0);
}

// ---------------------------------------------------------------------------
// Non-null context_schema tests (H2 coverage)
// ---------------------------------------------------------------------------

/// Return type for schema rejection assertions that also captures receipt state
/// for replay verification.
#[allow(dead_code)]
struct SchemaRejectionReceipt {
    command_id: Uuid,
    response_digest: String,
}

/// Helper: attempt creation with a context_payload and assert schema validation fails
/// as a deterministic failure (COMPLETED receipt with error, no runtime facts).
async fn assert_schema_rejection(
    pool: &PgPool,
    cmd: CreateWorkflowInstanceCommand,
    principal_id: Uuid,
) -> SchemaRejectionReceipt {
    let idem_key = cmd.idempotency_key.clone();
    let err = create_workflow_instance(pool, cmd).await;

    assert!(
        matches!(
            err,
            Err(CreateWorkflowInstanceError::ContextValidationFailed(_))
        ),
        "expected ContextValidationFailed, got {:?}",
        err
    );

    // Receipt EXISTS — deterministic failure is persisted as COMPLETED
    let receipt: (Uuid, String, i32, String) = sqlx::query_as(
        "SELECT command_id, receipt_status::TEXT, response_status, COALESCE(response_digest, '') \
         FROM workflow_command_receipts \
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(&idem_key)
    .fetch_one(pool)
    .await
    .expect("receipt must exist for deterministic schema failure");

    assert_eq!(
        receipt.1, "COMPLETED",
        "schema failure receipt must be COMPLETED"
    );
    assert_eq!(
        receipt.2, 422,
        "schema failure receipt must have 422 status"
    );
    assert!(
        !receipt.3.is_empty(),
        "schema failure receipt must have response_digest"
    );

    // No runtime facts created
    let instance_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_instances WHERE created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(instance_count, 0, "no instance after schema rejection");

    let ctx_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions cr \
         JOIN workflow_instances i ON i.workflow_instance_id = cr.workflow_instance_id \
         WHERE i.created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(ctx_count, 0, "no context revision after schema rejection");

    let visit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_node_visits nv \
         JOIN workflow_instances i ON i.workflow_instance_id = nv.workflow_instance_id \
         WHERE i.created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(visit_count, 0, "no visit after schema rejection");

    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_events e \
         JOIN workflow_instances i ON i.workflow_instance_id = e.workflow_instance_id \
         WHERE i.created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(event_count, 0, "no event after schema rejection");

    SchemaRejectionReceipt {
        command_id: receipt.0,
        response_digest: receipt.3,
    }
}

#[tokio::test]
async fn test_context_schema_valid_accepted() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) =
        seed_published_definition_with_schema(&pool, domain_id, &required_fields_schema()).await;

    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"title": "test", "priority": 1});

    let result = create_workflow_instance(&pool, cmd)
        .await
        .expect("valid schema context should succeed");
    verify_creation(&pool, &result, principal_id, domain_id, ver_id).await;
}

#[tokio::test]
async fn test_context_schema_required_field_missing() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) =
        seed_published_definition_with_schema(&pool, domain_id, &required_fields_schema()).await;

    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"priority": 1});

    assert_schema_rejection(&pool, cmd, principal_id).await;
}

#[tokio::test]
async fn test_context_schema_type_error_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) =
        seed_published_definition_with_schema(&pool, domain_id, &required_fields_schema()).await;

    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"title": "x", "priority": "high"});

    assert_schema_rejection(&pool, cmd, principal_id).await;
}

#[tokio::test]
async fn test_context_schema_additional_properties_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) =
        seed_published_definition_with_schema(&pool, domain_id, &required_fields_schema()).await;

    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"title": "x", "priority": 1, "extra": "oops"});

    assert_schema_rejection(&pool, cmd, principal_id).await;
}

#[tokio::test]
async fn test_context_schema_local_ref_accepted() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;

    // Schema with a local $ref using #/$defs
    let schema = serde_json::json!({
        "$defs": {
            "positiveInt": {
                "type": "integer",
                "minimum": 1
            }
        },
        "type": "object",
        "properties": {
            "count": {"$ref": "#/$defs/positiveInt"}
        },
        "additionalProperties": false
    });

    let (_d, ver_id) = seed_published_definition_with_schema(&pool, domain_id, &schema).await;

    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"count": 5});

    let result = create_workflow_instance(&pool, cmd)
        .await
        .expect("local $ref context should succeed");
    verify_creation(&pool, &result, principal_id, domain_id, ver_id).await;
}

#[tokio::test]
async fn test_context_schema_failure_replays_completed_error_receipt() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) =
        seed_published_definition_with_schema(&pool, domain_id, &required_fields_schema()).await;

    // First call — should fail with COMPLETED receipt
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd1 = make_command(principal_id, domain_id, ver_id);
    cmd1.idempotency_key = idempotency_key.clone();
    cmd1.context_payload = serde_json::json!({"priority": 1}); // missing "title"

    let receipt = assert_schema_rejection(&pool, cmd1.clone(), principal_id).await;

    // Second call with same idempotency key — must return the same persisted error
    // without re-running creation logic
    let err2 = create_workflow_instance(&pool, cmd1).await;

    assert!(
        matches!(
            err2,
            Err(CreateWorkflowInstanceError::ContextValidationFailed(_))
        ),
        "replay must return same error, got {:?}",
        err2
    );

    // Exactly one receipt — replay did not create a duplicate
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idempotency_key).fetch_one(&pool).await.expect("count");
    assert_eq!(receipt_count, 1, "replay must not create a second receipt");

    // Receipt command_id and response_digest unchanged
    let receipt2: (Uuid, String, i32, String) = sqlx::query_as(
        "SELECT command_id, receipt_status::TEXT, response_status, COALESCE(response_digest, '') \
         FROM workflow_command_receipts \
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(&idempotency_key)
    .fetch_one(&pool)
    .await
    .expect("receipt");
    assert_eq!(receipt2.0, receipt.command_id, "command_id must not change");
    assert_eq!(receipt2.1, "COMPLETED");
    assert_eq!(receipt2.2, 422);
    assert_eq!(
        receipt2.3, receipt.response_digest,
        "response_digest must not change"
    );

    // Still no runtime facts
    let instance_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_instances WHERE created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(instance_count, 0, "no instance after schema failure replay");
}
