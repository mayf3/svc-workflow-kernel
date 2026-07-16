//! Integration tests for the Definition Application Service.
//!
//! Requires a running PostgreSQL 16 instance with the `svc_workflow` database.
//! Run with: `cargo test -- --test-threads=1` or `cargo test`

#![allow(unused_imports, unused_variables)]

mod common;

use common::{
    create_pool, seed_domain_owner, seed_principal_and_domain, seed_principal_domain_with_owner,
    seed_second_principal, seed_workflow_definition,
};

use svc_workflow::application::definition::commands::{
    CreateDefinition, CreateDraftVersion, DeprecateVersion, PublishVersion, RawNodeDefinition,
    RawTransitionDefinition, ReplaceDraftGraph, RevokeVersion,
};
use svc_workflow::application::definition::queries::{
    GetCompleteVersionGraph, GetDefinition, GetDefinitionVersion, ListDefinitionVersions,
};
use svc_workflow::application::definition::DefinitionService;
use svc_workflow::domain::definition::error::DefinitionError;
use svc_workflow::store::postgres::definition_repository::PgDefinitionRepository;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_definition() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    let cmd = CreateDefinition {
        actor_principal_id: principal_id,
        owner_domain_id: domain_id,
        definition_key: "test-flow".to_string(),
        display_name: "Test Flow".to_string(),
        description: None,
        metadata: None,
    };

    let def = service
        .create_definition(cmd)
        .await
        .expect("should create definition");
    assert_eq!(def.definition_key, "test-flow");
    assert_eq!(def.display_name, "Test Flow");
    assert_eq!(def.domain_id.into_uuid(), domain_id);
}

#[tokio::test]
async fn test_definition_key_unique_within_domain() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    let cmd = CreateDefinition {
        actor_principal_id: principal_id,
        owner_domain_id: domain_id,
        definition_key: "unique-key".to_string(),
        display_name: "Test".to_string(),
        description: None,
        metadata: None,
    };
    service
        .create_definition(cmd)
        .await
        .expect("first creation should succeed");

    let cmd2 = CreateDefinition {
        actor_principal_id: principal_id,
        owner_domain_id: domain_id,
        definition_key: "unique-key".to_string(),
        display_name: "Test 2".to_string(),
        description: None,
        metadata: None,
    };
    let err = service
        .create_definition(cmd2)
        .await
        .expect_err("duplicate key should fail");
    assert!(matches!(err, DefinitionError::DefinitionKeyConflict));
}

#[tokio::test]
async fn test_create_draft_version() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, _, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    let cmd = CreateDraftVersion {
        actor_principal_id: principal_id,
        workflow_definition_id: def_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        json_schema_dialect: None,
        validator_version: None,
        metadata: None,
    };

    let version = service
        .create_draft_version(cmd)
        .await
        .expect("should create draft version");
    assert!(version.version_number >= 1);
    assert_eq!(version.version_status.to_string(), "DRAFT");
}

#[tokio::test]
async fn test_concurrent_draft_version_numbers() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, _, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    // Create first version (version 2, since seed creates version 1)
    let cmd1 = CreateDraftVersion {
        actor_principal_id: principal_id,
        workflow_definition_id: def_id,
        context_schema: None,
        json_schema_dialect: None,
        validator_version: None,
        metadata: None,
    };
    let v1 = service
        .create_draft_version(cmd1)
        .await
        .expect("first version");
    assert_eq!(v1.version_number, 2);

    // Create second version (should be version 3)
    let cmd2 = CreateDraftVersion {
        actor_principal_id: principal_id,
        workflow_definition_id: def_id,
        context_schema: None,
        json_schema_dialect: None,
        validator_version: None,
        metadata: None,
    };
    let v2 = service
        .create_draft_version(cmd2)
        .await
        .expect("second version");
    assert_eq!(v2.version_number, 3);
}

