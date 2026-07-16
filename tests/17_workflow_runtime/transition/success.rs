use super::*;

/// DRAFT → NORMAL primary ADVANCE succeeds.
#[tokio::test]
async fn test_transition_draft_to_normal_advance() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    // Indexes: 0=domain,1=ver,2=draft,3=normal,4=term,5=draft_adv,6=normal_adv,7=ret,8=term_trans
    let (_, ver_id, _, normal_id, _, draft_adv_id, _, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let create_cmd = make_command(principal_id, domain_id, ver_id);
    let create_result = create_workflow_instance(&pool, create_cmd).await.unwrap();

    let cmd = make_transition_command(
        principal_id,
        create_result.workflow_instance_id,
        1,
        draft_adv_id,
        None,
    );
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.workflow_state_version, 2);
    assert_eq!(result.event_sequence, 2);
    assert_ne!(
        result.current_node_visit_id,
        create_result.current_node_visit_id
    );
    assert_eq!(result.submission_id, None);
    assert_eq!(
        result.current_context_revision_id,
        create_result.current_context_revision_id
    );

    // Verify instance projection
    let inst: (i32, Uuid, Uuid) = sqlx::query_as(
        "SELECT workflow_state_version, current_context_revision_id, current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(result.workflow_instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(inst.0, 2);
    assert_eq!(inst.1, result.current_context_revision_id);
    assert_eq!(inst.2, result.current_node_visit_id);

    // Verify new visit targets normal node
    let visit: (Uuid, i32, Uuid) = sqlx::query_as(
        "SELECT node_id, visit_number, assignee_principal_id FROM workflow_node_visits WHERE node_visit_id = $1",
    ).bind(result.current_node_visit_id).fetch_one(&pool).await.unwrap();
    assert_eq!(visit.0, normal_id);
    assert_eq!(visit.1, 1); // first visit to normal
    assert_eq!(visit.2, principal_id);

    // Verify exactly one transition event
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1 AND event_type = 'WORKFLOW_TRANSITION_COMMITTED'",
    ).bind(result.workflow_instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
}

/// NORMAL → TERMINAL primary ADVANCE succeeds (normal completion).
#[tokio::test]
async fn test_transition_normal_to_terminal_advance() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, term_id, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    // First advance DRAFT→NORMAL
    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    // Then advance NORMAL→TERMINAL using normal_adv_id
    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, None);
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.workflow_state_version, 3);

    let visit_node: (Uuid,) =
        sqlx::query_as("SELECT node_id FROM workflow_node_visits WHERE node_visit_id = $1")
            .bind(result.current_node_visit_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(visit_node.0, term_id);
}

/// RETURN to earlier non-terminal node succeeds.
#[tokio::test]
async fn test_transition_return_succeeds() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, draft_id, _, _, draft_adv_id, _, ret_id, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    // Get the current visit for RETURN root cause
    let current_visit: (Uuid,) = sqlx::query_as(
        "SELECT current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let return_payload = serde_json::json!({
        "rootCauseNodeVisitId": current_visit.0.to_string(),
        "relatedSubmissionIds": [],
        "reasonCode": "NEEDS_REVISION",
        "reason": "Need more details",
    });

    let cmd = make_transition_command(principal_id, instance_id, 2, ret_id, Some(return_payload));
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.workflow_state_version, 3);

    let visit_node: (Uuid, i32) = sqlx::query_as(
        "SELECT node_id, visit_number FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(result.current_node_visit_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(visit_node.0, draft_id);
    assert_eq!(visit_node.1, 2); // second visit to draft
    assert!(result.submission_id.is_some());
}

