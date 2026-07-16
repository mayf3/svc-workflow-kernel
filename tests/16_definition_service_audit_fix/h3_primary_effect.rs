//! H-3: Primary transition must be ADVANCE integration tests.

use super::*;
use svc_workflow::application::definition::commands::PublishVersion;
use svc_workflow::domain::definition::error::DefinitionError;

#[tokio::test]
async fn test_primary_effect_not_advance_rejected() {
    let (pool, service) = create_service().await;
    let (owner, domain_id) = seed_principal_and_domain(&pool).await;
    seed_domain_owner(&pool, domain_id, owner).await;
    let assignee = seed_assignee_principal(&pool).await;

    let (_def_id, version_id) =
        create_draft_version_with_graph(&pool, &service, owner, domain_id, assignee).await;

    sqlx::query(
        "UPDATE workflow_transition_definitions SET transition_effect = 'RETURN' WHERE definition_version_id = $1 AND transition_key = 'advance-dev'",
    )
    .bind(version_id)
    .execute(&pool)
    .await
    .expect("update transition effect");

    let result = service
        .publish_version(PublishVersion {
            actor_principal_id: owner,
            definition_version_id: version_id,
        })
        .await;
    assert!(
        result.is_err(),
        "primary with RETURN effect should be rejected"
    );
    match result.unwrap_err() {
        DefinitionError::GraphValidationFailed(errors) => {
            assert!(
                errors.iter().any(|e| e.code == "PRIMARY_NOT_ADVANCE"),
                "expected PRIMARY_NOT_ADVANCE, got: {:?}",
                errors
            );
        }
        other => panic!("expected GraphValidationFailed, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_primary_advance_allowed() {
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
        "valid ADVANCE primary should be allowed, got: {:?}",
        result.err()
    );
}
