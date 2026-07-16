use super::*;

use svc_workflow::application::workflow_instance::query_service::WorkflowQueryService;
use svc_workflow::application::workflow_instance::query_types::{
    GetWorkflowInstanceDetail, ListAssignedToMe, WorkflowInstanceDetail,
};
use svc_workflow::domain::workflow_instance::recovery::AdminEmergencyOperation;

#[tokio::test]
async fn move_to_node_commits_exactly_one_visit_event_and_projection_change() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let before = count_instance_facts(&pool, fixture.instance).await;
    let result = run_override(
        &pool,
        override_command(
            &fixture,
            AdminEmergencyOperation::MoveToNode,
            fixture.normal,
        ),
    )
    .await
    .unwrap();
    assert_eq!(result.source_node_visit_id, fixture.initial_visit);
    assert_ne!(result.current_node_visit_id, fixture.initial_visit);
    assert_eq!(result.workflow_state_version, 2);
    assert_eq!(result.event_sequence, 2);
    assert!(!result.replayed);
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 2, 0, 2)
    );
    assert_eq!(before, (1, 1, 0, 1));

    let visit: (Uuid, Option<Uuid>, Option<Uuid>, i32) = sqlx::query_as(
        "SELECT node_id, assignee_principal_id, entered_by_transition_id, visit_number
         FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(result.current_node_visit_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(visit, (fixture.normal, Some(fixture.creator), None, 1));
    let event: (String, String, Uuid, Uuid, Uuid, Option<Uuid>, i32, i32) = sqlx::query_as(
        "SELECT event_type, transition_effect::text, source_node_visit_id,
                target_node_visit_id, context_revision_id, submission_id,
                old_workflow_state_version, new_workflow_state_version
         FROM workflow_events WHERE command_id = $1",
    )
    .bind(result.command_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event.0, "ADMIN_EMERGENCY_OVERRIDE_COMMITTED");
    assert_eq!(event.1, "ADVANCE");
    assert_eq!(event.2, fixture.initial_visit);
    assert_eq!(event.3, result.current_node_visit_id);
    assert_eq!(event.4, fixture.initial_context);
    assert_eq!(event.5, None);
    assert_eq!((event.6, event.7), (1, 2));
}

#[tokio::test]
async fn terminate_creates_unassigned_terminal_visit_and_query_does_not_assign_work() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let result = run_override(
        &pool,
        override_command(
            &fixture,
            AdminEmergencyOperation::TerminateInstance,
            fixture.terminal,
        ),
    )
    .await
    .unwrap();
    let visit: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT n.node_type::text, v.assignee_principal_id
         FROM workflow_node_visits v JOIN workflow_node_definitions n ON n.node_id = v.node_id
         WHERE v.node_visit_id = $1",
    )
    .bind(result.current_node_visit_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(visit, ("TERMINAL".to_string(), None));

    let query = WorkflowQueryService::new(pool.clone());
    let WorkflowInstanceDetail::Full(detail) = query
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: fixture.creator,
            workflow_instance_id: fixture.instance,
        })
        .await
        .unwrap()
    else {
        panic!("domain owner should retain full terminal visibility")
    };
    assert!(detail.instance.is_terminal);
    assert_eq!(detail.current_visit.assignee_principal_id, None);
    let assigned = query
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: fixture.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(assigned
        .items
        .iter()
        .all(|item| item.detail.instance.workflow_instance_id != fixture.instance));
}

#[tokio::test]
async fn deprecated_and_revoked_versions_allow_override() {
    let pool = create_pool().await;
    let deprecated = seed_recovery_fixture(&pool).await;
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'DEPRECATED'
         WHERE definition_version_id = $1",
    )
    .bind(deprecated.version)
    .execute(&pool)
    .await
    .unwrap();
    assert!(run_override(
        &pool,
        override_command(
            &deprecated,
            AdminEmergencyOperation::MoveToNode,
            deprecated.normal,
        )
    )
    .await
    .is_ok());

    let revoked = seed_recovery_fixture(&pool).await;
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'REVOKED'
         WHERE definition_version_id = $1",
    )
    .bind(revoked.version)
    .execute(&pool)
    .await
    .unwrap();
    assert!(run_override(
        &pool,
        override_command(
            &revoked,
            AdminEmergencyOperation::TerminateInstance,
            revoked.terminal,
        )
    )
    .await
    .is_ok());
}

#[tokio::test]
async fn operation_target_matrix_is_enforced_without_new_facts() {
    let pool = create_pool().await;
    let move_fixture = seed_recovery_fixture(&pool).await;
    let before = count_instance_facts(&pool, move_fixture.instance).await;
    assert!(run_override(
        &pool,
        override_command(
            &move_fixture,
            AdminEmergencyOperation::MoveToNode,
            move_fixture.terminal,
        )
    )
    .await
    .is_err());
    assert_eq!(
        count_instance_facts(&pool, move_fixture.instance).await,
        before
    );

    let terminate_fixture = seed_recovery_fixture(&pool).await;
    assert!(run_override(
        &pool,
        override_command(
            &terminate_fixture,
            AdminEmergencyOperation::TerminateInstance,
            terminate_fixture.normal,
        )
    )
    .await
    .is_err());
    assert_eq!(
        count_instance_facts(&pool, terminate_fixture.instance).await,
        (1, 1, 0, 1)
    );
}

#[tokio::test]
async fn override_can_move_to_same_non_terminal_node_but_always_creates_a_visit() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let result = run_override(
        &pool,
        override_command(&fixture, AdminEmergencyOperation::MoveToNode, fixture.draft),
    )
    .await
    .unwrap();
    assert_ne!(result.current_node_visit_id, fixture.initial_visit);
    let visit_number: i32 = sqlx::query_scalar(
        "SELECT visit_number FROM workflow_node_visits WHERE node_visit_id = $1",
    )
    .bind(result.current_node_visit_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(visit_number, 2);
}
