#![allow(clippy::needless_borrow)]
//! Test: Workflow Event constraints.
//!
//! Event sequence uniqueness, command_id uniqueness,
//! cross-instance entity references, and immutability.

mod common;

/// Helper to create a minimal instance with a submission for event tests.
/// Returns (instance_id, ctx_id, visit_id, sub_id, trans_id, author_id).
async fn create_instance_with_submission(
    pool: &sqlx::PgPool,
) -> (
    uuid::Uuid,
    uuid::Uuid,
    uuid::Uuid,
    uuid::Uuid,
    uuid::Uuid,
    uuid::Uuid,
) {
    let (creator_id, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_, def_ver_id, node_id, trans_id) =
        common::seed_workflow_definition(&pool, domain_id).await;

    let instance_id = uuid::Uuid::new_v4();
    let ctx_id = uuid::Uuid::new_v4();
    let visit_id = uuid::Uuid::new_v4();
    let sub_id = uuid::Uuid::new_v4();
    let digest = sha256_hex(b"{}");

    let mut tx = pool.begin().await.expect("begin tx");

    sqlx::query(
        r#"INSERT INTO workflow_instances (workflow_instance_id, domain_id, definition_version_id, created_by_principal_id, current_context_revision_id, current_node_visit_id, workflow_state_version) VALUES ($1,$2,$3,$4,$5,$6,1)"#
    )
    .bind(instance_id).bind(domain_id).bind(def_ver_id).bind(creator_id).bind(ctx_id).bind(visit_id)
    .execute(&mut *tx).await.expect("insert instance");

    sqlx::query(
        r#"INSERT INTO workflow_context_revisions (context_revision_id, workflow_instance_id, revision_number, previous_revision_id, payload, payload_digest, created_by_principal_id) VALUES ($1,$2,1,NULL,'{}'::jsonb,$3,$4)"#
    )
    .bind(ctx_id).bind(instance_id).bind(&digest).bind(creator_id)
    .execute(&mut *tx).await.expect("insert ctx");

    sqlx::query(
        r#"INSERT INTO workflow_node_visits (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id) VALUES ($1,$2,$3,1,$4)"#
    )
    .bind(visit_id).bind(instance_id).bind(node_id).bind(creator_id)
    .execute(&mut *tx).await.expect("insert visit");

    sqlx::query(
        r#"INSERT INTO workflow_submissions (submission_id, workflow_instance_id, source_node_visit_id, context_revision_id, author_principal_id, transition_id, payload, payload_digest, schema_version) VALUES ($1,$2,$3,$4,$5,$6,'{}'::jsonb,$7,'v1')"#
    )
    .bind(sub_id).bind(instance_id).bind(visit_id).bind(ctx_id).bind(creator_id).bind(trans_id).bind(&digest)
    .execute(&mut *tx).await.expect("insert submission");

    tx.commit().await.expect("commit tx");

    (instance_id, ctx_id, visit_id, sub_id, trans_id, creator_id)
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

// ============================================================
// Tests
// ============================================================

#[tokio::test]
async fn test_event_sequence_unique() {
    let pool = common::create_pool().await;
    let (instance_id, ctx_id, visit_id, _sub_id, _trans_id, actor_id) =
        create_instance_with_submission(&pool).await;

    // Insert first event
    let event_id1 = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             event_type, transition_effect, source_node_visit_id, target_node_visit_id,
             context_revision_id, submission_id, actor_principal_id,
             old_workflow_state_version, new_workflow_state_version)
        VALUES ($1,$2,1,'v1','WORKFLOW_INSTANCE_CREATED',NULL,NULL,$3,NULL,NULL,$4,0,1)
        "#,
    )
    .bind(event_id1)
    .bind(instance_id)
    .bind(visit_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("first event should succeed");

    // Try to insert another event with same sequence (must also match state version)
    let event_id2 = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             event_type, transition_effect, source_node_visit_id, target_node_visit_id,
             context_revision_id, submission_id, actor_principal_id,
             old_workflow_state_version, new_workflow_state_version)
        VALUES ($1,$2,1,'v1','WORKFLOW_CONTEXT_REVISED',NULL,$3,$3,$4,NULL,$5,0,1)
        "#,
    )
    .bind(event_id2)
    .bind(instance_id)
    .bind(visit_id)
    .bind(ctx_id)
    .bind(actor_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("unique constraint") || err_str.contains("violates unique"),
                "expected unique constraint violation for duplicate event_sequence, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected unique constraint violation for duplicate event_sequence"),
    }
}

