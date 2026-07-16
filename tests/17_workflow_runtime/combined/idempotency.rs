use super::*;

async fn setup(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (version_id, _, _, advance_id, _) = seed_combined_graph(
        pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
        None,
        None,
    )
    .await;
    let created = create_combined_instance(
        pool,
        principal_id,
        domain_id,
        version_id,
        serde_json::json!({}),
    )
    .await;
    (principal_id, advance_id, created.workflow_instance_id)
}

#[tokio::test]
async fn combined_same_key_and_hash_replays_stored_result() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let command = make_combined_command(principal_id, instance_id, 1, advance_id);
    let first = revise_context_and_transition(&pool, command.clone())
        .await
        .unwrap();
    let replay = revise_context_and_transition(&pool, command).await.unwrap();

    assert_eq!(first.workflow_state_version, replay.workflow_state_version);
    assert_eq!(
        first.current_context_revision_id,
        replay.current_context_revision_id
    );
    assert_eq!(first.current_node_visit_id, replay.current_node_visit_id);
    assert_eq!(first.submission_id, replay.submission_id);
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
          (SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1), \
          (SELECT COUNT(*) FROM workflow_submissions WHERE workflow_instance_id = $1), \
          (SELECT COUNT(*) FROM workflow_node_visits WHERE workflow_instance_id = $1), \
          (SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1)",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (2, 1, 2, 2));
}

#[tokio::test]
async fn combined_same_key_different_hash_writes_attempt_audit() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let first_command = make_combined_command(principal_id, instance_id, 1, advance_id);
    let mut conflicting_command = first_command.clone();
    conflicting_command.context_payload = serde_json::json!({"title": "different"});

    revise_context_and_transition(&pool, first_command.clone())
        .await
        .unwrap();
    let error = revise_context_and_transition(&pool, conflicting_command)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::IdempotencyConflict { .. }
    ));

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_attempt_audits \
         WHERE principal_id = $1 AND idempotency_key = $2 \
           AND attempt_type = 'IDEMPOTENCY_CONFLICT'",
    )
    .bind(principal_id)
    .bind(first_command.idempotency_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
}

#[tokio::test]
async fn combined_deterministic_failure_is_stably_replayed() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let schema = serde_json::json!({
        "type": "object",
        "required": ["title"],
        "properties": {"title": {"type": "string"}}
    });
    let (version_id, _, _, advance_id, _) = seed_combined_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
        Some(&schema),
        None,
    )
    .await;
    let created = create_combined_instance(
        &pool,
        principal_id,
        domain_id,
        version_id,
        serde_json::json!({"title": "initial"}),
    )
    .await;
    let mut command =
        make_combined_command(principal_id, created.workflow_instance_id, 1, advance_id);
    command.context_payload = serde_json::json!({"title": 1});

    for _ in 0..2 {
        let error = revise_context_and_transition(&pool, command.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ReviseContextAndTransitionError::ContextValidationFailed(_)
        ));
    }
    let receipt: (String, i32, i64) = sqlx::query_as(
        "SELECT receipt_status::TEXT, response_status, \
                (SELECT COUNT(*) FROM workflow_command_receipts r2 \
                 WHERE r2.principal_id = $1 AND r2.idempotency_key = $2) \
         FROM workflow_command_receipts \
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(command.idempotency_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt, ("COMPLETED".to_string(), 422, 1));
}

#[tokio::test]
async fn revise_context_receipt_uses_its_own_command_type() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let created =
        create_workflow_instance(&pool, make_command(principal_id, domain_id, version_id))
            .await
            .unwrap();
    let command = make_revise_command(
        principal_id,
        created.workflow_instance_id,
        1,
        serde_json::json!({"title": "revised"}),
    );
    let idempotency_key = command.idempotency_key.clone();
    revise_workflow_context(&pool, command).await.unwrap();
    let command_type: String = sqlx::query_scalar(
        "SELECT command_type FROM workflow_command_receipts \
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(idempotency_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(command_type, "REVISE_WORKFLOW_CONTEXT");
}
