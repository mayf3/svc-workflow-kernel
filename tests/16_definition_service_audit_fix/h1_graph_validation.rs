//! H-1: Directed reachability integration tests.

use super::*;
use svc_workflow::application::definition::commands::PublishVersion;
use svc_workflow::domain::definition::error::DefinitionError;

#[tokio::test]
async fn test_directed_unreachable_node_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let isolated_node_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'isolated', 'Isolated', 10, 'NORMAL', 'WORKFLOW_CREATOR')",
    )
    .bind(isolated_node_id)
    .bind(version_id)
    .execute(&pool)
    .await
    .expect("insert isolated node");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(result.is_err(), "isolated node should be rejected");
    match result.unwrap_err() {
        DefinitionError::GraphValidationFailed(errors) => {
            assert!(
                errors.iter().any(|e| e.code == "NODE_NOT_REACHABLE"),
                "expected NODE_NOT_REACHABLE, got: {:?}",
                errors
            );
            assert!(
                errors.iter().any(|e| e.message.contains("isolated")),
                "error should mention 'isolated', got: {:?}",
                errors
            );
        }
        other => panic!("expected GraphValidationFailed, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_node_only_reachable_via_backwards_edge_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let orphan_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'orphan', 'Orphan', 5, 'NORMAL', 'WORKFLOW_CREATOR')",
    )
    .bind(orphan_id)
    .bind(version_id)
    .execute(&pool)
    .await
    .expect("insert orphan node");

    let draft_node_id: (uuid::Uuid,) = sqlx::query_as(
        "SELECT node_id FROM workflow_node_definitions WHERE definition_version_id = $1 AND node_key = 'draft'",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("get draft node");

    let rev_trans_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect) VALUES ($1, $2, 'orphan-to-draft', 'Reverse', $3, $4, 'RETURN')",
    )
    .bind(rev_trans_id)
    .bind(version_id)
    .bind(orphan_id)
    .bind(draft_node_id.0)
    .execute(&pool)
    .await
    .expect("insert reverse transition");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(
        result.is_err(),
        "orphan (only reverse edge) should be rejected"
    );
    match result.unwrap_err() {
        DefinitionError::GraphValidationFailed(errors) => {
            assert!(
                errors.iter().any(|e| e.code == "NODE_NOT_REACHABLE"),
                "expected NODE_NOT_REACHABLE, got: {:?}",
                errors
            );
        }
        other => panic!("expected GraphValidationFailed, got: {:?}", other),
    }
}