#[tokio::test]
async fn test_event_command_id_unique() {
    let pool = common::create_pool().await;
    let (instance_id, ctx_id, visit_id, _prev_sub_id, _trans_id, actor_id) =
        create_instance_with_submission(&pool).await;

    // Create a receipt first
    let cmd_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash,
             receipt_status, response_status, response_body)
        VALUES ($1,$2,'test-key-1','TEST','0000000000000000000000000000000000000000000000000000000000000000'::text,
                'COMPLETED',200,'{}'::jsonb)
        "#,
    )
    .bind(cmd_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert receipt");

    // Insert first event referencing the command
    let event_id1 = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             command_id, event_type, transition_effect, source_node_visit_id,
             target_node_visit_id, context_revision_id, submission_id,
             actor_principal_id, old_workflow_state_version, new_workflow_state_version)
        VALUES ($1,$2,1,'v1',$3,'WORKFLOW_INSTANCE_CREATED',NULL,NULL,$4,NULL,NULL,$5,0,1)
        "#,
    )
    .bind(event_id1)
    .bind(instance_id)
    .bind(cmd_id)
    .bind(visit_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("first event with command should succeed");

    // Try to insert another event with same command_id
    let event_id2 = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             command_id, event_type, transition_effect, source_node_visit_id,
             target_node_visit_id, context_revision_id, submission_id,
             actor_principal_id, old_workflow_state_version, new_workflow_state_version)
        VALUES ($1,$2,2,'v1',$3,'WORKFLOW_CONTEXT_REVISED',NULL,$4,$4,$5,NULL,$6,1,2)
        "#,
    )
    .bind(event_id2)
    .bind(instance_id)
    .bind(cmd_id)
    .bind(visit_id)
    .bind(ctx_id)
    .bind(actor_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("unique constraint")
                    || err_str.contains("idx_wf_event_unique_command"),
                "expected unique constraint violation for duplicate command_id in event, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected unique constraint violation for duplicate command_id in event"),
    }
}

#[tokio::test]
async fn test_event_cannot_mix_instances_for_visits() {
    let pool = common::create_pool().await;
    let (instance1, _, _visit1, _, _, _) = create_instance_with_submission(&pool).await;
    let (_instance2, _, visit2, _, _, _) = create_instance_with_submission(&pool).await;

    let actor = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, enabled) VALUES ($1, 'HUMAN', 'Actor', TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .expect("insert actor");

    // Try to create event for instance1 but using instance2's visit
    let event_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             event_type, source_node_visit_id, target_node_visit_id,
             actor_principal_id, old_workflow_state_version, new_workflow_state_version)
        VALUES ($1,$2,1,'v1','TEST',$3,$4,$5,0,1)
        "#,
    )
    .bind(event_id)
    .bind(instance1) // event for instance1
    .bind(visit2) // but visit from instance2
    .bind(visit2)
    .bind(actor)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("foreign key constraint")
                    || err_str.contains("fk_event_source_visit_same_instance"),
                "expected FK violation for cross-instance event visit reference, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected FK violation for cross-instance event visit reference"),
    }
}

#[tokio::test]
async fn test_event_immutable() {
    let pool = common::create_pool().await;
    let (instance_id, ctx_id, visit_id, sub_id, _trans_id, actor_id) =
        create_instance_with_submission(&pool).await;

    let event_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_events
            (event_id, workflow_instance_id, event_sequence, event_schema_version,
             event_type, transition_effect, source_node_visit_id, target_node_visit_id,
             context_revision_id, submission_id, actor_principal_id,
             old_workflow_state_version, new_workflow_state_version)
        VALUES ($1,$2,1,'v1','WORKFLOW_INSTANCE_CREATED',NULL,NULL,$3,$4,$5,$6,0,1)
        "#,
    )
    .bind(event_id)
    .bind(instance_id)
    .bind(visit_id)
    .bind(ctx_id)
    .bind(sub_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert event");

    // Try to UPDATE the event
    let result = sqlx::query(
        "UPDATE workflow_events SET event_data = '{\"x\":1}'::jsonb WHERE event_id = $1",
    )
    .bind(event_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_events_immutable") || err_str.contains("immutable record"),
                "expected trigger rejection of UPDATE on event, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected trigger rejection of UPDATE on event"),
    }
}
