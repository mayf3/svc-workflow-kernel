#![allow(clippy::needless_borrow)]
//! Test: Definition graph immutability after PUBLISHED/DEPRECATED/REVOKED.
//!
//! Covers workflow_node_definitions and workflow_transition_definitions.
//! Tests cover all 3 operation types (INSERT, UPDATE, DELETE) across
//! all 4 version statuses (DRAFT, PUBLISHED, DEPRECATED, REVOKED).

mod common;

/// Create a definition version in a specific status with nodes and transitions.
/// Returns (domain_id, def_ver_id, node_id, trans_id).
async fn create_definition_with_status(
    pool: &sqlx::PgPool,
    status: &str,
) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid) {
    let (_principal_id, domain_id) = common::seed_principal_and_domain(&pool).await;
    let (_def_id, ver_id, node_id, trans_id) =
        common::seed_workflow_definition(&pool, domain_id).await;

    match status {
        "DRAFT" => {}
        "PUBLISHED" => {
            sqlx::query("UPDATE workflow_definition_versions SET version_status = 'PUBLISHED'::definition_version_status, published_at = now() WHERE definition_version_id = $1")
                .bind(ver_id).execute(pool).await.expect("publish version");
        }
        "DEPRECATED" => {
            sqlx::query("UPDATE workflow_definition_versions SET version_status = 'PUBLISHED'::definition_version_status, published_at = now() WHERE definition_version_id = $1")
                .bind(ver_id).execute(pool).await.expect("publish version");
            sqlx::query("UPDATE workflow_definition_versions SET version_status = 'DEPRECATED'::definition_version_status, deprecated_at = now() WHERE definition_version_id = $1")
                .bind(ver_id).execute(pool).await.expect("deprecate version");
        }
        "REVOKED" => {
            sqlx::query("UPDATE workflow_definition_versions SET version_status = 'PUBLISHED'::definition_version_status, published_at = now() WHERE definition_version_id = $1")
                .bind(ver_id).execute(pool).await.expect("publish version");
            sqlx::query("UPDATE workflow_definition_versions SET version_status = 'REVOKED'::definition_version_status, revoked_at = now() WHERE definition_version_id = $1")
                .bind(ver_id).execute(pool).await.expect("revoke version");
        }
        _ => panic!("unknown status: {}", status),
    }
    (domain_id, ver_id, node_id, trans_id)
}

// ============================================================
// Node Definition — DRAFT allows modifications
// ============================================================

#[tokio::test]
async fn test_node_def_draft_allows_insert() {
    let pool = common::create_pool().await;
    let (_, ver_id, _, _) = create_definition_with_status(&pool, "DRAFT").await;

    let principal_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO principals (principal_id, principal_type, display_name, enabled) VALUES ($1, 'HUMAN', 'Test', TRUE)")
        .bind(principal_id).execute(&pool).await.expect("insert principal");

    let new_node_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type, fixed_principal_id) VALUES ($1,$2,'new-node','New',2,'NORMAL','FIXED_PRINCIPAL',$3)"#
    )
    .bind(new_node_id).bind(ver_id).bind(principal_id)
    .execute(&pool).await;

    assert!(
        result.is_ok(),
        "DRAFT version should allow INSERT on node_definitions"
    );
}

#[tokio::test]
async fn test_node_def_draft_allows_update() {
    let pool = common::create_pool().await;
    let (_, _, node_id, _) = create_definition_with_status(&pool, "DRAFT").await;

    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET display_name = 'Updated' WHERE node_id = $1",
    )
    .bind(node_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_ok(),
        "DRAFT version should allow UPDATE on node_definitions"
    );
}

#[tokio::test]
async fn test_node_def_draft_allows_delete() {
    let pool = common::create_pool().await;
    let (_, _, node_id, trans_id) = create_definition_with_status(&pool, "DRAFT").await;

    sqlx::query("DELETE FROM workflow_transition_definitions WHERE transition_id = $1")
        .bind(trans_id)
        .execute(&pool)
        .await
        .expect("delete referencing transition");

    let result = sqlx::query("DELETE FROM workflow_node_definitions WHERE node_id = $1")
        .bind(node_id)
        .execute(&pool)
        .await;

    assert!(
        result.is_ok(),
        "DRAFT version should allow DELETE on node_definitions"
    );
}

