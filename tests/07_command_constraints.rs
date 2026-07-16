#![allow(clippy::needless_borrow)]
//! Test: Command Receipt constraints.
//!
//! Idempotency key uniqueness, COMPLETED immutability,
//! status transition lifecycle.

mod common;

#[tokio::test]
async fn test_receipt_idempotency_key_unique() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;

    let cmd_id1 = uuid::Uuid::new_v4();
    let digest = "0000000000000000000000000000000000000000000000000000000000000000";

    // Insert first receipt
    sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash,
             receipt_status)
        VALUES ($1, $2, 'same-key', 'TEST', $3, 'PROCESSING')
        "#,
    )
    .bind(cmd_id1)
    .bind(principal_id)
    .bind(digest)
    .execute(&pool)
    .await
    .expect("first receipt should succeed");

    // Try to insert with same (principal_id, idempotency_key)
    let cmd_id2 = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash,
             receipt_status)
        VALUES ($1, $2, 'same-key', 'TEST', $3, 'PROCESSING')
        "#,
    )
    .bind(cmd_id2)
    .bind(principal_id)
    .bind(digest)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("unique constraint")
                    || err_str.contains("idx_wf_receipt_idempotency"),
                "expected unique constraint violation for duplicate idempotency key, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected unique constraint violation for duplicate idempotency key"),
    }
}

#[tokio::test]
async fn test_completed_receipt_cannot_update() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;

    let cmd_id = uuid::Uuid::new_v4();
    let digest = "0000000000000000000000000000000000000000000000000000000000000000";

    // Insert COMPLETED receipt
    sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash,
             receipt_status, response_status, response_body)
        VALUES ($1, $2, 'test-key', 'TEST', $3, 'COMPLETED', 200, '{}'::jsonb)
        "#,
    )
    .bind(cmd_id)
    .bind(principal_id)
    .bind(digest)
    .execute(&pool)
    .await
    .expect("insert COMPLETED receipt");

    // Try to UPDATE it
    let result = sqlx::query(
        r#"
        UPDATE workflow_command_receipts
        SET response_status = 500
        WHERE command_id = $1
        "#,
    )
    .bind(cmd_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_command_receipts_completed_immutable")
                    || err_str.contains("immutable record"),
                "expected trigger rejection of UPDATE on COMPLETED receipt, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected trigger rejection of UPDATE on COMPLETED receipt"),
    }
}

#[tokio::test]
async fn test_completed_receipt_cannot_delete() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;

    let cmd_id = uuid::Uuid::new_v4();
    let digest = "0000000000000000000000000000000000000000000000000000000000000000";

    sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash,
             receipt_status, response_status, response_body)
        VALUES ($1, $2, 'test-key-2', 'TEST', $3, 'COMPLETED', 200, '{}'::jsonb)
        "#,
    )
    .bind(cmd_id)
    .bind(principal_id)
    .bind(digest)
    .execute(&pool)
    .await
    .expect("insert COMPLETED receipt");

    // Try to DELETE it
    let result = sqlx::query("DELETE FROM workflow_command_receipts WHERE command_id = $1")
        .bind(cmd_id)
        .execute(&pool)
        .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_command_receipts_completed_immutable")
                    || err_str.contains("immutable record"),
                "expected trigger rejection of DELETE on COMPLETED receipt, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected trigger rejection of DELETE on COMPLETED receipt"),
    }
}

#[tokio::test]
async fn test_receipt_status_transition_valid() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;

    let cmd_id = uuid::Uuid::new_v4();
    let digest = "0000000000000000000000000000000000000000000000000000000000000000";

    // Insert PROCESSING receipt
    sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash,
             receipt_status)
        VALUES ($1, $2, 'test-key-3', 'TEST', $3, 'PROCESSING')
        "#,
    )
    .bind(cmd_id)
    .bind(principal_id)
    .bind(digest)
    .execute(&pool)
    .await
    .expect("insert PROCESSING receipt");

    // Valid transition: PROCESSING -> COMPLETED
    let result = sqlx::query(
        r#"
        UPDATE workflow_command_receipts
        SET receipt_status = 'COMPLETED', response_status = 200, response_body = '{}'::jsonb
        WHERE command_id = $1
        "#,
    )
    .bind(cmd_id)
    .execute(&pool)
    .await;

    assert!(result.is_ok(), "PROCESSING -> COMPLETED should be allowed");
}

