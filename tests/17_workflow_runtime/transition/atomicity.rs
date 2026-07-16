//! Atomicity fault injection tests for ExecuteWorkflowTransition.
//!
//! Uses the same TriggerGuard RAII pattern established in PR 3A and PR 3B.

use super::*;
use sqlx::Connection;
use std::thread;
use tokio::runtime::Builder as TokioBuilder;

/// RAII guard that installs a conditional BEFORE INSERT/UPDATE trigger for fault injection.
///
/// Pattern matches PR 3A / PR 3B:
/// - Unique UUID suffix in trigger/function names
/// - Condition scoped to test-specific principal / instance / command
/// - No CREATE OR REPLACE
/// - Drop cleans up on dedicated thread + runtime + fresh connection
struct TriggerGuard {
    suffix: String,
    table_or_kind: String,
    is_receipt: bool,
}

impl TriggerGuard {
    /// Install a BEFORE INSERT trigger that raises an exception when condition matches.
    async fn install_table(
        pool: &sqlx::PgPool,
        on_table: &str,
        col_check_expression: &str,
    ) -> Self {
        Self::install_table_operation(pool, on_table, "INSERT", col_check_expression).await
    }

    async fn install_table_operation(
        pool: &sqlx::PgPool,
        on_table: &str,
        operation: &str,
        col_check_expression: &str,
    ) -> Self {
        let suffix = Uuid::new_v4().to_string().replace('-', "");
        let fn_name = format!("fn_test_fail_{}", suffix);
        let trg_name = format!("trg_test_fail_{}", suffix);

        // Defensive cleanup
        let _ = sqlx::query(&format!(
            "DROP TRIGGER IF EXISTS {} ON {}",
            trg_name, on_table
        ))
        .execute(pool)
        .await;
        let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {}()", fn_name))
            .execute(pool)
            .await;

        sqlx::query(&format!(
            "CREATE FUNCTION {}() RETURNS TRIGGER AS $$
             BEGIN
               IF {} THEN
                 RAISE EXCEPTION 'test_injected_failure: {} blocked';
               END IF;
               RETURN NEW;
             END;
             $$ LANGUAGE plpgsql",
            fn_name, col_check_expression, on_table
        ))
        .execute(pool)
        .await
        .expect("create function");

        sqlx::query(&format!(
            "CREATE TRIGGER {} BEFORE {} ON {} FOR EACH ROW EXECUTE FUNCTION {}()",
            trg_name, operation, on_table, fn_name
        ))
        .execute(pool)
        .await
        .expect("create trigger");

        Self {
            suffix,
            table_or_kind: on_table.to_string(),
            is_receipt: false,
        }
    }

    /// Install a BEFORE UPDATE trigger on workflow_command_receipts.
    async fn install_receipt_update(pool: &sqlx::PgPool, condition: &str) -> Self {
        let suffix = Uuid::new_v4().to_string().replace('-', "");
        let fn_name = format!("fn_test_fail_{}", suffix);
        let trg_name = format!("trg_test_fail_{}", suffix);

        let _ = sqlx::query(&format!(
            "DROP TRIGGER IF EXISTS {} ON workflow_command_receipts",
            trg_name
        ))
        .execute(pool)
        .await;
        let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {}()", fn_name))
            .execute(pool)
            .await;

        sqlx::query(&format!(
            "CREATE FUNCTION {}() RETURNS TRIGGER AS $$
             BEGIN
               IF {} THEN
                 RAISE EXCEPTION 'test_injected_failure: receipt update blocked';
               END IF;
               RETURN NEW;
             END;
             $$ LANGUAGE plpgsql",
            fn_name, condition
        ))
        .execute(pool)
        .await
        .expect("create function");

        sqlx::query(&format!(
            "CREATE TRIGGER {} BEFORE UPDATE ON workflow_command_receipts FOR EACH ROW EXECUTE FUNCTION {}()",
            trg_name, fn_name
        ))
        .execute(pool)
        .await
        .expect("create trigger");

        Self {
            suffix,
            table_or_kind: "receipt".to_string(),
            is_receipt: true,
        }
    }
}

impl Drop for TriggerGuard {
    fn drop(&mut self) {
        let suffix = self.suffix.clone();
        let table_or_kind = self.table_or_kind.clone();
        let is_receipt = self.is_receipt;

        thread::spawn(move || {
            let rt = TokioBuilder::new_current_thread()
                .enable_all()
                .build()
                .expect("build cleanup runtime");

            rt.block_on(async {
                let conn_str = "postgres://postgres:postgres@localhost:5432/svc_workflow";
                let Ok(mut conn) = sqlx::PgConnection::connect(conn_str).await else {
                    return;
                };

                let fn_name = format!("fn_test_fail_{}", suffix);
                let trg_name = format!("trg_test_fail_{}", suffix);

                if is_receipt {
                    let _ = sqlx::query(&format!(
                        "DROP TRIGGER IF EXISTS {} ON workflow_command_receipts",
                        trg_name
                    ))
                    .execute(&mut conn)
                    .await;
                } else {
                    let _ = sqlx::query(&format!(
                        "DROP TRIGGER IF EXISTS {} ON {}",
                        trg_name, table_or_kind
                    ))
                    .execute(&mut conn)
                    .await;
                }
                let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {}()", fn_name))
                    .execute(&mut conn)
                    .await;
            });
        })
        .join()
        .ok();
    }
}

