use super::*;

use sqlx::Connection;

const TEST_DB_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

struct TriggerGuard {
    table: String,
    suffix: String,
}

impl TriggerGuard {
    async fn install(pool: &PgPool, table: &str, operation: &str, condition: &str) -> Self {
        let suffix = Uuid::new_v4().to_string().replace('-', "");
        let function = format!("fn_legacy_import_fail_{suffix}");
        let trigger = format!("trg_legacy_import_fail_{suffix}");
        let mut transaction = pool.begin().await.unwrap();
        // Follow the same relation-lock order as the snapshot concurrency tests.
        // This prevents trigger DDL from queueing ahead of a receipt write while
        // another test holds workflow_transition_definitions exclusively.
        sqlx::query("LOCK TABLE workflow_transition_definitions IN ACCESS SHARE MODE")
            .execute(&mut *transaction)
            .await
            .unwrap();
        sqlx::query(&format!(
            "CREATE FUNCTION {function}() RETURNS TRIGGER AS $$
             BEGIN
               IF {condition} THEN
                 RAISE EXCEPTION 'legacy import injected failure' USING ERRCODE='23000';
               END IF;
               RETURN NEW;
             END; $$ LANGUAGE plpgsql"
        ))
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TRIGGER {trigger} BEFORE {operation} ON {table}
             FOR EACH ROW EXECUTE FUNCTION {function}()"
        ))
        .execute(&mut *transaction)
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        Self {
            table: table.to_string(),
            suffix,
        }
    }
}

impl Drop for TriggerGuard {
    fn drop(&mut self) {
        let table = self.table.clone();
        let suffix = self.suffix.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let Ok(mut connection) = sqlx::PgConnection::connect(TEST_DB_URL).await else {
                    return;
                };
                let Ok(mut transaction) = connection.begin().await else {
                    return;
                };
                if sqlx::query("LOCK TABLE workflow_transition_definitions IN ACCESS SHARE MODE")
                    .execute(&mut *transaction)
                    .await
                    .is_err()
                {
                    return;
                }
                let trigger = format!("trg_legacy_import_fail_{suffix}");
                let function = format!("fn_legacy_import_fail_{suffix}");
                let _ = sqlx::query(&format!("DROP TRIGGER IF EXISTS {trigger} ON {table}"))
                    .execute(&mut *transaction)
                    .await;
                let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {function}()"))
                    .execute(&mut *transaction)
                    .await;
                let _ = transaction.commit().await;
            });
        })
        .join()
        .ok();
    }
}

#[tokio::test]
async fn every_import_write_stage_rolls_back_the_receipt_and_facts() {
    let stages = [
        ("workflow_command_receipts", "INSERT", "receipt_insert"),
        ("workflow_instances", "INSERT", "instance"),
        ("workflow_context_revisions", "INSERT", "context"),
        ("workflow_node_visits", "INSERT", "visit"),
        ("workflow_events", "INSERT", "event"),
        ("workflow_command_receipts", "UPDATE", "receipt_update"),
        ("workflow_security_audits", "INSERT", "security"),
    ];
    for (table, operation, stage) in stages {
        let fixture = fixture(ImportedNodeKind::Normal).await;
        let condition = match stage {
            "receipt_insert" => format!(
                "NEW.command_type = 'IMPORT_LEGACY_WORKFLOW_INSTANCE' AND NEW.principal_id = '{}'",
                fixture.service
            ),
            "instance" => format!(
                "NEW.external_reference = '{}'",
                fixture.command.external_reference()
            ),
            "context" => format!("NEW.created_by_principal_id = '{}'", fixture.owner),
            "visit" => format!("NEW.node_id = '{}'", fixture.node),
            "event" => format!(
                "NEW.event_type = 'WORKFLOW_INSTANCE_IMPORTED' AND NEW.actor_principal_id = '{}'",
                fixture.service
            ),
            "receipt_update" => format!(
                "OLD.command_type = 'IMPORT_LEGACY_WORKFLOW_INSTANCE' AND OLD.principal_id = '{}' AND NEW.receipt_status = 'COMPLETED'",
                fixture.service
            ),
            "security" => format!(
                "NEW.action = 'LEGACY_WORKFLOW_IMPORT_COMMITTED' AND NEW.principal_id = '{}'",
                fixture.service
            ),
            _ => unreachable!(),
        };
        let _guard = TriggerGuard::install(&fixture.pool, table, operation, &condition).await;
        assert!(matches!(
            run(&fixture).await.unwrap_err(),
            LegacyImportError::StorageError(_)
        ));
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_command_receipts
             WHERE principal_id=$1 AND idempotency_key=$2",
        )
        .bind(fixture.service)
        .bind(fixture.command.idempotency_key())
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        let instance_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_instances WHERE external_reference=$1",
        )
        .bind(fixture.command.external_reference())
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!((receipt_count, instance_count), (0, 0), "stage {stage}");
    }
}
