use super::*;

use svc_workflow::application::workflow_instance::query_types::*;

#[tokio::test]
async fn assigned_to_me_uses_only_current_non_terminal_visit_and_returns_execution_context() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let advanced = execute_workflow_transition(
        &pool,
        make_transition_command(
            seed.creator,
            created.workflow_instance_id,
            1,
            seed.draft_advance,
            Some(serde_json::json!({"work": "ready"})),
        ),
    )
    .await
    .unwrap();
    let service = query_service(&pool);
    let page = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let item = &page.items[0];
    assert_eq!(
        item.detail.instance.workflow_instance_id,
        created.workflow_instance_id
    );
    assert_eq!(
        item.detail.current_node_visit_id,
        advanced.current_node_visit_id
    );
    assert_eq!(
        item.detail.current_visit.assignee_principal_id,
        Some(seed.assignee)
    );
    assert_eq!(
        item.detail.current_visit.instructions.as_deref(),
        Some("Review instructions")
    );
    assert_eq!(item.detail.instance.workflow_state_version, 2);
    assert_eq!(item.upstream_submissions.len(), 1);
    assert!(item.detail.outgoing_transitions.iter().all(|transition| {
        transition.submission_schema.is_some() && transition.executable_for_actor
    }));
    let assigned_timeline = service
        .list_workflow_timeline(ListWorkflowTimeline {
            actor_principal_id: seed.assignee,
            workflow_instance_id: created.workflow_instance_id,
            after_event_sequence: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(
        assigned_timeline
            .items
            .iter()
            .map(|event| event.event_sequence)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        service
            .list_context_revisions(ListContextRevisions {
                actor_principal_id: seed.assignee,
                workflow_instance_id: created.workflow_instance_id,
                after_revision_number: None,
                limit: None,
            })
            .await
            .unwrap()
            .items
            .len(),
        1
    );

    let returned = execute_workflow_transition(
        &pool,
        make_transition_command(
            seed.assignee,
            created.workflow_instance_id,
            2,
            seed.return_transition,
            Some(serde_json::json!({
                "reasonCode": "FIX",
                "reason": "revise",
                "rootCauseNodeVisitId": advanced.current_node_visit_id.to_string(),
                "relatedSubmissionIds": [advanced.submission_id.unwrap().to_string()]
            })),
        ),
    )
    .await
    .unwrap();
    assert!(service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: None,
            limit: None,
        })
        .await
        .unwrap()
        .items
        .is_empty());
    let creator_page = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(creator_page.items.len(), 1);
    assert_eq!(
        creator_page.items[0].detail.current_node_visit_id,
        returned.current_node_visit_id
    );
    assert_eq!(creator_page.items[0].return_feedback_events.len(), 1);

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
    assert!(service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn assigned_worklist_keeps_deprecated_and_revoked_instances_with_stable_blocking() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
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
    let service = query_service(&pool);
    let published = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(published.items[0]
        .detail
        .outgoing_transitions
        .iter()
        .all(|transition| transition.executable_for_actor));

    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'DEPRECATED'
         WHERE definition_version_id = $1",
    )
    .bind(seed.version)
    .execute(&pool)
    .await
    .unwrap();
    let deprecated = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(deprecated.items[0]
        .detail
        .outgoing_transitions
        .iter()
        .all(|transition| transition.executable_for_actor));

    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'REVOKED'
         WHERE definition_version_id = $1",
    )
    .bind(seed.version)
    .execute(&pool)
    .await
    .unwrap();
    let revoked = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(revoked.items.len(), 1);
    assert!(revoked.items[0]
        .detail
        .outgoing_transitions
        .iter()
        .all(|transition| transition.blocked_reason
            == Some(TransitionBlockedReason::DefinitionVersionRevoked)));
}

