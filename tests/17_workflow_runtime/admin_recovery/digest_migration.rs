use super::*;

use svc_workflow::domain::workflow_instance::recovery::{
    BeforeSnapshotV1, WorkflowProjection, BEFORE_SNAPSHOT_SCHEMA_VERSION,
};

#[test]
fn before_snapshot_v1_jcs_and_sha256_are_golden() {
    let snapshot = BeforeSnapshotV1::new(
        Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
        Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
        Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap(),
        Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap(),
        &WorkflowProjection {
            current_context_revision_id: Some(
                Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap(),
            ),
            current_node_visit_id: Some(
                Uuid::parse_str("66666666-6666-6666-6666-666666666666").unwrap(),
            ),
            workflow_state_version: 7,
        },
    );
    assert_eq!(snapshot.schema_version, BEFORE_SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(
        snapshot.canonical_json().unwrap(),
        concat!(
            "{\"createdByPrincipalId\":\"44444444-4444-4444-4444-444444444444\",",
            "\"currentContextRevisionId\":\"55555555-5555-5555-5555-555555555555\",",
            "\"currentNodeVisitId\":\"66666666-6666-6666-6666-666666666666\",",
            "\"definitionVersionId\":\"33333333-3333-3333-3333-333333333333\",",
            "\"domainId\":\"22222222-2222-2222-2222-222222222222\",",
            "\"schemaVersion\":\"WORKFLOW_INSTANCE_BEFORE_SNAPSHOT_V1\",",
            "\"workflowInstanceId\":\"11111111-1111-1111-1111-111111111111\",",
            "\"workflowStateVersion\":7}"
        )
    );
    assert_eq!(
        snapshot.digest().unwrap(),
        "45dd7c7019adb7190517f36f5118c0ded7fc7c352a4015f353cd573ee2f018a3"
    );
}

#[tokio::test]
async fn migration_enforces_new_terminal_and_non_terminal_shapes() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let nullable: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name, column_name FROM information_schema.columns
         WHERE table_schema = 'public' AND is_nullable = 'YES'
           AND (table_name, column_name) IN (
             ('workflow_node_definitions', 'assignee_ref_type'),
             ('workflow_node_visits', 'assignee_principal_id'))
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(nullable.len(), 2);
    let validated: bool = sqlx::query_scalar(
        "SELECT convalidated FROM pg_constraint
         WHERE conname = 'chk_node_assignee_shape'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !validated,
        "legacy definition rows must remain grandfathered"
    );

    let definition = Uuid::new_v4();
    let draft_version = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_definitions
         (workflow_definition_id, domain_id, definition_key, display_name)
         VALUES ($1, $2, $3, 'Constraint Test')",
    )
    .bind(definition)
    .bind(fixture.domain)
    .bind(format!("constraint-{}", Uuid::new_v4().simple()))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions
         (definition_version_id, workflow_definition_id, version_number, version_status)
         VALUES ($1, $2, 1, 'DRAFT')",
    )
    .bind(draft_version)
    .bind(definition)
    .execute(&pool)
    .await
    .unwrap();
    let bad_terminal = sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type)
         VALUES ($1, $2, $3, 'Bad Terminal', 99, 'TERMINAL', 'WORKFLOW_CREATOR')",
    )
    .bind(Uuid::new_v4())
    .bind(draft_version)
    .bind(format!("bad-{}", Uuid::new_v4().simple()))
    .execute(&pool)
    .await;
    assert!(bad_terminal.is_err());

    let bad_non_terminal = sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type)
         VALUES ($1, $2, $3, 'Bad Normal', 98, 'NORMAL', NULL)",
    )
    .bind(Uuid::new_v4())
    .bind(draft_version)
    .bind(format!("bad-normal-{}", Uuid::new_v4().simple()))
    .execute(&pool)
    .await;
    assert!(bad_non_terminal.is_err());

    let foreign_node = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type)
         VALUES ($1, $2, $3, 'Foreign Normal', 97, 'NORMAL', 'WORKFLOW_CREATOR')",
    )
    .bind(foreign_node)
    .bind(draft_version)
    .bind(format!("foreign-normal-{}", Uuid::new_v4().simple()))
    .execute(&pool)
    .await
    .unwrap();
    let cross_version_visit = sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number,
          assignee_principal_id)
         VALUES ($1, $2, $3, 1, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.instance)
    .bind(foreign_node)
    .bind(fixture.creator)
    .execute(&pool)
    .await;
    assert!(cross_version_visit.is_err());

    let bad_visit = sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number,
          assignee_principal_id)
         VALUES ($1, $2, $3, 99, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.instance)
    .bind(fixture.terminal)
    .bind(fixture.creator)
    .execute(&pool)
    .await;
    assert!(bad_visit.is_err());

    let bad_normal_visit = sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number,
          assignee_principal_id)
         VALUES ($1, $2, $3, 99, NULL)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.instance)
    .bind(fixture.normal)
    .execute(&pool)
    .await;
    assert!(bad_normal_visit.is_err());
}
