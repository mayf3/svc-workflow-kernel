#![allow(clippy::needless_borrow)]
//! Test: Data size limit enforcement.
//!
//! All size checks use pg_column_size() for jsonb columns, which measures
//! the storage size of the binary JSONB representation (including type overhead).
//! These are defensive database-level hard limits; finer-grained validation
//! should also be performed in the Rust service layer.

mod common;

/// Generate a JSON string value of approximately `target_bytes` bytes.
/// The result is a JSON object with a single key whose value has `size` chars.
fn make_large_json(target_bytes: usize) -> serde_json::Value {
    let content_size = target_bytes.saturating_sub(32);
    let value = "x".repeat(content_size);
    serde_json::json!({ "data": value })
}

#[tokio::test]
async fn test_context_payload_size_limit() {
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

    // Create a payload > 1 MiB
    let large_payload = make_large_json(1_200_000);

    let mut tx = pool.begin().await.expect("begin tx");

    sqlx::query(
        r#"INSERT INTO workflow_instances (workflow_instance_id, domain_id, definition_version_id, created_by_principal_id, current_context_revision_id, current_node_visit_id, workflow_state_version) VALUES ($1,$2,$3,$4,$5,$6,1)"#
    )
    .bind(instance_id).bind(domain_id).bind(def_ver_id).bind(creator_id).bind(ctx_id).bind(visit_id)
    .execute(&mut *tx).await.expect("insert instance");

    sqlx::query(
        r#"INSERT INTO workflow_node_visits (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id) VALUES ($1,$2,$3,1,$4)"#
    )
    .bind(visit_id).bind(instance_id).bind(node_id).bind(creator_id)
    .execute(&mut *tx).await.expect("insert visit");

    // Try to insert a context revision with large payload
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_context_revisions
            (context_revision_id, workflow_instance_id, revision_number,
             previous_revision_id, payload, payload_digest, created_by_principal_id)
        VALUES ($1, $2, 1, NULL, $3, $4, $5)
        "#,
    )
    .bind(ctx_id)
    .bind(instance_id)
    .bind(&large_payload)
    .bind(&digest)
    .bind(creator_id)
    .execute(&mut *tx)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("chk_ctx_payload_size") || err_str.contains("CHECK"),
                "expected CHECK constraint violation for oversized context payload, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected CHECK constraint failure for oversized context payload"),
    }

    // Rollback is fine since we expect failure
    let _ = tx.rollback().await;
}

#[tokio::test]
async fn test_instance_metadata_size_limit() {
    let pool = common::create_pool().await;
    let (creator_id, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, def_ver_id, _, _) = common::seed_workflow_definition(&pool, domain_id).await;

    // Create metadata > 64 KiB
    let large_metadata = make_large_json(80_000);

    let result = sqlx::query(
        r#"
        INSERT INTO workflow_instances
            (workflow_instance_id, domain_id, definition_version_id,
             created_by_principal_id, metadata, workflow_state_version)
        VALUES ($1, $2, $3, $4, $5, 1)
        "#,
    )
    .bind(uuid::Uuid::new_v4())
    .bind(domain_id)
    .bind(def_ver_id)
    .bind(creator_id)
    .bind(&large_metadata)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("chk_instance_metadata_size") || err_str.contains("CHECK"),
                "expected CHECK constraint violation for oversized metadata, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected CHECK constraint failure for oversized metadata"),
    }
}

