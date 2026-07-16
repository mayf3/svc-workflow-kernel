//! H-5: Domain authorization integration tests.

use super::*;
use svc_workflow::application::definition::commands::{PublishVersion, ValidateDraftVersion};
use svc_workflow::application::definition::queries::GetDefinition;
use svc_workflow::application::definition::queries::ListDefinitionVersions;
use svc_workflow::domain::definition::error::DefinitionError;

#[tokio::test]
async fn test_cross_domain_read_denied() {
    let (pool, service) = create_service().await;
    let (owner_a, domain_a) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_a, owner_a).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (def_id, _version_id) =
        create_draft_version_with_graph(&pool, &service, owner_a, domain_a, assignee).await;

    let (owner_b, _domain_b) = seed_second_domain_with_owner(&pool).await;

    let result = service
        .get_definition(GetDefinition {
            actor_principal_id: owner_b,
            workflow_definition_id: def_id,
        })
        .await;
    assert!(result.is_err(), "cross-domain read should be denied");
    match result.unwrap_err() {
        DefinitionError::PermissionDenied => {}
        other => panic!("expected PermissionDenied, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_cross_domain_list_versions_denied() {
    let (pool, service) = create_service().await;
    let (owner_a, domain_a) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_a, owner_a).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (def_id, _version_id) =
        create_draft_version_with_graph(&pool, &service, owner_a, domain_a, assignee).await;

    let (owner_b, _domain_b) = seed_second_domain_with_owner(&pool).await;

    let result = service
        .list_definition_versions(ListDefinitionVersions {
            actor_principal_id: owner_b,
            workflow_definition_id: def_id,
        })
        .await;
    assert!(result.is_err(), "cross-domain list should be denied");
}

#[tokio::test]
async fn test_domain_owner_can_read() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (def_id, _version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let result = service
        .get_definition(GetDefinition {
            actor_principal_id: owner,
            workflow_definition_id: def_id,
        })
        .await;
    assert!(result.is_ok(), "domain owner should be able to read");
}

#[tokio::test]
async fn test_validate_draft_version_requires_owner() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let stranger = seed_second_principal(&pool).await;
    let result = service
        .validate_draft_version(ValidateDraftVersion {
            actor_principal_id: stranger,
            definition_version_id: version_id,
        })
        .await;
    assert!(result.is_err(), "non-owner should not be able to validate");
    match result.unwrap_err() {
        DefinitionError::PermissionDenied => {}
        other => panic!("expected PermissionDenied, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_domain_owner_can_validate() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let result = service
        .validate_draft_version(ValidateDraftVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(result.is_ok(), "domain owner should be able to validate");
}

#[tokio::test]
async fn test_disabled_principal_cannot_read() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (def_id, _version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(owner)
        .execute(&pool)
        .await
        .expect("disable principal");

    let result = service
        .get_definition(GetDefinition {
            actor_principal_id: owner,
            workflow_definition_id: def_id,
        })
        .await;
    match result.unwrap_err() {
        DefinitionError::PrincipalDisabled => {}
        other => panic!("expected PrincipalDisabled, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_disabled_domain_blocks_write() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    sqlx::query("UPDATE domains SET enabled = FALSE WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .expect("disable domain");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    match result.unwrap_err() {
        DefinitionError::DomainDisabled => {}
        other => panic!("expected DomainDisabled, got: {:?}", other),
    }
}
