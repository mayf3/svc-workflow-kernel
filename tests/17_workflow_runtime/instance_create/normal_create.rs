//! Normal creation tests (1-8).

use super::*;
use svc_workflow::domain::definition::digest;

#[tokio::test]
async fn test_create_success_wf_creator() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_domain, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let cmd = make_command(principal_id, domain_id, ver_id);
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    verify_creation(&pool, &result, principal_id, domain_id, ver_id).await;
}

#[tokio::test]
async fn test_create_success_domain_owner_assignee() {
    let pool = create_pool().await;
    let (owner_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_domain, ver_id) = seed_published_definition_domain_owner(&pool, domain_id).await;
    let cmd = make_command(owner_id, domain_id, ver_id);
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    verify_creation(&pool, &result, owner_id, domain_id, ver_id).await;
}

#[tokio::test]
async fn test_create_success_fixed_principal_assignee() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let fixed_id = seed_second_principal(&pool).await;
    let binding_id = Uuid::new_v4();
    sqlx::query("INSERT INTO domain_role_bindings (binding_id, domain_id, principal_id, role_key, enabled) VALUES ($1, $2, $3, 'MEMBER', TRUE)")
        .bind(binding_id).bind(domain_id).bind(fixed_id).execute(&pool).await.expect("insert binding");
    let (_domain, ver_id) =
        seed_published_definition_fixed_principal(&pool, domain_id, fixed_id).await;
    let cmd = make_command(principal_id, domain_id, ver_id);
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    verify_creation(&pool, &result, principal_id, domain_id, ver_id).await;
}

#[tokio::test]
async fn test_create_all_records_present() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_domain, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let cmd = make_command(principal_id, domain_id, ver_id);
    let idem_key = cmd.idempotency_key.clone();
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    verify_creation(&pool, &result, principal_id, domain_id, ver_id).await;
    let receipt: (String, i32,) = sqlx::query_as(
        "SELECT receipt_status::TEXT, response_status FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idem_key).fetch_one(&pool).await.expect("receipt");
    assert_eq!(receipt.0, "COMPLETED");
    assert_eq!(receipt.1, 200);
}

#[tokio::test]
async fn test_create_current_pointers_correct() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_domain, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let cmd = make_command(principal_id, domain_id, ver_id);
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    verify_creation(&pool, &result, principal_id, domain_id, ver_id).await;
    let inst: (Uuid, Uuid) = sqlx::query_as(
        "SELECT current_context_revision_id, current_node_visit_id FROM workflow_instances WHERE workflow_instance_id = $1",
    ).bind(result.workflow_instance_id).fetch_one(&pool).await.expect("instance");
    assert_eq!(inst.0, result.current_context_revision_id);
    assert_eq!(inst.1, result.current_node_visit_id);
}

#[tokio::test]
async fn test_create_event_field_matrix_correct() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_domain, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let cmd = make_command(principal_id, domain_id, ver_id);
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    let ev: (Option<Uuid>, Uuid, Uuid, Option<Uuid>, i32, i32, Uuid) = sqlx::query_as(
        "SELECT source_node_visit_id, target_node_visit_id, context_revision_id, submission_id, old_workflow_state_version, new_workflow_state_version, actor_principal_id FROM workflow_events WHERE workflow_instance_id = $1",
    ).bind(result.workflow_instance_id).fetch_one(&pool).await.expect("event");
    assert!(ev.0.is_none());
    assert_eq!(ev.1, result.current_node_visit_id);
    assert_eq!(ev.2, result.current_context_revision_id);
    assert!(ev.3.is_none());
    assert_eq!(ev.4, 0);
    assert_eq!(ev.5, 1);
    assert_eq!(ev.6, principal_id);
}

#[tokio::test]
async fn test_create_context_digest_readback() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_domain, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let cmd = make_command(principal_id, domain_id, ver_id);
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    let (payload_digest,): (String,) = sqlx::query_as(
        "SELECT payload_digest FROM workflow_context_revisions WHERE context_revision_id = $1",
    )
    .bind(result.current_context_revision_id)
    .fetch_one(&pool)
    .await
    .expect("context");
    let recomputed =
        digest::compute_json_digest(&serde_json::json!({"hello": "world"})).expect("digest");
    assert_eq!(payload_digest, recomputed);
}

#[tokio::test]
async fn test_create_response_digest_readback() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_domain, ver_id) = seed_published_definition_wf_creator(&pool, domain_id).await;
    let cmd = make_command(principal_id, domain_id, ver_id);
    let idem_key = cmd.idempotency_key.clone();
    let result = create_workflow_instance(&pool, cmd).await.expect("create");
    let (response_digest,): (String,) = sqlx::query_as(
        "SELECT response_digest FROM workflow_command_receipts WHERE principal_id = $1 AND idempotency_key = $2",
    ).bind(principal_id).bind(&idem_key).fetch_one(&pool).await.expect("receipt");
    let expected = digest::compute_json_digest(&serde_json::json!({
        "workflowInstanceId": result.workflow_instance_id, "workflowStateVersion": 1,
        "currentContextRevisionId": result.current_context_revision_id,
        "currentNodeVisitId": result.current_node_visit_id, "eventSequence": 1,
    }))
    .expect("digest");
    assert_eq!(response_digest, expected);
}
