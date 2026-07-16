//! Principal and authorization tests (15-19).

use super::*;

#[tokio::test]
async fn test_disabled_principal_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(principal_id)
        .execute(&pool)
        .await
        .expect("disable");
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::PrincipalDisabled
    ));
}

#[tokio::test]
async fn test_no_domain_membership_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_and_domain(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::DomainMembershipRequired
    ));
}

#[tokio::test]
async fn test_cross_domain_principal_rejected() {
    let pool = create_pool().await;
    let (principal_a, _domain_a) = seed_principal_domain_with_owner(&pool).await;
    let (_principal_b, domain_b) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_b).await;
    let err = create_workflow_instance(&pool, make_command(principal_a, domain_b, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::DomainMembershipRequired
    ));
}

#[tokio::test]
async fn test_disabled_domain_owner_assignee_rejected() {
    let pool = create_pool().await;
    let (owner_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (caller_id, _) = seed_principal_and_domain(&pool).await;
    let binding_id = Uuid::new_v4();
    sqlx::query("INSERT INTO domain_role_bindings (binding_id, domain_id, principal_id, role_key, enabled) VALUES ($1, $2, $3, 'MEMBER', TRUE)")
        .bind(binding_id).bind(domain_id).bind(caller_id).execute(&pool).await.expect("binding");
    let (_d, ver_id) = seed_published_definition_domain_owner(&pool, domain_id).await;
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(owner_id)
        .execute(&pool)
        .await
        .expect("disable owner");
    let err = create_workflow_instance(&pool, make_command(caller_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::AssigneeResolutionFailed(_)
    ));
}

#[tokio::test]
async fn test_disabled_fixed_principal_assignee_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let fixed_id = seed_second_principal(&pool).await;
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(fixed_id)
        .execute(&pool)
        .await
        .expect("disable");
    let (_d, ver_id) = seed_published_definition_fixed_principal(&pool, domain_id, fixed_id).await;
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::AssigneeResolutionFailed(_)
    ));
}