#[tokio::test]
async fn creator_drafts_use_runtime_draft_latest_context_status_and_combined_assignee_gate() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let first = create_query_instance(&pool, &seed).await;
    revise_workflow_context(
        &pool,
        ReviseWorkflowContextCommand {
            principal_id: PrincipalId::from_uuid(seed.creator),
            idempotency_key: Uuid::new_v4().to_string(),
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(first.workflow_instance_id),
            expected_workflow_state_version: 1,
            context_payload: serde_json::json!({"title": "latest"}),
        },
    )
    .await
    .unwrap();

    let (_, fixed_version, _, _, _, _, _, _, _) = seed_transition_graph(
        &pool,
        seed.domain,
        "FIXED_PRINCIPAL",
        "FIXED_PRINCIPAL",
        Some(seed.assignee),
    )
    .await;
    let mut fixed_command = make_command(seed.creator, seed.domain, fixed_version);
    fixed_command.context_payload = serde_json::json!({"title": "fixed-draft"});
    let fixed = create_workflow_instance(&pool, fixed_command)
        .await
        .unwrap();

    let service = query_service(&pool);
    let page = service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: None,
            limit: Some(50),
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    let own_assigned = page
        .items
        .iter()
        .find(|item| item.detail.instance.workflow_instance_id == first.workflow_instance_id)
        .unwrap();
    assert_eq!(
        own_assigned.detail.current_context.payload,
        serde_json::json!({"title": "latest"})
    );
    assert!(own_assigned.context_editable);
    assert!(own_assigned.combined_executable);
    let fixed_assigned = page
        .items
        .iter()
        .find(|item| item.detail.instance.workflow_instance_id == fixed.workflow_instance_id)
        .unwrap();
    assert!(fixed_assigned.context_editable);
    assert!(!fixed_assigned.combined_executable);

    execute_workflow_transition(
        &pool,
        make_transition_command(
            seed.creator,
            first.workflow_instance_id,
            2,
            seed.draft_advance,
            Some(serde_json::json!({})),
        ),
    )
    .await
    .unwrap();
    let after_advance = service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(!after_advance
        .items
        .iter()
        .any(|item| item.detail.instance.workflow_instance_id == first.workflow_instance_id));
}

#[tokio::test]
async fn creator_draft_desc_pagination_and_status_editability_are_stable() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let first = create_query_instance(&pool, &seed).await;
    let second = create_query_instance(&pool, &seed).await;
    let service = query_service(&pool);
    let page1 = service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: None,
            limit: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 1);
    assert!(page1.next_cursor.is_some());
    let page2 = service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: page1.next_cursor,
            limit: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(page2.items.len(), 1);
    assert_ne!(
        page1.items[0].detail.instance.workflow_instance_id,
        page2.items[0].detail.instance.workflow_instance_id
    );
    assert_eq!(
        [first.workflow_instance_id, second.workflow_instance_id]
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
        [
            page1.items[0].detail.instance.workflow_instance_id,
            page2.items[0].detail.instance.workflow_instance_id,
        ]
        .into_iter()
        .collect()
    );

    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'DEPRECATED'
         WHERE definition_version_id = $1",
    )
    .bind(seed.version)
    .execute(&pool)
    .await
    .unwrap();
    assert!(service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap()
        .items
        .iter()
        .all(|item| item.context_editable));
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'REVOKED'
         WHERE definition_version_id = $1",
    )
    .bind(seed.version)
    .execute(&pool)
    .await
    .unwrap();
    assert!(service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap()
        .items
        .iter()
        .all(|item| !item.context_editable && !item.combined_executable));
}

#[tokio::test]
async fn assigned_upstream_payload_is_capped_at_fifty_and_marks_truncation() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let mut state = 1;
    for cycle in 0..26 {
        let advanced = execute_workflow_transition(
            &pool,
            make_transition_command(
                seed.creator,
                created.workflow_instance_id,
                state,
                seed.draft_advance,
                Some(serde_json::json!({"cycle": cycle})),
            ),
        )
        .await
        .unwrap();
        state += 1;
        execute_workflow_transition(
            &pool,
            make_transition_command(
                seed.assignee,
                created.workflow_instance_id,
                state,
                seed.return_transition,
                Some(serde_json::json!({
                    "reasonCode": "LOOP",
                    "reason": "again",
                    "rootCauseNodeVisitId": advanced.current_node_visit_id.to_string(),
                    "relatedSubmissionIds": []
                })),
            ),
        )
        .await
        .unwrap();
        state += 1;
    }
    let page = query_service(&pool)
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].upstream_submissions.len(), 50);
    assert_eq!(page.items[0].return_feedback_events.len(), 26);
    assert!(page.items[0].submissions_truncated);
    assert!(!page.items[0].return_events_truncated);
}
