//! H-2: Assignee rules integration tests.

use super::*;
use svc_workflow::application::definition::commands::{
    CreateDraftVersion, PublishVersion, ReplaceDraftGraph,
};
use svc_workflow::domain::definition::error::DefinitionError;

#[tokio::test]
async fn test_terminal_node_with_fixed_principal_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let result = sqlx::query(
        "UPDATE workflow_node_definitions SET assignee_ref_type = 'FIXED_PRINCIPAL', fixed_principal_id = $1 WHERE definition_version_id = $2 AND node_type = 'TERMINAL'",
    )
    .bind(assignee)
    .bind(version_id)
    .execute(&pool)
    .await
    ;
    assert!(
        result.is_err(),
        "database must reject a new Terminal assignee"
    );
}

#[tokio::test]
async fn test_non_terminal_without_assignee_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let version_id = {
        let def_id = uuid::Uuid::new_v4();
        let def_key = format!("test-def-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        sqlx::query(
            "INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1, $2, $3, 'Test Definition')",
        )
        .bind(def_id)
        .bind(domain_id)
        .bind(&def_key)
        .execute(&pool)
        .await
        .expect("failed to insert definition");

        let create_cmd = CreateDraftVersion {
            actor_principal_id: owner,
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
        version.id.into_uuid()
    };

    let (mut raw_nodes, raw_transitions) = valid_raw_graph();
    raw_nodes[2].assignee_ref_type = Some("FIXED_PRINCIPAL".to_string());
    raw_nodes[2].fixed_principal_id = Some(assignee);
    raw_nodes[2].primary_advance_transition_key = None;

    let replace_cmd = ReplaceDraftGraph {
        actor_principal_id: owner,
        definition_version_id: version_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        nodes: raw_nodes,
        transitions: raw_transitions,
    };
    let result = service.replace_draft_graph(replace_cmd).await;
    assert!(
        result.is_err(),
        "terminal with FIXED_PRINCIPAL should be rejected"
    );
    match result.unwrap_err() {
        DefinitionError::FixedPrincipalInvalid(_) => {}
        DefinitionError::GraphValidationFailed(errors) => {
            assert!(
                errors.iter().any(|e| e.code == "TERMINAL_HAS_ASSIGNEE"),
                "expected TERMINAL_HAS_ASSIGNEE, got: {:?}",
                errors
            );
        }
        other => panic!(
            "expected FixedPrincipalInvalid or GraphValidationFailed, got: {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_workflow_creator_with_fixed_id_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) = {
        let def_id = uuid::Uuid::new_v4();
        let def_key = format!("test-def-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        sqlx::query(
            "INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1, $2, $3, 'Test Definition')",
        )
        .bind(def_id)
        .bind(domain_id)
        .bind(&def_key)
        .execute(&pool)
        .await
        .expect("failed to insert definition");

        let create_cmd = CreateDraftVersion {
            actor_principal_id: owner,
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
        (def_id, version.id.into_uuid())
    };

    let (mut raw_nodes, raw_transitions) = valid_raw_graph();
    raw_nodes[0].fixed_principal_id = Some(assignee);

    let replace_cmd = ReplaceDraftGraph {
        actor_principal_id: owner,
        definition_version_id: version_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        nodes: raw_nodes,
        transitions: raw_transitions,
    };
    let result = service.replace_draft_graph(replace_cmd).await;
    assert!(
        result.is_err(),
        "WORKFLOW_CREATOR with fixed ID should be rejected"
    );
    match result.unwrap_err() {
        DefinitionError::FixedPrincipalInvalid(_) => {}
        DefinitionError::GraphValidationFailed(errors) => {
            assert!(
                errors
                    .iter()
                    .any(|e| e.code == "UNEXPECTED_FIXED_PRINCIPAL"),
                "expected UNEXPECTED_FIXED_PRINCIPAL, got: {:?}",
                errors
            );
        }
        other => panic!(
            "expected FixedPrincipalInvalid or GraphValidationFailed, got: {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_fixed_principal_missing_id_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;

    let (_def_id, version_id) = {
        let def_id = uuid::Uuid::new_v4();
        let def_key = format!("test-def-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        sqlx::query(
            "INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1, $2, $3, 'Test Definition')",
        )
        .bind(def_id)
        .bind(domain_id)
        .bind(&def_key)
        .execute(&pool)
        .await
        .expect("failed to insert definition");

        let create_cmd = CreateDraftVersion {
            actor_principal_id: owner,
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
        (def_id, version.id.into_uuid())
    };

    let (mut raw_nodes, raw_transitions) = valid_raw_graph();
    raw_nodes[1].assignee_ref_type = Some("FIXED_PRINCIPAL".to_string());
    raw_nodes[1].fixed_principal_id = None;

    let replace_cmd = ReplaceDraftGraph {
        actor_principal_id: owner,
        definition_version_id: version_id,
        context_schema: Some(serde_json::json!({"type": "object"})),
        nodes: raw_nodes,
        transitions: raw_transitions,
    };
    let result = service.replace_draft_graph(replace_cmd).await;
    assert!(
        result.is_err(),
        "FIXED_PRINCIPAL without ID should be rejected"
    );
    match result.unwrap_err() {
        DefinitionError::FixedPrincipalInvalid(_) => {}
        DefinitionError::GraphValidationFailed(errors) => {
            assert!(
                errors
                    .iter()
                    .any(|e| e.code == "FIXED_PRINCIPAL_MISSING_ID"),
                "expected FIXED_PRINCIPAL_MISSING_ID, got: {:?}",
                errors
            );
        }
        other => panic!(
            "expected FixedPrincipalInvalid or GraphValidationFailed, got: {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_fixed_principal_disabled_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let disabled = seed_disabled_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, disabled).await;

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(result.is_err(), "disabled principal should be rejected");
    match result.unwrap_err() {
        DefinitionError::FixedPrincipalInvalid(_) => {}
        other => panic!("expected FixedPrincipalInvalid, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_terminal_without_assignee_allowed() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(
        result.is_ok(),
        "terminal without assignee should be allowed, got: {:?}",
        result.err()
    );
}
