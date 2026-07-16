use super::*;

use svc_workflow::domain::workflow_instance::recovery::{
    AdminEmergencyOperation, RecoveryError, COMMAND_TYPE_REBUILD_PROJECTION,
};

fn rebuild_hash(
    command: &svc_workflow::domain::workflow_instance::recovery::RebuildProjectionCommand,
) -> String {
    let envelope = serde_json::json!({
        "commandSchemaVersion": command.command_schema_version,
        "commandType": COMMAND_TYPE_REBUILD_PROJECTION,
        "routeParameters": {},
        "requestBody": {
            "principalId": command.principal_id.to_string(),
            "workflowInstanceId": command.workflow_instance_id.to_string(),
            "expectedBeforeSnapshotDigest": command.expected_before_snapshot_digest,
        }
    });
    jcs_canonicalize::sha256_jcs_hex(&envelope).unwrap()
}

async fn disable_admin_binding(pool: &PgPool, fixture: &RecoveryFixture) {
    sqlx::query(
        "UPDATE domain_role_bindings SET enabled = FALSE, disabled_at = now()
         WHERE domain_id = $1 AND principal_id = $2 AND role_key = 'WORKFLOW_ADMIN'",
    )
    .bind(fixture.domain)
    .bind(fixture.admin)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn completed_replay_revalidates_current_actor_and_binding() {
    let pool = create_pool().await;
    let disabled = seed_recovery_fixture(&pool).await;
    let rebuild = rebuild_command(&disabled);
    run_rebuild(&pool, rebuild.clone()).await.unwrap();
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(disabled.admin)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        run_rebuild(&pool, rebuild).await.unwrap_err(),
        RecoveryError::PrincipalDisabled
    );

    let revoked = seed_recovery_fixture(&pool).await;
    let override_command = override_command(
        &revoked,
        AdminEmergencyOperation::MoveToNode,
        revoked.normal,
    );
    let override_key = override_command.idempotency_key.clone();
    let first = run_override(&pool, override_command.clone()).await.unwrap();
    let stored_before: (String, serde_json::Value, String) = sqlx::query_as(
        "SELECT receipt_status::text, response_body, response_digest
         FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(revoked.admin)
    .bind(&override_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    disable_admin_binding(&pool, &revoked).await;
    assert_eq!(
        run_override(&pool, override_command).await.unwrap_err(),
        RecoveryError::PermissionDenied
    );
    assert_eq!(
        count_instance_facts(&pool, revoked.instance).await,
        (1, 2, 0, 2)
    );
    let stored_command: Uuid = sqlx::query_scalar(
        "SELECT command_id FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(revoked.admin)
    .bind(&override_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_command, first.command_id);
    let stored_after: (String, serde_json::Value, String) = sqlx::query_as(
        "SELECT receipt_status::text, response_body, response_digest
         FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(revoked.admin)
    .bind(override_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_after, stored_before);
    let denial_details: serde_json::Value = sqlx::query_scalar(
        "SELECT details FROM workflow_security_audits
         WHERE principal_id = $1 AND resource_id = $2
           AND action = 'ADMIN_EMERGENCY_OVERRIDE_REJECTED'
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(revoked.admin)
    .bind(revoked.instance.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    let denial_text = denial_details.to_string();
    assert!(!denial_text.contains(&first.command_id.to_string()));
    assert!(!denial_text.contains(&stored_before.2));
}

#[tokio::test]
async fn conflict_and_processing_do_not_bypass_revoked_authorization() {
    let pool = create_pool().await;
    let conflict_fixture = seed_recovery_fixture(&pool).await;
    let original = rebuild_command(&conflict_fixture);
    let key = original.idempotency_key.clone();
    run_rebuild(&pool, original).await.unwrap();
    disable_admin_binding(&pool, &conflict_fixture).await;
    let mut conflict = rebuild_command(&conflict_fixture);
    conflict.idempotency_key = key.clone();
    conflict.expected_before_snapshot_digest = Some("0".repeat(64));
    assert_eq!(
        run_rebuild(&pool, conflict).await.unwrap_err(),
        RecoveryError::PermissionDenied
    );
    let completed: (String, i64) = sqlx::query_as(
        "SELECT receipt_status::text,
                (SELECT COUNT(*) FROM workflow_command_attempt_audits a
                 WHERE a.command_id = r.command_id AND a.attempt_type = 'PERMISSION_DENIED')
         FROM workflow_command_receipts r
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(conflict_fixture.admin)
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed, ("COMPLETED".to_string(), 1));

    let processing_fixture = seed_recovery_fixture(&pool).await;
    let processing = rebuild_command(&processing_fixture);
    let processing_hash = rebuild_hash(&processing);
    sqlx::query(
        "INSERT INTO workflow_command_receipts
         (command_id, principal_id, idempotency_key, command_type, request_hash)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(processing_fixture.admin)
    .bind(&processing.idempotency_key)
    .bind(COMMAND_TYPE_REBUILD_PROJECTION)
    .bind(processing_hash)
    .execute(&pool)
    .await
    .unwrap();
    disable_admin_binding(&pool, &processing_fixture).await;
    assert_eq!(
        run_rebuild(&pool, processing.clone()).await.unwrap_err(),
        RecoveryError::PermissionDenied
    );
    let status: String = sqlx::query_scalar(
        "SELECT receipt_status::text FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(processing_fixture.admin)
    .bind(processing.idempotency_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "PROCESSING");
}

#[tokio::test]
async fn cross_instance_key_conflict_is_opaque_after_current_authorization() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let original = rebuild_command(&fixture);
    let key = original.idempotency_key.clone();
    run_rebuild(&pool, original).await.unwrap();
    let second = create_workflow_instance(
        &pool,
        make_command(fixture.creator, fixture.domain, fixture.version),
    )
    .await
    .unwrap();
    let mut conflict = rebuild_command(&fixture);
    conflict.idempotency_key = key;
    conflict.workflow_instance_id = WorkflowInstanceId::from_uuid(second.workflow_instance_id);
    assert_eq!(
        run_rebuild(&pool, conflict).await.unwrap_err(),
        RecoveryError::IdempotencyConflict
    );
}
