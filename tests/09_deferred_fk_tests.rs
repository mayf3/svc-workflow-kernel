#![allow(clippy::needless_borrow)]
//! Test: Deferred composite foreign keys for initial circular references.
//!
//! The initial creation of a WorkflowInstance must simultaneously
//! satisfy FKs from instance -> context revision and instance -> node visit,
//! even though those records also reference the instance.
//! DEFERRABLE INITIALLY DEFERRED constraints allow this within a single transaction.

mod common;

#[tokio::test]
async fn test_circular_reference_in_one_transaction() {
    let pool = common::create_pool().await;
    let (creator_id, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, def_ver_id, node_id, _) = common::seed_workflow_definition(&pool, domain_id).await;

    let instance_id = uuid::Uuid::new_v4();
    let ctx_id = uuid::Uuid::new_v4();
    let visit_id = uuid::Uuid::new_v4();
    let digest = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(b"{}"))
    };

    // Everything in one transaction with DEFERRED FKs
    let mut tx = pool.begin().await.expect("begin transaction");

    // 1. Insert instance (references ctx_id and visit_id that don't exist yet)
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
    .expect("step 1: insert instance BEFORE ctx and visit exist");

    // 2. Insert context revision (references instance_id)
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
    .expect("step 2: insert context revision");

    // 3. Insert node visit (references instance_id)
    sqlx::query(
        r#"
        INSERT INTO workflow_node_visits
            (node_visit_id, workflow_instance_id, node_id, visit_number,
             assignee_principal_id)
        VALUES ($1, $2, $3, 1, $4)
        "#,
    )
    .bind(visit_id)
    .bind(instance_id)
    .bind(node_id)
    .bind(creator_id)
    .execute(&mut *tx)
    .await
    .expect("step 3: insert node visit");

    // Commit should succeed because all circular references resolve
    tx.commit()
        .await
        .expect("transaction should commit successfully");

    // Verify the data is accessible
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::int8 FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("query");

    assert_eq!(row.0, 1, "instance should exist after deferred commit");
}

#[tokio::test]
async fn test_circular_ref_fails_if_missing_entity() {
    let pool = common::create_pool().await;
    let (creator_id, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, def_ver_id, _node_id, _) = common::seed_workflow_definition(&pool, domain_id).await;

    let instance_id = uuid::Uuid::new_v4();
    let ctx_id = uuid::Uuid::new_v4();
    let visit_id = uuid::Uuid::new_v4();

    // Start a transaction but DON'T insert the node visit
    let mut tx = pool.begin().await.expect("begin transaction");

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
    .bind({
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(b"{}"))
    })
    .bind(creator_id)
    .execute(&mut *tx)
    .await
    .expect("insert ctx");

    // MISSING: node_visit insert

    // Commit should fail because node_visit FK is not satisfied
    let result = tx.commit().await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("foreign key constraint")
                    || err_str.contains("fk_instance_current_visit"),
                "expected FK violation for missing node visit, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected FK violation for missing node visit"),
    }
}