#[tokio::test]
async fn test_event_data_size_limit() {
    let pool = common::create_pool().await;

    // Create instance with submission
    let (instance_id, ctx_id, visit_id, sub_id, _trans_id, actor_id) = {
        let (principal_id, domain_id) = common::seed_principal_and_domain(&pool).await;
        let (_, def_ver_id, node_id, trans_id) =
            common::seed_workflow_definition(&pool, domain_id).await;
        let iid = uuid::Uuid::new_v4();
        let cid = uuid::Uuid::new_v4();
        let vid = uuid::Uuid::new_v4();
        let sid = uuid::Uuid::new_v4();
        let dig = {
            use sha2::Digest;
            hex::encode(sha2::Sha256::digest(b"{}"))
        };

        let mut tx = pool.begin().await.expect("begin tx");
        sqlx::query("INSERT INTO workflow_instances (workflow_instance_id, domain_id, definition_version_id, created_by_principal_id, current_context_revision_id, current_node_visit_id, workflow_state_version) VALUES ($1,$2,$3,$4,$5,$6,1)")
            .bind(iid).bind(domain_id).bind(def_ver_id).bind(principal_id).bind(cid).bind(vid)
            .execute(&mut *tx).await.expect("inst");
        sqlx::query("INSERT INTO workflow_context_revisions (context_revision_id, workflow_instance_id, revision_number, previous_revision_id, payload, payload_digest, created_by_principal_id) VALUES ($1,$2,1,NULL,'{}'::jsonb,$3,$4)")
            .bind(cid).bind(iid).bind(&dig).bind(principal_id)
            .execute(&mut *tx).await.expect("ctx");
        sqlx::query("INSERT INTO workflow_node_visits (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id) VALUES ($1,$2,$3,1,$4)")
            .bind(vid).bind(iid).bind(node_id).bind(principal_id)
            .execute(&mut *tx).await.expect("visit");
        sqlx::query("INSERT INTO workflow_submissions (submission_id, workflow_instance_id, source_node_visit_id, context_revision_id, author_principal_id, transition_id, payload, payload_digest, schema_version) VALUES ($1,$2,$3,$4,$5,$6,'{}'::jsonb,$7,'v1')")
            .bind(sid).bind(iid).bind(vid).bind(cid).bind(principal_id).bind(trans_id).bind(&dig)
            .execute(&mut *tx).await.expect("sub");
        tx.commit().await.expect("commit");
        (iid, cid, vid, sid, trans_id, principal_id)
    };

    // Create event_data > 256 KiB
    let large_event_data = make_large_json(300_000);

    let event_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             event_type, transition_effect, source_node_visit_id, target_node_visit_id,
             context_revision_id, submission_id, event_data, actor_principal_id,
             old_workflow_state_version, new_workflow_state_version)
        VALUES ($1,$2,1,'v1','WORKFLOW_INSTANCE_CREATED',NULL,NULL,$3,$4,$5,$6,$7,0,1)
        "#,
    )
    .bind(event_id)
    .bind(instance_id)
    .bind(visit_id)
    .bind(ctx_id)
    .bind(sub_id)
    .bind(&large_event_data)
    .bind(actor_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("chk_event_data_size") || err_str.contains("CHECK"),
                "expected CHECK constraint violation for oversized event_data, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected CHECK constraint failure for oversized event_data"),
    }
}

#[tokio::test]
async fn test_small_payloads_are_accepted() {
    let pool = common::create_pool().await;
    let (creator_id, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, def_ver_id, node_id, _) = common::seed_workflow_definition(&pool, domain_id).await;

    let instance_id = uuid::Uuid::new_v4();
    let ctx_id = uuid::Uuid::new_v4();
    let visit_id = uuid::Uuid::new_v4();
    let digest = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(b"{\"small\":\"ok\"}"))
    };

    let mut tx = pool.begin().await.expect("begin tx");
    sqlx::query(
        r#"INSERT INTO workflow_instances (workflow_instance_id, domain_id, definition_version_id, created_by_principal_id, current_context_revision_id, current_node_visit_id, workflow_state_version) VALUES ($1,$2,$3,$4,$5,$6,1)"#
    )
    .bind(instance_id).bind(domain_id).bind(def_ver_id).bind(creator_id).bind(ctx_id).bind(visit_id)
    .execute(&mut *tx).await.expect("inst");

    sqlx::query(
        r#"INSERT INTO workflow_context_revisions (context_revision_id, workflow_instance_id, revision_number, previous_revision_id, payload, payload_digest, created_by_principal_id) VALUES ($1,$2,1,NULL,'{"small":"ok"}'::jsonb,$3,$4)"#
    )
    .bind(ctx_id).bind(instance_id).bind(&digest).bind(creator_id)
    .execute(&mut *tx).await.expect("ctx");

    sqlx::query(
        r#"INSERT INTO workflow_node_visits (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id) VALUES ($1,$2,$3,1,$4)"#
    )
    .bind(visit_id).bind(instance_id).bind(node_id).bind(creator_id)
    .execute(&mut *tx).await.expect("visit");

    tx.commit().await.expect("commit");

    // Verify the data exists
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::int8 FROM workflow_context_revisions WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(count.0, 1, "small payload should have been inserted");
}

// ============================================================
// Missing constraint tests (MEDIUM #3 in audit)
// ============================================================

