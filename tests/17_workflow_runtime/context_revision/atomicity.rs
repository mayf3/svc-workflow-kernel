//! Atomicity and fault injection tests for ReviseWorkflowContext.
//!
//! Fault-injection tests use **conditional triggers with unique DDL names** to avoid
//! polluting concurrent test runs. Each trigger only fires for records belonging
//! to the specific test's `principal_id`. Trigger and function names include a
//! random UUID suffix so that concurrent test threads never collide.
//!
//! Cleanup uses a RAII Drop guard (TriggerGuard) — even on panic the trigger is
//! removed via a dedicated thread with its own tokio runtime and a fresh
//! database connection.

#![allow(dead_code)]

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
/// Cleanup uses a fresh `PgConnection` on a dedicated thread+runtime to avoid
/// nested-runtime issues and pool-runtime affinity problems.
struct TriggerGuard {
    suffix: String,
    table_or_kind: String,
}

impl TriggerGuard {
    /// Install a BEFORE INSERT trigger that blocks inserts matching a principal check.
    async fn install(pool: &PgPool, on_table: &str, col_check_expression: &str) -> Self {
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

        // Create function — raises only when the column check expression matches
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
        }
    }
}

impl Drop for TriggerGuard {
    fn drop(&mut self) {
        let suffix = self.suffix.clone();
        let on_table = self.table_or_kind.clone();

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
                let fn_name = format!("fn_test_fail_{suffix}");
                let trg_name = format!("trg_test_fail_{suffix}");
                let _ = sqlx::query(&format!("DROP TRIGGER IF EXISTS {trg_name} ON {on_table}"))
                    .execute(&mut conn)
                    .await;
                let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {fn_name}()"))
                    .execute(&mut conn)
                    .await;
            });
        })
        .join()
        .ok();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

async fn seeded_instance(pool: &PgPool) -> (Uuid, Uuid) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (_d, ver_id) = seed_published_definition_wf_creator(pool, domain_id).await;
    let r = create_workflow_instance(pool, make_command(principal_id, domain_id, ver_id))
        .await
        .expect("create");
    (principal_id, r.workflow_instance_id)
}

#[tokio::test]
async fn test_revise_revision_insert_failure_rolls_back() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance(&pool).await;

    // _guard drops trigger even on panic via RAII Drop
    let _guard = TriggerGuard::install(
        &pool,
        "workflow_context_revisions",
        &format!("NEW.created_by_principal_id = '{principal_id}'"),
    )
    .await;

    let err = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await;

    assert!(
        err.is_err(),
        "revision insert failure must fail the command"
    );

    // The new revision was rolled back — only original revision remains
    let rev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions \
         WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rev_count, 1, "only the original revision should remain");

    // Instance projection unchanged
    let (ctx_id, sv): (Uuid, i32) = sqlx::query_as(
        "SELECT current_context_revision_id, workflow_state_version \
         FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("instance");
    assert_eq!(sv, 1, "state version unchanged after rollback");
    // Verify ctx_id matches original (not the blocked revision)
    let orig_ctx_id: Uuid = sqlx::query_scalar(
        "SELECT context_revision_id FROM workflow_context_revisions \
         WHERE workflow_instance_id = $1 AND revision_number = 1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("original ctx");
    assert_eq!(
        ctx_id, orig_ctx_id,
        "current_context_revision_id unchanged after rollback"
    );

    // No CONTEXT_REVISED event
    let ev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_events \
         WHERE workflow_instance_id = $1 AND event_type = 'CONTEXT_REVISED'",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(ev_count, 0, "no CONTEXT_REVISED event after rollback");

    // guard dropped here → trigger cleaned up in Drop
}

#[tokio::test]
async fn test_revise_event_insert_failure_rolls_back() {
    let pool = create_pool().await;
    let (principal_id, instance_id) = seeded_instance(&pool).await;

    // _guard drops trigger even on panic via RAII Drop
    let _guard = TriggerGuard::install(
        &pool,
        "workflow_events",
        &format!("NEW.actor_principal_id = '{principal_id}'"),
    )
    .await;

    let err = revise_workflow_context(
        &pool,
        make_revise_command(principal_id, instance_id, 1, serde_json::json!({"v": 2})),
    )
    .await;

    assert!(err.is_err(), "event insert failure must fail the command");

    // Everything rolled back: revision, instance update, receipt
    let rev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions \
         WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rev_count, 1, "revision rolled back after event failure");

    let (ctx_id, sv): (Uuid, i32) = sqlx::query_as(
        "SELECT current_context_revision_id, workflow_state_version \
         FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("instance");
    assert_eq!(sv, 1, "state version rolled back after event failure");

    let orig_ctx_id: Uuid = sqlx::query_scalar(
        "SELECT context_revision_id FROM workflow_context_revisions \
         WHERE workflow_instance_id = $1 AND revision_number = 1",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("original ctx");
    assert_eq!(
        ctx_id, orig_ctx_id,
        "current_context_revision_id rolled back after event failure"
    );

    // No CONTEXT_REVISED event
    let ev_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_events \
         WHERE workflow_instance_id = $1 AND event_type = 'CONTEXT_REVISED'",
    )
    .bind(instance_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(ev_count, 0, "no CONTEXT_REVISED event after rollback");

    // guard dropped here → trigger cleaned up in Drop
}
