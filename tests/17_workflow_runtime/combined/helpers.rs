use super::*;

/// Seed the PR 3D graph while still DRAFT, then publish it directly for runtime tests.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn seed_combined_graph(
    pool: &PgPool,
    domain_id: Uuid,
    draft_assignee: &str,
    normal_assignee: &str,
    fixed_principal_id: Option<Uuid>,
    context_schema: Option<&serde_json::Value>,
    submission_schema: Option<&serde_json::Value>,
) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
    let definition_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let draft_id = Uuid::new_v4();
    let normal_id = Uuid::new_v4();
    let primary_advance_id = Uuid::new_v4();
    let secondary_advance_id = Uuid::new_v4();
    let key = format!("combined-{}", &Uuid::new_v4().to_string()[..8]);

    sqlx::query(
        "INSERT INTO workflow_definitions \
         (workflow_definition_id, domain_id, definition_key, display_name) \
         VALUES ($1, $2, $3, 'Combined Test')",
    )
    .bind(definition_id)
    .bind(domain_id)
    .bind(key)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions \
         (definition_version_id, workflow_definition_id, version_number, \
          version_status, context_schema) VALUES ($1, $2, 1, 'DRAFT', $3)",
    )
    .bind(version_id)
    .bind(definition_id)
    .bind(context_schema)
    .execute(pool)
    .await
    .unwrap();

    let draft_fixed = (draft_assignee == "FIXED_PRINCIPAL")
        .then_some(fixed_principal_id)
        .flatten();
    let normal_fixed = (normal_assignee == "FIXED_PRINCIPAL")
        .then_some(fixed_principal_id)
        .flatten();
    sqlx::query(
        "INSERT INTO workflow_node_definitions \
         (node_id, definition_version_id, node_key, display_name, order_index, \
          node_type, assignee_ref_type, fixed_principal_id) \
         VALUES ($1, $2, 'draft', 'Draft', 0, 'DRAFT', $3::assignee_ref_type, $4)",
    )
    .bind(draft_id)
    .bind(version_id)
    .bind(draft_assignee)
    .bind(draft_fixed)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_definitions \
         (node_id, definition_version_id, node_key, display_name, order_index, \
          node_type, assignee_ref_type, fixed_principal_id) \
         VALUES ($1, $2, 'review', 'Review', 1, 'NORMAL', $3::assignee_ref_type, $4)",
    )
    .bind(normal_id)
    .bind(version_id)
    .bind(normal_assignee)
    .bind(normal_fixed)
    .execute(pool)
    .await
    .unwrap();
    for (transition_id, transition_key) in [
        (primary_advance_id, "primary-advance"),
        (secondary_advance_id, "secondary-advance"),
    ] {
        sqlx::query(
            "INSERT INTO workflow_transition_definitions \
             (transition_id, definition_version_id, transition_key, display_name, \
              source_node_id, target_node_id, transition_effect, submission_schema) \
             VALUES ($1, $2, $3, 'Advance', $4, $5, 'ADVANCE', $6)",
        )
        .bind(transition_id)
        .bind(version_id)
        .bind(transition_key)
        .bind(draft_id)
        .bind(normal_id)
        .bind(submission_schema)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE workflow_node_definitions SET primary_advance_transition_id = $1 \
         WHERE node_id = $2",
    )
    .bind(primary_advance_id)
    .bind(draft_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED' \
         WHERE definition_version_id = $1",
    )
    .bind(version_id)
    .execute(pool)
    .await
    .unwrap();

    (
        version_id,
        draft_id,
        normal_id,
        primary_advance_id,
        secondary_advance_id,
    )
}

pub(crate) fn make_combined_command(
    principal_id: Uuid,
    instance_id: Uuid,
    expected_version: i32,
    transition_id: Uuid,
) -> ReviseContextAndTransitionCommand {
    ReviseContextAndTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: Uuid::new_v4().to_string(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: expected_version,
        transition_definition_id: TransitionId::from_uuid(transition_id),
        context_payload: serde_json::json!({"title": "revised"}),
        submission_payload: serde_json::json!({"summary": "ready"}),
    }
}

pub(crate) async fn create_combined_instance(
    pool: &PgPool,
    principal_id: Uuid,
    domain_id: Uuid,
    version_id: Uuid,
    initial_context: serde_json::Value,
) -> CreateWorkflowInstanceResult {
    let mut command = make_command(principal_id, domain_id, version_id);
    command.context_payload = initial_context;
    create_workflow_instance(pool, command).await.unwrap()
}
