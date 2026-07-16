use super::*;

/// Same key/hash replay returns same result.
#[tokio::test]
async fn test_transition_same_key_hash_replay() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let idempotency_key = Uuid::new_v4().to_string();

    let cmd1 = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: idempotency_key.clone(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 2,
        transition_definition_id: TransitionId::from_uuid(normal_adv),
        submission_payload: None,
    };

    let r1 = execute_workflow_transition(&pool, cmd1.clone())
        .await
        .unwrap();
    assert_eq!(r1.workflow_state_version, 3);

    // Replay with same command
    let r2 = execute_workflow_transition(&pool, cmd1).await.unwrap();
    assert_eq!(r2.workflow_state_version, 3);
    assert_eq!(r2.current_node_visit_id, r1.current_node_visit_id);
}

/// Replay does not increase state version.
#[tokio::test]
async fn test_transition_replay_no_state_version_increase() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let key = Uuid::new_v4().to_string();
    let cmd = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: key,
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 2,
        transition_definition_id: TransitionId::from_uuid(normal_adv),
        submission_payload: None,
    };

    let r1 = execute_workflow_transition(&pool, cmd.clone())
        .await
        .unwrap();
    let r2 = execute_workflow_transition(&pool, cmd).await.unwrap();
    assert_eq!(r1.workflow_state_version, r2.workflow_state_version);
}

/// Same key, different payload → IdempotencyConflict.
#[tokio::test]
async fn test_transition_same_key_different_payload_conflict() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let key = Uuid::new_v4().to_string();

    let cmd1 = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: key.clone(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 2,
        transition_definition_id: TransitionId::from_uuid(normal_adv),
        submission_payload: None,
    };

    // First succeeds
    execute_workflow_transition(&pool, cmd1).await.unwrap();

    // Different payload with same key
    let cmd2 = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: key,
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 2,
        transition_definition_id: TransitionId::from_uuid(normal_adv),
        submission_payload: Some(serde_json::json!({"different": "payload"})),
    };

    let err = execute_workflow_transition(&pool, cmd2).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::IdempotencyConflict { .. }
    ));
}

/// Conflict writes an attempt audit.
#[tokio::test]
async fn test_transition_conflict_writes_attempt_audit() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let key = Uuid::new_v4().to_string();

    let cmd1 = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: key.clone(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 2,
        transition_definition_id: TransitionId::from_uuid(normal_adv),
        submission_payload: None,
    };

    execute_workflow_transition(&pool, cmd1).await.unwrap();

    let cmd2 = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: key,
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 2,
        transition_definition_id: TransitionId::from_uuid(normal_adv),
        submission_payload: Some(serde_json::json!({"diff": "payload"})),
    };

    let idempotency_key2 = cmd2.idempotency_key.clone();
    let _ = execute_workflow_transition(&pool, cmd2).await;

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_attempt_audits WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idempotency_key2).fetch_one(&pool).await.unwrap();
    assert_eq!(audit_count, 1);
}

/// expectedVersion correct succeeds.
#[tokio::test]
async fn test_transition_expected_version_correct() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv, None);
    let result = execute_workflow_transition(&pool, cmd).await;
    assert!(result.is_ok());
}

/// expectedVersion too old → conflict.
#[tokio::test]
async fn test_transition_expected_version_too_old_conflict() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    // State is 2, expect 1
    let cmd = make_transition_command(principal_id, instance_id, 1, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::WorkflowStateVersionConflict {
            expected: 1,
            actual: 2,
        }
    ));
}

/// expectedVersion too new → conflict.
#[tokio::test]
async fn test_transition_expected_version_too_new_conflict() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    // State is 2, expect 3
    let cmd = make_transition_command(principal_id, instance_id, 3, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::WorkflowStateVersionConflict {
            expected: 3,
            actual: 2,
        }
    ));
}
