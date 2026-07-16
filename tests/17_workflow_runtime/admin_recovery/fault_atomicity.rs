use super::*;

use sqlx::Connection;
use svc_workflow::domain::workflow_instance::recovery::{AdminEmergencyOperation, RecoveryError};

const DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

struct FaultGuard {
    table: &'static str,
    trigger: String,
    function: String,
}

impl FaultGuard {
    async fn install(
        pool: &PgPool,
        prefix: &str,
        table: &'static str,
        operation: &str,
        condition: String,
    ) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let function = format!("{prefix}_fail_{suffix}");
        let trigger = format!("{prefix}_fail_trg_{suffix}");
        sqlx::query(&format!(
            "CREATE FUNCTION {function}() RETURNS trigger AS $$ BEGIN
               IF {condition} THEN RAISE EXCEPTION 'forced PR5 {prefix} failure'; END IF;
               RETURN NEW;
             END; $$ LANGUAGE plpgsql"
        ))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TRIGGER {trigger} BEFORE {operation} ON {table}
             FOR EACH ROW EXECUTE FUNCTION {function}()"
        ))
        .execute(pool)
        .await
        .unwrap();
        Self {
            table,
            trigger,
            function,
        }
    }
}

impl Drop for FaultGuard {
    fn drop(&mut self) {
        let table = self.table;
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
                let _ = sqlx::query(&format!("DROP TRIGGER IF EXISTS {trigger} ON {table}"))
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

async fn assert_no_receipt(pool: &PgPool, actor: Uuid, key: &str) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(actor)
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

async fn assert_no_recovery_audit(pool: &PgPool, actor: Uuid, instance: Uuid) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_security_audits
         WHERE principal_id = $1 AND resource_id = $2
           AND action LIKE 'ADMIN_EMERGENCY_OVERRIDE%'",
    )
    .bind(actor)
    .bind(instance.to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn visit_insert_failure_rolls_back_override_receipt_and_audits() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let command = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    let key = command.idempotency_key.clone();
    let _guard = FaultGuard::install(
        &pool,
        "pr5_visit",
        "workflow_node_visits",
        "INSERT",
        format!("NEW.workflow_instance_id = '{}'::uuid", fixture.instance),
    )
    .await;
    assert!(matches!(
        run_override(&pool, command).await.unwrap_err(),
        RecoveryError::StorageError(_)
    ));
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
    assert_no_receipt(&pool, fixture.admin, &key).await;
    assert_no_recovery_audit(&pool, fixture.admin, fixture.instance).await;
}

#[tokio::test]
async fn receipt_completion_failure_rolls_back_override_facts_and_receipt() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let command = override_command(
        &fixture,
        AdminEmergencyOperation::TerminateInstance,
        fixture.terminal,
    );
    let key = command.idempotency_key.clone();
    let _guard = FaultGuard::install(
        &pool,
        "pr5_receipt",
        "workflow_command_receipts",
        "UPDATE",
        format!(
            "NEW.principal_id = '{}'::uuid AND NEW.idempotency_key = '{}' \
             AND NEW.receipt_status = 'COMPLETED'",
            fixture.admin, key
        ),
    )
    .await;
    assert!(matches!(
        run_override(&pool, command).await.unwrap_err(),
        RecoveryError::StorageError(_)
    ));
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
    assert_no_receipt(&pool, fixture.admin, &key).await;
    assert_no_recovery_audit(&pool, fixture.admin, fixture.instance).await;
}

#[tokio::test]
async fn security_audit_failure_rolls_back_override_facts_and_receipt() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let command = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    let key = command.idempotency_key.clone();
    let _guard = FaultGuard::install(
        &pool,
        "pr5_security",
        "workflow_security_audits",
        "INSERT",
        format!(
            "NEW.principal_id = '{}'::uuid AND NEW.resource_id = '{}' \
             AND NEW.action = 'ADMIN_EMERGENCY_OVERRIDE_COMMITTED'",
            fixture.admin, fixture.instance
        ),
    )
    .await;
    assert!(matches!(
        run_override(&pool, command).await.unwrap_err(),
        RecoveryError::StorageError(_)
    ));
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
    assert_no_receipt(&pool, fixture.admin, &key).await;
    assert_no_recovery_audit(&pool, fixture.admin, fixture.instance).await;
}
