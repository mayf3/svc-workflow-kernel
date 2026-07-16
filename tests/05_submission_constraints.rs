#![allow(clippy::needless_borrow)]
//! Test: Submission constraints.
//!
//! Submission uniqueness, cross-instance protection, immutability.

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
async fn test_submission_unique_per_visit() {
    let pool = common::create_pool().await;
    let (instance_id, ctx_id, visit_id) = create_minimal_instance(&pool).await;

    let (_, _, _, trans_id) = {
        let (domain_id,): (uuid::Uuid,) = sqlx::query_as(
            "SELECT domain_id FROM workflow_instances WHERE workflow_instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .expect("get domain");
        common::seed_workflow_definition(&pool, domain_id).await
    };

    let (author,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT assignee_principal_id FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(visit_id)
    .fetch_one(&pool)
    .await
    .expect("get assignee");

    let digest = sha256_hex(b"{}");

    let sub_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_submissions
            (submission_id, workflow_instance_id, source_node_visit_id,
             context_revision_id, author_principal_id, transition_id,
             payload, payload_digest, schema_version)
        VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, 'v1')
        "#,
    )
    .bind(sub_id)
    .bind(instance_id)
    .bind(visit_id)
    .bind(ctx_id)
    .bind(author)
    .bind(trans_id)
    .bind(&digest)
    .execute(&pool)
    .await
    .expect("first submission should succeed");

    let sub_id2 = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_submissions
            (submission_id, workflow_instance_id, source_node_visit_id,
             context_revision_id, author_principal_id, transition_id,
             payload, payload_digest, schema_version)
        VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, 'v1')
        "#,
    )
    .bind(sub_id2)
    .bind(instance_id)
    .bind(visit_id)
    .bind(ctx_id)
    .bind(author)
    .bind(trans_id)
    .bind(sha256_hex(b"{}"))
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("unique constraint") || err_str.contains("violates unique"),
                "expected unique constraint violation for duplicate submission per visit, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected unique constraint violation for duplicate submission per visit"),
    }
}

#[tokio::test]
async fn test_submission_cannot_mix_instances() {
    let pool = common::create_pool().await;
    let (instance1, ctx1, visit1) = create_minimal_instance(&pool).await;
    let (_instance2, _ctx2, visit2) = create_minimal_instance(&pool).await;

    let (author,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT assignee_principal_id FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(visit1)
    .fetch_one(&pool)
    .await
    .expect("get assignee");

    let (_, _, _, trans_id) = {
        let (domain_id,): (uuid::Uuid,) = sqlx::query_as(
            "SELECT domain_id FROM workflow_instances WHERE workflow_instance_id = $1",
        )
        .bind(instance1)
        .fetch_one(&pool)
        .await
        .expect("get domain");
        common::seed_workflow_definition(&pool, domain_id).await
    };

    let sub_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_submissions
            (submission_id, workflow_instance_id, source_node_visit_id,
             context_revision_id, author_principal_id, transition_id,
             payload, payload_digest, schema_version)
        VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, 'v1')
        "#,
    )
    .bind(sub_id)
    .bind(instance1)
    .bind(visit2)
    .bind(ctx1)
    .bind(author)
    .bind(trans_id)
    .bind(sha256_hex(b"{}"))
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("foreign key constraint")
                    || err_str.contains("fk_submission_visit_same_instance"),
                "expected FK violation for cross-instance submission, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected FK violation for cross-instance submission"),
    }
}

#[tokio::test]
async fn test_submission_immutable() {
    let pool = common::create_pool().await;
    let (instance_id, ctx_id, visit_id) = create_minimal_instance(&pool).await;

    let (_, _, _, trans_id) = {
        let (domain_id,): (uuid::Uuid,) = sqlx::query_as(
            "SELECT domain_id FROM workflow_instances WHERE workflow_instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(&pool)
        .await
        .expect("get domain");
        common::seed_workflow_definition(&pool, domain_id).await
    };

    let (author,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT assignee_principal_id FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(visit_id)
    .fetch_one(&pool)
    .await
    .expect("get assignee");

    let sub_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_submissions
            (submission_id, workflow_instance_id, source_node_visit_id,
             context_revision_id, author_principal_id, transition_id,
             payload, payload_digest, schema_version)
        VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, 'v1')
        "#,
    )
    .bind(sub_id)
    .bind(instance_id)
    .bind(visit_id)
    .bind(ctx_id)
    .bind(author)
    .bind(trans_id)
    .bind(sha256_hex(b"{}"))
    .execute(&pool)
    .await
    .expect("insert submission");

    let result = sqlx::query(
        "UPDATE workflow_submissions SET payload = '{\"x\":1}'::jsonb WHERE submission_id = $1",
    )
    .bind(sub_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_submissions_immutable")
                    || err_str.contains("immutable record"),
                "expected trigger rejection of UPDATE on submission, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected trigger rejection of UPDATE on submission"),
    }
}
