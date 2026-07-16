//! Idempotency tests (25-34, 10 tests).

use super::*;

#[tokio::test]
async fn test_same_key_same_request_returns_same_instance() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.idempotency_key = idempotency_key.clone();
    let r1 = create_workflow_instance(&pool, cmd.clone())
        .await
        .expect("first");
    let r2 = create_workflow_instance(&pool, cmd).await.expect("second");
    assert_eq!(r1.workflow_instance_id, r2.workflow_instance_id);
    assert_eq!(
        r1.current_context_revision_id,
        r2.current_context_revision_id
    );
    assert_eq!(r1.current_node_visit_id, r2.current_node_visit_id);
}

#[tokio::test]
async fn test_replay_does_not_create_second_event() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.idempotency_key = idempotency_key;
    let r1 = create_workflow_instance(&pool, cmd.clone())
        .await
        .expect("first");
    let r2 = create_workflow_instance(&pool, cmd).await.expect("second");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1")
            .bind(r1.workflow_instance_id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 1);
    assert_eq!(r1.workflow_instance_id, r2.workflow_instance_id);
}

#[tokio::test]
async fn test_different_request_same_key_conflict() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd1 = make_command(principal_id, domain_id, ver_id);
    cmd1.idempotency_key = idempotency_key.clone();
    cmd1.context_payload = serde_json::json!({"v": 1});
    let _r1 = create_workflow_instance(&pool, cmd1).await.expect("first");
    let mut cmd2 = make_command(principal_id, domain_id, ver_id);
    cmd2.idempotency_key = idempotency_key.clone();
    cmd2.context_payload = serde_json::json!({"v": 2});
    let err = create_workflow_instance(&pool, cmd2).await.unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::IdempotencyConflict { .. }
    ));
}

#[tokio::test]
async fn test_conflict_writes_attempt_audit() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd1 = make_command(principal_id, domain_id, ver_id);
    cmd1.idempotency_key = idempotency_key.clone();
    cmd1.context_payload = serde_json::json!({"v": 1});
    let _ = create_workflow_instance(&pool, cmd1).await.expect("first");
    let mut cmd2 = make_command(principal_id, domain_id, ver_id);
    cmd2.idempotency_key = idempotency_key.clone();
    cmd2.context_payload = serde_json::json!({"v": 2});
    let _ = create_workflow_instance(&pool, cmd2).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_attempt_audits WHERE idempotency_key = $1",
    )
    .bind(&idempotency_key)
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_conflict_does_not_modify_original_receipt() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd1 = make_command(principal_id, domain_id, ver_id);
    cmd1.idempotency_key = idempotency_key.clone();
    cmd1.context_payload = serde_json::json!({"v": 1});
    let _r1 = create_workflow_instance(&pool, cmd1).await.expect("first");
    let mut cmd2 = make_command(principal_id, domain_id, ver_id);
    cmd2.idempotency_key = idempotency_key.clone();
    cmd2.context_payload = serde_json::json!({"v": 2});
    let _ = create_workflow_instance(&pool, cmd2).await;
    let (resp_status,): (i32,) = sqlx::query_as(
        "SELECT response_status FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idempotency_key).fetch_one(&pool).await.expect("receipt");
    assert_eq!(resp_status, 200);
}

#[tokio::test]
async fn test_different_principal_same_key_allowed() {
    let pool = create_pool().await;
    let (principal_a, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (principal_b, _) = seed_principal_and_domain(&pool).await;
    let binding_id = Uuid::new_v4();
    sqlx::query("INSERT INTO domain_role_bindings (binding_id, domain_id, principal_id, role_key, enabled) VALUES ($1, $2, $3, 'MEMBER', TRUE)")
        .bind(binding_id).bind(domain_id).bind(principal_b).execute(&pool).await.expect("binding");
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd1 = make_command(principal_a, domain_id, ver_id);
    cmd1.idempotency_key = idempotency_key.clone();
    let r1 = create_workflow_instance(&pool, cmd1)
        .await
        .expect("principal_a");
    let mut cmd2 = make_command(principal_b, domain_id, ver_id);
    cmd2.idempotency_key = idempotency_key;
    let r2 = create_workflow_instance(&pool, cmd2)
        .await
        .expect("principal_b");
    assert_ne!(r1.workflow_instance_id, r2.workflow_instance_id);
}

#[tokio::test]
async fn test_deterministic_failure_replayable() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    sqlx::query("UPDATE domains SET enabled = FALSE WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .expect("disable");
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.idempotency_key = idempotency_key.clone();
    let err1 = create_workflow_instance(&pool, cmd.clone())
        .await
        .unwrap_err();
    assert!(matches!(err1, CreateWorkflowInstanceError::DomainDisabled));
    let err2 = create_workflow_instance(&pool, cmd).await.unwrap_err();
    assert!(matches!(err2, CreateWorkflowInstanceError::DomainDisabled));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idempotency_key).fetch_one(&pool).await.expect("count");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_concurrent_same_idempotent_request() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd1 = make_command(principal_id, domain_id, ver_id);
    cmd1.idempotency_key = idempotency_key.clone();
    let mut cmd2 = make_command(principal_id, domain_id, ver_id);
    cmd2.idempotency_key = idempotency_key.clone();
    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { create_workflow_instance(&pool1, cmd1).await }),
        tokio::spawn(async move { create_workflow_instance(&pool2, cmd2).await }),
    );
    let r1 = r1.expect("join");
    let r2 = r2.expect("join");
    match (&r1, &r2) {
        (Ok(a), Ok(b)) => assert_eq!(a.workflow_instance_id, b.workflow_instance_id),
        (Ok(result), Err(CreateWorkflowInstanceError::CommandStillProcessing))
        | (Err(CreateWorkflowInstanceError::CommandStillProcessing), Ok(result)) => {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let pool3 = pool.clone();
            let mut retry = make_command(principal_id, domain_id, ver_id);
            retry.idempotency_key = idempotency_key;
            let r = create_workflow_instance(&pool3, retry)
                .await
                .expect("retry");
            assert_eq!(r.workflow_instance_id, result.workflow_instance_id);
        }
        _ => {
            if r1.is_err() && r2.is_err() {
                panic!("both failed: {:?}, {:?}", r1, r2);
            }
        }
    }
}

