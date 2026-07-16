use super::*;
use sqlx::Connection;

const TEST_DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

struct TriggerGuard {
    suffix: String,
    table: String,
}

impl TriggerGuard {
    async fn install(pool: &PgPool, table: &str, operation: &str, condition: &str) -> Self {
        let suffix = Uuid::new_v4().to_string().replace('-', "");
        let function = format!("fn_combined_fail_{suffix}");
        let trigger = format!("trg_combined_fail_{suffix}");
        sqlx::query(&format!(
            "CREATE FUNCTION {function}() RETURNS TRIGGER AS $$ \
             BEGIN IF {condition} THEN \
               RAISE EXCEPTION 'combined_test_injected_failure'; \
             END IF; RETURN NEW; END; $$ LANGUAGE plpgsql"
        ))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TRIGGER {trigger} BEFORE {operation} ON {table} \
             FOR EACH ROW EXECUTE FUNCTION {function}()"
        ))
        .execute(pool)
        .await
        .unwrap();
        Self {
            suffix,
            table: table.to_string(),
        }
    }
}

impl Drop for TriggerGuard {
    fn drop(&mut self) {
        let suffix = self.suffix.clone();
        let table = self.table.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let Ok(mut connection) = sqlx::PgConnection::connect(TEST_DATABASE_URL).await
                else {
                    return;
                };
                let function = format!("fn_combined_fail_{suffix}");
                let trigger = format!("trg_combined_fail_{suffix}");
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

async fn setup(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(pool).await;
    let (version_id, _, _, advance_id, _) = seed_combined_graph(
        pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
        None,
        None,
    )
    .await;
    let created = create_combined_instance(
        pool,
        principal_id,
        domain_id,
        version_id,
        serde_json::json!({}),
    )
    .await;
    (principal_id, advance_id, created.workflow_instance_id)
}

async fn assert_no_partial_combined_facts(pool: &PgPool, instance_id: Uuid, principal_id: Uuid) {
    let state: (i32, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT workflow_state_version, \
          (SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1), \
          (SELECT COUNT(*) FROM workflow_submissions WHERE workflow_instance_id = $1), \
          (SELECT COUNT(*) FROM workflow_node_visits WHERE workflow_instance_id = $1), \
          (SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1), \
          (SELECT COUNT(*) FROM workflow_command_receipts \
           WHERE command_type = 'REVISE_CONTEXT_AND_TRANSITION' \
             AND principal_id = $2) \
         FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(instance_id)
    .bind(principal_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(state, (1, 1, 0, 1, 1, 0));
}

#[tokio::test]
async fn submission_insert_failure_rolls_back_new_context_revision() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let _guard = TriggerGuard::install(
        &pool,
        "workflow_submissions",
        "INSERT",
        &format!("NEW.author_principal_id = '{principal_id}'::uuid"),
    )
    .await;
    let error = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, instance_id, 1, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::StorageError(_)
    ));
    assert_no_partial_combined_facts(&pool, instance_id, principal_id).await;
}

#[tokio::test]
async fn instance_update_failure_rolls_back_all_new_facts() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let _guard = TriggerGuard::install(
        &pool,
        "workflow_instances",
        "UPDATE",
        &format!(
            "NEW.workflow_instance_id = '{instance_id}'::uuid \
             AND NEW.workflow_state_version = 2"
        ),
    )
    .await;
    let error = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, instance_id, 1, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::StorageError(_)
    ));
    assert_no_partial_combined_facts(&pool, instance_id, principal_id).await;
}

#[tokio::test]
async fn event_insert_failure_rolls_back_projection_and_facts() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let _guard = TriggerGuard::install(
        &pool,
        "workflow_events",
        "INSERT",
        &format!(
            "NEW.actor_principal_id = '{principal_id}'::uuid \
             AND NEW.event_type = 'WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED'"
        ),
    )
    .await;
    let error = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, instance_id, 1, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::StorageError(_)
    ));
    assert_no_partial_combined_facts(&pool, instance_id, principal_id).await;
}

#[tokio::test]
async fn receipt_completion_failure_rolls_back_everything_including_receipt() {
    let pool = create_pool().await;
    let (principal_id, advance_id, instance_id) = setup(&pool).await;
    let _guard = TriggerGuard::install(
        &pool,
        "workflow_command_receipts",
        "UPDATE",
        &format!(
            "NEW.principal_id = '{principal_id}'::uuid \
             AND NEW.command_type = 'REVISE_CONTEXT_AND_TRANSITION' \
             AND NEW.receipt_status = 'COMPLETED'"
        ),
    )
    .await;
    let error = revise_context_and_transition(
        &pool,
        make_combined_command(principal_id, instance_id, 1, advance_id),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ReviseContextAndTransitionError::StorageError(_)
    ));
    assert_no_partial_combined_facts(&pool, instance_id, principal_id).await;
}
