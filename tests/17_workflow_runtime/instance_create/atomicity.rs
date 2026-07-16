//! Atomicity tests: fault injection, deterministic failure, deferred FK, and event counts.
//!
//! Fault-injection tests use **conditional triggers with unique DDL names** to avoid
//! polluting concurrent test runs. Each trigger only fires for records belonging
//! to the specific test's `principal_id`. Trigger and function names include a
//! random UUID suffix so that concurrent test threads never collide.
//!
//! Trigger cleanup uses [`TriggerGuard`] — a RAII guard whose `Drop` impl removes
//! the trigger and function even if the test body panics. The Drop impl spawns a
//! dedicated thread with its own tokio runtime to safely execute the async DDL cleanup.

use super::*;
use sqlx::Connection;

/// Test database URL (must match `common/mod.rs`).
const TEST_DB_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

// ---------------------------------------------------------------------------
// RAII trigger guard
// ---------------------------------------------------------------------------

/// RAII guard that drops a test trigger and its function when the guard is
/// dropped (including on panic).
///
/// # Panic safety
///
/// Cleanup uses a **fresh `PgConnection`** (not a pooled connection) on a
/// dedicated thread+runtime. This avoids:
/// 1. Nested-runtime panics (Drop runs inside a `#[tokio::test]`)
/// 2. Pool connections being tied to the original runtime
struct TriggerGuard {
    suffix: String,
    /// For receipt triggers, this is the literal string `"__receipt__"`.
    /// For table triggers, this is the SQL table name.
    table_or_kind: String,
    /// Whether this is the special receipt-completion trigger.
    is_receipt: bool,
}

impl TriggerGuard {
    /// Install a BEFORE INSERT trigger that blocks inserts for a specific principal.
    async fn install_table(pool: &PgPool, on_table: &str, col_check_expression: &str) -> Self {
        let suffix = Uuid::new_v4().to_string().replace('-', "");
        let fn_name = format!("fn_test_fail_{suffix}");
        let trg_name = format!("trg_test_fail_{suffix}");

        // Defensive cleanup — remove orphan objects from a previous crash
        let _ = sqlx::query(&format!("DROP TRIGGER IF EXISTS {trg_name} ON {on_table}"))
            .execute(pool)
            .await;
        let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {fn_name}()"))
            .execute(pool)
            .await;

        // Create function — raises only when the check expression matches
        sqlx::query(&format!(
            "CREATE FUNCTION {fn_name}() RETURNS TRIGGER AS $$
             BEGIN
                 IF {col_check_expression} THEN
                     RAISE EXCEPTION 'test_injected_failure: {on_table} insert blocked'
                     USING ERRCODE = '23000';
                 END IF;
                 RETURN NEW;
             END;
             $$ LANGUAGE plpgsql"
        ))
        .execute(pool)
        .await
        .expect("create trigger function");

        sqlx::query(&format!(
            "CREATE TRIGGER {trg_name} BEFORE INSERT ON {on_table} \
             FOR EACH ROW EXECUTE FUNCTION {fn_name}()"
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

    /// Install a BEFORE UPDATE trigger on `workflow_command_receipts` that blocks
    /// the PROCESSING → COMPLETED transition for a specific principal.
    async fn install_receipt_update(pool: &PgPool, principal_id: Uuid) -> Self {
        let suffix = Uuid::new_v4().to_string().replace('-', "");
        let fn_name = format!("fn_test_fail_rcpt_{suffix}");
        let trg_name = format!("trg_test_fail_rcpt_{suffix}");

        let _ = sqlx::query(&format!(
            "DROP TRIGGER IF EXISTS {trg_name} ON workflow_command_receipts"
        ))
        .execute(pool)
        .await;
        let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {fn_name}()"))
            .execute(pool)
            .await;

        sqlx::query(&format!(
            "CREATE FUNCTION {fn_name}() RETURNS TRIGGER AS $$
             BEGIN
                 IF NEW.receipt_status = 'COMPLETED'
                    AND OLD.receipt_status = 'PROCESSING'
                    AND OLD.principal_id = '{principal_id}' THEN
                     RAISE EXCEPTION 'test_injected_failure: receipt completion blocked'
                     USING ERRCODE = '23000';
                 END IF;
                 RETURN NEW;
             END;
             $$ LANGUAGE plpgsql"
        ))
        .execute(pool)
        .await
        .expect("create receipt trigger function");

        sqlx::query(&format!(
            "CREATE TRIGGER {trg_name} BEFORE UPDATE ON workflow_command_receipts \
             FOR EACH ROW EXECUTE FUNCTION {fn_name}()"
        ))
        .execute(pool)
        .await
        .expect("create receipt trigger");

        Self {
            suffix,
            table_or_kind: "__receipt__".to_string(),
            is_receipt: true,
        }
    }
}

impl Drop for TriggerGuard {
    fn drop(&mut self) {
        let suffix = self.suffix.clone();
        let on_table = self.table_or_kind.clone();
        let is_receipt = self.is_receipt;

        // Spawn a dedicated thread+runtime with a fresh PgConnection.
        // We don't use the original PgPool because pool internals are tied to
        // the original tokio runtime.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .expect("build cleanup runtime");
            rt.block_on(async move {
                let Ok(mut conn) = sqlx::PgConnection::connect(TEST_DB_URL).await else {
                    return; // best-effort cleanup
                };
                if is_receipt {
                    let fn_name = format!("fn_test_fail_rcpt_{suffix}");
                    let trg_name = format!("trg_test_fail_rcpt_{suffix}");
                    let _ = sqlx::query(&format!(
                        "DROP TRIGGER IF EXISTS {trg_name} ON workflow_command_receipts"
                    ))
                    .execute(&mut conn)
                    .await;
                    let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {fn_name}()"))
                        .execute(&mut conn)
                        .await;
                } else {
                    let fn_name = format!("fn_test_fail_{suffix}");
                    let trg_name = format!("trg_test_fail_{suffix}");
                    let _ =
                        sqlx::query(&format!("DROP TRIGGER IF EXISTS {trg_name} ON {on_table}"))
                            .execute(&mut conn)
                            .await;
                    let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {fn_name}()"))
                        .execute(&mut conn)
                        .await;
                }
            });
        })
        .join()
        .ok();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_exactly_one_event_per_creation() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let result = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1")
            .bind(result.workflow_instance_id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_command_id_matches_event() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let result = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_events e JOIN workflow_command_receipts r ON e.command_id = r.command_id WHERE e.workflow_instance_id = $1",
    ).bind(result.workflow_instance_id).fetch_one(&pool).await.expect("count");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_deferred_fk_committed_successfully() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let result = create_workflow_instance(&pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    let fk_ok: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflow_instances i \
         JOIN workflow_context_revisions cr ON cr.context_revision_id = i.current_context_revision_id AND cr.workflow_instance_id = i.workflow_instance_id \
         JOIN workflow_node_visits nv ON nv.node_visit_id = i.current_node_visit_id AND nv.workflow_instance_id = i.workflow_instance_id \
         WHERE i.workflow_instance_id = $1)",
    ).bind(result.workflow_instance_id).fetch_one(&pool).await.expect("check");
    assert!(fk_ok, "circular FKs must resolve");
}

#[tokio::test]
async fn test_event_failure_rolls_back_everything() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;