#[tokio::test]
async fn test_concurrent_different_request_hash() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();
    let mut cmd_a = make_command(principal_id, domain_id, ver_id);
    cmd_a.idempotency_key = idempotency_key.clone();
    cmd_a.context_payload = serde_json::json!({"variant": "A"});
    let mut cmd_b = make_command(principal_id, domain_id, ver_id);
    cmd_b.idempotency_key = idempotency_key.clone();
    cmd_b.context_payload = serde_json::json!({"variant": "B"});

    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { create_workflow_instance(&pool1, cmd_a).await }),
        tokio::spawn(async move { create_workflow_instance(&pool2, cmd_b).await }),
    );
    let r1 = r1.expect("join");
    let r2 = r2.expect("join");

    // One succeeds (creates instance), one gets IdempotencyConflict
    // The success might be either call depending on runtime scheduling
    match (&r1, &r2) {
        (Ok(_), Err(CreateWorkflowInstanceError::IdempotencyConflict { .. }))
        | (Err(CreateWorkflowInstanceError::IdempotencyConflict { .. }), Ok(_)) => {
            // Expected: one succeeds, one conflicts
        }
        (
            Err(CreateWorkflowInstanceError::CommandStillProcessing),
            Err(CreateWorkflowInstanceError::IdempotencyConflict { .. }),
        )
        | (
            Err(CreateWorkflowInstanceError::IdempotencyConflict { .. }),
            Err(CreateWorkflowInstanceError::CommandStillProcessing),
        ) => {
            // One got conflict, other still processing — wait and retry
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let pool3 = pool.clone();
            let mut retry = make_command(principal_id, domain_id, ver_id);
            retry.idempotency_key = idempotency_key;
            let _ = create_workflow_instance(&pool3, retry).await;
        }
        (
            Err(CreateWorkflowInstanceError::CommandStillProcessing),
            Err(CreateWorkflowInstanceError::CommandStillProcessing),
        ) => {
            // Both still processing — wait and check that one succeeded after
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let pool3 = pool.clone();
            let mut retry = make_command(principal_id, domain_id, ver_id);
            retry.idempotency_key = idempotency_key;
            let retry_result = create_workflow_instance(&pool3, retry).await;
            // One of A or B should have succeeded
            assert!(
                retry_result.is_ok()
                    || matches!(
                        retry_result,
                        Err(CreateWorkflowInstanceError::IdempotencyConflict { .. })
                    ),
                "retry should resolve to success or conflict: {:?}",
                retry_result
            );
        }
        _ => {
            panic!("unexpected concurrent result: r1={:?}, r2={:?}", r1, r2);
        }
    }

    // Exactly one instance
    let instance_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_instances WHERE created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(instance_count, 1, "exactly one instance must exist");
}

#[tokio::test]
async fn test_processing_receipt_not_taken_over() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let idempotency_key = Uuid::new_v4().to_string();

    // Manually insert a PROCESSING receipt to simulate an in-flight request
    let cmd_id = Uuid::new_v4();
    let request_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    sqlx::query(
        "INSERT INTO workflow_command_receipts (command_id, principal_id, idempotency_key, command_type, request_hash, receipt_status) VALUES ($1, $2, $3, 'CREATE_WORKFLOW_INSTANCE', $4, 'PROCESSING')",
    )
    .bind(cmd_id).bind(principal_id).bind(&idempotency_key).bind(request_hash)
    .execute(&pool).await.expect("insert processing receipt");

    // Second request with same key should get CommandStillProcessing
    let mut cmd = make_command(principal_id, domain_id, ver_id);
    cmd.idempotency_key = idempotency_key;
    let err = create_workflow_instance(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::CommandStillProcessing
    ));

    // Verify the original PROCESSING receipt is untouched
    let (status,): (String,) = sqlx::query_as(
        "SELECT receipt_status::TEXT FROM workflow_command_receipts WHERE command_id = $1",
    )
    .bind(cmd_id)
    .fetch_one(&pool)
    .await
    .expect("receipt");
    assert_eq!(
        status, "PROCESSING",
        "original receipt must remain PROCESSING"
    );
}
