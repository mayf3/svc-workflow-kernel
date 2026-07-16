//! Success path tests for ReviseWorkflowContext.

use super::*;

async fn create_instance(pool: &PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(pool, domain_id).await;
    let result = create_workflow_instance(pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create instance");
    (
        principal_id,
        result.workflow_instance_id,
        result.current_context_revision_id,
        result.current_node_visit_id,
    )
}

#[tokio::test]
async fn test_revise_context_by_creator_succeeds() {
    let pool = create_pool().await;
    let (principal_id, instance_id, prev_rev_id, visit_id) = create_instance(&pool).await;
    let result = revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            instance_id,
            1,
            serde_json::json!({"title": "updated", "priority": 2}),
        ),
    )
    .await
    .expect("revise should succeed");
    verify_revision(&pool, &result, instance_id, 1, 2, prev_rev_id, visit_id).await;
}

#[tokio::test]
async fn test_revise_revision2_previous_points_to_revision1() {
    let pool = create_pool().await;
    let (principal_id, instance_id, prev_rev_id, _visit_id) = create_instance(&pool).await;
    let r1 = revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            instance_id,
            1,
            serde_json::json!({"title": "v2"}),
        ),
    )
    .await
    .expect("first revise");
    let row: (i32, Option<Uuid>) = sqlx::query_as(
        "SELECT revision_number, previous_revision_id FROM workflow_context_revisions WHERE context_revision_id = $1",
    ).bind(r1.current_context_revision_id).fetch_one(&pool).await.expect("ctx");
    assert_eq!(row.0, 2);
    assert_eq!(row.1, Some(prev_rev_id));
}

#[tokio::test]
async fn test_revise_revision3_after_revision2() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _rev1, _visit_id) = create_instance(&pool).await;
    let r1 = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .expect("revise to v2");
    let r2 = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 2, serde_json::json!({"v": 3})),
    )
    .await
    .expect("revise to v3");
    assert_eq!(r2.workflow_state_version, 3);
    assert_eq!(r2.event_sequence, 3);
    let row: (i32, Option<Uuid>) = sqlx::query_as(
        "SELECT revision_number, previous_revision_id FROM workflow_context_revisions WHERE context_revision_id = $1",
    ).bind(r2.current_context_revision_id).fetch_one(&pool).await.expect("ctx");
    assert_eq!(row.0, 3);
    assert_eq!(row.1, Some(r1.current_context_revision_id));
}

#[tokio::test]
async fn test_revise_current_node_visit_unchanged() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _rev1, visit_id) = create_instance(&pool).await;
    let result = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .expect("revise");
    assert_eq!(result.current_node_visit_id, visit_id);
    let inst: Uuid = sqlx::query_scalar(
        "SELECT current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("inst");
    assert_eq!(inst, visit_id);
}

#[tokio::test]
async fn test_revise_payload_digest_readback() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _rev1, _visit_id) = create_instance(&pool).await;
    let payload = serde_json::json!({"title": "digest test", "nested": {"a": 1}});
    let result = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, payload.clone()),
    )
    .await
    .expect("revise");
    let stored_digest: String = sqlx::query_scalar(
        "SELECT payload_digest FROM workflow_context_revisions WHERE context_revision_id = $1",
    )
    .bind(result.current_context_revision_id)
    .fetch_one(&pool)
    .await
    .expect("digest");
    let expected =
        svc_workflow::domain::definition::digest::compute_json_digest(&payload).expect("compute");
    assert_eq!(stored_digest, expected);
}

#[tokio::test]
async fn test_revise_event_data_digest_readback() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _prev_rev_id, _visit_id) = create_instance(&pool).await;
    let _result = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .expect("revise");
    let event_data_json: serde_json::Value = sqlx::query_scalar(
        "SELECT event_data FROM workflow_events WHERE workflow_instance_id = $1 AND event_type = 'CONTEXT_REVISED'",
    ).bind(instance_id).fetch_one(&pool).await.expect("event_data");
    let event_data_digest: String = sqlx::query_scalar(
        "SELECT event_data_digest FROM workflow_events WHERE workflow_instance_id = $1 AND event_type = 'CONTEXT_REVISED'",
    ).bind(instance_id).fetch_one(&pool).await.expect("digest");
    let expected = svc_workflow::domain::definition::digest::compute_json_digest(&event_data_json)
        .expect("compute");
    assert_eq!(event_data_digest, expected);
}

#[tokio::test]
async fn test_revise_response_digest_readback() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _rev1, _visit_id) = create_instance(&pool).await;
    let cmd = make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2}));
    let idem_key = cmd.idempotency_key.clone();
    let result = revise_workflow_context(&pool, cmd).await.expect("revise");
    let stored_digest: String = sqlx::query_scalar(
        "SELECT response_digest FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idem_key).fetch_one(&pool).await.expect("digest");
    let expected_response = serde_json::json!({
        "workflowInstanceId": instance_id,
        "workflowStateVersion": result.workflow_state_version,
        "currentContextRevisionId": result.current_context_revision_id,
        "currentNodeVisitId": result.current_node_visit_id,
        "eventSequence": result.event_sequence,
    });
    let expected =
        svc_workflow::domain::definition::digest::compute_json_digest(&expected_response)
            .expect("compute");
    assert_eq!(stored_digest, expected);
}

#[tokio::test]
async fn test_revise_exactly_one_event() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _rev1, _visit_id) = create_instance(&pool).await;
    revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .expect("revise");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1 AND event_type = 'CONTEXT_REVISED'",
    ).bind(instance_id).fetch_one(&pool).await.expect("count");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_revise_event_submission_null() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _rev1, _visit_id) = create_instance(&pool).await;
    revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .expect("revise");
    let submission_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT submission_id FROM workflow_events WHERE workflow_instance_id = $1 AND event_type = 'CONTEXT_REVISED'",
    ).bind(instance_id).fetch_one(&pool).await.expect("ev");
    assert!(submission_id.is_none(), "submission must be NULL");
}

#[tokio::test]
async fn test_revise_consecutive_event_sequence() {
    let pool = create_pool().await;
    let (principal_id, instance_id, _rev1, _visit_id) = create_instance(&pool).await;
    let r1 = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .expect("revise 1");
    assert_eq!(r1.event_sequence, 2);
    let r2 = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 2, serde_json::json!({"v": 3})),
    )
    .await
    .expect("revise 2");
    assert_eq!(r2.event_sequence, 3);
}
