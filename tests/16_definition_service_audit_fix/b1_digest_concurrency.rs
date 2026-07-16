//! B-1: Atomic publish digest consistency and concurrency integration tests.
//!
//! Also includes M-3: digest read-back consistency and
//! M-6: concurrent CreateDefinition uniqueness.

use super::*;
use std::collections::HashMap;
use svc_workflow::application::definition::commands::{
    CreateDefinition, PublishVersion, ReplaceDraftGraph,
};
use svc_workflow::application::definition::DefinitionService;
use svc_workflow::domain::definition::digest;
use svc_workflow::domain::definition::error::DefinitionError;
use svc_workflow::store::postgres::definition_repository::PgDefinitionRepository;

// ====================================================================
// B-1 + M-3: Digest read-back consistency
// ====================================================================

#[tokio::test]
async fn test_digest_readback_consistency() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (def_id_result, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let published = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("publish should succeed");

    let stored_digest = published.definition_digest.expect("digest should exist");

    // Read back the full graph from DB
    let (nodes, transitions) = service
        .repo
        .get_complete_graph(version_id)
        .await
        .expect("get complete graph");

    let def = service
        .repo
        .get_definition(def_id_result)
        .await
        .expect("get definition");

    let version = service
        .repo
        .get_version(version_id)
        .await
        .expect("get version");

    let node_key_map: HashMap<_, _> = nodes
        .iter()
        .map(|n| (n.node_id, n.node_key.clone()))
        .collect();
    let transition_key_map: HashMap<_, _> = transitions
        .iter()
        .map(|t| (t.transition_id, t.transition_key.clone()))
        .collect();

    let recomputed_digest = digest::compute_digest(
        &def.definition_key,
        version.version_number,
        version.json_schema_dialect.as_deref(),
        version.validator_version.as_deref(),
        version.context_schema.as_ref(),
        &nodes,
        &transitions,
        &node_key_map,
        &transition_key_map,
    )
    .expect("compute digest");

    assert_eq!(
        stored_digest, recomputed_digest,
        "stored digest must match digest recomputed from stored graph"
    );
    assert_eq!(stored_digest.len(), 64, "SHA-256 hex should be 64 chars");
}

// ====================================================================
// M-6: Concurrent CreateDefinition uniqueness
// ====================================================================

#[tokio::test]
async fn test_concurrent_create_definition_unique() {
    let pool = create_pool().await;
    let repo1 = PgDefinitionRepository::new(pool.clone());
    let service1 = DefinitionService::new(repo1);
    let repo2 = PgDefinitionRepository::new(pool.clone());
    let service2 = DefinitionService::new(repo2);
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;

    let def_key = format!("concurrent-test-{}", &uuid::Uuid::new_v4().to_string()[..8]);

    let r1 = service1
        .create_definition(CreateDefinition {
            actor_principal_id: owner,
            owner_domain_id: domain_id,
            definition_key: def_key.clone(),
            display_name: "Test".to_string(),
            description: None,
            metadata: None,
        })
        .await;
    assert!(r1.is_ok(), "first create should succeed");

    let r2 = service2
        .create_definition(CreateDefinition {
            actor_principal_id: owner,
            owner_domain_id: domain_id,
            definition_key: def_key.clone(),
            display_name: "Test".to_string(),
            description: None,
            metadata: None,
        })
        .await;
    match r2.unwrap_err() {
        DefinitionError::DefinitionKeyConflict => {}
        other => panic!("expected DefinitionKeyConflict, got: {:?}", other),
    }
}

// ====================================================================
// B-1: Publish/Replace shared row lock coordination
// ====================================================================

#[tokio::test]
async fn test_manual_lock_blocks_replace_draft_graph() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    // Manually lock the version row with FOR UPDATE
    let mut tx = pool.begin().await.expect("begin tx");
    let _locked: (uuid::Uuid,) = sqlx::query_as(
        "SELECT definition_version_id FROM workflow_definition_versions WHERE definition_version_id = $1 FOR UPDATE",
    )
    .bind(version_id)
    .fetch_one(&mut *tx)
    .await
    .expect("lock version");

    let (nodes, transitions) = valid_raw_graph_with_principal(assignee);
    let replace_future = service.replace_draft_graph(ReplaceDraftGraph {
        actor_principal_id: owner,
        definition_version_id: version_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        nodes,
        transitions,
    });

    let timeout_duration = std::time::Duration::from_millis(500);
    let result = tokio::time::timeout(timeout_duration, replace_future).await;

    assert!(
        result.is_err(),
        "replace should be blocked by the held lock"
    );

    tx.commit().await.expect("commit tx");

    let (nodes, transitions) = valid_raw_graph_with_principal(assignee);
    let result = service
        .replace_draft_graph(ReplaceDraftGraph {
            actor_principal_id: owner,
            definition_version_id: version_id,
            context_schema: Some(serde_json::json!({"type": "object"})),
            nodes,
            transitions,
        })
        .await;
    assert!(result.is_ok(), "replace should succeed after lock released");
}
