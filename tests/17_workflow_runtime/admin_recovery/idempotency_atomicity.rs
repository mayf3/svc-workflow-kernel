use super::*;

use sqlx::Connection;
use svc_workflow::domain::workflow_instance::recovery::{
    AdminEmergencyOperation, RecoveryError, COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE,
    COMMAND_TYPE_REBUILD_PROJECTION,
};

const DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

struct EventFailureGuard {
    trigger: String,
    function: String,
}

struct InstanceUpdateFailureGuard {
    trigger: String,
    function: String,
}

impl InstanceUpdateFailureGuard {
    async fn install(pool: &PgPool, instance: Uuid) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let function = format!("rebuild_update_fail_{suffix}");
        let trigger = format!("rebuild_update_fail_trg_{suffix}");
        sqlx::query(&format!(
            "CREATE FUNCTION {function}() RETURNS trigger AS $$ BEGIN
               IF NEW.workflow_instance_id = '{instance}'::uuid THEN
                 RAISE EXCEPTION 'forced rebuild update failure';
               END IF;
               RETURN NEW;
             END; $$ LANGUAGE plpgsql"
        ))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TRIGGER {trigger} BEFORE UPDATE ON workflow_instances
             FOR EACH ROW EXECUTE FUNCTION {function}()"
        ))
        .execute(pool)
        .await
        .unwrap();
        Self { trigger, function }
    }
}

impl Drop for InstanceUpdateFailureGuard {
    fn drop(&mut self) {
        let trigger = self.trigger.clone();
        let function = self.function.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let Ok(mut connection) = sqlx::PgConnection::connect(DATABASE_URL).await else {
                    return;
                };
                let _ = sqlx::query(&format!(
                    "DROP TRIGGER IF EXISTS {trigger} ON workflow_instances"
                ))
                .execute(&mut connection)
                .await;
                let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {function}()"))
                    .execute(&mut connection)
                    .await;
            });
        })
        .join()
        .ok();
    }
}

impl EventFailureGuard {
    async fn install(pool: &PgPool, instance: Uuid) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let function = format!("admin_event_fail_{suffix}");
        let trigger = format!("admin_event_fail_trg_{suffix}");
        sqlx::query(&format!(
            "CREATE FUNCTION {function}() RETURNS trigger AS $$ BEGIN
               IF NEW.workflow_instance_id = '{instance}'::uuid
                  AND NEW.event_type = 'ADMIN_EMERGENCY_OVERRIDE_COMMITTED' THEN
                 RAISE EXCEPTION 'forced admin event failure';
               END IF;
               RETURN NEW;
             END; $$ LANGUAGE plpgsql"
        ))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TRIGGER {trigger} BEFORE INSERT ON workflow_events
             FOR EACH ROW EXECUTE FUNCTION {function}()"
        ))
        .execute(pool)
        .await
        .unwrap();
        Self { trigger, function }
    }
}

impl Drop for EventFailureGuard {
    fn drop(&mut self) {
        let trigger = self.trigger.clone();
        let function = self.function.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let Ok(mut connection) = sqlx::PgConnection::connect(DATABASE_URL).await else {
                    return;
                };
                let _ = sqlx::query(&format!(
                    "DROP TRIGGER IF EXISTS {trigger} ON workflow_events"
                ))
                .execute(&mut connection)
                .await;
                let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {function}()"))
                    .execute(&mut connection)
                    .await;
            });
        })
        .join()
        .ok();
    }
}

#[tokio::test]
async fn both_commands_replay_success_without_second_mutation() {
    let pool = create_pool().await;
    let rebuild_fixture = seed_recovery_fixture(&pool).await;
    let rebuild = rebuild_command(&rebuild_fixture);
    let first = run_rebuild(&pool, rebuild.clone()).await.unwrap();
    let second = run_rebuild(&pool, rebuild).await.unwrap();
    assert_eq!(first.command_id, second.command_id);
    assert!(!first.replayed);
    assert!(second.replayed);
    assert_eq!(
        count_instance_facts(&pool, rebuild_fixture.instance).await,
        (1, 1, 0, 1)
    );

    let override_fixture = seed_recovery_fixture(&pool).await;
    let command = override_command(
        &override_fixture,
        AdminEmergencyOperation::MoveToNode,
        override_fixture.normal,
    );
    let first = run_override(&pool, command.clone()).await.unwrap();
    let second = run_override(&pool, command).await.unwrap();
    assert_eq!(first.command_id, second.command_id);
    assert!(!first.replayed);
    assert!(second.replayed);
    assert_eq!(
        count_instance_facts(&pool, override_fixture.instance).await,
        (1, 2, 0, 2)
    );
}

#[tokio::test]
async fn hash_conflict_and_deterministic_failure_are_stable() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let original = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    let key = original.idempotency_key.clone();
    run_override(&pool, original).await.unwrap();
    let mut conflict = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    conflict.idempotency_key = key;
    conflict.reason = "different approved reason".to_string();
    assert!(matches!(
        run_override(&pool, conflict).await.unwrap_err(),
        RecoveryError::IdempotencyConflict
    ));

    let failure_fixture = seed_recovery_fixture(&pool).await;
    let mut stale = override_command(
        &failure_fixture,
        AdminEmergencyOperation::TerminateInstance,
        failure_fixture.terminal,
    );
    stale.expected_workflow_state_version = 44;
    let first = run_override(&pool, stale.clone()).await.unwrap_err();
    let second = run_override(&pool, stale).await.unwrap_err();
    assert_eq!(first, second);
    assert_eq!(
        count_instance_facts(&pool, failure_fixture.instance).await,
        (1, 1, 0, 1)
    );
}

