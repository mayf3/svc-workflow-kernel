use std::sync::Arc;

use super::*;
use tokio::sync::Barrier;

use svc_workflow::application::workflow_instance::query_types::*;

#[tokio::test]
async fn detail_snapshot_is_wholly_before_or_after_a_concurrent_transition() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    for _ in 0..10 {
        let created = create_query_instance(&pool, &seed).await;
        let barrier = Arc::new(Barrier::new(3));
        let query_barrier = barrier.clone();
        let query_service = query_service(&pool);
        let instance = created.workflow_instance_id;
        let creator = seed.creator;
        let query_task = tokio::spawn(async move {
            query_barrier.wait().await;
            query_service
                .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                    actor_principal_id: creator,
                    workflow_instance_id: instance,
                })
                .await
        });
        let command_barrier = barrier.clone();
        let command_pool = pool.clone();
        let transition = seed.draft_advance;
        let command_task = tokio::spawn(async move {
            command_barrier.wait().await;
            execute_workflow_transition(
                &command_pool,
                make_transition_command(
                    creator,
                    instance,
                    1,
                    transition,
                    Some(serde_json::json!({})),
                ),
            )
            .await
        });
        barrier.wait().await;
        let detail = query_task.await.unwrap().unwrap();
        command_task.await.unwrap().unwrap();
        match detail {
            WorkflowInstanceDetail::Full(pre) => {
                assert_eq!(pre.instance.workflow_state_version, 1);
                assert_eq!(pre.instance.current_node.node_type, "DRAFT");
                assert_eq!(
                    pre.current_context_revision_id,
                    created.current_context_revision_id
                );
                assert_eq!(pre.current_node_visit_id, created.current_node_visit_id);
            }
            WorkflowInstanceDetail::HistoricalParticipant(post) => {
                assert_eq!(post.instance.workflow_state_version, 2);
                assert_eq!(post.instance.current_node.node_type, "NORMAL");
            }
        }
    }
}

#[tokio::test]
async fn repeatable_read_detail_stays_pre_revision_when_later_read_is_blocked() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE workflow_transition_definitions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();

    let service = query_service(&pool);
    let instance = created.workflow_instance_id;
    let creator = seed.creator;
    let query_task = tokio::spawn(async move {
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: creator,
                workflow_instance_id: instance,
            })
            .await
    });
    for _ in 0..100 {
        if query_task.is_finished() {
            break;
        }
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'
               AND query LIKE '%FROM workflow_transition_definitions t%')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if waiting {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !query_task.is_finished(),
        "query should be paused after establishing its snapshot"
    );
    let revised = revise_workflow_context(
        &pool,
        ReviseWorkflowContextCommand {
            principal_id: PrincipalId::from_uuid(seed.creator),
            idempotency_key: Uuid::new_v4().to_string(),
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(instance),
            expected_workflow_state_version: 1,
            context_payload: serde_json::json!({"title": "revised concurrently"}),
        },
    )
    .await
    .unwrap();
    blocker.rollback().await.unwrap();

    let WorkflowInstanceDetail::Full(pre) = query_task.await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(pre.instance.workflow_state_version, 1);
    assert_eq!(
        pre.current_context_revision_id,
        created.current_context_revision_id
    );
    assert_eq!(
        pre.current_context.payload,
        serde_json::json!({"title": "initial"})
    );
    let WorkflowInstanceDetail::Full(post) = query_service(&pool)
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: seed.creator,
            workflow_instance_id: instance,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(post.instance.workflow_state_version, 2);
    assert_eq!(
        post.current_context_revision_id,
        revised.current_context_revision_id
    );
    assert_eq!(
        post.current_context.payload,
        serde_json::json!({"title": "revised concurrently"})
    );
}
