use super::*;

use svc_workflow::application::workflow_instance::query_types::*;

fn timeline(
    actor: Uuid,
    instance: Uuid,
    after: Option<i32>,
    limit: Option<u32>,
) -> ListWorkflowTimeline {
    ListWorkflowTimeline {
        actor_principal_id: actor,
        workflow_instance_id: instance,
        after_event_sequence: after,
        limit,
    }
}

#[tokio::test]
async fn timeline_is_sequence_keyset_paginated_and_contains_complete_event_fields() {
    let pool = create_pool().await;
    let completed = complete_query_instance(&pool).await;
    let service = query_service(&pool);

    let first = service
        .list_workflow_timeline(timeline(
            completed.seed.owner,
            completed.instance,
            None,
            Some(2),
        ))
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.event_sequence)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(first.next_cursor, Some(2));
    let second = service
        .list_workflow_timeline(timeline(
            completed.seed.owner,
            completed.instance,
            first.next_cursor,
            Some(100),
        ))
        .await
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.event_sequence)
            .collect::<Vec<_>>(),
        vec![3, 4, 5]
    );
    assert_eq!(second.next_cursor, None);
    let return_event = second
        .items
        .iter()
        .find(|item| item.transition_effect.as_deref() == Some("RETURN"))
        .unwrap();
    assert_eq!(return_event.event_type, "WORKFLOW_TRANSITION_COMMITTED");
    assert_eq!(
        return_event.submission_id,
        Some(completed.feedback_submission)
    );
    assert_eq!(
        return_event.old_workflow_state_version + 1,
        return_event.new_workflow_state_version
    );
    assert_eq!(
        return_event.event_sequence,
        return_event.new_workflow_state_version
    );
    assert!(return_event.source_node_visit_id.is_some());
    assert!(return_event.target_node_visit_id.is_some());

    assert!(matches!(
        service
            .list_workflow_timeline(timeline(
                completed.seed.owner,
                completed.instance,
                None,
                Some(0)
            ))
            .await
            .unwrap_err(),
        WorkflowQueryError::InvalidPagination(_)
    ));
    assert!(matches!(
        service
            .list_workflow_timeline(timeline(
                completed.seed.owner,
                completed.instance,
                Some(-1),
                None
            ))
            .await
            .unwrap_err(),
        WorkflowQueryError::InvalidPagination(_)
    ));
}

