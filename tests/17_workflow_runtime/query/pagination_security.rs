use super::*;

use svc_workflow::application::workflow_instance::query_types::*;

#[tokio::test]
async fn assigned_worklist_desc_keyset_has_no_duplicates_or_gaps() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let mut expected = std::collections::HashSet::new();
    for _ in 0..2 {
        let created = create_query_instance(&pool, &seed).await;
        expected.insert(created.workflow_instance_id);
        execute_workflow_transition(
            &pool,
            make_transition_command(
                seed.creator,
                created.workflow_instance_id,
                1,
                seed.draft_advance,
                Some(serde_json::json!({})),
            ),
        )
        .await
        .unwrap();
    }
    let service = query_service(&pool);
    let first = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: None,
            limit: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.next_cursor.is_some());
    let second = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: first.next_cursor,
            limit: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_ne!(
        first.items[0].detail.instance.workflow_instance_id,
        second.items[0].detail.instance.workflow_instance_id
    );
    let actual: std::collections::HashSet<Uuid> = [
        first.items[0].detail.instance.workflow_instance_id,
        second.items[0].detail.instance.workflow_instance_id,
    ]
    .into_iter()
    .collect();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn submission_time_uuid_cursor_orders_identical_timestamps() {
    let pool = create_pool().await;
    let completed = complete_query_instance(&pool).await;
    let context: Uuid = sqlx::query_scalar(
        "SELECT current_context_revision_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(completed.instance)
    .fetch_one(&pool)
    .await
    .unwrap();
    let same_time = chrono::Utc::now();
    let mut ids = Vec::new();
    for visit_number in [200, 201] {
        let visit = Uuid::new_v4();
        let submission = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workflow_node_visits
             (node_visit_id, workflow_instance_id, node_id, visit_number,
              assignee_principal_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(visit)
        .bind(completed.instance)
        .bind(completed.seed.draft)
        .bind(visit_number)
        .bind(completed.seed.owner)
        .bind(same_time)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_submissions
             (submission_id, workflow_instance_id, source_node_visit_id,
              context_revision_id, author_principal_id, transition_id,
              payload, payload_digest, schema_version, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, 'v1', $8)",
        )
        .bind(submission)
        .bind(completed.instance)
        .bind(visit)
        .bind(context)
        .bind(completed.seed.owner)
        .bind(completed.seed.draft_advance)
        .bind("b".repeat(64))
        .bind(same_time)
        .execute(&pool)
        .await
        .unwrap();
        ids.push(submission);
    }
    ids.sort();
    let page = query_service(&pool)
        .list_submission_history(ListSubmissionHistory {
            actor_principal_id: completed.seed.owner,
            workflow_instance_id: completed.instance,
            after: Some(TimeUuidCursor {
                created_at: same_time,
                id: ids[0],
            }),
            limit: Some(100),
        })
        .await
        .unwrap();
    assert!(page.items.iter().any(|item| item.submission_id == ids[1]));
    assert!(!page.items.iter().any(|item| item.submission_id == ids[0]));
}

#[tokio::test]
async fn cross_instance_related_submission_uuid_does_not_expose_return_payload_or_event() {
    let pool = create_pool().await;
    let first = complete_query_instance(&pool).await;
    let second = complete_query_instance(&pool).await;
    let actor = second.seed.creator;
    let context: Uuid = sqlx::query_scalar(
        "SELECT current_context_revision_id FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(first.instance)
    .fetch_one(&pool)
    .await
    .unwrap();
    let visit = Uuid::new_v4();
    let feedback = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id)
         VALUES ($1, $2, $3, 300, $4)",
    )
    .bind(visit)
    .bind(first.instance)
    .bind(first.seed.normal)
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_submissions
         (submission_id, workflow_instance_id, source_node_visit_id, context_revision_id,
          author_principal_id, transition_id, payload, payload_digest, schema_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'v1')",
    )
    .bind(feedback)
    .bind(first.instance)
    .bind(visit)
    .bind(context)
    .bind(first.seed.assignee)
    .bind(first.seed.return_transition)
    .bind(serde_json::json!({
        "reasonCode": "CROSS",
        "reason": "must not leak",
        "relatedSubmissionIds": [second.creator_submission.to_string()]
    }))
    .bind("c".repeat(64))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          event_type, transition_effect, source_node_visit_id, context_revision_id,
          submission_id, actor_principal_id, old_workflow_state_version,
          new_workflow_state_version)
         VALUES ($1, $2, 6, 'v1', 'WORKFLOW_TRANSITION_COMMITTED', 'RETURN',
                 $3, $4, $5, $6, 5, 6)",
    )
    .bind(Uuid::new_v4())
    .bind(first.instance)
    .bind(visit)
    .bind(context)
    .bind(feedback)
    .bind(first.seed.assignee)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_instances SET workflow_state_version = 6 WHERE workflow_instance_id = $1",
    )
    .bind(first.instance)
    .execute(&pool)
    .await
    .unwrap();

    let service = query_service(&pool);
    let submissions = service
        .list_submission_history(ListSubmissionHistory {
            actor_principal_id: actor,
            workflow_instance_id: first.instance,
            after: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(!submissions
        .items
        .iter()
        .any(|item| item.submission_id == feedback));
    let events = service
        .list_workflow_timeline(ListWorkflowTimeline {
            actor_principal_id: actor,
            workflow_instance_id: first.instance,
            after_event_sequence: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(!events.items.iter().any(|item| item.event_sequence == 6));
}
