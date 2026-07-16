use super::*;

fn size_detail(
    result: Result<CreateWorkflowInstanceResult, CreateWorkflowInstanceError>,
) -> String {
    match result.unwrap_err() {
        CreateWorkflowInstanceError::SizeLimitExceeded(detail) => detail,
        error => panic!("expected size failure, got {error:?}"),
    }
}

fn validation_detail(
    result: Result<CreateWorkflowInstanceResult, CreateWorkflowInstanceError>,
) -> String {
    match result.unwrap_err() {
        CreateWorkflowInstanceError::ContextValidationFailed(detail) => detail,
        error => panic!("expected context failure, got {error:?}"),
    }
}

fn assignee_detail(
    result: Result<CreateWorkflowInstanceResult, CreateWorkflowInstanceError>,
) -> String {
    match result.unwrap_err() {
        CreateWorkflowInstanceError::AssigneeResolutionFailed(detail) => detail,
        error => panic!("expected assignee failure, got {error:?}"),
    }
}

#[tokio::test]
async fn missing_principal_is_an_identity_failure_without_a_receipt() {
    let pool = create_pool().await;
    let (_, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let missing = Uuid::new_v4();
    let command = make_command(missing, domain_id, version_id);
    let key = command.idempotency_key.clone();

    assert!(matches!(
        create_workflow_instance(&pool, command).await,
        Err(CreateWorkflowInstanceError::PrincipalNotFound)
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
async fn metadata_size_failure_replays_the_exact_persisted_detail() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let mut command = make_command(principal_id, domain_id, version_id);
    command.metadata = serde_json::json!({"data": "x".repeat(64 * 1024 + 1)});
    let key = command.idempotency_key.clone();

    let first_detail = size_detail(create_workflow_instance(&pool, command.clone()).await);
    let first_receipt = receipt(&pool, principal_id, &key).await;
    let replay_detail = size_detail(create_workflow_instance(&pool, command).await);
    let replay_receipt = receipt(&pool, principal_id, &key).await;

    assert_eq!(first_detail, "metadata exceeds 64 KiB");
    assert_eq!(replay_detail, first_detail);
    assert_eq!(first_receipt, replay_receipt);
    assert_eq!(first_receipt.0, "COMPLETED");
    assert_eq!(first_receipt.1, 413);
    assert_eq!(first_receipt.2["detail"], first_detail);
}

#[tokio::test]
async fn context_schema_failure_replays_the_exact_validation_detail() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let schema = serde_json::json!({
        "type": "object", "required": ["title"],
        "properties": {"title": {"type": "string"}}
    });
    let (_, version_id) = seed_published_definition_with_schema(&pool, domain_id, &schema).await;
    let mut command = make_command(principal_id, domain_id, version_id);
    command.context_payload = serde_json::json!({});
    let key = command.idempotency_key.clone();

    let first_detail = validation_detail(create_workflow_instance(&pool, command.clone()).await);
    let first_receipt = receipt(&pool, principal_id, &key).await;
    let replay_detail = validation_detail(create_workflow_instance(&pool, command).await);

    assert_eq!(replay_detail, first_detail);
    assert_eq!(first_receipt.2["detail"], first_detail);
}

#[tokio::test]
async fn assignee_failure_survives_target_reenable_with_exact_detail() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let target_id = seed_second_principal(&pool).await;
    let (_, version_id) =
        seed_published_definition_fixed_principal(&pool, domain_id, target_id).await;
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(target_id)
        .execute(&pool)
        .await
        .unwrap();
    let command = make_command(principal_id, domain_id, version_id);
    let key = command.idempotency_key.clone();

    let first_detail = assignee_detail(create_workflow_instance(&pool, command.clone()).await);
    let first_receipt = receipt(&pool, principal_id, &key).await;
    sqlx::query("UPDATE principals SET enabled = TRUE WHERE principal_id = $1")
        .bind(target_id)
        .execute(&pool)
        .await
        .unwrap();
    let replay_detail = assignee_detail(create_workflow_instance(&pool, command).await);

    assert_eq!(replay_detail, first_detail);
    assert_eq!(first_receipt.1, 422);
    assert_eq!(first_receipt.2["detail"], first_detail);
}

#[tokio::test]
async fn disabled_principal_failure_survives_reenable() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(principal_id)
        .execute(&pool)
        .await
        .unwrap();
    let command = make_command(principal_id, domain_id, version_id);
    let key = command.idempotency_key.clone();
    assert!(matches!(
        create_workflow_instance(&pool, command.clone()).await,
        Err(CreateWorkflowInstanceError::PrincipalDisabled)
    ));
    sqlx::query("UPDATE principals SET enabled = TRUE WHERE principal_id = $1")
        .bind(principal_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        create_workflow_instance(&pool, command).await,
        Err(CreateWorkflowInstanceError::PrincipalDisabled)
    ));
    let stored = receipt(&pool, principal_id, &key).await;
    assert_eq!(stored.0, "COMPLETED");
    assert_eq!(stored.1, 403);
}

#[tokio::test]
async fn disabled_domain_failure_survives_reenable() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    sqlx::query("UPDATE domains SET enabled = FALSE WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .unwrap();
    let command = make_command(principal_id, domain_id, version_id);
    let key = command.idempotency_key.clone();
    assert!(matches!(
        create_workflow_instance(&pool, command.clone()).await,
        Err(CreateWorkflowInstanceError::DomainDisabled)
    ));
    sqlx::query("UPDATE domains SET enabled = TRUE WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        create_workflow_instance(&pool, command).await,
        Err(CreateWorkflowInstanceError::DomainDisabled)
    ));
    let stored = receipt(&pool, principal_id, &key).await;
    assert_eq!(stored.1, 403);
    assert_eq!(stored.2["error"], "domain_disabled");
}

#[tokio::test]
async fn membership_failure_survives_a_later_binding() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_and_domain(&pool).await;
    let (_, version_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let command = make_command(principal_id, domain_id, version_id);
    let key = command.idempotency_key.clone();
    assert!(matches!(
        create_workflow_instance(&pool, command.clone()).await,
        Err(CreateWorkflowInstanceError::DomainMembershipRequired)
    ));
    seed_domain_owner(&pool, domain_id, principal_id).await;
    assert!(matches!(
        create_workflow_instance(&pool, command).await,
        Err(CreateWorkflowInstanceError::DomainMembershipRequired)
    ));
    assert_eq!(receipt(&pool, principal_id, &key).await.1, 403);
}
