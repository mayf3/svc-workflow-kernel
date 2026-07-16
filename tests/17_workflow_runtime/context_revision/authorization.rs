//! Authorization and node-type gate tests for ReviseWorkflowContext.

use super::*;

async fn seeded_instance(pool: &PgPool) -> (Uuid, Uuid) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(pool, domain_id).await;
    let r = create_workflow_instance(pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    (principal_id, r.workflow_instance_id)
}

#[tokio::test]
async fn test_revise_non_creator_rejected() {
    let pool = create_pool().await;
    let (creator_id, instance_id) = seeded_instance(&pool).await;
    let other_id = seed_second_principal(&pool).await;
    let err = revise_workflow_context(
        &pool,
        make_revise_command(other_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ReviseWorkflowContextError::PrincipalNotFound));
    let _ = creator_id;
}

#[tokio::test]
async fn test_revise_disabled_creator_rejected() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance(&pool).await;
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(principal_id)
        .execute(&pool)
        .await
        .expect("disable");
    let err = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ReviseWorkflowContextError::PrincipalDisabled));
}

#[tokio::test]
async fn test_revise_normal_node_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id, normal_node_id) =
        seed_published_definition_normal_node(&pool, domain_id).await;
    let r = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let normal_visit_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_visits (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id) VALUES ($1, $2, $3, 1, $4)"
    ).bind(normal_visit_id).bind(r.workflow_instance_id).bind(normal_node_id)
        .bind(principal_id).execute(&pool).await.expect("insert normal visit");
    sqlx::query(
        "UPDATE workflow_instances SET current_node_visit_id = $1 WHERE workflow_instance_id = $2",
    )
    .bind(normal_visit_id)
    .bind(r.workflow_instance_id)
    .execute(&pool)
    .await
    .expect("update instance");
    let err = revise_workflow_context(
        &pool,
        make_revise_command(
            principal_id,
            r.workflow_instance_id,
            1,
            serde_json::json!({"v": 2}),
        ),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        ReviseWorkflowContextError::CurrentNodeNotDraft
    ));
}

#[tokio::test]
async fn test_revise_deprecated_version_allowed() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance(&pool).await;
    let ver_id: Uuid = sqlx::query_scalar(
        "SELECT definition_version_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("def ver");
    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'DEPRECATED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(&pool).await.expect("deprecate");
    revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .expect("revise under DEPRECATED must succeed");
}

#[tokio::test]
async fn test_revise_revoked_version_rejected() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance(&pool).await;
    let ver_id: Uuid = sqlx::query_scalar(
        "SELECT definition_version_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("def ver");
    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'REVOKED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(&pool).await.expect("revoke");
    let err = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        ReviseWorkflowContextError::DefinitionVersionRevoked
    ));
}
