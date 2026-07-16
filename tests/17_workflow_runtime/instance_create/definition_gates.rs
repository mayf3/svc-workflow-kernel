//! Definition version gate tests (9-14).

use super::*;

#[tokio::test]
async fn test_draft_version_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let def_id = Uuid::new_v4();
    let ver_id = Uuid::new_v4();
    let def_key = format!("draft-{}", &Uuid::new_v4().to_string()[..8]);
    sqlx::query("INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1, $2, $3, 'DraftDef')")
        .bind(def_id).bind(domain_id).bind(&def_key).execute(&pool).await.expect("def");
    sqlx::query("INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status) VALUES ($1, $2, 1, 'DRAFT')")
        .bind(ver_id).bind(def_id).execute(&pool).await.expect("ver");
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::VersionNotPublished
    ));
}

#[tokio::test]
async fn test_deprecated_version_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'DEPRECATED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(&pool).await.expect("deprecate");
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::VersionNotPublished
    ));
}

#[tokio::test]
async fn test_revoked_version_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'REVOKED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(&pool).await.expect("revoke");
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::VersionNotPublished
    ));
}

#[tokio::test]
async fn test_cross_domain_version_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let other_domain_id = Uuid::new_v4();
    let other_domain_key = format!("other-{}", &Uuid::new_v4().to_string()[..8]);
    sqlx::query("INSERT INTO domains (domain_id, domain_key, display_name, enabled) VALUES ($1, $2, 'Other', TRUE)")
        .bind(other_domain_id).bind(&other_domain_key).execute(&pool).await.expect("other domain");
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, other_domain_id).await;
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::CrossDomainViolation
    ));
}

#[tokio::test]
async fn test_disabled_domain_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    sqlx::query("UPDATE domains SET enabled = FALSE WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .expect("disable");
    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(err, CreateWorkflowInstanceError::DomainDisabled));
}

#[tokio::test]
async fn test_no_draft_node_defensive_failure() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    // Create a published definition with NO DRAFT node (only TERMINAL)
    let def_id = Uuid::new_v4();
    let ver_id = Uuid::new_v4();
    let def_key = format!("test-{}", &Uuid::new_v4().to_string()[..8]);
    sqlx::query("INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1, $2, $3, 'NoDraft')")
        .bind(def_id).bind(domain_id).bind(&def_key).execute(&pool).await.expect("def");
    sqlx::query("INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status) VALUES ($1, $2, 1, 'DRAFT')")
        .bind(ver_id).bind(def_id).execute(&pool).await.expect("ver");
    let term_id = Uuid::new_v4();
    sqlx::query("INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'done', 'Done', 0, 'TERMINAL', NULL)")
        .bind(term_id).bind(ver_id).execute(&pool).await.expect("terminal");
    // Only one node, non-DRAFT — no DRAFT node exists
    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'PUBLISHED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(&pool).await.expect("publish");

    let err = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CreateWorkflowInstanceError::InternalConsistency(_)
    ));
}