#[tokio::test]
async fn historical_timeline_contains_only_own_feedback_and_terminal_outcome() {
    let pool = create_pool().await;
    let completed = complete_query_instance(&pool).await;
    let service = query_service(&pool);

    let creator = service
        .list_workflow_timeline(timeline(
            completed.seed.creator,
            completed.instance,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        creator
            .items
            .iter()
            .map(|item| item.event_sequence)
            .collect::<Vec<_>>(),
        vec![2, 3, 4, 5]
    );
    assert!(creator
        .items
        .iter()
        .any(|item| item.submission_id == Some(completed.creator_submission)));
    assert!(creator
        .items
        .iter()
        .any(|item| item.submission_id == Some(completed.feedback_submission)));
    assert!(creator
        .items
        .iter()
        .any(|item| item.transition_effect.as_deref() == Some("TERMINATE")));
    assert!(!creator.items.iter().any(|item| item.event_sequence == 1));

    let assignee = service
        .list_workflow_timeline(timeline(
            completed.seed.assignee,
            completed.instance,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        assignee
            .items
            .iter()
            .map(|item| item.event_sequence)
            .collect::<Vec<_>>(),
        vec![3, 5]
    );
}

#[tokio::test]
async fn context_history_is_full_only_ascending_and_restricted_attempt_is_audited() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    for (expected, title) in [(1, "second"), (2, "third")] {
        revise_workflow_context(
            &pool,
            ReviseWorkflowContextCommand {
                principal_id: PrincipalId::from_uuid(seed.creator),
                idempotency_key: Uuid::new_v4().to_string(),
                command_schema_version: "v1".to_string(),
                workflow_instance_id: WorkflowInstanceId::from_uuid(created.workflow_instance_id),
                expected_workflow_state_version: expected,
                context_payload: serde_json::json!({"title": title}),
            },
        )
        .await
        .unwrap();
    }
    let service = query_service(&pool);
    let first = service
        .list_context_revisions(ListContextRevisions {
            actor_principal_id: seed.owner,
            workflow_instance_id: created.workflow_instance_id,
            after_revision_number: None,
            limit: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.revision_number)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(first.next_cursor, Some(2));
    assert_eq!(
        first.items[1].previous_revision_id,
        Some(first.items[0].context_revision_id)
    );
    let second = service
        .list_context_revisions(ListContextRevisions {
            actor_principal_id: seed.owner,
            workflow_instance_id: created.workflow_instance_id,
            after_revision_number: first.next_cursor,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(
        second.items[0].payload,
        serde_json::json!({"title": "third"})
    );

    execute_workflow_transition(
        &pool,
        make_transition_command(
            seed.creator,
            created.workflow_instance_id,
            3,
            seed.draft_advance,
            Some(serde_json::json!({})),
        ),
    )
    .await
    .unwrap();
    execute_workflow_transition(
        &pool,
        make_transition_command(
            seed.assignee,
            created.workflow_instance_id,
            4,
            seed.normal_advance,
            Some(serde_json::json!({})),
        ),
    )
    .await
    .unwrap();
    let error = service
        .list_context_revisions(ListContextRevisions {
            actor_principal_id: seed.creator,
            workflow_instance_id: created.workflow_instance_id,
            after_revision_number: None,
            limit: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error, WorkflowQueryError::RestrictedHistoryNotVisible);
    let details: serde_json::Value = sqlx::query_scalar(
        "SELECT details FROM workflow_security_audits WHERE principal_id = $1
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(seed.creator)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(details["reason"], "RESTRICTED_SCOPE");
}

#[tokio::test]
async fn visit_history_masks_instructions_for_historical_participants_and_uses_uuid_tiebreaker() {
    let pool = create_pool().await;
    let completed = complete_query_instance(&pool).await;
    let same_time = chrono::Utc::now();
    let extra_a = Uuid::new_v4();
    let extra_b = Uuid::new_v4();
    for (id, number) in [(extra_a, 100), (extra_b, 101)] {
        sqlx::query(
            "INSERT INTO workflow_node_visits
             (node_visit_id, workflow_instance_id, node_id, visit_number,
              assignee_principal_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(completed.instance)
        .bind(completed.seed.draft)
        .bind(number)
        .bind(completed.seed.creator)
        .bind(same_time)
        .execute(&pool)
        .await
        .unwrap();
    }
    let service = query_service(&pool);
    let full = service
        .list_node_visits(ListNodeVisits {
            actor_principal_id: completed.seed.owner,
            workflow_instance_id: completed.instance,
            after: None,
            limit: Some(100),
        })
        .await
        .unwrap();
    assert!(full.items.iter().any(|item| item.instructions.is_some()));
    let tied: Vec<_> = full
        .items
        .iter()
        .filter(|item| item.created_at == same_time)
        .map(|item| item.node_visit_id)
        .collect();
    let mut sorted = tied.clone();
    sorted.sort();
    assert_eq!(tied, sorted);

    let restricted = service
        .list_node_visits(ListNodeVisits {
            actor_principal_id: completed.seed.creator,
            workflow_instance_id: completed.instance,
            after: None,
            limit: Some(100),
        })
        .await
        .unwrap();
    assert!(restricted
        .items
        .iter()
        .all(|item| item.assignee_principal_id == Some(completed.seed.creator)));
    assert!(restricted
        .items
        .iter()
        .all(|item| item.instructions.is_none()));
    let cursor_page = service
        .list_node_visits(ListNodeVisits {
            actor_principal_id: completed.seed.owner,
            workflow_instance_id: completed.instance,
            after: Some(TimeUuidCursor {
                created_at: same_time,
                id: sorted[0],
            }),
            limit: Some(100),
        })
        .await
        .unwrap();
    assert!(cursor_page
        .items
        .iter()
        .any(|item| item.node_visit_id == sorted[1]));
}

#[tokio::test]
async fn submission_history_full_and_restricted_filters_preserve_context_binding_and_exact_feedback(
) {
    let pool = create_pool().await;
    let completed = complete_query_instance(&pool).await;
    let service = query_service(&pool);
    let full = service
        .list_submission_history(ListSubmissionHistory {
            actor_principal_id: completed.seed.owner,
            workflow_instance_id: completed.instance,
            after: None,
            limit: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(full.items.len(), 2);
    assert!(full.next_cursor.is_some());
    assert!(full
        .items
        .iter()
        .all(|item| item.context_revision_id != Uuid::nil()));
    assert!(full
        .items
        .iter()
        .all(|item| item.source_node.node_id != Uuid::nil()));

    let creator = service
        .list_submission_history(ListSubmissionHistory {
            actor_principal_id: completed.seed.creator,
            workflow_instance_id: completed.instance,
            after: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(creator.items.len(), 3);
    assert!(creator
        .items
        .iter()
        .any(|item| item.submission_id == completed.feedback_submission));
    assert!(creator.items.iter().all(|item| {
        item.author_principal_id == completed.seed.creator || item.transition_effect == "RETURN"
    }));
    assert!(!creator.items.iter().any(|item| {
        item.transition_effect == "TERMINATE" && item.author_principal_id == completed.seed.assignee
    }));
}