#[tokio::test]
async fn test_submission_payload_size_limit() {
    let pool = common::create_pool().await;
    let (instance_id, ctx_id, visit_id) = {
        let (creator_id, domain_id) = common::seed_principal_and_domain(&pool).await;
        let (_, def_ver_id, node_id, _) = common::seed_workflow_definition(&pool, domain_id).await;
        let iid = uuid::Uuid::new_v4();
        let cid = uuid::Uuid::new_v4();
        let vid = uuid::Uuid::new_v4();
        let dig = {
            use sha2::Digest;
            hex::encode(sha2::Sha256::digest(b"{}"))
        };
        let mut tx = pool.begin().await.expect("begin tx");
        sqlx::query("INSERT INTO workflow_instances (workflow_instance_id, domain_id, definition_version_id, created_by_principal_id, current_context_revision_id, current_node_visit_id, workflow_state_version) VALUES ($1,$2,$3,$4,$5,$6,1)")
            .bind(iid).bind(domain_id).bind(def_ver_id).bind(creator_id).bind(cid).bind(vid)
            .execute(&mut *tx).await.expect("inst");
        sqlx::query("INSERT INTO workflow_context_revisions (context_revision_id, workflow_instance_id, revision_number, previous_revision_id, payload, payload_digest, created_by_principal_id) VALUES ($1,$2,1,NULL,'{}'::jsonb,$3,$4)")
            .bind(cid).bind(iid).bind(&dig).bind(creator_id)
            .execute(&mut *tx).await.expect("ctx");
        sqlx::query("INSERT INTO workflow_node_visits (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id) VALUES ($1,$2,$3,1,$4)")
            .bind(vid).bind(iid).bind(node_id).bind(creator_id)
            .execute(&mut *tx).await.expect("visit");
        tx.commit().await.expect("commit");
        (iid, cid, vid)
    };

    // Get a transition and author
    let (trans_id,): (uuid::Uuid,) =
        sqlx::query_as("SELECT transition_id FROM workflow_transition_definitions LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("get trans");
    let (author_id,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT assignee_principal_id FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(visit_id)
    .fetch_one(&pool)
    .await
    .expect("get author");

    let large_payload = make_large_json(1_200_000);
    let sub_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO workflow_submissions (submission_id, workflow_instance_id, source_node_visit_id, context_revision_id, author_principal_id, transition_id, payload, payload_digest, schema_version) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'v1')"#
    )
    .bind(sub_id).bind(instance_id).bind(visit_id).bind(ctx_id).bind(author_id).bind(trans_id)
    .bind(&large_payload).bind(sha256_hex(b"{}"))
    .execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("chk_submission_payload_size") || err_str.contains("CHECK"),
                "expected CHECK constraint violation for oversized submission payload, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected CHECK constraint failure for oversized submission payload"),
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

#[tokio::test]
async fn test_definition_metadata_size_limit() {
    let pool = common::create_pool().await;
    let (_, domain_id) = common::seed_principal_and_domain(&pool).await;
    let def_id = uuid::Uuid::new_v4();

    let large_metadata = make_large_json(80_000);
    let result = sqlx::query(
        r#"INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name, metadata) VALUES ($1,$2,'def-size-test','Test',$3)"#
    )
    .bind(def_id).bind(domain_id).bind(&large_metadata)
    .execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("chk_def_metadata_size") || err_str.contains("CHECK"),
                "expected CHECK constraint violation for oversized definition metadata, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected CHECK constraint failure for oversized definition metadata"),
    }
}

#[tokio::test]
async fn test_definition_version_metadata_size_limit() {
    let pool = common::create_pool().await;
    let (_, domain_id) = common::seed_principal_and_domain(&pool).await;
    let def_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1,$2,'def-ver-size-test','Test')")
        .bind(def_id).bind(domain_id).execute(&pool).await.expect("insert def");

    let large_metadata = make_large_json(80_000);
    let ver_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, metadata) VALUES ($1,$2,1,'DRAFT',$3)"#
    )
    .bind(ver_id).bind(def_id).bind(&large_metadata)
    .execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(err_str.contains("chk_def_ver_metadata_size") || err_str.contains("CHECK"),
                "expected CHECK constraint violation for oversized definition version metadata, got: {}", err_str);
        }
        Ok(_) => {
            panic!("expected CHECK constraint failure for oversized definition version metadata")
        }
    }
}

#[tokio::test]
async fn test_receipt_response_body_size_limit() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;

    let large_body = make_large_json(1_200_000);
    let digest = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let result = sqlx::query(
        r#"INSERT INTO workflow_command_receipts (command_id, principal_id, idempotency_key, command_type, request_hash, receipt_status, response_status, response_body) VALUES ($1,$2,'receipt-size-test','TEST',$3,'COMPLETED',200,$4)"#
    )
    .bind(uuid::Uuid::new_v4()).bind(principal_id).bind(digest).bind(&large_body)
    .execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("chk_receipt_response_size") || err_str.contains("CHECK"),
                "expected CHECK constraint violation for oversized receipt response body, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected CHECK constraint failure for oversized receipt response body"),
    }
}
