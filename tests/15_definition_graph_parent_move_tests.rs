//! Graph parent-version escape tests.
//!
//! Verifies that the migration 0008 trigger fix prevents
//! moving node/transition records between definition versions
//! when either the source or target version is non-DRAFT.
//!
//! Each test uses direct SQL to hit the storage boundary,
//! not the Application Service, because the escape path
//! is a database-level invariant enforced by triggers.

#![allow(unused_variables)]

mod common;

use common::{create_pool, seed_principal_domain_with_owner, seed_workflow_definition};

// ---------------------------------------------------------------------------
// Five escape-path scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_published_to_draft_node_move_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    // Publish the version first
    publish_version_simple(&pool, principal_id, ver_id, def_id).await;

    // Get a node from the published version
    let published_node: (uuid::Uuid,) = sqlx::query_as(
        "SELECT node_id FROM workflow_node_definitions WHERE definition_version_id = $1 LIMIT 1",
    )
    .bind(ver_id)
    .fetch_one(&pool)
    .await
    .expect("get published node");
    let published_node_id = published_node.0;

    // Create a new DRAFT version to try moving the node to
    let draft_ver_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema) VALUES ($1, $2, 100, 'DRAFT', '{}'::jsonb)",
    )
    .bind(draft_ver_id)
    .bind(def_id)
    .execute(&pool)
    .await
    .expect("insert draft version");

    // Try to move node from PUBLISHED to DRAFT
    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET definition_version_id = $1 WHERE node_id = $2",
    )
    .bind(draft_ver_id)
    .bind(published_node_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "PUBLISHED → DRAFT node move must be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("graph_immutable")
            || err_msg.contains("cannot change definition_version_id"),
        "error must mention graph immutability: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_draft_to_published_node_move_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    // Publish the version
    publish_version_simple(&pool, principal_id, ver_id, def_id).await;

    // Create a new DRAFT version and insert a node
    let draft_ver_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema) VALUES ($1, $2, 200, 'DRAFT', '{}'::jsonb)",
    )
    .bind(draft_ver_id)
    .bind(def_id)
    .execute(&pool)
    .await
    .expect("insert draft version");

    let draft_node_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'draft-node', 'Draft Node', 0, 'DRAFT', 'WORKFLOW_CREATOR')",
    )
    .bind(draft_node_id)
    .bind(draft_ver_id)
    .execute(&pool)
    .await
    .expect("insert draft node");

    // Try to move node from DRAFT to PUBLISHED
    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET definition_version_id = $1 WHERE node_id = $2",
    )
    .bind(ver_id)
    .bind(draft_node_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "DRAFT → PUBLISHED node move must be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("graph_immutable"),
        "error must mention graph immutability: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_published_to_draft_transition_move_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    // Publish the version
    publish_version_simple(&pool, principal_id, ver_id, def_id).await;

    // Get a transition from the published version
    let published_trans: (uuid::Uuid,) = sqlx::query_as(
        "SELECT transition_id FROM workflow_transition_definitions WHERE definition_version_id = $1 LIMIT 1"
    )
    .bind(ver_id)
    .fetch_one(&pool)
    .await
    .expect("get published transition");

    // Create a new DRAFT version
    let draft_ver_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema) VALUES ($1, $2, 300, 'DRAFT', '{}'::jsonb)",
    )
    .bind(draft_ver_id)
    .bind(def_id)
    .execute(&pool)
    .await
    .expect("insert draft version");

    // Try to move transition from PUBLISHED to DRAFT
    let result = sqlx::query(
        "UPDATE workflow_transition_definitions SET definition_version_id = $1 WHERE transition_id = $2",
    )
    .bind(draft_ver_id)
    .bind(published_trans.0)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "PUBLISHED → DRAFT transition move must be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("graph_immutable")
            || err_msg.contains("cannot change definition_version_id"),
        "error must mention graph immutability: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_draft_to_published_transition_move_rejected() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    // Publish the version
    publish_version_simple(&pool, principal_id, ver_id, def_id).await;

    // Get published nodes to reference in the draft transition
    let published_nodes: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT node_id FROM workflow_node_definitions WHERE definition_version_id = $1 ORDER BY node_key"
    )
    .bind(ver_id)
    .fetch_all(&pool)
    .await
    .expect("get published nodes");

    // Create a new DRAFT version
    let draft_ver_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema) VALUES ($1, $2, 400, 'DRAFT', '{}'::jsonb)",
    )
    .bind(draft_ver_id)
    .bind(def_id)
    .execute(&pool)
    .await
    .expect("insert draft version");

    // Insert a transition into the DRAFT version (referencing published nodes for the FK)
    let draft_trans_id = uuid::Uuid::new_v4();
    if published_nodes.len() >= 2 {
        sqlx::query(
            "INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect) VALUES ($1, $2, 'draft-trans', 'Draft Trans', $3, $4, 'ADVANCE')",
        )
        .bind(draft_trans_id)
        .bind(draft_ver_id)
        .bind(published_nodes[0].0)
        .bind(published_nodes[1].0)
        .execute(&pool)
        .await
        .expect("insert draft transition");

        // Try to move transition from DRAFT to PUBLISHED
        let result = sqlx::query(
            "UPDATE workflow_transition_definitions SET definition_version_id = $1 WHERE transition_id = $2",
        )
        .bind(ver_id)
        .bind(draft_trans_id)
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "DRAFT → PUBLISHED transition move must be rejected"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("graph_immutable"),
            "error must mention graph immutability: {}",
            err_msg
        );
    }
}

