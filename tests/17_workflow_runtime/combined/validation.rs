use super::*;

async fn basic_instance(pool: &PgPool) -> (Uuid, Uuid, Uuid, CreateWorkflowInstanceResult) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (version_id, _, _, advance_id, secondary_id) = seed_combined_graph(
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
    (principal_id, advance_id, secondary_id, created)
}

#[tokio::test]
async fn combined_requires_workflow_creator() {
    let pool = create_pool().await;
    let (_, advance_id, _, created) = basic_instance(&pool).await;
    let other_principal = seed_second_principal(&pool).await;
    let error = revise_context_and_transition(
        &pool,
        make_combined_command(other_principal, created.workflow_instance_id, 1, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::PrincipalNotCreator
    ));
}

#[tokio::test]
async fn combined_requires_current_visit_assignee_independently() {
    let pool = create_pool().await;
    let (creator_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let fixed_assignee = seed_second_principal(&pool).await;
    let (version_id, _, _, advance_id, _) = seed_combined_graph(
        &pool,
        domain_id,
        "FIXED_PRINCIPAL",
        "WORKFLOW_CREATOR",
        Some(fixed_assignee),
        None,
        None,
    )
    .await;
    let created = create_combined_instance(
        &pool,
        creator_id,
        domain_id,
        version_id,
        serde_json::json!({}),
    )
    .await;
    let error = revise_context_and_transition(
        &pool,
        make_combined_command(creator_id, created.workflow_instance_id, 1, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::PrincipalNotAssignee
    ));
}

#[tokio::test]
async fn combined_is_draft_only() {
    let pool = create_pool().await;
    let (principal_id, advance_id, _, created) = basic_instance(&pool).await;
    execute_workflow_transition(
        &pool,
        make_transition_command(
            principal_id,
            created.workflow_instance_id,
            1,
            advance_id,
            None,
        ),
    )
    .await
    .unwrap();

    let error = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, created.workflow_instance_id, 2, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::CurrentNodeNotDraft
    ));
}

#[tokio::test]
async fn combined_accepts_only_primary_advance() {
    let pool = create_pool().await;
    let (principal_id, _, secondary_id, created) = basic_instance(&pool).await;
    let error = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, created.workflow_instance_id, 1, secondary_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::TransitionNotApplicable(_)
    ));
}

#[tokio::test]
async fn combined_validates_both_payload_schemas_without_partial_facts() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let context_schema = serde_json::json!({
        "type": "object",
        "required": ["title"],
        "properties": {"title": {"type": "string"}}
    });
    let submission_schema = serde_json::json!({
        "type": "object",
        "required": ["summary"],
        "properties": {"summary": {"type": "string"}}
    });
    let (version_id, _, _, advance_id, _) = seed_combined_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
        Some(&context_schema),
        Some(&submission_schema),
    )
    .await;

    for (context, submission, expected_context_error) in [
        (
            serde_json::json!({"title": 1}),
            serde_json::json!({"summary": "ok"}),
            true,
        ),
        (
            serde_json::json!({"title": "ok"}),
            serde_json::json!({"summary": 1}),
            false,
        ),
    ] {
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
        command.context_payload = context;
        command.submission_payload = submission;
        let error = revise_context_and_transition(&pool, command)
            .await
            .unwrap_err();
        if expected_context_error {
            assert!(matches!(
                error,
                ReviseContextAndTransitionError::ContextValidationFailed(_)
            ));
        } else {
            assert!(matches!(
                error,
                ReviseContextAndTransitionError::SubmissionValidationFailed(_)
            ));
        }
        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT \
              (SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1), \
              (SELECT COUNT(*) FROM workflow_submissions WHERE workflow_instance_id = $1), \
              (SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1)",
        )
        .bind(created.workflow_instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(counts, (1, 0, 1));
    }
}

#[tokio::test]
async fn combined_rejects_stale_state_version_and_revoked_definition() {
    let pool = create_pool().await;
    let (principal_id, advance_id, _, created) = basic_instance(&pool).await;
    let stale = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, created.workflow_instance_id, 2, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        stale,
        ReviseContextAndTransitionError::WorkflowStateVersionConflict {
            expected: 2,
            actual: 1
        }
    ));

    let version_id: Uuid = sqlx::query_scalar(
        "SELECT definition_version_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(created.workflow_instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'REVOKED' \
         WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .execute(&pool)
    .await
    .unwrap();
    let revoked = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, created.workflow_instance_id, 1, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        revoked,
        ReviseContextAndTransitionError::DefinitionVersionRevoked
    ));
}
