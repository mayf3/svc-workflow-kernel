//! Idempotency tests for ReviseWorkflowContext.

use super::*;

#[tokio::test]
async fn test_revise_same_key_hash_replays_same_revision() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let idem_key = Uuid::new_v4().to_string();
    let mut cmd = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": 2}),
    );
    cmd.idempotency_key = idem_key.clone();
    let r1 = revise_workflow_context(&pool, cmd.clone())
        .await
        .expect("first");
    let r2 = revise_workflow_context(&pool, cmd).await.expect("replay");
    assert_eq!(
        r1.current_context_revision_id,
        r2.current_context_revision_id
    );
}

#[tokio::test]
async fn test_revise_replay_does_not_increase_state_version() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let idem_key = Uuid::new_v4().to_string();
    let mut cmd = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": 2}),
    );
    cmd.idempotency_key = idem_key.clone();
    let r1 = revise_workflow_context(&pool, cmd.clone())
        .await
        .expect("first");
    assert_eq!(r1.workflow_state_version, 2);
    let r2 = revise_workflow_context(&pool, cmd).await.expect("replay");
    assert_eq!(
        r2.workflow_state_version, 2,
        "replay must not increase state version"
    );
    let sv: i32 = sqlx::query_scalar(
        "SELECT workflow_state_version FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(r.workflow_instance_id)
    .fetch_one(&pool)
    .await
    .expect("sv");
    assert_eq!(sv, 2, "instance state version must remain 2");
}

#[tokio::test]
async fn test_revise_same_key_different_payload_conflict() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let idem_key = Uuid::new_v4().to_string();
    let mut cmd_a = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": "A"}),
    );
    cmd_a.idempotency_key = idem_key.clone();
    let _ = revise_workflow_context(&pool, cmd_a).await.expect("first");
    let mut cmd_b = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": "B"}),
    );
    cmd_b.idempotency_key = idem_key;
    let err = revise_workflow_context(&pool, cmd_b).await.unwrap_err();
    assert!(matches!(
        &err,
        ReviseWorkflowContextError::IdempotencyConflict { .. }
    ));
}

#[tokio::test]
async fn test_revise_conflict_writes_attempt_audit() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let idem_key = Uuid::new_v4().to_string();
    let mut cmd_a = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": "A"}),
    );
    cmd_a.idempotency_key = idem_key.clone();
    let _ = revise_workflow_context(&pool, cmd_a).await.expect("first");
    let mut cmd_b = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": "B"}),
    );
    cmd_b.idempotency_key = idem_key.clone();
    let _ = revise_workflow_context(&pool, cmd_b).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_attempt_audits WHERE idempotency_key = $1",
    )
    .bind(&idem_key)
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(count, 1);
}
