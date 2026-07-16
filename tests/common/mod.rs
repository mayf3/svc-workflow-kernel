//! Test helper utilities for PostgreSQL-backed integration tests.
//!
//! The seed functions below may appear unused in individual test binaries
//! because they are compiled per test file. Allow dead_code at module level.

#![allow(dead_code)]

use sqlx::PgPool;

/// Default test database URL.
const TEST_DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

/// Create a new pool for a test and ensure migrations are applied.
///
/// SQLx migration tracking is idempotent: the `_sqlx_migrations` table records
/// which migrations have been applied. Calling `run()` multiple times is safe
/// and will only apply pending migrations.
pub async fn create_pool() -> PgPool {
    let pool = PgPool::connect(TEST_DATABASE_URL)
        .await
        .expect("failed to connect to test database");

    let migrator = sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
        .await
        .expect("failed to load migrations");
    migrator
        .run(&pool)
        .await
        .expect("failed to run migrations on test database");

    pool
}

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

/// Seed a minimal set of principals and a domain for tests.
/// Returns (principal_id, domain_id).
pub async fn seed_principal_and_domain(pool: &PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let principal_id = uuid::Uuid::new_v4();
    let domain_id = uuid::Uuid::new_v4();
    let domain_key = format!("test-domain-{}", &uuid::Uuid::new_v4().to_string()[..8]);

    sqlx::query(
        r#"
        INSERT INTO principals (principal_id, principal_type, display_name, email, enabled)
        VALUES ($1, 'HUMAN', 'Test User', 'test@example.com', TRUE)
        "#,
    )
    .bind(principal_id)
    .execute(pool)
    .await
    .expect("failed to insert test principal");

    sqlx::query(
        r#"
        INSERT INTO domains (domain_id, domain_key, display_name, enabled)
        VALUES ($1, $2, 'Test Domain', TRUE)
        "#,
    )
    .bind(domain_id)
    .bind(&domain_key)
    .execute(pool)
    .await
    .expect("failed to insert test domain");

    (principal_id, domain_id)
}

/// Seed a principal, domain, and domain owner binding in one call.
/// Returns (principal_id, domain_id).
pub async fn seed_principal_domain_with_owner(pool: &PgPool) -> (uuid::Uuid, uuid::Uuid) {
    let (principal_id, domain_id) = seed_principal_and_domain(pool).await;
    seed_domain_owner(pool, domain_id, principal_id).await;
    (principal_id, domain_id)
}

/// Seed a second principal (for multiple-principal tests).
pub async fn seed_second_principal(pool: &PgPool) -> uuid::Uuid {
    let principal_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO principals (principal_id, principal_type, display_name, email, enabled)
        VALUES ($1, 'AGENT', 'Test Agent', 'agent@example.com', TRUE)
        "#,
    )
    .bind(principal_id)
    .execute(pool)
    .await
    .expect("failed to insert second principal");
    principal_id
}

/// Seed a domain owner binding.
pub async fn seed_domain_owner(
    pool: &PgPool,
    domain_id: uuid::Uuid,
    principal_id: uuid::Uuid,
) -> uuid::Uuid {
    let binding_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO domain_role_bindings (binding_id, domain_id, principal_id, role_key, enabled)
        VALUES ($1, $2, $3, 'DOMAIN_OWNER', TRUE)
        "#,
    )
    .bind(binding_id)
    .bind(domain_id)
    .bind(principal_id)
    .execute(pool)
    .await
    .expect("failed to insert domain owner binding");
    binding_id
}

/// Seed a complete minimal workflow definition with one node and one transition.
/// Returns (definition_id, version_id, node_id, transition_id).
pub async fn seed_workflow_definition(
    pool: &PgPool,
    domain_id: uuid::Uuid,
) -> (uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid) {
    let def_id = uuid::Uuid::new_v4();
    let ver_id = uuid::Uuid::new_v4();
    let node_id = uuid::Uuid::new_v4();
    let trans_id = uuid::Uuid::new_v4();

    let def_key = format!("test-def-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    sqlx::query(
        r#"
        INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name)
        VALUES ($1, $2, $3, 'Test Definition')
        "#,
    )
    .bind(def_id)
    .bind(domain_id)
    .bind(&def_key)
    .execute(pool)
    .await
    .expect("failed to insert workflow definition");

    sqlx::query(
        r#"
        INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema, submission_schema)
        VALUES ($1, $2, 1, 'DRAFT', '{"type":"object"}'::jsonb, '{"type":"object"}'::jsonb)
        "#,
    )
    .bind(ver_id)
    .bind(def_id)
    .execute(pool)
    .await
    .expect("failed to insert definition version");

    let principal_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO principals (principal_id, principal_type, display_name, email, enabled)
        VALUES ($1, 'HUMAN', 'Assignee', 'assignee@example.com', TRUE)
        "#,
    )
    .bind(principal_id)
    .execute(pool)
    .await
    .expect("failed to insert assignee principal");

    sqlx::query(
        r#"
        INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type, fixed_principal_id)
        VALUES ($1, $2, 'draft', 'Draft', 0, 'DRAFT', 'FIXED_PRINCIPAL', $3)
        "#,
    )
    .bind(node_id)
    .bind(ver_id)
    .bind(principal_id)
    .execute(pool)
    .await
    .expect("failed to insert node definition");

    let terminal_node_id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type)
        VALUES ($1, $2, 'done', 'Done', 1, 'TERMINAL', NULL)
        "#,
    )
    .bind(terminal_node_id)
    .bind(ver_id)
    .execute(pool)
    .await
    .expect("failed to insert terminal node");

    sqlx::query(
        r#"
        INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect)
        VALUES ($1, $2, 'advance-done', 'Complete', $3, $4, 'ADVANCE')
        "#,
    )
    .bind(trans_id)
    .bind(ver_id)
    .bind(node_id)
    .bind(terminal_node_id)
    .execute(pool)
    .await
    .expect("failed to insert transition");

    (def_id, ver_id, node_id, trans_id)
}