    // _guard drops trigger even on panic via RAII Drop
    let _guard = TriggerGuard::install_table(
        &pool,
        "workflow_events",
        &format!("NEW.actor_principal_id = '{principal_id}'"),
    )
    .await;

    let cmd = make_command(principal_id, domain_id, ver_id);
    let err = create_workflow_instance(&pool, cmd).await;

    assert!(
        err.is_err(),
        "creation must fail when event insert is blocked"
    );

    // No instance for this principal — entire transaction rolled back
    let instance_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_instances WHERE created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(instance_count, 0, "no instance after event failure");
    // guard dropped here → trigger cleaned up in Drop
}

#[tokio::test]
async fn test_infrastructure_failure_no_residual_receipt() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;

    let _guard = TriggerGuard::install_table(
        &pool,
        "workflow_instances",
        &format!("NEW.created_by_principal_id = '{principal_id}'"),
    )
    .await;

    let cmd = make_command(principal_id, domain_id, ver_id);
    let err = create_workflow_instance(&pool, cmd).await;

    assert!(err.is_err(), "infrastructure failure must return error");

    // No receipt for this principal — transaction fully rolled back
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts WHERE principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(receipt_count, 0, "no receipt after infrastructure failure");
    // guard dropped here → trigger cleaned up in Drop
}

#[tokio::test]
async fn test_receipt_completion_failure_rolls_back_all_runtime_facts() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;

    // _guard drops trigger even on panic via RAII Drop
    let _guard = TriggerGuard::install_receipt_update(&pool, principal_id).await;

    let cmd = make_command(principal_id, domain_id, ver_id);
    let idem_key = cmd.idempotency_key.clone();
    let err = create_workflow_instance(&pool, cmd).await;

    assert!(
        err.is_err(),
        "creation must fail when receipt completion is blocked"
    );

    // No runtime facts — the entire transaction rolled back
    let instance_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_instances WHERE created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(
        instance_count, 0,
        "no instance after receipt completion failure"
    );

    // No receipt either — the PROCESSING receipt was rolled back with the transaction
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idem_key).fetch_one(&pool).await.expect("count");
    assert_eq!(
        receipt_count, 0,
        "no receipt after receipt completion failure (tx rolled back)"
    );
    // guard dropped here → trigger cleaned up in Drop
}

#[tokio::test]
async fn test_deterministic_failure_no_runtime_facts_left() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;

    sqlx::query("UPDATE domains SET enabled = FALSE WHERE domain_id = $1")
        .bind(domain_id)
        .execute(&pool)
        .await
        .expect("disable");

    let cmd = make_command(principal_id, domain_id, ver_id);
    let idem_key = cmd.idempotency_key.clone();
    let err = create_workflow_instance(&pool, cmd).await;
    assert!(matches!(
        err,
        Err(CreateWorkflowInstanceError::DomainDisabled)
    ));

    // Receipt exists — deterministic failure is persisted
    let receipt_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2)",
    ).bind(principal_id).bind(&idem_key).fetch_one(&pool).await.expect("check");
    assert!(
        receipt_exists,
        "receipt must exist for deterministic failure"
    );

    // But no runtime facts
    let instance_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_instances WHERE created_by_principal_id = $1",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(instance_count, 0, "no instance for deterministic failure");
}