// ============================================================
// Node Definition — PUBLISHED rejects modifications
// ============================================================

#[tokio::test]
async fn test_node_def_published_rejects_insert() {
    let pool = common::create_pool().await;
    let (_, ver_id, _, _) = create_definition_with_status(&pool, "PUBLISHED").await;

    let principal_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO principals (principal_id, principal_type, display_name, enabled) VALUES ($1, 'HUMAN', 'Test', TRUE)")
        .bind(principal_id).execute(&pool).await.expect("insert principal");

    let new_node_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type, fixed_principal_id) VALUES ($1,$2,'new-node','New',2,'NORMAL','FIXED_PRINCIPAL',$3)"#
    )
    .bind(new_node_id).bind(ver_id).bind(principal_id)
    .execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_node_definitions_graph_immutable"),
                "expected graph_immutable rejection of INSERT on PUBLISHED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of INSERT on PUBLISHED version"),
    }
}

#[tokio::test]
async fn test_node_def_published_rejects_update() {
    let pool = common::create_pool().await;
    let (_, _, node_id, _) = create_definition_with_status(&pool, "PUBLISHED").await;

    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET display_name = 'Hacked' WHERE node_id = $1",
    )
    .bind(node_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_node_definitions_graph_immutable"),
                "expected graph_immutable rejection of UPDATE on PUBLISHED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of UPDATE on PUBLISHED version"),
    }
}

#[tokio::test]
async fn test_node_def_published_rejects_delete() {
    let pool = common::create_pool().await;
    let (_, _, node_id, _) = create_definition_with_status(&pool, "PUBLISHED").await;

    let result = sqlx::query("DELETE FROM workflow_node_definitions WHERE node_id = $1")
        .bind(node_id)
        .execute(&pool)
        .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_node_definitions_graph_immutable"),
                "expected graph_immutable rejection of DELETE on PUBLISHED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of DELETE on PUBLISHED version"),
    }
}

// ============================================================
// Node Definition — DEPRECATED rejects modifications
// ============================================================

#[tokio::test]
async fn test_node_def_deprecated_rejects_update() {
    let pool = common::create_pool().await;
    let (_, _, node_id, _) = create_definition_with_status(&pool, "DEPRECATED").await;

    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET display_name = 'Hacked' WHERE node_id = $1",
    )
    .bind(node_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_node_definitions_graph_immutable"),
                "expected graph_immutable rejection of UPDATE on DEPRECATED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of UPDATE on DEPRECATED version"),
    }
}

// ============================================================
// Node Definition — REVOKED rejects modifications
// ============================================================

#[tokio::test]
async fn test_node_def_revoked_rejects_update() {
    let pool = common::create_pool().await;
    let (_, _, node_id, _) = create_definition_with_status(&pool, "REVOKED").await;

    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET display_name = 'Hacked' WHERE node_id = $1",
    )
    .bind(node_id)
    .execute(&pool)
    .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_node_definitions_graph_immutable"),
                "expected graph_immutable rejection of UPDATE on REVOKED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of UPDATE on REVOKED version"),
    }
}

// ============================================================
// Transition Definition — DRAFT allows modifications
// ============================================================

#[tokio::test]
async fn test_trans_def_draft_allows_insert() {
    let pool = common::create_pool().await;
    let (_, ver_id, node_id1, _) = create_definition_with_status(&pool, "DRAFT").await;

    let (target_node_id,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT node_id FROM workflow_node_definitions WHERE definition_version_id = $1 AND node_key = 'done'"
    ).bind(ver_id).fetch_one(&pool).await.expect("get terminal node");

    let new_trans_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect) VALUES ($1,$2,'new-trans','New',$3,$4,'ADVANCE')"#
    ).bind(new_trans_id).bind(ver_id).bind(node_id1).bind(target_node_id)
    .execute(&pool).await;

    assert!(
        result.is_ok(),
        "DRAFT version should allow INSERT on transition_definitions"
    );
}

#[tokio::test]
async fn test_trans_def_draft_allows_update() {
    let pool = common::create_pool().await;
    let (_, _, _, trans_id) = create_definition_with_status(&pool, "DRAFT").await;

    let result = sqlx::query("UPDATE workflow_transition_definitions SET display_name = 'Updated' WHERE transition_id = $1")
        .bind(trans_id).execute(&pool).await;

    assert!(
        result.is_ok(),
        "DRAFT version should allow UPDATE on transition_definitions"
    );
}

