use super::*;

fn size_detail(
    result: Result<
        svc_workflow::application::workflow_instance::execute_transition::ExecuteWorkflowTransitionResult,
        ExecuteWorkflowTransitionError,
    >,
) -> String {
    match result.unwrap_err() {
        ExecuteWorkflowTransitionError::SizeLimitExceeded(detail) => detail,
        error => panic!("expected size failure, got {error:?}"),
    }
}

fn invalid_reference_detail(
    result: Result<
        svc_workflow::application::workflow_instance::execute_transition::ExecuteWorkflowTransitionResult,
        ExecuteWorkflowTransitionError,
    >,
) -> String {
    match result.unwrap_err() {
        ExecuteWorkflowTransitionError::InvalidReturnReferences(detail) => detail,
        error => panic!("expected invalid references, got {error:?}"),
    }
}

fn assignee_detail(
    result: Result<
        svc_workflow::application::workflow_instance::execute_transition::ExecuteWorkflowTransitionResult,
        ExecuteWorkflowTransitionError,
    >,
) -> String {
    match result.unwrap_err() {
        ExecuteWorkflowTransitionError::AssigneeResolutionFailed(detail) => detail,
        error => panic!("expected assignee failure, got {error:?}"),
    }
}

#[tokio::test]
async fn missing_principal_is_an_identity_failure_without_a_receipt() {
    let pool = create_pool().await;
    let missing = Uuid::new_v4();
    let command = make_transition_command(missing, Uuid::new_v4(), 1, Uuid::new_v4(), None);
    let key = command.idempotency_key.clone();
    assert!(matches!(
        execute_workflow_transition(&pool, command).await,
        Err(ExecuteWorkflowTransitionError::PrincipalNotFound)
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts WHERE idempotency_key = $1",
    )
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn disabled_principal_failure_survives_reenable() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id, _, _, _, draft_advance, _, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;
    let created =
        create_workflow_instance(&pool, make_command(principal_id, domain_id, version_id))
            .await
            .unwrap();
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(principal_id)
        .execute(&pool)
        .await
        .unwrap();
    let command = make_transition_command(
        principal_id,
        created.workflow_instance_id,
        1,
        draft_advance,
        None,
    );
    let key = command.idempotency_key.clone();
    assert!(matches!(
        execute_workflow_transition(&pool, command.clone()).await,
        Err(ExecuteWorkflowTransitionError::PrincipalDisabled)
    ));
    sqlx::query("UPDATE principals SET enabled = TRUE WHERE principal_id = $1")
        .bind(principal_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        execute_workflow_transition(&pool, command).await,
        Err(ExecuteWorkflowTransitionError::PrincipalDisabled)
    ));
    assert_eq!(receipt(&pool, principal_id, &key).await.1, 403);
}

#[tokio::test]
async fn submission_size_failure_replays_the_exact_persisted_detail() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id, _, _, _, draft_advance, _, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;
    let created =
        create_workflow_instance(&pool, make_command(principal_id, domain_id, version_id))
            .await
            .unwrap();
    let command = make_transition_command(
        principal_id,
        created.workflow_instance_id,
        1,
        draft_advance,
        Some(serde_json::json!({"data": "x".repeat(1024 * 1024 + 1)})),
    );
    let key = command.idempotency_key.clone();

    let first_detail = size_detail(execute_workflow_transition(&pool, command.clone()).await);
    let first_receipt = receipt(&pool, principal_id, &key).await;
    let replay_detail = size_detail(execute_workflow_transition(&pool, command).await);
    let replay_receipt = receipt(&pool, principal_id, &key).await;

    assert_eq!(first_detail, "submission payload exceeds 1 MiB");
    assert_eq!(replay_detail, first_detail);
    assert_eq!(first_receipt, replay_receipt);
    assert_eq!(first_receipt.1, 413);
    assert_eq!(first_receipt.2["detail"], first_detail);
}

#[tokio::test]
async fn invalid_return_reference_replays_the_exact_detail() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id, _, _, _, draft_advance, _, return_id, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;
    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_advance, version_id)
            .await;
    let command = make_transition_command(
        principal_id,
        instance_id,
        2,
        return_id,
        Some(serde_json::json!({
            "rootCauseNodeVisitId": Uuid::new_v4(),
            "relatedSubmissionIds": [],
            "reasonCode": "REWORK", "reason": "retry"
        })),
    );
    let key = command.idempotency_key.clone();

    let first = invalid_reference_detail(execute_workflow_transition(&pool, command.clone()).await);
    let stored = receipt(&pool, principal_id, &key).await;
    let replay = invalid_reference_detail(execute_workflow_transition(&pool, command).await);
    assert_eq!(replay, first);
    assert_eq!(stored.2["detail"], first);
}

#[tokio::test]
async fn target_assignee_failure_survives_reenable_with_exact_detail() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let target_id = seed_second_principal(&pool).await;
    let (_, version_id, _, _, _, draft_advance, _, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "FIXED_PRINCIPAL",
        Some(target_id),
    )
    .await;
    let created =
        create_workflow_instance(&pool, make_command(principal_id, domain_id, version_id))
            .await
            .unwrap();
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(target_id)
        .execute(&pool)
        .await
        .unwrap();
    let command = make_transition_command(
        principal_id,
        created.workflow_instance_id,
        1,
        draft_advance,
        None,
    );
    let key = command.idempotency_key.clone();

    let first = assignee_detail(execute_workflow_transition(&pool, command.clone()).await);
    let stored = receipt(&pool, principal_id, &key).await;
    sqlx::query("UPDATE principals SET enabled = TRUE WHERE principal_id = $1")
        .bind(target_id)
        .execute(&pool)
        .await
        .unwrap();
    let replay = assignee_detail(execute_workflow_transition(&pool, command).await);
    assert_eq!(replay, first);
    assert_eq!(stored.2["detail"], first);
}

#[tokio::test]
async fn state_version_conflict_survives_the_version_becoming_current() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id, _, _, _, draft_advance, _, _, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;
    let created =
        create_workflow_instance(&pool, make_command(principal_id, domain_id, version_id))
            .await
            .unwrap();
    let command = make_transition_command(
        principal_id,
        created.workflow_instance_id,
        2,
        draft_advance,
        None,
    );
    let key = command.idempotency_key.clone();
    assert!(matches!(
        execute_workflow_transition(&pool, command.clone()).await,
        Err(
            ExecuteWorkflowTransitionError::WorkflowStateVersionConflict {
                expected: 2,
                actual: 1
            }
        )
    ));
    revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            created.workflow_instance_id,
            1,
            serde_json::json!({"revision": 2}),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        execute_workflow_transition(&pool, command).await,
        Err(
            ExecuteWorkflowTransitionError::WorkflowStateVersionConflict {
                expected: 2,
                actual: 1
            }
        )
    ));
    assert_eq!(receipt(&pool, principal_id, &key).await.2["actual"], 1);
}
