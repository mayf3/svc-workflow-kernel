use super::*;

#[derive(sqlx::FromRow)]
struct CombinedEventRow {
    event_type: String,
    transition_effect: Option<String>,
    source_node_visit_id: Option<Uuid>,
    target_node_visit_id: Option<Uuid>,
    context_revision_id: Option<Uuid>,
    submission_id: Option<Uuid>,
    from_node_id: Option<Uuid>,
    to_node_id: Option<Uuid>,
    event_sequence: i32,
    old_workflow_state_version: i32,
    new_workflow_state_version: i32,
    event_data: serde_json::Value,
    event_data_digest: String,
}

#[tokio::test]
async fn combined_command_commits_one_atomic_state_change() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (version_id, draft_id, normal_id, advance_id, _) = seed_combined_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "WORKFLOW_CREATOR",
        None,
        None,
        None,
    )
    .await;
    let created = create_combined_instance(
        &pool,
        principal_id,
        domain_id,
        version_id,
        serde_json::json!({"title": "initial"}),
    )
    .await;

    let old_context_id = created.current_context_revision_id;
    let source_visit_id = created.current_node_visit_id;
    let context_payload = serde_json::json!({"title": "revised", "priority": 2});
    let submission_payload = serde_json::json!({"summary": "ready", "checks": ["fmt"]});
    let mut command =
        make_combined_command(principal_id, created.workflow_instance_id, 1, advance_id);
    command.context_payload = context_payload.clone();
    command.submission_payload = submission_payload.clone();

    let result = revise_context_and_transition(&pool, command).await.unwrap();

    assert_eq!(result.workflow_state_version, 2);
    assert_eq!(result.event_sequence, 2);
    assert_eq!(result.source_node_visit_id, source_visit_id);
    assert_ne!(result.current_context_revision_id, old_context_id);
    assert_ne!(result.current_node_visit_id, source_visit_id);

    let instance: (Uuid, Uuid, i32) = sqlx::query_as(
        "SELECT current_context_revision_id, current_node_visit_id, workflow_state_version \
         FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(created.workflow_instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(instance.0, result.current_context_revision_id);
    assert_eq!(instance.1, result.current_node_visit_id);
    assert_eq!(instance.2, 2);

    let context: (i32, Option<Uuid>, serde_json::Value, String) = sqlx::query_as(
        "SELECT revision_number, previous_revision_id, payload, payload_digest \
         FROM workflow_context_revisions WHERE context_revision_id = $1",
    )
    .bind(result.current_context_revision_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(context.0, 2);
    assert_eq!(context.1, Some(old_context_id));
    assert_eq!(context.2, context_payload);
    assert_eq!(
        context.3,
        svc_workflow::domain::definition::digest::compute_json_digest(&context.2).unwrap()
    );

    let submission: (Uuid, Uuid, Uuid, serde_json::Value, String) = sqlx::query_as(
        "SELECT source_node_visit_id, context_revision_id, transition_id, payload, payload_digest \
         FROM workflow_submissions WHERE submission_id = $1",
    )
    .bind(result.submission_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(submission.0, source_visit_id);
    assert_eq!(submission.1, result.current_context_revision_id);
    assert_eq!(submission.2, advance_id);
    assert_eq!(submission.3, submission_payload);
    assert_eq!(
        submission.4,
        svc_workflow::domain::definition::digest::compute_json_digest(&submission.3).unwrap()
    );

    let visit: (Uuid, i32, Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT node_id, visit_number, assignee_principal_id, entered_by_transition_id \
         FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(result.current_node_visit_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(visit, (normal_id, 1, principal_id, Some(advance_id)));

    let event: CombinedEventRow = sqlx::query_as(
        "SELECT event_type, transition_effect::TEXT, source_node_visit_id, \
                target_node_visit_id, context_revision_id, submission_id, from_node_id, to_node_id, \
                event_sequence, old_workflow_state_version, new_workflow_state_version, \
                event_data, event_data_digest \
         FROM workflow_events WHERE workflow_instance_id = $1 AND event_sequence = 2",
    )
    .bind(created.workflow_instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        event.event_type,
        "WORKFLOW_CONTEXT_REVISED_AND_TRANSITION_COMMITTED"
    );
    assert_eq!(event.transition_effect.as_deref(), Some("ADVANCE"));
    assert_eq!(event.source_node_visit_id, Some(source_visit_id));
    assert_eq!(
        event.target_node_visit_id,
        Some(result.current_node_visit_id)
    );
    assert_eq!(
        event.context_revision_id,
        Some(result.current_context_revision_id)
    );
    assert_eq!(event.submission_id, Some(result.submission_id));
    assert_eq!(event.from_node_id, Some(draft_id));
    assert_eq!(event.to_node_id, Some(normal_id));
    assert_eq!(
        (
            event.event_sequence,
            event.old_workflow_state_version,
            event.new_workflow_state_version,
        ),
        (2, 1, 2)
    );
    assert_eq!(
        event.event_data_digest,
        svc_workflow::domain::definition::digest::compute_json_digest(&event.event_data).unwrap()
    );

    let event_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), \
                COUNT(*) FILTER (WHERE event_type = 'CONTEXT_REVISED'), \
                COUNT(*) FILTER (WHERE event_type = 'WORKFLOW_TRANSITION_COMMITTED') \
         FROM workflow_events WHERE workflow_instance_id = $1",
    )
    .bind(created.workflow_instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event_counts, (2, 0, 0));

    let receipt: (String, i32, serde_json::Value, String) = sqlx::query_as(
        "SELECT command_type, response_status, response_body, response_digest \
         FROM workflow_command_receipts \
         WHERE principal_id = $1 AND command_type = 'REVISE_CONTEXT_AND_TRANSITION'",
    )
    .bind(principal_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt.0, "REVISE_CONTEXT_AND_TRANSITION");
    assert_eq!(receipt.1, 200);
    assert_eq!(receipt.2["submissionId"], result.submission_id.to_string());
    assert_eq!(
        receipt.3,
        svc_workflow::domain::definition::digest::compute_json_digest(&receipt.2).unwrap()
    );
}

#[tokio::test]
async fn combined_command_resolves_domain_owner_target_assignee() {
    let pool = create_pool().await;
    let (creator_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let owner_id: Uuid = sqlx::query_scalar(
        "SELECT principal_id FROM domain_role_bindings \
         WHERE domain_id = $1 AND role_key = 'DOMAIN_OWNER' AND enabled",
    )
    .bind(domain_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (version_id, _, _, advance_id, _) = seed_combined_graph(
        &pool,
        domain_id,
        "WORKFLOW_CREATOR",
        "DOMAIN_OWNER",
        None,
        None,
        None,
    )
    .await;
    let created = create_combined_instance(
        &pool,
        creator_id,
        domain_id,
        version_id,
        serde_json::json!({}),
    )
    .await;
    let result = revise_context_and_transition(
        &pool,
        make_combined_command(creator_id, created.workflow_instance_id, 1, advance_id),
    )
    .await
    .unwrap();
    let assignee: Uuid = sqlx::query_scalar(
        "SELECT assignee_principal_id FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(result.current_node_visit_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(assignee, owner_id);
}
