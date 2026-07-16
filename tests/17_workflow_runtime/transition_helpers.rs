//! Seed helpers and test builders for transition tests.

use super::*;

/// Seed a published definition with DRAFT → NORMAL → TERMINAL graph,
/// including RETURN (NORMAL→DRAFT) and TERMINATE (NORMAL→TERMINAL) transitions.
/// Returns (domain_id, definition_version_id, draft_node_id, normal_node_id,
///          terminal_node_id, draft_advance_id, normal_advance_id, return_id, terminate_id).
#[allow(clippy::too_many_arguments, dead_code)]
pub(crate) async fn seed_transition_graph(
    pool: &PgPool,
    domain_id: Uuid,
    draft_assignee: &str,
    normal_assignee: &str,
    fixed_principal_id: Option<Uuid>,
) -> (Uuid, Uuid, Uuid, Uuid, Uuid, Uuid, Uuid, Uuid, Uuid) {
    let def_id = Uuid::new_v4();
    let ver_id = Uuid::new_v4();
    let def_key = format!("tgraph-{}", &Uuid::new_v4().to_string()[..8]);

    sqlx::query("INSERT INTO workflow_definitions (workflow_definition_id, domain_id, definition_key, display_name) VALUES ($1, $2, $3, 'Trans Graph')")
        .bind(def_id).bind(domain_id).bind(&def_key)
        .execute(pool).await.expect("insert def");

    sqlx::query("INSERT INTO workflow_definition_versions (definition_version_id, workflow_definition_id, version_number, version_status, context_schema) VALUES ($1, $2, 1, 'DRAFT', NULL)")
        .bind(ver_id).bind(def_id).execute(pool).await.expect("insert version");

    let draft_id = Uuid::new_v4();
    let normal_id = Uuid::new_v4();
    let term_id = Uuid::new_v4();

    let draft_fixed = if draft_assignee == "FIXED_PRINCIPAL" {
        fixed_principal_id
    } else {
        None
    };
    let normal_fixed = if normal_assignee == "FIXED_PRINCIPAL" {
        fixed_principal_id
    } else {
        None
    };

    sqlx::query("INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type, fixed_principal_id) VALUES ($1, $2, 'draft', 'Draft', 0, 'DRAFT', $3::assignee_ref_type, $4)")
        .bind(draft_id).bind(ver_id).bind(draft_assignee).bind(draft_fixed)
        .execute(pool).await.expect("insert draft node");
    sqlx::query("INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type, fixed_principal_id) VALUES ($1, $2, 'review', 'Review', 1, 'NORMAL', $3::assignee_ref_type, $4)")
        .bind(normal_id).bind(ver_id).bind(normal_assignee).bind(normal_fixed)
        .execute(pool).await.expect("insert normal node");
    sqlx::query("INSERT INTO workflow_node_definitions (node_id, definition_version_id, node_key, display_name, order_index, node_type, assignee_ref_type) VALUES ($1, $2, 'done', 'Done', 2, 'TERMINAL', NULL)")
        .bind(term_id).bind(ver_id).execute(pool).await.expect("insert terminal node");

    let adv_id = Uuid::new_v4();
    sqlx::query("INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect) VALUES ($1, $2, 'advance-draft', 'To Review', $3, $4, 'ADVANCE')")
        .bind(adv_id).bind(ver_id).bind(draft_id).bind(normal_id)
        .execute(pool).await.expect("insert advance draft→normal");
    sqlx::query("UPDATE workflow_node_definitions SET primary_advance_transition_id = $1 WHERE node_id = $2")
        .bind(adv_id).bind(draft_id).execute(pool).await.expect("set primary on draft");

    let adv2_id = Uuid::new_v4();
    sqlx::query("INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect) VALUES ($1, $2, 'advance-done', 'To Done', $3, $4, 'ADVANCE')")
        .bind(adv2_id).bind(ver_id).bind(normal_id).bind(term_id)
        .execute(pool).await.expect("insert advance normal→done");
    sqlx::query("UPDATE workflow_node_definitions SET primary_advance_transition_id = $1 WHERE node_id = $2")
        .bind(adv2_id).bind(normal_id).execute(pool).await.expect("set primary on normal");

    let ret_id = Uuid::new_v4();
    sqlx::query("INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect, submission_schema) VALUES ($1, $2, 'return-to-draft', 'Return to Draft', $3, $4, 'RETURN', '{\"type\":\"object\",\"required\":[\"reasonCode\",\"reason\"],\"properties\":{\"reasonCode\":{\"type\":\"string\"},\"reason\":{\"type\":\"string\"},\"rootCauseNodeVisitId\":{\"type\":\"string\"},\"relatedSubmissionIds\":{\"type\":\"array\",\"items\":{\"type\":\"string\"}}}}'::jsonb)")
        .bind(ret_id).bind(ver_id).bind(normal_id).bind(draft_id)
        .execute(pool).await.expect("insert return transition");

    let term_trans_id = Uuid::new_v4();
    sqlx::query("INSERT INTO workflow_transition_definitions (transition_id, definition_version_id, transition_key, display_name, source_node_id, target_node_id, transition_effect, submission_schema) VALUES ($1, $2, 'terminate', 'Terminate', $3, $4, 'TERMINATE', '{\"type\":\"object\",\"required\":[\"reasonCode\",\"reason\"],\"properties\":{\"reasonCode\":{\"type\":\"string\"},\"reason\":{\"type\":\"string\"}}}'::jsonb)")
        .bind(term_trans_id).bind(ver_id).bind(normal_id).bind(term_id)
        .execute(pool).await.expect("insert terminate transition");

    sqlx::query("UPDATE workflow_definition_versions SET version_status = 'PUBLISHED' WHERE definition_version_id = $1")
        .bind(ver_id).execute(pool).await.expect("publish version");

    (
        domain_id,
        ver_id,
        draft_id,
        normal_id,
        term_id,
        adv_id,
        adv2_id,
        ret_id,
        term_trans_id,
    )
}

