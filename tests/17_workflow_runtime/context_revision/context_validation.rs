//! Context schema and size validation tests for ReviseWorkflowContext.

use super::*;

fn schema() -> serde_json::Value {
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

async fn seeded_instance_with_schema(pool: &PgPool) -> (Uuid, Uuid) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (_d, ver_id) = seed_published_definition_with_schema(pool, domain_id, &schema()).await;
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.context_payload = serde_json::json!({"title": "initial", "priority": 0});
    let r = create_workflow_instance(pool, cmd).await.expect("create");
    (principal_id, r.workflow_instance_id)
}

async fn assert_revise_schema_rejection(
    pool: &PgPool,
    principal_id: Uuid,
    instance_id: Uuid,
    payload: serde_json::Value,
) {
    let err = revise_workflow_context(
        pool,
        make_revise_command(principal_id, instance_id, 1, payload),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, ReviseWorkflowContextError::ContextValidationFailed(_)),
        "expected ContextValidationFailed, got {:?}",
        err
    );
    let rev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(rev_count, 1, "no new revision after schema rejection");
}

#[tokio::test]
async fn test_revise_no_schema_any_json_accepted() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            r.workflow_instance_id,
            1,
            serde_json::json!({"anything": "goes"}),
        ),
    )
    .await
    .expect("any JSON should be accepted when no schema");
}

#[tokio::test]
async fn test_revise_schema_valid_accepted() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance_with_schema(&pool).await;
    revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            instance_id,
            1,
            serde_json::json!({"title": "test", "priority": 1}),
        ),
    )
    .await
    .expect("valid schema context should succeed");
}

#[tokio::test]
async fn test_revise_schema_required_field_missing() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance_with_schema(&pool).await;
    assert_revise_schema_rejection(
        &pool,
        principal_id,
        instance_id,
        serde_json::json!({"priority": 1}),
    )
    .await;
}

#[tokio::test]
async fn test_revise_schema_type_error() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance_with_schema(&pool).await;
    assert_revise_schema_rejection(
        &pool,
        principal_id,
        instance_id,
        serde_json::json!({"title": "x", "priority": "high"}),
    )
    .await;
}

#[tokio::test]
async fn test_revise_schema_additional_properties() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance_with_schema(&pool).await;
    assert_revise_schema_rejection(
        &pool,
        principal_id,
        instance_id,
        serde_json::json!({"title": "x", "priority": 1, "extra": "oops"}),
    )
    .await;
}

#[tokio::test]
async fn test_revise_payload_too_large() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance_with_schema(&pool).await;
    let big_str = "x".repeat(1024 * 1024 + 1);
    let err = revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            instance_id,
            1,
            serde_json::json!({"data": big_str}),
        ),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        &err,
        ReviseWorkflowContextError::SizeLimitExceeded(_)
    ));
}

#[tokio::test]
async fn test_revise_schema_failure_replays() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance_with_schema(&pool).await;
    let payload = serde_json::json!({"priority": 1});
    let cmd1 = make_revise_command(principal_id, instance_id, 1, payload.clone());
    let idem_key = cmd1.idempotency_key.clone();
    let err1 = revise_workflow_context(&pool, cmd1).await.unwrap_err();
    assert!(matches!(
        &err1,
        ReviseWorkflowContextError::ContextValidationFailed(_)
    ));
    let cmd2 = make_revise_command(principal_id, instance_id, 1, payload);
    // Use the same idempotency key
    let cmd2 = ReviseWorkflowContextCommand {
        idempotency_key: idem_key.clone(),
        ..cmd2
    };
    let err2 = revise_workflow_context(&pool, cmd2).await.unwrap_err();
    assert!(
        matches!(
            &err2,
            ReviseWorkflowContextError::ContextValidationFailed(_)
        ),
        "replay must return same error"
    );
    let rev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rev_count, 1, "no new revision after schema failure replay");
}
