use super::*;

use svc_workflow::domain::workflow_instance::recovery::RecoveryError;

#[tokio::test]
async fn only_enabled_non_service_workflow_admin_is_authorized() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;

    let mut outsider = rebuild_command(&fixture);
    outsider.principal_id = PrincipalId::from_uuid(fixture.outsider);
    let outsider_key = outsider.idempotency_key.clone();
    assert_eq!(
        run_rebuild(&pool, outsider).await.unwrap_err(),
        RecoveryError::PermissionDenied
    );

    let mut owner = rebuild_command(&fixture);
    owner.principal_id = PrincipalId::from_uuid(fixture.creator);
    assert_eq!(
        run_rebuild(&pool, owner).await.unwrap_err(),
        RecoveryError::PermissionDenied,
        "DOMAIN_OWNER must not inherit WORKFLOW_ADMIN"
    );

    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(fixture.admin)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        run_rebuild(&pool, rebuild_command(&fixture))
            .await
            .unwrap_err(),
        RecoveryError::PrincipalDisabled
    );

    let receipt: (String, i32) = sqlx::query_as(
        "SELECT receipt_status::text, response_status FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(fixture.outsider)
    .bind(outsider_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt, ("COMPLETED".to_string(), 403));
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_security_audits
         WHERE principal_id = $1 AND resource_id = $2",
    )
    .bind(fixture.outsider)
    .bind(fixture.instance.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn service_admin_and_disabled_binding_are_rejected() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let service = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals
         (principal_id, principal_type, display_name, enabled)
         VALUES ($1, 'SERVICE', 'Recovery Service', TRUE)",
    )
    .bind(service)
    .execute(&pool)
    .await
    .unwrap();
    bind_workflow_admin(&pool, fixture.domain, service).await;
    let mut command = rebuild_command(&fixture);
    command.principal_id = PrincipalId::from_uuid(service);
    assert_eq!(
        run_rebuild(&pool, command).await.unwrap_err(),
        RecoveryError::PrincipalTypeNotAllowed
    );

    sqlx::query(
        "UPDATE domain_role_bindings SET enabled = FALSE, disabled_at = now()
         WHERE domain_id = $1 AND principal_id = $2 AND role_key = 'WORKFLOW_ADMIN'",
    )
    .bind(fixture.domain)
    .bind(fixture.admin)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        run_rebuild(&pool, rebuild_command(&fixture))
            .await
            .unwrap_err(),
        RecoveryError::PermissionDenied
    );
}

#[tokio::test]
async fn disabled_domain_does_not_block_recovery() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    sqlx::query("UPDATE domains SET enabled = FALSE WHERE domain_id = $1")
        .bind(fixture.domain)
        .execute(&pool)
        .await
        .unwrap();
    let result = run_rebuild(&pool, rebuild_command(&fixture)).await.unwrap();
    assert!(!result.changed);
}

#[tokio::test]
async fn missing_principal_is_rejected_before_receipt_boundary() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let missing = Uuid::new_v4();
    let mut command = rebuild_command(&fixture);
    command.principal_id = PrincipalId::from_uuid(missing);
    let key = command.idempotency_key.clone();
    assert_eq!(
        run_rebuild(&pool, command).await.unwrap_err(),
        RecoveryError::PrincipalNotFound
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(missing)
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 0);
}

#[tokio::test]
async fn unauthorized_actor_cannot_distinguish_missing_and_existing_instances() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let mut existing = rebuild_command(&fixture);
    existing.principal_id = PrincipalId::from_uuid(fixture.outsider);
    let existing_error = run_rebuild(&pool, existing).await.unwrap_err();
    let mut missing = rebuild_command(&fixture);
    missing.principal_id = PrincipalId::from_uuid(fixture.outsider);
    missing.workflow_instance_id = WorkflowInstanceId::from_uuid(Uuid::new_v4());
    let missing_error = run_rebuild(&pool, missing).await.unwrap_err();
    assert_eq!(existing_error, RecoveryError::PermissionDenied);
    assert_eq!(missing_error, existing_error);
}
