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
async fn concurrent_same_key_combined_commands_return_same_result() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let command = make_combined_command(principal_id, instance_id, 1, advance_id);
    let other_pool = create_pool().await;
    let other_command = command.clone();
    let first = tokio::spawn(async move { revise_context_and_transition(&pool, command).await });
    let second =
        tokio::spawn(
            async move { revise_context_and_transition(&other_pool, other_command).await },
        );
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap().unwrap();
    let second = second.unwrap().unwrap();
    assert_eq!(
        first.current_context_revision_id,
        second.current_context_revision_id
    );
    assert_eq!(first.current_node_visit_id, second.current_node_visit_id);
    assert_eq!(first.submission_id, second.submission_id);
}

#[tokio::test]
async fn concurrent_different_key_combined_commands_linearize_on_instance() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let first_command = make_combined_command(principal_id, instance_id, 1, advance_id);
    let second_command = make_combined_command(principal_id, instance_id, 1, advance_id);
    let other_pool = create_pool().await;
    let first =
        tokio::spawn(async move { revise_context_and_transition(&pool, first_command).await });
    let second =
        tokio::spawn(
            async move { revise_context_and_transition(&other_pool, second_command).await },
        );
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.is_ok(), second.is_ok());
    let loser = first.err().or_else(|| second.err()).unwrap();
    assert!(matches!(
        loser,
        ReviseContextAndTransitionError::WorkflowStateVersionConflict {
            expected: 1,
            actual: 2
        }
    ));
}

#[tokio::test]
async fn combined_and_context_revision_cannot_both_commit_same_version() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let combined = make_combined_command(principal_id, instance_id, 1, advance_id);
    let revision = make_revise_command(
        principal_id,
        instance_id,
        1,
        serde_json::json!({"title": "revision-only"}),
    );
    let other_pool = create_pool().await;
    let combined_handle =
        tokio::spawn(async move { revise_context_and_transition(&pool, combined).await });
    let revision_handle =
        tokio::spawn(async move { revise_workflow_context(&other_pool, revision).await });
    let (combined, revision) = tokio::join!(combined_handle, revision_handle);
    assert_ne!(combined.unwrap().is_ok(), revision.unwrap().is_ok());

    let pool = create_pool().await;
    let state_version: i32 = sqlx::query_scalar(
        "SELECT workflow_state_version FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state_version, 2);
}

#[tokio::test]
async fn combined_and_transition_cannot_both_commit_same_version() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let combined = make_combined_command(principal_id, instance_id, 1, advance_id);
    let transition = make_transition_command(principal_id, instance_id, 1, advance_id, None);
    let other_pool = create_pool().await;
    let combined_handle =
        tokio::spawn(async move { revise_context_and_transition(&pool, combined).await });
    let transition_handle =
        tokio::spawn(async move { execute_workflow_transition(&other_pool, transition).await });
    let (combined, transition) = tokio::join!(combined_handle, transition_handle);
    assert_ne!(combined.unwrap().is_ok(), transition.unwrap().is_ok());

    let pool = create_pool().await;
    let state_version: i32 = sqlx::query_scalar(
        "SELECT workflow_state_version FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state_version, 2);
}
