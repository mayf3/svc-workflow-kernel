use super::*;

use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

const DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

#[derive(Clone, Copy)]
enum BindingMutation {
    InsertEnabledMigration,
    EnableDisabledMigration,
    RetagEnabledBinding,
}

async fn seed_service_principal(pool: &PgPool, label: &str) -> Uuid {
    let principal_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals
         (principal_id, principal_type, display_name, enabled)
         VALUES ($1, 'SERVICE', $2, TRUE)",
    )
    .bind(principal_id)
    .bind(label)
    .execute(pool)
    .await
    .unwrap();
    principal_id
}

async fn seed_binding(
    pool: &PgPool,
    domain_id: Uuid,
    principal_id: Uuid,
    role_key: &str,
    enabled: bool,
) -> Uuid {
    let binding_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(binding_id)
    .bind(domain_id)
    .bind(principal_id)
    .bind(role_key)
    .bind(enabled)
    .execute(pool)
    .await
    .unwrap();
    binding_id
}

async fn wait_for_definition_lock_wait(
    observer: &PgPool,
    import_pid: i32,
    definition_blocker_pid: i32,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked_by_definition: bool =
                sqlx::query_scalar("SELECT $1 = ANY(pg_blocking_pids($2))")
                    .bind(definition_blocker_pid)
                    .bind(import_pid)
                    .fetch_one(observer)
                    .await
                    .unwrap();
            if blocked_by_definition {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("import must reach and wait on the locked definition version");
}

async fn assert_binding_mutation_times_out(
    pool: &PgPool,
    mutation: BindingMutation,
    domain_id: Uuid,
    principal_id: Uuid,
    binding_id: Uuid,
) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL statement_timeout = '300ms'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let result = match mutation {
        BindingMutation::InsertEnabledMigration => {
            sqlx::query(
                "INSERT INTO domain_role_bindings
             (binding_id, domain_id, principal_id, role_key, enabled)
             VALUES ($1, $2, $3, 'WORKFLOW_MIGRATION', TRUE)",
            )
            .bind(binding_id)
            .bind(domain_id)
            .bind(principal_id)
            .execute(&mut *tx)
            .await
        }
        BindingMutation::EnableDisabledMigration => {
            sqlx::query("UPDATE domain_role_bindings SET enabled = TRUE WHERE binding_id = $1")
                .bind(binding_id)
                .execute(&mut *tx)
                .await
        }
        BindingMutation::RetagEnabledBinding => {
            sqlx::query(
                "UPDATE domain_role_bindings
             SET role_key = 'WORKFLOW_MIGRATION' WHERE binding_id = $1",
            )
            .bind(binding_id)
            .execute(&mut *tx)
            .await
        }
    };
    let error = result.expect_err("authorization mutation must wait for import locks");
    let sqlstate = match &error {
        sqlx::Error::Database(database) => database.code(),
        other => panic!("expected PostgreSQL statement timeout, got {other:?}"),
    };
    assert_eq!(sqlstate.as_deref(), Some("57014"));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn import_locks_the_complete_migration_authorization_predicate() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let disabled_principal = seed_service_principal(&fixture.pool, "disabled migration").await;
    let retag_principal = seed_service_principal(&fixture.pool, "import staging").await;
    let disabled_binding = seed_binding(
        &fixture.pool,
        fixture.domain,
        disabled_principal,
        "WORKFLOW_MIGRATION",
        false,
    )
    .await;
    let retag_binding = seed_binding(
        &fixture.pool,
        fixture.domain,
        retag_principal,
        "IMPORT_STAGING",
        true,
    )
    .await;

    let mut definition_blocker = fixture.pool.begin().await.unwrap();
    let definition_blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *definition_blocker)
        .await
        .unwrap();
    sqlx::query(
        "SELECT definition_version_id FROM workflow_definition_versions
         WHERE definition_version_id = $1 FOR UPDATE",
    )
    .bind(fixture.version)
    .fetch_one(&mut *definition_blocker)
    .await
    .unwrap();

    let import_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(DATABASE_URL)
        .await
        .unwrap();
    let import_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&import_pool)
        .await
        .unwrap();
    let command = fixture.command.clone();
    let import_task =
        tokio::spawn(async move { import_legacy_workflow_instance(&import_pool, command).await });

    wait_for_definition_lock_wait(&fixture.pool, import_pid, definition_blocker_pid).await;

    assert_binding_mutation_times_out(
        &fixture.pool,
        BindingMutation::InsertEnabledMigration,
        fixture.domain,
        retag_principal,
        Uuid::new_v4(),
    )
    .await;
    assert_binding_mutation_times_out(
        &fixture.pool,
        BindingMutation::EnableDisabledMigration,
        fixture.domain,
        disabled_principal,
        disabled_binding,
    )
    .await;
    assert_binding_mutation_times_out(
        &fixture.pool,
        BindingMutation::RetagEnabledBinding,
        fixture.domain,
        retag_principal,
        retag_binding,
    )
    .await;

    definition_blocker.commit().await.unwrap();
    let imported = tokio::time::timeout(Duration::from_secs(5), import_task)
        .await
        .expect("import must finish after definition lock is released")
        .expect("import task must not panic")
        .expect("import must succeed");
    assert_eq!(imported.workflow_state_version, 1);

    let enabled_migration_bindings: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_role_bindings
         WHERE domain_id = $1 AND role_key = 'WORKFLOW_MIGRATION' AND enabled = TRUE",
    )
    .bind(fixture.domain)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(enabled_migration_bindings, 1);
}