/// Create an instance and advance from DRAFT to the NORMAL node.
#[allow(dead_code)]
pub(crate) async fn create_and_advance_to_normal(
    pool: &PgPool,
    principal_id: Uuid,
    domain_id: Uuid,
    advance_transition_id: Uuid,
    ver_id: Uuid,
) -> (Uuid, Uuid, Uuid) {
    let create_cmd = make_command(principal_id, domain_id, ver_id);
    let create_result = create_workflow_instance(pool, create_cmd)
        .await
        .expect("create");
    let instance_id = create_result.workflow_instance_id;

    let trans_cmd = ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: Uuid::new_v4().to_string(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(instance_id),
        expected_workflow_state_version: 1,
        transition_definition_id: TransitionId::from_uuid(advance_transition_id),
        submission_payload: None,
    };

    let trans_result = execute_workflow_transition(pool, trans_cmd)
        .await
        .expect("advance to normal");
    (
        principal_id,
        instance_id,
        trans_result.current_node_visit_id,
    )
}

/// Build a transition command for testing.
#[allow(dead_code)]
pub(crate) fn make_transition_command(
    principal_id: Uuid,
    workflow_instance_id: Uuid,
    expected_state_version: i32,
    transition_definition_id: Uuid,
    submission_payload: Option<serde_json::Value>,
) -> ExecuteWorkflowTransitionCommand {
    ExecuteWorkflowTransitionCommand {
        principal_id: PrincipalId::from_uuid(principal_id),
        idempotency_key: Uuid::new_v4().to_string(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(workflow_instance_id),
        expected_workflow_state_version: expected_state_version,
        transition_definition_id: TransitionId::from_uuid(transition_definition_id),
        submission_payload,
    }
}
