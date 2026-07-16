#![allow(clippy::needless_borrow)]
//! Test: Context Revision constraints.
//!
//! Context Revision uniqueness, immutability, cross-instance protection.

mod common;

/// Helper to create a minimal workflow instance, returning key IDs.
async fn create_minimal_instance(pool: &sqlx::PgPool) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid) {
    let (creator_id, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, def_ver_id, node_id, _) = common::seed_workflow_definition(&pool, domain_id).await;

    let instance_id = uuid::Uuid::new_v4();
    let ctx_id = uuid::Uuid::new_v4();
    let visit_id = uuid::Uuid::new_v4();
    let digest = sha256_hex(b"{}");

    let mut tx = pool.begin().await.expect("begin tx");

    sqlx::query(
        r#"
        INSERT INTO workflow_instances
            (workflow_instance_id, domain_id, definition_version_id,
             created_by_principal_id, current_context_revision_id,
             current_node_visit_id, workflow_state_version)
        VALUES ($1, $2, $3, $4, $5, $6, 1)
        "#,
    )
    .bind(instance_id)
    .bind(domain_id)
    .bind(def_ver_id)
    .bind(creator_id)
    .bind(ctx_id)
    .bind(visit_id)
    .execute(&mut *tx)
    .await
    .expect("insert instance");

    sqlx::query(
        r#"
        INSERT INTO workflow_context_revisions
            (context_revision_id, workflow_instance_id, revision_number,
             previous_revision_id, payload, payload_digest, created_by_principal_id)
        VALUES ($1, $2, 1, NULL, '{}'::jsonb, $3, $4)
        "#,
    )
    .bind(ctx_id)
    .bind(instance_id)
    .bind(&digest)
    .bind(creator_id)
    .execute(&mut *tx)
    .await
    .expect("insert context revision");

    sqlx::query(
        r#"
        INSERT INTO workflow_node_visits
            (node_visit_id, workflow_instance_id, node_id, visit_number,
             assignee_principal_id, entered_by_transition_id)
        VALUES ($1, $2, $3, 1, $4, NULL)
        "#,
    )
    .bind(visit_id)
    .bind(instance_id)
    .bind(node_id)
    .bind(creator_id)
    .execute(&mut *tx)
    .await
    .expect("insert node visit");

    tx.commit().await.expect("commit tx");
    (instance_id, ctx_id, visit_id)
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

// ============================================================
// Tests
// ============================================================

#[tokio::test]
async fn test_context_revision_number_unique_within_instance() {
    let pool = common::create_pool().await;
    let (instance_id, _ctx_id, _visit_id) = create_minimal_instance(&pool).await;

    let creator_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, enabled) VALUES ($1, 'HUMAN', 'Creator', TRUE)",
    )
    .bind(creator_id)
    .execute(&pool)
    .await
    .expect("insert creator");

    let new_ctx_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_context_revisions
            (context_revision_id, workflow_instance_id, revision_number,
             previous_revision_id, payload, payload_digest, created_by_principal_id)
        VALUES ($1, $2, 1, NULL, '{}'::jsonb, $3, $4)
        "#,
    )
    .bind(new_ctx_id)
    .bind(instance_id)
    .bind(sha256_hex(b"{}"))
    .bind(creator_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("unique constraint") || err_str.contains("violates unique"),
                "expected unique constraint violation for duplicate revision_number, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected unique constraint violation but insert succeeded"),
    }
}

#[tokio::test]
async fn test_context_revision_cannot_reference_other_instance() {
    let pool = common::create_pool().await;
    let (instance1, _, _) = create_minimal_instance(&pool).await;
    let (instance2, _, _) = create_minimal_instance(&pool).await;

    let creator_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, enabled) VALUES ($1, 'HUMAN', 'Creator', TRUE)",
    )
    .bind(creator_id)
    .execute(&pool)
    .await
    .expect("insert creator");

    let (ctx1,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT context_revision_id FROM workflow_context_revisions WHERE workflow_instance_id = $1 LIMIT 1"
    )
    .bind(instance1)
    .fetch_one(&pool)
    .await
    .expect("get ctx from instance1");

    let ctx_rev2 = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_context_revisions
            (context_revision_id, workflow_instance_id, revision_number,
             previous_revision_id, payload, payload_digest, created_by_principal_id)
        VALUES ($1, $2, 2, $3, '{}'::jsonb, $4, $5)
        "#,
    )
    .bind(ctx_rev2)
    .bind(instance2)
    .bind(ctx1)
    .bind(sha256_hex(b"{}"))
    .bind(creator_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("foreign key constraint")
                    || err_str.contains("fk_previous_revision"),
                "expected FK violation for cross-instance previous_revision, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected FK violation for cross-instance previous_revision"),
    }
}

#[tokio::test]
async fn test_context_revision_immutable() {
    let pool = common::create_pool().await;
    let (_instance_id, ctx_id, _) = create_minimal_instance(&pool).await;

    let result = sqlx::query(
        "UPDATE workflow_context_revisions SET payload = '{\"x\":1}'::jsonb WHERE context_revision_id = $1",
    )
    .bind(ctx_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_context_revisions_immutable")
                    || err_str.contains("immutable record"),
                "expected trigger rejection of UPDATE, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected trigger rejection of UPDATE on context revision"),
    }
}

#[tokio::test]
async fn test_context_revision_cannot_delete() {
    let pool = common::create_pool().await;
    let (_instance_id, ctx_id, _) = create_minimal_instance(&pool).await;

    let result =
        sqlx::query("DELETE FROM workflow_context_revisions WHERE context_revision_id = $1")
            .bind(ctx_id)
            .execute(&pool)
            .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_context_revisions_immutable")
                    || err_str.contains("immutable record"),
                "expected trigger rejection of DELETE, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected trigger rejection of DELETE on context revision"),
    }
}