#[tokio::test]
async fn test_trans_def_draft_allows_delete() {
    let pool = common::create_pool().await;
    let (_, _, _, trans_id) = create_definition_with_status(&pool, "DRAFT").await;

    let result =
        sqlx::query("DELETE FROM workflow_transition_definitions WHERE transition_id = $1")
            .bind(trans_id)
            .execute(&pool)
            .await;

    assert!(
        result.is_ok(),
        "DRAFT version should allow DELETE on transition_definitions"
    );
}

// ============================================================
// Transition Definition — PUBLISHED rejects modifications
// ============================================================

#[tokio::test]
async fn test_trans_def_published_rejects_insert() {
    let pool = common::create_pool().await;
    let (_, ver_id, node_id1, _) = create_definition_with_status(&pool, "PUBLISHED").await;

    let (target_node_id,): (uuid::Uuid,) = sqlx::query_as(
        "SELECT node_id FROM workflow_node_definitions WHERE definition_version_id = $1 AND node_key = 'done'"
    ).bind(ver_id).fetch_one(&pool).await.expect("get terminal node");

    let new_trans_id = uuid::Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect) VALUES ($1,$2,'new-trans','New',$3,$4,'ADVANCE')"#
    ).bind(new_trans_id).bind(ver_id).bind(node_id1).bind(target_node_id)
    .execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_transition_definitions_graph_immutable"),
                "expected graph_immutable rejection of INSERT on PUBLISHED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of INSERT on PUBLISHED version"),
    }
}

#[tokio::test]
async fn test_trans_def_published_rejects_update() {
    let pool = common::create_pool().await;
    let (_, _, _, trans_id) = create_definition_with_status(&pool, "PUBLISHED").await;

    let result = sqlx::query("UPDATE workflow_transition_definitions SET display_name = 'Hacked' WHERE transition_id = $1")
        .bind(trans_id).execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_transition_definitions_graph_immutable"),
                "expected graph_immutable rejection of UPDATE on PUBLISHED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of UPDATE on PUBLISHED version"),
    }
}

#[tokio::test]
async fn test_trans_def_published_rejects_delete() {
    let pool = common::create_pool().await;
    let (_, _, _, trans_id) = create_definition_with_status(&pool, "PUBLISHED").await;

    let result =
        sqlx::query("DELETE FROM workflow_transition_definitions WHERE transition_id = $1")
            .bind(trans_id)
            .execute(&pool)
            .await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_transition_definitions_graph_immutable"),
                "expected graph_immutable rejection of DELETE on PUBLISHED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of DELETE on PUBLISHED version"),
    }
}

// ============================================================
// Transition Definition — DEPRECATED/REVOKED rejects modifications
// ============================================================

#[tokio::test]
async fn test_trans_def_deprecated_rejects_update() {
    let pool = common::create_pool().await;
    let (_, _, _, trans_id) = create_definition_with_status(&pool, "DEPRECATED").await;

    let result = sqlx::query("UPDATE workflow_transition_definitions SET display_name = 'Hacked' WHERE transition_id = $1")
        .bind(trans_id).execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_transition_definitions_graph_immutable"),
                "expected graph_immutable rejection of UPDATE on DEPRECATED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of UPDATE on DEPRECATED version"),
    }
}

#[tokio::test]
async fn test_trans_def_revoked_rejects_update() {
    let pool = common::create_pool().await;
    let (_, _, _, trans_id) = create_definition_with_status(&pool, "REVOKED").await;

    let result = sqlx::query("UPDATE workflow_transition_definitions SET display_name = 'Hacked' WHERE transition_id = $1")
        .bind(trans_id).execute(&pool).await;

    match result {
        Err(e) => {
            let err_str = e.to_string();
            assert!(
                err_str.contains("graph_immutable")
                    || err_str.contains("trg_transition_definitions_graph_immutable"),
                "expected graph_immutable rejection of UPDATE on REVOKED version, got: {}",
                err_str
            );
        }
        Ok(_) => panic!("expected graph_immutable rejection of UPDATE on REVOKED version"),
    }
}
