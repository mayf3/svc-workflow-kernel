//! Comprehensive tests for Definition Service audit fixes (B-1, B-2, H-1..H-5, M-1..M-6).
//!
//! Each test module corresponds to a specific audit finding.
//! Tests use real PostgreSQL 16 and the same connection helpers as other integration tests.
#![allow(clippy::needless_borrow)]

#[path = "common/mod.rs"]
mod common;

use common::*;
use sqlx::PgPool;

use svc_workflow::application::definition::commands::{
    CreateDraftVersion, RawNodeDefinition, RawTransitionDefinition, ReplaceDraftGraph,
};
use svc_workflow::application::definition::DefinitionRepository;
use svc_workflow::application::definition::DefinitionService;
use svc_workflow::store::postgres::definition_repository::PgDefinitionRepository;

// ---------------------------------------------------------------------------
// Helper: create a pool + service backed by the real DB
// ---------------------------------------------------------------------------

pub(crate) async fn create_service() -> (PgPool, DefinitionService<PgDefinitionRepository>) {
    let pool = create_pool().await;
    let repo = PgDefinitionRepository::new(pool.clone());
    let service = DefinitionService::new(repo);
    (pool, service)
}

// ---------------------------------------------------------------------------
// Helper: create a complete valid graph for testing
// ---------------------------------------------------------------------------

pub(crate) fn valid_raw_graph() -> (Vec<RawNodeDefinition>, Vec<RawTransitionDefinition>) {
    let nodes = vec![
        RawNodeDefinition {
            node_key: "draft".to_string(),
            display_name: "Draft".to_string(),
            order_index: 0,
            node_type: "DRAFT".to_string(),
            assignee_ref_type: Some("WORKFLOW_CREATOR".to_string()),
            fixed_principal_id: None,
            instructions: None,
            primary_advance_transition_key: Some("advance-dev".to_string()),
            metadata: None,
        },
        RawNodeDefinition {
            node_key: "dev_self_check".to_string(),
            display_name: "Dev Self Check".to_string(),
            order_index: 1,
            node_type: "NORMAL".to_string(),
            assignee_ref_type: Some("FIXED_PRINCIPAL".to_string()),
            fixed_principal_id: None, // will be set at test time
            instructions: None,
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
    ];
    let transitions = vec![
        RawTransitionDefinition {
            transition_key: "advance-dev".to_string(),
            display_name: "Advance to Dev".to_string(),
            source_node_key: "draft".to_string(),
            target_node_key: "dev_self_check".to_string(),
            transition_effect: "ADVANCE".to_string(),
            submission_schema: None,
            metadata: None,
        },
        RawTransitionDefinition {
            transition_key: "advance-done".to_string(),
            display_name: "Complete".to_string(),
            source_node_key: "dev_self_check".to_string(),
            target_node_key: "done".to_string(),
            transition_effect: "ADVANCE".to_string(),
            submission_schema: None,
            metadata: None,
        },
    ];
    (nodes, transitions)
}

/// Create a valid raw graph with a fixed principal ID for the NORMAL node.
pub(crate) fn valid_raw_graph_with_principal(
    principal_id: uuid::Uuid,
) -> (Vec<RawNodeDefinition>, Vec<RawTransitionDefinition>) {
    let (mut nodes, transitions) = valid_raw_graph();
    nodes[1].fixed_principal_id = Some(principal_id);
    (nodes, transitions)
}

/// Seed an assignee principal and return its ID.
pub(crate) async fn seed_assignee_principal(pool: &PgPool) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, email, enabled) VALUES ($1, 'AGENT', 'Assignee', 'assignee@test.com', TRUE)",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("failed to seed assignee principal");
    id
}

/// Seed a disabled principal.
pub(crate) async fn seed_disabled_principal(pool: &PgPool) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, email, enabled) VALUES ($1, 'AGENT', 'Disabled', 'disabled@test.com', FALSE)",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("failed to seed disabled principal");
    id
}

/// Seed a second domain with its own owner.
pub(crate) async fn seed_second_domain_with_owner(pool: &PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let principal_id = uuid::Uuid::new_v4();
    let domain_id = uuid::Uuid::new_v4();
    let domain_key = format!("second-domain-{}", &uuid::Uuid::new_v4().to_string()[..8]);

    sqlx::query("INSERT INTO principals (principal_id, principal_type, display_name, email, enabled) VALUES ($1, 'HUMAN', 'Second Owner', 'second@test.com', TRUE)")
        .bind(principal_id)
        .execute(pool)
        .await
        .expect("failed to insert second principal");

    sqlx::query("INSERT INTO domains (domain_id, domain_key, display_name, enabled) VALUES ($1, $2, 'Second Domain', TRUE)")
        .bind(domain_id)
        .bind(&domain_key)
        .execute(pool)
        .await
        .expect("failed to insert second domain");

    seed_domain_owner(pool, domain_id, principal_id).await;
    (principal_id, domain_id)
}

/// Create a definition version in DRAFT with a graph set up.
pub(crate) async fn create_draft_version_with_graph(
    pool: &PgPool,
    service: &DefinitionService<PgDefinitionRepository>,
    actor: uuid::Uuid,
    domain_id: uuid::Uuid,
    assignee_id: uuid::Uuid,
) -> (uuid::Uuid, uuid::Uuid) {
    let def_id = uuid::Uuid::new_v4();
    let def_key = format!("test-def-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    sqlx::query(
        "INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1, $2, $3, 'Test Definition')",
    )
    .bind(def_id)
    .bind(domain_id)
    .bind(&def_key)
    .execute(pool)
    .await
    .expect("failed to insert definition");

    let create_cmd = CreateDraftVersion {
        actor_principal_id: actor,
        workflow_definition_id: def_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        json_schema_dialect: Some("https://json-schema.org/draft/2020-12/schema".to_string()),
        validator_version: Some("v1".to_string()),
        metadata: None,
    };
    let version = service
        .create_draft_version(create_cmd)
        .await
        .expect("create draft version");
    let version_id = version.id.into_uuid();

    // Replace graph
    let (mut raw_nodes, raw_transitions) = valid_raw_graph();
    raw_nodes[1].fixed_principal_id = Some(assignee_id);

    let replace_cmd = ReplaceDraftGraph {
        actor_principal_id: actor,
        definition_version_id: version_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        nodes: raw_nodes,
        transitions: raw_transitions,
    };
    service
        .replace_draft_graph(replace_cmd)
        .await
        .expect("replace draft graph");

    (def_id, version_id)
}

// Sub-modules (in 16_definition_service_audit_fix/ directory)
#[path = "16_definition_service_audit_fix/b1_digest_concurrency.rs"]
mod b1_digest_concurrency;
#[path = "16_definition_service_audit_fix/b2_schema_validation.rs"]
mod b2_schema_validation;
#[path = "16_definition_service_audit_fix/h1_graph_validation.rs"]
mod h1_graph_validation;
#[path = "16_definition_service_audit_fix/h2_assignee_rules.rs"]
mod h2_assignee_rules;
#[path = "16_definition_service_audit_fix/h3_primary_effect.rs"]
mod h3_primary_effect;
#[path = "16_definition_service_audit_fix/h4_lifecycle_actors.rs"]
mod h4_lifecycle_actors;
#[path = "16_definition_service_audit_fix/h5_authorization.rs"]
mod h5_authorization;
