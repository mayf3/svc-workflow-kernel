//! Concurrency and state version conflict tests for ReviseWorkflowContext.

use super::*;

#[tokio::test]
async fn test_revise_expected_version_correct_succeeds() {
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
            serde_json::json!({"v": 2}),
        ),
    )
    .await
    .expect("expected version 1 correct");
}

#[tokio::test]
async fn test_revise_stale_version_conflict() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    // First revise succeeds with version 1
    revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            r.workflow_instance_id,
            1,
            serde_json::json!({"v": 2}),
        ),
    )
    .await
    .expect("first revise");
    // Second revise with stale version 1 should conflict (current is 2)
    let err = revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            r.workflow_instance_id,
            1,
            serde_json::json!({"v": 3}),
        ),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            &err,
            ReviseWorkflowContextError::WorkflowStateVersionConflict {
                expected: 1,
                actual: 2
            }
        ),
        "expected version conflict (1 vs 2), got {:?}",
        err
    );
}

#[tokio::test]
async fn test_revise_conflict_no_revision_created() {
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
            serde_json::json!({"v": 2}),
        ),
    )
    .await
    .expect("first revise");
    let _ = revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            r.workflow_instance_id,
            1,
            serde_json::json!({"v": 3}),
        ),
    )
    .await;
    let rev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1",
    )
    .bind(r.workflow_instance_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rev_count, 2, "only 2 revisions (original + first revise)");
}

#[tokio::test]
async fn test_revise_two_different_keys_same_version_one_succeeds() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let cmd_a = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": "A"}),
    );
    let cmd_b = make_revise_command(
        principal_id,
        r.workflow_instance_id,
        1,
        serde_json::json!({"v": "B"}),
    );
    let pool1 = pool.clone();
    let pool2 = pool.clone();
    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { revise_workflow_context(&pool1, cmd_a).await }),
        tokio::spawn(async move { revise_workflow_context(&pool2, cmd_b).await }),
    );
    let r1 = r1.expect("join");
    let r2 = r2.expect("join");
    match (&r1, &r2) {
        (Ok(_), Err(ReviseWorkflowContextError::WorkflowStateVersionConflict { .. }))
        | (Err(ReviseWorkflowContextError::WorkflowStateVersionConflict { .. }), Ok(_)) => {}
        (Ok(a), Ok(b)) => {
            panic!(
                "both succeeded: one should have conflicted. a={:?}, b={:?}",
                a, b
            );
        }
        _ => {
            panic!("unexpected results: r1={:?}, r2={:?}", r1, r2);
        }
    }
    let rev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1",
    )
    .bind(r.workflow_instance_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rev_count, 2, "only 2 revisions (original + one revise)");
    let ev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1 AND event_type = 'CONTEXT_REVISED'"
    ).bind(r.workflow_instance_id).fetch_one(&pool).await.expect("count");
    assert_eq!(ev_count, 1, "exactly one CONTEXT_REVISED event");
}
