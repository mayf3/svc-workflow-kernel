use super::*;

/// Same key/hash concurrent → both return same result.
#[tokio::test]
async fn test_transition_concurrent_same_key_hash() {
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

    let pool2 = create_pool().await;
    let cmd2 = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: key,
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 2,
        transition_definition_id: TransitionId::from_uuid(normal_adv),
        submission_payload: None,
    };

    let h1 = tokio::spawn(async move { execute_workflow_transition(&pool, cmd1).await });
    let h2 = tokio::spawn(async move { execute_workflow_transition(&pool2, cmd2).await });

    let (r1, r2) = tokio::join!(h1, h2);
    let r1 = r1.unwrap().unwrap();
    let r2 = r2.unwrap().unwrap();

    assert_eq!(r1.workflow_state_version, r2.workflow_state_version);
    assert_eq!(r1.current_node_visit_id, r2.current_node_visit_id);
}

/// Different key, same expectedVersion → one succeeds, one conflicts.
#[tokio::test]
async fn test_transition_concurrent_different_key_same_version() {
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

    let pool2 = create_pool().await;

    let c1 = make_transition_command(principal_id, instance_id, 2, normal_adv, None);
    let c2 = make_transition_command(principal_id, instance_id, 2, normal_adv, None);

    let h1 = tokio::spawn(async move { execute_workflow_transition(&pool, c1).await });
    let h2 = tokio::spawn(async move { execute_workflow_transition(&pool2, c2).await });

    let (r1, r2) = tokio::join!(h1, h2);
    let r1 = r1.unwrap();
    let r2 = r2.unwrap();

    // One must succeed, one must conflict
    match (&r1, &r2) {
        (Ok(_), Ok(_)) => panic!("both succeeded"),
        (Err(_), Err(_)) => panic!("both failed"),
        _ => {}
    }
}
