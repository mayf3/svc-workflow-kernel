//! H-4: Lifecycle actor fields integration tests.

use super::*;
use svc_workflow::application::definition::commands::{
    DeprecateVersion, PublishVersion, RevokeVersion,
};

#[tokio::test]
async fn test_publish_sets_actor() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let published = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("publish should succeed");

    assert!(
        published.published_at.is_some(),
        "published_at should be set"
    );
    assert_eq!(
        published.published_by_principal_id.map(|id| id.into_uuid()),
        Some(owner),
        "published_by_principal_id should match actor"
    );
}

#[tokio::test]
async fn test_deprecate_sets_actor() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("publish");

    let deprecated = service
        .deprecate_version(DeprecateVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("deprecate");

    assert!(
        deprecated.deprecated_at.is_some(),
        "deprecated_at should be set"
    );
    assert_eq!(
        deprecated
            .deprecated_by_principal_id
            .map(|id| id.into_uuid()),
        Some(owner),
        "deprecated_by_principal_id should match actor"
    );
}

#[tokio::test]
async fn test_revoke_sets_actor() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("publish");

    let revoked = service
        .revoke_version(RevokeVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("revoke");

    assert!(revoked.revoked_at.is_some(), "revoked_at should be set");
    assert_eq!(
        revoked.revoked_by_principal_id.map(|id| id.into_uuid()),
        Some(owner),
        "revoked_by_principal_id should match actor"
    );
}

#[tokio::test]
async fn test_three_stage_actors_all_preserved() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("publish");

    service
        .deprecate_version(DeprecateVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("deprecate");

    service
        .revoke_version(RevokeVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await
        .expect("revoke");

    let row: (Option<uuid::Uuid>, Option<uuid::Uuid>, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("get version");

    assert_eq!(row.0, Some(owner), "published_by should be preserved");
    assert_eq!(row.1, Some(owner), "deprecated_by should be preserved");
    assert_eq!(row.2, Some(owner), "revoked_by should be preserved");
}

#[tokio::test]
async fn test_unpublished_version_actor_fields_null() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    let row: (Option<uuid::Uuid>, Option<uuid::Uuid>, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT published_by_principal_id, deprecated_by_principal_id, revoked_by_principal_id FROM workflow_definition_versions WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("get version");

    assert!(row.0.is_none(), "published_by should be null for draft");
    assert!(row.1.is_none(), "deprecated_by should be null for draft");
    assert!(row.2.is_none(), "revoked_by should be null for draft");
}
