#![allow(clippy::needless_borrow)]
//! Test: migrations apply cleanly on an empty database.

mod common;

#[tokio::test]
async fn test_migrations_apply_successfully() {
    let pool = common::create_pool().await;

    // Verify that key tables exist
    let tables = vec![
        "principals",
        "domains",
        "domain_role_bindings",
        "workflow_definitions",
        "workflow_definition_versions",
        "workflow_node_definitions",
        "workflow_transition_definitions",
        "workflow_instances",
        "workflow_context_revisions",
        "workflow_node_visits",
        "workflow_submissions",
        "workflow_events",
        "workflow_command_receipts",
        "workflow_command_attempt_audits",
        "workflow_security_audits",
    ];

    for table in &tables {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::int8 FROM pg_tables WHERE tablename = $1 AND schemaname = 'public'",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("query failed");

        assert!(row.0 > 0, "table '{}' does not exist", table);
    }
}

#[tokio::test]
async fn test_enums_exist() {
    let pool = common::create_pool().await;

    let enums = vec![
        "principal_type",
        "definition_version_status",
        "node_type",
        "assignee_ref_type",
        "transition_effect",
        "receipt_status",
    ];

    for enum_name in &enums {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::int8 FROM pg_type WHERE typname = $1 AND typtype = 'e'",
        )
        .bind(enum_name)
        .fetch_one(&pool)
        .await
        .expect("query failed");

        assert!(row.0 > 0, "enum '{}' does not exist", enum_name);
    }
}