/// Helper to create an instance advanced to NORMAL node.
async fn setup_transition_instance(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (_, ver_id, _, _, _, draft_adv, _, _, _) = seed_transition_graph(
        pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
    )
    .await;
    let (_, instance_id, source_visit_id) =
        create_and_advance_to_normal(pool, principal_id, domain_id, draft_adv, ver_id).await;
    (
        principal_id,
        instance_id,
        source_visit_id,
        draft_adv,
        ver_id,
    )
}

/// Submission INSERT failure rolls back everything.
#[tokio::test]
async fn test_transition_submission_insert_failure_rolls_back() {
    let pool = create_pool().await;
    let (principal_id, instance_id, source_visit_id, av_id, _) =
        setup_transition_instance(&pool).await;

    let payload = serde_json::json!({"test": "data"});
    let condition = format!("NEW.author_principal_id = '{}'", principal_id);
    let _guard = TriggerGuard::install_table(&pool, "workflow_submissions", &condition).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, av_id, Some(payload));
    let err = execute_workflow_transition(&pool, cmd).await;
    assert!(err.is_err());

    // No new submission, visit, event, or state change
    let inst: (i32, Uuid) = sqlx::query_as(
        "SELECT workflow_state_version, current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(inst.0, 2);
    assert_eq!(inst.1, source_visit_id);

    let ev_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1")
            .bind(instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ev_count, 2); // creation + first transition only
}

/// NodeVisit INSERT failure rolls back everything.
#[tokio::test]
async fn test_transition_visit_insert_failure_rolls_back() {
    let pool = create_pool().await;
    let (principal_id, instance_id, source_visit_id, draft_adv, _) =
        setup_transition_instance(&pool).await;

    // Block INSERT on node_visits where assignee matches our principal
    let condition = format!("NEW.assignee_principal_id = '{}'", principal_id);
    let _guard = TriggerGuard::install_table(&pool, "workflow_node_visits", &condition).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, draft_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await;
    assert!(err.is_err());

    let inst: (i32, Uuid) = sqlx::query_as(
        "SELECT workflow_state_version, current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(inst.0, 2);
    assert_eq!(inst.1, source_visit_id);
}

/// Instance UPDATE failure rolls back everything.
#[tokio::test]
async fn test_transition_instance_update_failure_rolls_back() {
    let pool = create_pool().await;
    let (principal_id, instance_id, source_visit_id, draft_adv, _) =
        setup_transition_instance(&pool).await;

    let condition = format!("OLD.workflow_instance_id = '{}'", instance_id);
    let _guard =
        TriggerGuard::install_table_operation(&pool, "workflow_instances", "UPDATE", &condition)
            .await;

    let cmd = make_transition_command(principal_id, instance_id, 2, draft_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await;
    assert!(err.is_err());

    let inst: (i32, Uuid) = sqlx::query_as(
        "SELECT workflow_state_version, current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(inst.0, 2);
    assert_eq!(inst.1, source_visit_id);
}

/// Event INSERT failure rolls back everything.
#[tokio::test]
async fn test_transition_event_insert_failure_rolls_back() {
    let pool = create_pool().await;
    let (principal_id, instance_id, source_visit_id, draft_adv, _) =
        setup_transition_instance(&pool).await;

    let condition = format!("NEW.actor_principal_id = '{}'", principal_id);
    let _guard = TriggerGuard::install_table(&pool, "workflow_events", &condition).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, draft_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await;
    assert!(err.is_err());

    let inst: (i32, Uuid) = sqlx::query_as(
        "SELECT workflow_state_version, current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(inst.0, 2);
    assert_eq!(inst.1, source_visit_id);
}

/// Receipt Completion failure rolls back everything.
#[tokio::test]
async fn test_transition_receipt_completion_failure_rolls_back() {
    let pool = create_pool().await;
    let (principal_id, instance_id, source_visit_id, draft_adv, _) =
        setup_transition_instance(&pool).await;

    let condition = format!(
        "OLD.principal_id = '{}' AND OLD.receipt_status = 'PROCESSING' AND NEW.receipt_status = 'COMPLETED'",
        principal_id
    );
    let _guard = TriggerGuard::install_receipt_update(&pool, &condition).await;

    let cmd = make_transition_command(principal_id, instance_id, 2, draft_adv, None);
    let err = execute_workflow_transition(&pool, cmd).await;
    assert!(err.is_err());

    // No new receipt (should be rolled back), no state changes
    let inst: (i32, Uuid) = sqlx::query_as(
        "SELECT workflow_state_version, current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(instance_id).fetch_one(&pool).await.unwrap();
    assert_eq!(inst.0, 2);
    assert_eq!(inst.1, source_visit_id);
}