/// TERMINATE to TERMINAL node succeeds.
#[tokio::test]
async fn test_transition_terminate_succeeds() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, term_id, draft_adv_id, _, _, term_trans_id) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let payload = serde_json::json!({
        "reasonCode": "DUPLICATE",
        "reason": "This instance is a duplicate",
    });

    let cmd = make_transition_command(principal_id, instance_id, 2, term_trans_id, Some(payload));
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.workflow_state_version, 3);

    let visit_node: (Uuid,) =
        sqlx::query_as("SELECT node_id FROM workflow_node_visits WHERE node_visit_id = $1")
            .bind(result.current_node_visit_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(visit_node.0, term_id);
}

/// Transition with no submission (schema is NULL) succeeds.
#[tokio::test]
async fn test_transition_no_submission_advance() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    // Use normal_adv_id (NORMAL→TERMINAL) which has no submission_schema
    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, None);
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.workflow_state_version, 3);
    assert_eq!(result.submission_id, None);
}

/// Transition with submission (schema is NULL) creates submission.
#[tokio::test]
async fn test_transition_with_submission_null_schema() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let payload = serde_json::json!({"result": "completed"});
    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, Some(payload));
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.workflow_state_version, 3);
    assert!(result.submission_id.is_some());
}

/// stateVersion +1 and eventSequence = new stateVersion.
#[tokio::test]
async fn test_transition_state_version_and_event_sequence() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, None);
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.workflow_state_version, 3);
    assert_eq!(result.event_sequence, 3);
}

/// current Context Revision is unchanged.
#[tokio::test]
async fn test_transition_context_revision_unchanged() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let ctx_before: (Uuid,) = sqlx::query_as(
        "SELECT current_context_revision_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, None);
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.current_context_revision_id, ctx_before.0);
}

/// Event source/target visit fields are correct.
#[tokio::test]
async fn test_transition_event_source_target() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, source_visit_id) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, None);
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    assert_eq!(result.source_node_visit_id, source_visit_id);

    let ev: (Uuid, Uuid) = sqlx::query_as(
        "SELECT source_node_visit_id, target_node_visit_id FROM workflow_events \
         WHERE workflow_instance_id = $1 AND event_type = 'WORKFLOW_TRANSITION_COMMITTED' ORDER BY event_sequence DESC",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(ev.0, source_visit_id);
    assert_eq!(ev.1, result.current_node_visit_id);
}

/// command_id matches event.
#[tokio::test]
async fn test_transition_command_id_matches_event() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, None);
    let _ = execute_workflow_transition(&pool, cmd).await.unwrap();

    let receipt: (Uuid,) = sqlx::query_as(
        "SELECT command_id FROM workflow_command_receipts \
         WHERE principal_id = $1 AND response_status = 200 \
         AND command_type = 'EXECUTE_WORKFLOW_TRANSITION' \
         ORDER BY completed_at DESC LIMIT 1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let ev_cmd: (Uuid,) = sqlx::query_as(
        "SELECT command_id FROM workflow_events \
         WHERE workflow_instance_id = $1 AND event_type = 'WORKFLOW_TRANSITION_COMMITTED' ORDER BY event_sequence DESC",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(ev_cmd.0, receipt.0);
}

/// Digest verification for submission payload.
#[tokio::test]
async fn test_transition_submission_digest_readback() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let payload = serde_json::json!({"result": "test_complete"});
    let cmd = make_transition_command(
        principal_id,
        instance_id,
        2,
        normal_adv_id,
        Some(payload.clone()),
    );
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();

    let sub_digest: (String,) =
        sqlx::query_as("SELECT payload_digest FROM workflow_submissions WHERE submission_id = $1")
            .bind(result.submission_id.unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();

    let expected_digest =
        svc_workflow::domain::definition::digest::compute_json_digest(&payload).unwrap();
    assert_eq!(sub_digest.0, expected_digest);
}

/// Exactly one event for a transition command.
#[tokio::test]
async fn test_transition_exactly_one_event() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv_id, normal_adv_id, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv_id, ver_id).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv_id, None);
    let _ = execute_workflow_transition(&pool, cmd).await.unwrap();

    // Should be 3 total events: creation + draft→normal + normal→terminal
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1")
            .bind(instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 3);
}
