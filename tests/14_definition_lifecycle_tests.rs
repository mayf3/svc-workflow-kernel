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
async fn test_published_to_deprecated() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);
    seed_minimal_and_publish(&service, principal_id, ver_id).await;

    let dep_cmd = DeprecateVersion {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
    };
    let deprecated = service
        .deprecate_version(dep_cmd)
        .await
        .expect("should deprecate");
    assert_eq!(deprecated.version_status.to_string(), "DEPRECATED");
}

#[tokio::test]
async fn test_published_to_revoked() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);
    seed_minimal_and_publish(&service, principal_id, ver_id).await;

    let rev_cmd = RevokeVersion {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
    };
    let revoked = service
        .revoke_version(rev_cmd)
        .await
        .expect("should revoke");
    assert_eq!(revoked.version_status.to_string(), "REVOKED");
}

#[tokio::test]
async fn test_deprecated_to_revoked() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);
    seed_minimal_and_publish(&service, principal_id, ver_id).await;

    service
        .deprecate_version(DeprecateVersion {
            actor_principal_id: principal_id,
            definition_version_id: ver_id,
        })
        .await
        .expect("deprecate");

    let revoked = service
        .revoke_version(RevokeVersion {
            actor_principal_id: principal_id,
            definition_version_id: ver_id,
        })
        .await
        .expect("revoke from deprecated");
    assert_eq!(revoked.version_status.to_string(), "REVOKED");
}

#[tokio::test]
async fn test_invalid_lifecycle_transition() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    // Try to deprecate a DRAFT version
    let dep_cmd = DeprecateVersion {
        actor_principal_id: principal_id,
        definition_version_id: ver_id,
    };
    let err = service
        .deprecate_version(dep_cmd)
        .await
        .expect_err("should reject deprecate on draft");
    assert!(matches!(err, DefinitionError::InvalidLifecycleTransition));
}

#[tokio::test]
async fn test_non_domain_owner_cannot_manage() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_and_domain(&pool).await;
    let stranger_id = seed_second_principal(&pool).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    let cmd = CreateDefinition {
        actor_principal_id: stranger_id,
        owner_domain_id: domain_id,
        definition_key: "unauthorized".to_string(),
        display_name: "Unauthorized".to_string(),
        description: None,
        metadata: None,
    };
    let err = service
        .create_definition(cmd)
        .await
        .expect_err("stranger should be denied");
    assert!(matches!(err, DefinitionError::PermissionDenied));
}

#[tokio::test]
async fn test_get_definition_and_versions() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (def_id, ver_id, _, _) = seed_workflow_definition(&pool, domain_id).await;

    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);

    // Get definition
    let def_query = GetDefinition {
        actor_principal_id: principal_id,
        workflow_definition_id: def_id,
    };
    let def_result = service
        .get_definition(def_query)
        .await
        .expect("get definition");
    assert!(def_result
        .definition
        .definition
        .definition_key
        .starts_with("test-def-"));

    // List versions
    let list_query = ListDefinitionVersions {
        actor_principal_id: principal_id,
        workflow_definition_id: def_id,
    };
    let list_result = service
        .list_definition_versions(list_query)
        .await
        .expect("list versions");
    assert!(
        !list_result.versions.is_empty(),
        "should have at least one version"
    );
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