#[tokio::test]
async fn test_receipt_status_transition_invalid() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;

    let cmd_id = uuid::Uuid::new_v4();
    let digest = "0000000000000000000000000000000000000000000000000000000000000000";

    sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash,
             receipt_status, response_status, response_body)
        VALUES ($1, $2, 'test-key-4', 'TEST', $3, 'COMPLETED', 200, '{}'::jsonb)
        "#,
    )
    .bind(cmd_id)
    .bind(principal_id)
    .bind(digest)
    .execute(&pool)
    .await
    .expect("insert COMPLETED receipt");

    // Try to set COMPLETED status to PROCESSING
    let result = sqlx::query(
        r#"
        UPDATE workflow_command_receipts
        SET receipt_status = 'PROCESSING'
        WHERE command_id = $1
        "#,
    )
    .bind(cmd_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_command_receipts_completed_immutable")
                    || err_str.contains("immutable record"),
                "expected rejection of modification on COMPLETED receipt, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected rejection of modification on COMPLETED receipt"),
    }
}

// ============================================================
// PROCESSING Receipt identity fields must be frozen
// ============================================================

async fn create_processing_receipt(
    pool: &sqlx::PgPool,
    principal_id: uuid::Uuid,
    key_suffix: &str,
) -> (uuid::Uuid, String) {
    let cmd_id = uuid::Uuid::new_v4();
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    sqlx::query(
        r#"
        INSERT INTO workflow_command_receipts
            (command_id, principal_id, idempotency_key, command_type, request_hash, receipt_status)
        VALUES ($1, $2, $3, 'TEST', $4, 'PROCESSING')
        "#,
    )
    .bind(cmd_id)
    .bind(principal_id)
    .bind(format!("identity-test-{}", key_suffix))
    .bind(digest)
    .execute(pool)
    .await
    .expect("insert PROCESSING receipt");
    (cmd_id, digest.to_string())
}

#[tokio::test]
async fn test_processing_receipt_cannot_change_request_hash() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;
    let (cmd_id, _) = create_processing_receipt(&pool, principal_id, "hash").await;

    let new_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let result =
        sqlx::query("UPDATE workflow_command_receipts SET request_hash = $1 WHERE command_id = $2")
            .bind(new_hash)
            .bind(cmd_id)
            .execute(&pool)
            .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_receipt_identity_immutable")
                    || err_str.contains("request_hash"),
                "expected rejection of request_hash change on PROCESSING receipt, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected rejection of request_hash change on PROCESSING receipt"),
    }
}

#[tokio::test]
async fn test_processing_receipt_cannot_change_command_type() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;
    let (cmd_id, _) = create_processing_receipt(&pool, principal_id, "ctype").await;

    let result = sqlx::query(
        "UPDATE workflow_command_receipts SET command_type = 'OTHER' WHERE command_id = $1",
    )
    .bind(cmd_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_receipt_identity_immutable")
                    || err_str.contains("command_type"),
                "expected rejection of command_type change on PROCESSING receipt, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected rejection of command_type change on PROCESSING receipt"),
    }
}

#[tokio::test]
async fn test_processing_receipt_can_complete() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;
    let (cmd_id, _) = create_processing_receipt(&pool, principal_id, "complete").await;

    // Should be able to complete a PROCESSING receipt
    let result = sqlx::query(
        r#"UPDATE workflow_command_receipts SET receipt_status = 'COMPLETED', response_status = 200, response_body = '{}'::jsonb WHERE command_id = $1"#
    )
    .bind(cmd_id)
    .execute(&pool).await;

    assert!(
        result.is_ok(),
        "PROCESSING -> COMPLETED should be allowed on receipt identity test"
    );
}

#[tokio::test]
async fn test_completed_receipt_all_fields_immutable() {
    let pool = common::create_pool().await;
    let (principal_id, _) = common::seed_principal_and_domain(&pool).await;
    let (cmd_id, _) = create_processing_receipt(&pool, principal_id, "all").await;

    // First complete it
    sqlx::query(
        r#"UPDATE workflow_command_receipts SET receipt_status = 'COMPLETED', response_status = 200, response_body = '{}'::jsonb WHERE command_id = $1"#
    )
    .bind(cmd_id)
    .execute(&pool).await
    .expect("complete receipt");

    // Now try to change response_body (should be rejected by completed_immutable)
    let result = sqlx::query(
        r#"UPDATE workflow_command_receipts SET response_body = '{"x":1}'::jsonb WHERE command_id = $1"#
    )
    .bind(cmd_id)
    .execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("trg_command_receipts_completed_immutable")
                    || err_str.contains("immutable record"),
                "expected rejection of response_body change on COMPLETED receipt, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected rejection of response_body change on COMPLETED receipt"),
    }
}
