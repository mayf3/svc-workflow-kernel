use super::*;

/// Version REVOKED rejects transition.
#[tokio::test]
async fn test_transition_revoked_version_rejected() {
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

    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'REVOKED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(&pool).await.unwrap();

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::DefinitionVersionRevoked
    ));
}

/// Version DEPRECATED allows transition.
#[tokio::test]
async fn test_transition_deprecated_version_allowed() {
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

    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'DEPRECATED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(&pool).await.unwrap();

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv, None);
    let result = execute_workflow_transition(&pool, cmd).await;
    assert!(result.is_ok());
}

/// Transition belongs to another version → rejected.
#[tokio::test]
async fn test_transition_wrong_version_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;

    let (_, ver1, _, _, _, draft1, _, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;
    let (_, _v2, _, _, _, adv2, _, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft1, ver1).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, adv2, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::TransitionNotApplicable(_)
    ));
}

/// Transition source doesn't match current node → rejected.
#[tokio::test]
async fn test_transition_wrong_source_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, _, _, ret_id, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    // Create instance at DRAFT and use RETURN (source=NORMAL) from DRAFT
    let create_cmd = make_command(principal_id, domain_id, ver_id);
    let create_result = create_workflow_instance(&pool, create_cmd).await.unwrap();

    let cmd = make_transition_command(
        principal_id,
        create_result.workflow_instance_id,
        1,
        ret_id,
        None,
    );
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::TransitionNotApplicable(_)
    ));
}