#[tokio::test]
async fn processing_receipt_is_not_taken_over() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let command = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    let envelope = serde_json::json!({
        "commandSchemaVersion": command.command_schema_version,
        "commandType": COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE,
        "routeParameters": {},
        "requestBody": {
            "principalId": command.principal_id.to_string(),
            "workflowInstanceId": command.workflow_instance_id.to_string(),
            "expectedWorkflowStateVersion": command.expected_workflow_state_version,
            "operation": command.operation.as_str(),
            "targetNodeId": command.target_node_id.to_string(),
            "reason": command.reason,
            "relatedReferences": command.related_references,
            "expectedBeforeSnapshotDigest": command.expected_before_snapshot_digest,
        }
    });
    let hash = jcs_canonicalize::sha256_jcs_hex(&envelope).unwrap();
    sqlx::query(
        "INSERT INTO workflow_command_receipts
         (command_id, principal_id, idempotency_key, command_type, request_hash)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.admin)
    .bind(&command.idempotency_key)
    .bind(COMMAND_TYPE_ADMIN_EMERGENCY_OVERRIDE)
    .bind(hash)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        run_override(&pool, command).await.unwrap_err(),
        RecoveryError::CommandStillProcessing
    );
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
}

#[tokio::test]
async fn infrastructure_event_failure_rolls_back_receipt_visit_projection_and_audits() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let command = override_command(
        &fixture,
        AdminEmergencyOperation::TerminateInstance,
        fixture.terminal,
    );
    let key = command.idempotency_key.clone();
    let _guard = EventFailureGuard::install(&pool, fixture.instance).await;
    assert!(matches!(
        run_override(&pool, command).await.unwrap_err(),
        RecoveryError::StorageError(_)
    ));
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(fixture.admin)
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_security_audits
         WHERE principal_id = $1 AND resource_id = $2
           AND action LIKE 'ADMIN_EMERGENCY_OVERRIDE%'",
    )
    .bind(fixture.admin)
    .bind(fixture.instance.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audits, 0);
}

#[tokio::test]
async fn concurrent_same_key_replays_one_override() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let command = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    let (left, right) = tokio::join!(
        run_override(&pool, command.clone()),
        run_override(&pool, command)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.command_id, right.command_id);
    assert_ne!(left.replayed, right.replayed);
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 2, 0, 2)
    );
}

#[tokio::test]
async fn rebuild_hash_conflict_processing_and_same_key_concurrency_are_stable() {
    let pool = create_pool().await;
    let conflict_fixture = seed_recovery_fixture(&pool).await;
    let original = rebuild_command(&conflict_fixture);
    let key = original.idempotency_key.clone();
    run_rebuild(&pool, original).await.unwrap();
    let mut conflict = rebuild_command(&conflict_fixture);
    conflict.idempotency_key = key;
    conflict.expected_before_snapshot_digest = Some("0".repeat(64));
    assert!(matches!(
        run_rebuild(&pool, conflict).await.unwrap_err(),
        RecoveryError::IdempotencyConflict
    ));

    let processing_fixture = seed_recovery_fixture(&pool).await;
    let processing = rebuild_command(&processing_fixture);
    let envelope = serde_json::json!({
        "commandSchemaVersion": processing.command_schema_version,
        "commandType": COMMAND_TYPE_REBUILD_PROJECTION,
        "routeParameters": {},
        "requestBody": {
            "principalId": processing.principal_id.to_string(),
            "workflowInstanceId": processing.workflow_instance_id.to_string(),
            "expectedBeforeSnapshotDigest": processing.expected_before_snapshot_digest,
        }
    });
    let hash = jcs_canonicalize::sha256_jcs_hex(&envelope).unwrap();
    sqlx::query(
        "INSERT INTO workflow_command_receipts
         (command_id, principal_id, idempotency_key, command_type, request_hash)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(processing_fixture.admin)
    .bind(&processing.idempotency_key)
    .bind(COMMAND_TYPE_REBUILD_PROJECTION)
    .bind(hash)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        run_rebuild(&pool, processing).await.unwrap_err(),
        RecoveryError::CommandStillProcessing
    );

    let concurrent_fixture = seed_recovery_fixture(&pool).await;
    let command = rebuild_command(&concurrent_fixture);
    let (left, right) = tokio::join!(
        run_rebuild(&pool, command.clone()),
        run_rebuild(&pool, command)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.command_id, right.command_id);
    assert_ne!(left.replayed, right.replayed);
}

#[tokio::test]
async fn rebuild_infrastructure_failure_rolls_back_receipt_and_projection_update() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    sqlx::query(
        "UPDATE workflow_instances SET workflow_state_version = 99
         WHERE workflow_instance_id = $1",
    )
    .bind(fixture.instance)
    .execute(&pool)
    .await
    .unwrap();
    let command = rebuild_command(&fixture);
    let key = command.idempotency_key.clone();
    let _guard = InstanceUpdateFailureGuard::install(&pool, fixture.instance).await;
    assert!(matches!(
        run_rebuild(&pool, command).await.unwrap_err(),
        RecoveryError::StorageError(_)
    ));
    let state: i32 = sqlx::query_scalar(
        "SELECT workflow_state_version FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(fixture.instance)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, 99);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(fixture.admin)
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 0);
}

#[tokio::test]
async fn rebuild_and_override_with_different_keys_linearize_on_instance() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let rebuild = rebuild_command(&fixture);
    let override_command = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    let (rebuilt, overridden) = tokio::join!(
        run_rebuild(&pool, rebuild),
        run_override(&pool, override_command)
    );
    assert!(rebuilt.is_ok());
    assert!(overridden.is_ok());
    let state: i32 = sqlx::query_scalar(
        "SELECT workflow_state_version FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(fixture.instance)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, 2);
}
