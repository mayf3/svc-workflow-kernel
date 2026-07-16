use super::*;

/// Schema non-null but payload is None → SubmissionRequired.
#[tokio::test]
async fn test_transition_submission_required() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, _, ret_id, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    // RETURN has a submission_schema, but we provide None
    let cmd = make_transition_command(principal_id, instance_id, 2, ret_id, None);
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::SubmissionRequired
    ));
}

/// Schema NULL and payload None → no submission, succeeds.
#[tokio::test]
async fn test_transition_schema_null_no_payload_succeeds() {
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

    // NORMAL→TERMINAL has no submission_schema
    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv, None);
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();
    assert_eq!(result.submission_id, None);
}

/// Schema NULL and payload Some → creates submission (no schema validation).
#[tokio::test]
async fn test_transition_schema_null_with_payload_creates_submission() {
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

    let payload = serde_json::json!({"any": "data"});
    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv, Some(payload));
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();
    assert!(result.submission_id.is_some());
}

/// Schema validation: required field missing.
#[tokio::test]
async fn test_transition_submission_required_field_missing() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, _, _, term_trans_id) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    // Missing both required fields
    let payload = serde_json::json!({"extra": "field"});
    let cmd = make_transition_command(principal_id, instance_id, 2, term_trans_id, Some(payload));
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::SubmissionValidationFailed(_)
    ));
}

/// Schema validation: type error.
#[tokio::test]
async fn test_transition_submission_type_error() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, _, _, term_trans_id) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let payload = serde_json::json!({"reasonCode": 123, "reason": "test"});
    let cmd = make_transition_command(principal_id, instance_id, 2, term_trans_id, Some(payload));
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::SubmissionValidationFailed(_)
    ));
}

/// Schema validation: valid payload succeeds.
#[tokio::test]
async fn test_transition_submission_valid_schema() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, _, _, term_trans_id) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let payload = serde_json::json!({"reasonCode": "DUPLICATE", "reason": "This is a duplicate"});
    let cmd = make_transition_command(principal_id, instance_id, 2, term_trans_id, Some(payload));
    let result = execute_workflow_transition(&pool, cmd).await.unwrap();
    assert!(result.submission_id.is_some());
}

/// Payload size > 1 MiB is rejected.
#[tokio::test]
async fn test_transition_submission_size_exceeded() {
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

    let large_data = "x".repeat(1024 * 1024 + 1);
    let payload = serde_json::json!({"data": large_data});

    let cmd = make_transition_command(principal_id, instance_id, 2, normal_adv, Some(payload));
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::SizeLimitExceeded(_)
    ));
}

/// RETURN validation: rootCauseNodeVisitId must belong to same instance.
#[tokio::test]
async fn test_transition_return_root_cause_wrong_instance() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, _, ret_id, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _source_visit_id) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let fake_visit = Uuid::new_v4();
    let payload = serde_json::json!({
        "rootCauseNodeVisitId": fake_visit.to_string(),
        "relatedSubmissionIds": [],
        "reasonCode": "NEEDS_REVISION",
        "reason": "Need changes",
    });

    let cmd = make_transition_command(principal_id, instance_id, 2, ret_id, Some(payload));
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::InvalidReturnReferences(_)
    ));
}

/// RETURN validation: relatedSubmissionIds must belong to same instance.
#[tokio::test]
async fn test_transition_return_related_submission_wrong_instance() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, ver_id, _, _, _, draft_adv, _, ret_id, _) = seed_transition_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;

    let (_, instance_id, _source_visit_id) =
        create_and_advance_to_normal(&pool, principal_id, domain_id, draft_adv, ver_id).await;

    let fake_sub = Uuid::new_v4();
    let payload = serde_json::json!({
        "rootCauseNodeVisitId": _source_visit_id.to_string(),
        "relatedSubmissionIds": [fake_sub.to_string()],
        "reasonCode": "NEEDS_REVISION",
        "reason": "Need changes",
    });

    let cmd = make_transition_command(principal_id, instance_id, 2, ret_id, Some(payload));
    let err = execute_workflow_transition(&pool, cmd).await.unwrap_err();
    assert!(matches!(
        err,
        ExecuteWorkflowTransitionError::InvalidReturnReferences(_)
    ));
}