#[tokio::test]
async fn test_replace_draft_graph_atomic() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    let cmd = ReplaceDraftGraph {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        nodes: vec![
            RawNodeDefinition {
                node_key: "draft".to_string(),
                display_name: "Draft".to_string(),
                order_index: 0,
                node_type: "DRAFT".to_string(),
                assignee_ref_type: Some("WORKFLOW_CREATOR".to_string()),
                fixed_principal_id: None,
                instructions: None,
                primary_advance_transition_key: Some("advance-review".to_string()),
                metadata: None,
            },
            RawNodeDefinition {
                node_key: "review".to_string(),
                display_name: "Review".to_string(),
                order_index: 1,
                node_type: "NORMAL".to_string(),
                assignee_ref_type: Some("FIXED_PRINCIPAL".to_string()),
                fixed_principal_id: Some(principal_id),
                instructions: Some("Review the work".to_string()),
                primary_advance_transition_key: Some("advance-done".to_string()),
                metadata: None,
            },
            RawNodeDefinition {
                node_key: "done".to_string(),
                display_name: "Done".to_string(),
                order_index: 2,
                node_type: "TERMINAL".to_string(),
                assignee_ref_type: None,
                fixed_principal_id: None,
                instructions: None,
                primary_advance_transition_key: None,
                metadata: None,
            },
        ],
        transitions: vec![
            RawTransitionDefinition {
                transition_key: "advance-review".to_string(),
                display_name: "Advance to Review".to_string(),
                source_node_key: "draft".to_string(),
                target_node_key: "review".to_string(),
                transition_effect: "ADVANCE".to_string(),
                submission_schema: None,
                metadata: None,
            },
            RawTransitionDefinition {
                transition_key: "advance-done".to_string(),
                display_name: "Complete".to_string(),
                source_node_key: "review".to_string(),
                target_node_key: "done".to_string(),
                transition_effect: "ADVANCE".to_string(),
                submission_schema: None,
                metadata: None,
            },
        ],
    };

    service
        .replace_draft_graph(cmd)
        .await
        .expect("should replace graph");

    // Verify graph was stored
    let graph_query = GetCompleteVersionGraph {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
    };
    let graph_result = service
        .get_complete_version_graph(graph_query)
        .await
        .expect("should get graph");
    assert_eq!(graph_result.graph.nodes.len(), 3);
    assert_eq!(graph_result.graph.transitions.len(), 2);
}

#[tokio::test]
async fn test_non_draft_version_rejects_replace() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    // Try to replace on a version that doesn't exist
    let fake_id = uuid::Uuid::new_v4();
    let cmd = ReplaceDraftGraph {
        actor_principal_id: principal_id,
        definition_version_id: fake_id,
        context_schema: None,
        nodes: vec![],
        transitions: vec![],
    };

    let err = service
        .replace_draft_graph(cmd)
        .await
        .expect_err("non-existent version should fail");
    assert!(matches!(err, DefinitionError::DefinitionVersionNotFound));
}

#[tokio::test]
async fn test_valid_template_can_publish() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    seed_minimal_and_publish(&service, principal_id, ver_id).await;
}

#[tokio::test]
async fn test_publish_persists_digest() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    seed_minimal_and_publish(&service, principal_id, ver_id).await;

    // Re-read and verify digest is persisted
    let get_cmd = GetDefinitionVersion {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
    };
    let read_back = service
        .get_definition_version(get_cmd)
        .await
        .expect("read back");
    let digest = read_back.version.version.unwrap().definition_digest;
    assert!(digest.is_some(), "digest should be persisted");
    assert_eq!(digest.as_ref().unwrap().len(), 64);
}

#[tokio::test]
async fn test_published_graph_immutable() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    seed_minimal_and_publish(&service, principal_id, ver_id).await;

    // Try to replace graph again on published version
    let replace_cmd2 = ReplaceDraftGraph {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
        context_schema: None,
        nodes: vec![],
        transitions: vec![],
    };
    let err = service
        .replace_draft_graph(replace_cmd2)
        .await
        .expect_err("should reject replace on published");
    assert!(matches!(err, DefinitionError::VersionNotDraft));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Seed a minimal 2-node graph (draft -> done) and publish it.
async fn seed_minimal_and_publish(
    service: &DefinitionService<PgDefinitionRepository>,
    principal_id: uuid::Uuid,
    ver_id: uuid::Uuid,
) {
    let replace_cmd = ReplaceDraftGraph {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        nodes: vec![
            RawNodeDefinition {
                node_key: "draft".to_string(),
                display_name: "Draft".to_string(),
                order_index: 0,
                node_type: "DRAFT".to_string(),
                assignee_ref_type: Some("WORKFLOW_CREATOR".to_string()),
                fixed_principal_id: None,
                instructions: None,
                primary_advance_transition_key: Some("advance-done".to_string()),
                metadata: None,
            },
            RawNodeDefinition {
                node_key: "done".to_string(),
                display_name: "Done".to_string(),
                order_index: 1,
                node_type: "TERMINAL".to_string(),
                assignee_ref_type: None,
                fixed_principal_id: None,
                instructions: None,
                primary_advance_transition_key: None,
                metadata: None,
            },
        ],
        transitions: vec![RawTransitionDefinition {
            transition_key: "advance-done".to_string(),
            display_name: "Complete".to_string(),
            source_node_key: "draft".to_string(),
            target_node_key: "done".to_string(),
            transition_effect: "ADVANCE".to_string(),
            submission_schema: None,
            metadata: None,
        }],
    };
    service
        .replace_draft_graph(replace_cmd)
        .await
        .expect("replace graph");

    let pub_cmd = PublishVersion {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
    };
    service.publish_version(pub_cmd).await.expect("publish");
}
