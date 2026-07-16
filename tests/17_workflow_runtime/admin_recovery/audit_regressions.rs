use super::*;

use svc_workflow::application::workflow_instance::query_service::WorkflowQueryService;
use svc_workflow::application::workflow_instance::query_types::ListWorkflowTimeline;
use svc_workflow::domain::definition::digest;
use svc_workflow::domain::workflow_instance::recovery::{AdminEmergencyOperation, RecoveryError};

#[tokio::test]
async fn override_rejects_projection_drift_before_writing_any_fact() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    sqlx::query(
        "UPDATE workflow_instances SET workflow_state_version = 99
         WHERE workflow_instance_id = $1",
    )
    .bind(fixture.instance)
    .execute(&pool)
    .await
    .unwrap();
    let command = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    let key = command.idempotency_key.clone();
    assert!(matches!(
        run_override(&pool, command).await.unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
    let receipt: (String, i32) = sqlx::query_as(
        "SELECT receipt_status::text, response_status
         FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(fixture.admin)
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt, ("COMPLETED".to_string(), 500));

    let stale = seed_recovery_fixture(&pool).await;
    let advance: Uuid = sqlx::query_scalar(
        "SELECT primary_advance_transition_id FROM workflow_node_definitions
         WHERE node_id = $1",
    )
    .bind(stale.draft)
    .fetch_one(&pool)
    .await
    .unwrap();
    execute_workflow_transition(
        &pool,
        make_transition_command(stale.creator, stale.instance, 1, advance, None),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_instances SET current_node_visit_id = $2
         WHERE workflow_instance_id = $1",
    )
    .bind(stale.instance)
    .bind(stale.initial_visit)
    .execute(&pool)
    .await
    .unwrap();
    let mut stale_command = override_command(
        &stale,
        AdminEmergencyOperation::TerminateInstance,
        stale.terminal,
    );
    stale_command.expected_workflow_state_version = 2;
    assert!(matches!(
        run_override(&pool, stale_command).await.unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));
    assert_eq!(
        count_instance_facts(&pool, stale.instance).await,
        (1, 2, 0, 2)
    );
}

#[tokio::test]
async fn replay_rejects_event_that_branches_back_to_an_old_context() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let revised = revise_workflow_context(
        &pool,
        ReviseWorkflowContextCommand {
            principal_id: PrincipalId::from_uuid(fixture.creator),
            idempotency_key: Uuid::new_v4().to_string(),
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(fixture.instance),
            expected_workflow_state_version: 1,
            context_payload: serde_json::json!({"revision": 2}),
        },
    )
    .await
    .unwrap();
    let (old_digest, new_digest): (String, String) = sqlx::query_as(
        "SELECT old.payload_digest, new.payload_digest
         FROM workflow_context_revisions old, workflow_context_revisions new
         WHERE old.context_revision_id = $1 AND new.context_revision_id = $2",
    )
    .bind(fixture.initial_context)
    .bind(revised.current_context_revision_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let data = serde_json::json!({
        "previous_context_revision_id": revised.current_context_revision_id,
        "new_context_revision_id": fixture.initial_context,
        "previous_payload_digest": new_digest,
        "new_payload_digest": old_digest,
        "current_node_id": fixture.draft,
    });
    let data_digest = digest::compute_json_digest(&data).unwrap();
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          event_type, source_node_visit_id, target_node_visit_id,
          context_revision_id, event_data, event_data_digest, actor_principal_id,
          old_workflow_state_version, new_workflow_state_version)
         VALUES ($1, $2, 3, 'v1', 'CONTEXT_REVISED', $3, $3, $4,
                 $5, $6, $7, 2, 3)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.instance)
    .bind(fixture.initial_visit)
    .bind(fixture.initial_context)
    .bind(data)
    .bind(data_digest)
    .bind(fixture.creator)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        run_rebuild(&pool, rebuild_command(&fixture))
            .await
            .unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));
}

#[tokio::test]
async fn override_reason_is_canonical_and_bounded_before_storage() {
    let pool = create_pool().await;
    for reason in [" leading", "trailing ", &"x".repeat(2001)] {
        let fixture = seed_recovery_fixture(&pool).await;
        let mut command = override_command(
            &fixture,
            AdminEmergencyOperation::MoveToNode,
            fixture.normal,
        );
        command.reason = reason.to_string();
        assert!(matches!(
            run_override(&pool, command).await.unwrap_err(),
            RecoveryError::InvalidInput(_)
        ));
        assert_eq!(
            count_instance_facts(&pool, fixture.instance).await,
            (1, 1, 0, 1)
        );
    }
}

#[tokio::test]
async fn rebuild_accepts_a_locked_draft_definition_for_defensive_repair() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'DRAFT'
         WHERE definition_version_id = $1",
    )
    .bind(fixture.version)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(run_rebuild(&pool, rebuild_command(&fixture)).await.is_ok());
}

#[tokio::test]
async fn historical_timeline_redacts_admin_reason_but_full_scope_retains_it() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let secret = "restricted incident reference INC-SECRET-42";
    let mut command = override_command(
        &fixture,
        AdminEmergencyOperation::TerminateInstance,
        fixture.terminal,
    );
    command.reason = secret.to_string();
    let result = run_override(&pool, command).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_node_visits SET assignee_principal_id = $2
         WHERE node_visit_id = $1",
    )
    .bind(result.current_node_visit_id)
    .bind(fixture.outsider)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let service = WorkflowQueryService::new(pool.clone());
    let restricted = service
        .list_workflow_timeline(ListWorkflowTimeline {
            actor_principal_id: fixture.outsider,
            workflow_instance_id: fixture.instance,
            after_event_sequence: None,
            limit: None,
        })
        .await
        .unwrap();
    let public = restricted
        .items
        .iter()
        .find(|event| event.event_type == "ADMIN_EMERGENCY_OVERRIDE_COMMITTED")
        .expect("terminal admin outcome remains visible");
    assert_eq!(public.event_data, None);
    assert_eq!(public.event_data_digest, None);

    let full = service
        .list_workflow_timeline(ListWorkflowTimeline {
            actor_principal_id: fixture.creator,
            workflow_instance_id: fixture.instance,
            after_event_sequence: None,
            limit: None,
        })
        .await
        .unwrap();
    let private = full
        .items
        .iter()
        .find(|event| event.event_type == "ADMIN_EMERGENCY_OVERRIDE_COMMITTED")
        .unwrap();
    assert_eq!(private.event_data.as_ref().unwrap()["reason"], secret);
    assert!(private.event_data_digest.is_some());
}
