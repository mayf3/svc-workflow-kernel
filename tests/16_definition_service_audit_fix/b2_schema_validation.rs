//! B-2: JSON Schema validation integration tests.

use super::*;
use svc_workflow::application::definition::commands::PublishVersion;
use svc_workflow::domain::definition::error::DefinitionError;

#[tokio::test]
async fn test_context_schema_patch_keeps_clears_and_replaces() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let replace = |context_schema| {
        let (nodes, transitions) = valid_raw_graph_with_principal(assignee);
        ReplaceDraftGraph {
            actor_principal_id: owner,
            definition_version_id: version_id,
            context_schema,
            nodes,
            transitions,
        }
    };

    service
        .replace_draft_graph(replace(None))
        .await
        .expect("None should preserve context_schema");
    let kept: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT context_schema FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("read preserved context_schema");
    assert_eq!(kept, Some(serde_json::json!({"type": "object"})));

    service
        .replace_draft_graph(replace(Some(serde_json::Value::Null)))
        .await
        .expect("JSON null should clear context_schema");
    let cleared: bool = sqlx::query_scalar(
        "SELECT context_schema IS NULL FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("read cleared context_schema");
    assert!(cleared, "JSON null must be stored as SQL NULL");

    let replacement = serde_json::json!({"type": "string", "minLength": 1});
    service
        .replace_draft_graph(replace(Some(replacement.clone())))
        .await
        .expect("object should replace context_schema");
    let replaced: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT context_schema FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("read replaced context_schema");
    assert_eq!(replaced, Some(replacement));
}

#[tokio::test]
async fn test_valid_context_schema_can_publish() {
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
        "valid context_schema should publish, got: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_invalid_schema_rejected_during_publish() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    sqlx::query("UPDATE workflow_definition_versions SET context_schema = '{\"type\": 123}'::jsonb WHERE definition_version_id = $1")
        .bind(version_id)
        .execute(&pool)
        .await
        .expect("update context schema");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(result.is_err(), "invalid schema should be rejected");
    match result.unwrap_err() {
        DefinitionError::GraphValidationFailed(errors) => {
            assert!(
                errors.iter().any(|e| e.code == "INVALID_CONTEXT_SCHEMA"),
                "expected INVALID_CONTEXT_SCHEMA error, got: {:?}",
                errors
            );
        }
        other => panic!("expected GraphValidationFailed, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_https_ref_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let https_schema = serde_json::json!({"$ref": "https://example.com/schema.json"});
    let trans_id: (uuid::Uuid,) = sqlx::query_as(
        "SELECT transition_id FROM workflow_transition_definitions WHERE definition_version_id = $1 LIMIT 1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("get transition");

    sqlx::query("UPDATE workflow_transition_definitions SET submission_schema = $1 WHERE transition_id = $2")
        .bind(&https_schema)
        .bind(trans_id.0)
        .execute(&pool)
        .await
        .expect("update submission_schema");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(result.is_err(), "https ref should be rejected");
    let err = result.unwrap_err();
    assert!(
        matches!(err, DefinitionError::GraphValidationFailed(_)),
        "expected GraphValidationFailed, got: {:?}",
        err
    );
}

#[tokio::test]
async fn test_file_ref_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let file_schema = serde_json::json!({"$ref": "file:///etc/passwd"});
    let trans_id: (uuid::Uuid,) = sqlx::query_as(
        "SELECT transition_id FROM workflow_transition_definitions WHERE definition_version_id = $1 LIMIT 1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("get transition");

    sqlx::query("UPDATE workflow_transition_definitions SET submission_schema = $1 WHERE transition_id = $2")
        .bind(&file_schema)
        .bind(trans_id.0)
        .execute(&pool)
        .await
        .expect("update submission_schema");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(result.is_err(), "file ref should be rejected");
}

#[tokio::test]
async fn test_local_fragment_ref_allowed() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let local_schema = serde_json::json!({
        "$defs": {
            "Address": {"type": "object"}
        },
        "$ref": "#/$defs/Address"
    });

    sqlx::query("UPDATE workflow_definition_versions SET context_schema = $1 WHERE definition_version_id = $2")
        .bind(&local_schema)
        .bind(version_id)
        .execute(&pool)
        .await
        .expect("update context_schema");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(
        result.is_ok(),
        "local fragment ref should be allowed, got: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_invalid_schema_version_stays_draft() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    sqlx::query("UPDATE workflow_definition_versions SET context_schema = '{\"type\": 123}'::jsonb WHERE definition_version_id = $1")
        .bind(version_id)
        .execute(&pool)
        .await
        .expect("update context_schema");

    let _ = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;

    let status: (String,) = sqlx::query_as(
        "SELECT version_status::TEXT FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("get status");

    assert_eq!(
        status.0, "DRAFT",
        "version should remain DRAFT after failed publish"
    );

    let row: (Option<String>, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT definition_digest, published_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("get version");

    assert!(
        row.0.is_none(),
        "digest should not be set on failed publish"
    );
    assert!(
        row.1.is_none(),
        "published_by should not be set on failed publish"
    );
}