#[tokio::test]
async fn test_draft_to_draft_node_move_allowed() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    // Insert a node in the existing DRAFT version (ver_id from seed is DRAFT)
    let draft_node_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'movable', 'Movable', 10, 'DRAFT', 'WORKFLOW_CREATOR')",
    )
    .bind(draft_node_id)
    .bind(ver_id)
    .execute(&pool)
    .await
    .expect("insert node in draft version");

    // Create another DRAFT version
    let draft_ver2_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema) VALUES ($1, $2, 500, 'DRAFT', '{}'::jsonb)",
    )
    .bind(draft_ver2_id)
    .bind(def_id)
    .execute(&pool)
    .await
    .expect("insert second draft version");

    // Moving DRAFT → DRAFT must be allowed
    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET definition_version_id = $1 WHERE node_id = $2",
    )
    .bind(draft_ver2_id)
    .bind(draft_node_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_ok(),
        "DRAFT → DRAFT node move must succeed: {:?}",
        result.err()
    );
}

// ---------------------------------------------------------------------------
// Helper: publish a minimal valid graph on the given version
// ---------------------------------------------------------------------------

async fn publish_version_simple(
    pool: &sqlx::PgPool,
    _principal_id: uuid::Uuid,
    ver_id: uuid::Uuid,
    _def_id: uuid::Uuid,
) {
    // Clean old nodes/transitions
    sqlx::query("DELETE FROM workflow_transition_definitions WHERE definition_version_id = $1")
        .bind(ver_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM workflow_node_definitions WHERE definition_version_id = $1")
        .bind(ver_id)
        .execute(pool)
        .await
        .ok();

    // Insert minimal valid graph: draft → done
    let n1 = uuid::Uuid::new_v4();
    let n2 = uuid::Uuid::new_v4();
    let t1 = uuid::Uuid::new_v4();

    sqlx::query(
        "INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'draft', 'Draft', 0, 'DRAFT', 'WORKFLOW_CREATOR')",
    ).bind(n1).bind(ver_id).execute(pool).await.expect("insert draft node");

    sqlx::query(
        "INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'done', 'Done', 1, 'TERMINAL', NULL)",
    ).bind(n2).bind(ver_id).execute(pool).await.expect("insert terminal node");

    sqlx::query(
        "INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect) VALUES ($1, $2, 'advance-done', 'Complete', $3, $4, 'ADVANCE')",
    ).bind(t1).bind(ver_id).bind(n1).bind(n2).execute(pool).await.expect("insert transition");

    sqlx::query("UPDATE workflow_node_definitions SET primary_advance_transition_id = $1 WHERE node_id = $2")
        .bind(t1).bind(n1).execute(pool).await.expect("update primary");

    // Publish
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED', definition_digest = '0000000000000000000000000000000000000000000000000000000000000000', published_at = now(), updated_at = now() WHERE definition_version_id = $1",
    ).bind(ver_id).execute(pool).await.expect("publish version");
}
