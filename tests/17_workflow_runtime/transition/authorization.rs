use super::*;

/// Current assignee succeeds.
#[tokio::test]
async fn test_transition_authorization_current_assignee_succeeds() {
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

/// Non-assignee principal (different principal) is rejected.
#[tokio::test]
async fn test_transition_non_assignee_rejected() {
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

    let other_id = seed_second_principal(&pool).await;

    let cmd = make_transition_command(other_id, instance_id, 2, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::PrincipalNotAssignee
    ));
}

/// Creator but not assignee is rejected.
#[tokio::test]
async fn test_transition_creator_not_assignee_rejected() {
    let pool = create_pool().await;
    let (creator_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let other_id = seed_second_principal(&pool).await;

    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "FIXED_PRINCIPAL",
        Some(other_id),
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, creator_id, domain_id, draft_adv, ver_id).await;

    let cmd = make_transition_command(creator_id, instance_id, 2, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::PrincipalNotAssignee
    ));
}

/// Domain Owner but not assignee is rejected.
#[tokio::test]
async fn test_transition_domain_owner_not_assignee_rejected() {
    let pool = create_pool().await;
    let (owner_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let other_id = seed_second_principal(&pool).await;

    let (_, ver_id, _, _, _, draft_adv, normal_adv, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "FIXED_PRINCIPAL",
        Some(other_id),
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, owner_id, domain_id, draft_adv, ver_id).await;

    let cmd = make_transition_command(owner_id, instance_id, 2, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::PrincipalNotAssignee
    ));
}

/// Disabled assignee is rejected.
#[tokio::test]
async fn test_transition_disabled_assignee_rejected() {
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

    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(principal_id)
        .execute(&pool)
        .await
        .unwrap();

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::PrincipalDisabled
    ));
}

/// Source node is TERMINAL → rejected.
#[tokio::test]
async fn test_transition_source_node_terminal_rejected() {
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

    // Advance to terminal
    let cmd1 = make_transition_command(principal_id, instance_id, 2, normal_adv, None);
    execute_workflow_transition(&pool, cmd1).await.unwrap();

    // Try to transition from terminal
    let cmd2 = make_transition_command(principal_id, instance_id, 3, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd2).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::SourceNodeTerminal
    ));
}
