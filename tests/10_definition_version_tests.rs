#![allow(clippy::needless_borrow)]
//! Test: Definition Version lifecycle constraints.
//!
//! Illegal status transitions and field immutability after PUBLISHED.

mod common;

#[tokio::test]
async fn test_published_cannot_revert_to_draft() {
    let pool = common::create_pool().await;
    let (_, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, ver_id, _, _) = common::seed_workflow_definition(&pool, domain_id).await;

    // Update version to PUBLISHED
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED', published_at = now() WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await
    .expect("update to PUBLISHED should succeed");

    // Try to revert to DRAFT
    let result = sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'DRAFT' WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_definition_version_status_transition")
                    || err_str.contains("illegal"),
                "expected rejection of PUBLISHED -> DRAFT, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected rejection of PUBLISHED -> DRAFT transition"),
    }
}

#[tokio::test]
async fn test_deprecated_cannot_revert_to_published() {
    let pool = common::create_pool().await;
    let (_, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, ver_id, _, _) = common::seed_workflow_definition(&pool, domain_id).await;

    // Go to PUBLISHED first
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED', published_at = now() WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await
    .expect("update to PUBLISHED");

    // Then DEPRECATED
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'DEPRECATED', deprecated_at = now() WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await
    .expect("update to DEPRECATED");

    // Try to go back to PUBLISHED
    let result = sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED' WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_definition_version_status_transition")
                    || err_str.contains("illegal"),
                "expected rejection of DEPRECATED -> PUBLISHED, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected rejection of DEPRECATED -> PUBLISHED transition"),
    }
}

#[tokio::test]
async fn test_revoked_cannot_change_to_anything() {
    let pool = common::create_pool().await;
    let (_, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, ver_id, _, _) = common::seed_workflow_definition(&pool, domain_id).await;

    // PUBLISHED -> REVOKED
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED', published_at = now() WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await
    .expect("update to PUBLISHED");

    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'REVOKED', revoked_at = now() WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await
    .expect("update to REVOKED");

    // Try to change from REVOKED
    let result = sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED' WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_definition_version_status_transition")
                    || err_str.contains("illegal"),
                "expected rejection of REVOKED -> anything, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected rejection of REVOKED -> PUBLISHED transition"),
    }
}

#[tokio::test]
async fn test_published_version_business_fields_immutable() {
    let pool = common::create_pool().await;
    let (_, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, ver_id, _, _) = common::seed_workflow_definition(&pool, domain_id).await;

    // PUBLISH the version
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED', published_at = now() WHERE definition_version_id = $1"
    )
    .bind(ver_id)
    .execute(&pool)
    .await
    .expect("update to PUBLISHED");

    // Try to modify context_schema
    let result = sqlx::query(
        r#"UPDATE workflow_definition_versions SET context_schema = '{"type":"array"}'::jsonb WHERE definition_version_id = $1"#
    )
    .bind(ver_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_definition_version_immutable")
                    || err_str.contains("immutable"),
                "expected trigger rejection of context_schema change on PUBLISHED version, got: {}",
                err_str
            );
        }
        Ok(_) => {
            panic!("expected trigger rejection of context_schema change on PUBLISHED version")
        }
    }
}

#[tokio::test]
async fn test_draft_version_allows_modification() {
    let pool = common::create_pool().await;
    let (_, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, ver_id, _, _) = common::seed_workflow_definition(&pool, domain_id).await;

    // DRAFT version should allow business field modification
    let result = sqlx::query(
        r#"UPDATE workflow_definition_versions SET context_schema = '{"type":"array"}'::jsonb WHERE definition_version_id = $1"#
    )
    .bind(ver_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_ok(),
        "DRAFT version should allow field modification"
    );
}
