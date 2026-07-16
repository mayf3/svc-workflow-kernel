use super::*;

use svc_workflow::application::workflow_instance::query_types::*;

async fn seed_draft_version_instance(pool: &PgPool, seed: &QueryFixture) -> Uuid {
    let definition = Uuid::new_v4();
    let version = Uuid::new_v4();
    let draft = Uuid::new_v4();
    let review = Uuid::new_v4();
    let transition = Uuid::new_v4();
    let key = format!("defensive-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO workflow_definitions
         (workflow_definition_id, domain_id, definition_key, display_name)
         VALUES ($1, $2, $3, 'Defensive Draft')",
    )
    .bind(definition)
    .bind(seed.domain)
    .bind(key)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions
         (definition_version_id, workflow_definition_id, version_number, version_status)
         VALUES ($1, $2, 1, 'DRAFT')",
    )
    .bind(version)
    .bind(definition)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type, fixed_principal_id, instructions)
         VALUES ($1, $2, 'draft', 'Draft', 0, 'DRAFT', 'FIXED_PRINCIPAL', $3,
                 'Defensive instructions')",
    )
    .bind(draft)
    .bind(version)
    .bind(seed.assignee)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type)
         VALUES ($1, $2, 'review', 'Review', 1, 'NORMAL', 'DOMAIN_OWNER')",
    )
    .bind(review)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_transition_definitions
         (transition_id, definition_version_id, transition_key, display_name,
          source_node_id, target_node_id, transition_effect)
         VALUES ($1, $2, 'advance', 'Advance', $3, $4, 'ADVANCE')",
    )
    .bind(transition)
    .bind(version)
    .bind(draft)
    .bind(review)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_node_definitions SET primary_advance_transition_id = $1 WHERE node_id = $2",
    )
    .bind(transition)
    .bind(draft)
    .execute(pool)
    .await
    .unwrap();

    let instance = Uuid::new_v4();
    let context = Uuid::new_v4();
    let visit = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO workflow_instances
         (workflow_instance_id, domain_id, definition_version_id, created_by_principal_id,
          workflow_state_version) VALUES ($1, $2, $3, $4, 1)",
    )
    .bind(instance)
    .bind(seed.domain)
    .bind(version)
    .bind(seed.creator)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_context_revisions
         (context_revision_id, workflow_instance_id, revision_number, payload,
          payload_digest, created_by_principal_id)
         VALUES ($1, $2, 1, '{\"draft\":true}'::jsonb, $3, $4)",
    )
    .bind(context)
    .bind(instance)
    .bind("a".repeat(64))
    .bind(seed.creator)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id)
         VALUES ($1, $2, $3, 1, $4)",
    )
    .bind(visit)
    .bind(instance)
    .bind(draft)
    .bind(seed.assignee)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_instances SET current_context_revision_id = $1,
          current_node_visit_id = $2 WHERE workflow_instance_id = $3",
    )
    .bind(context)
    .bind(visit)
    .bind(instance)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          event_type, target_node_visit_id, context_revision_id, actor_principal_id,
          to_node_id, old_workflow_state_version, new_workflow_state_version)
         VALUES ($1, $2, 1, 'v1', 'INSTANCE_CREATED', $3, $4, $5, $6, 0, 1)",
    )
    .bind(Uuid::new_v4())
    .bind(instance)
    .bind(visit)
    .bind(context)
    .bind(seed.creator)
    .bind(draft)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    instance
}

#[tokio::test]
async fn defensive_draft_versions_remain_visible_but_block_execution_and_editing() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let instance = seed_draft_version_instance(&pool, &seed).await;
    let service = query_service(&pool);
    let WorkflowInstanceDetail::Full(detail) = service
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: seed.assignee,
            workflow_instance_id: instance,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(detail.instance.definition_version_status, "DRAFT");
    assert_eq!(
        detail.outgoing_transitions[0].blocked_reason,
        Some(TransitionBlockedReason::DefinitionVersionDraft)
    );
    let assigned = service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.assignee,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(assigned.items.iter().any(|item| {
        item.detail.instance.workflow_instance_id == instance
            && item.detail.outgoing_transitions[0].blocked_reason
                == Some(TransitionBlockedReason::DefinitionVersionDraft)
    }));
    let drafts = service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    let draft = drafts
        .items
        .iter()
        .find(|item| item.detail.instance.workflow_instance_id == instance)
        .unwrap();
    assert!(!draft.context_editable);
    assert!(!draft.combined_executable);

    let version = detail.instance.definition_version_id;
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED'
         WHERE definition_version_id = $1",
    )
    .bind(version)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(seed.owner)
        .execute(&pool)
        .await
        .unwrap();
    let WorkflowInstanceDetail::Full(unavailable) = service
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: seed.assignee,
            workflow_instance_id: instance,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        unavailable.outgoing_transitions[0].blocked_reason,
        Some(TransitionBlockedReason::TargetAssigneeUnavailable)
    );
}

#[tokio::test]
async fn corrupt_historical_facts_neither_grant_visibility_nor_escape_global_guards() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let other = seed_query_fixture(&pool).await;
    let service = query_service(&pool);

    let visit_instance = create_query_instance(&pool, &seed).await;
    let corrupt_current_visit = Uuid::new_v4();
    let mut corruption = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *corruption)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id)
         VALUES ($1, $2, $3, 1, $4)",
    )
    .bind(corrupt_current_visit)
    .bind(visit_instance.workflow_instance_id)
    .bind(other.draft)
    .bind(seed.outsider)
    .execute(&mut *corruption)
    .await
    .unwrap();
    corruption.commit().await.unwrap();
    assert_eq!(
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: seed.outsider,
                workflow_instance_id: visit_instance.workflow_instance_id,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible
    );
    sqlx::query(
        "UPDATE workflow_instances SET current_node_visit_id = $1
         WHERE workflow_instance_id = $2",
    )
    .bind(corrupt_current_visit)
    .bind(visit_instance.workflow_instance_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: seed.outsider,
                workflow_instance_id: visit_instance.workflow_instance_id,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible
    );
    assert!(matches!(
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: seed.owner,
                workflow_instance_id: visit_instance.workflow_instance_id,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));

    let submission_instance = create_query_instance(&pool, &seed).await;
    sqlx::query(
        "INSERT INTO workflow_submissions
         (submission_id, workflow_instance_id, source_node_visit_id, context_revision_id,
          author_principal_id, transition_id, payload, payload_digest, schema_version)
         VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, 'v1')",
    )
    .bind(Uuid::new_v4())
    .bind(submission_instance.workflow_instance_id)
    .bind(submission_instance.current_node_visit_id)
    .bind(submission_instance.current_context_revision_id)
    .bind(seed.outsider)
    .bind(other.draft_advance)
    .bind("c".repeat(64))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: seed.outsider,
                workflow_instance_id: submission_instance.workflow_instance_id,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible
    );
    assert!(matches!(
        service
            .list_submission_history(ListSubmissionHistory {
                actor_principal_id: seed.owner,
                workflow_instance_id: submission_instance.workflow_instance_id,
                after: None,
                limit: Some(1),
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));
    assert!(matches!(
        service
            .list_assigned_to_me(ListAssignedToMe {
                actor_principal_id: seed.creator,
                before: None,
                limit: None,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));
    assert!(matches!(
        service
            .list_creator_owned_drafts(ListCreatorOwnedDrafts {
                actor_principal_id: seed.creator,
                before: None,
                limit: None,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));
}

#[tokio::test]
async fn context_chain_gaps_and_stale_heads_fail_before_the_first_page() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let service = query_service(&pool);

    let broken = create_query_instance(&pool, &seed).await;
    let broken_revision = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_context_revisions
         (context_revision_id, workflow_instance_id, revision_number, previous_revision_id,
          payload, payload_digest, created_by_principal_id)
         VALUES ($1, $2, 2, NULL, '{}'::jsonb, $3, $4)",
    )
    .bind(broken_revision)
    .bind(broken.workflow_instance_id)
    .bind("d".repeat(64))
    .bind(seed.creator)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_instances SET current_context_revision_id = $1
         WHERE workflow_instance_id = $2",
    )
    .bind(broken_revision)
    .bind(broken.workflow_instance_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service
            .list_context_revisions(ListContextRevisions {
                actor_principal_id: seed.owner,
                workflow_instance_id: broken.workflow_instance_id,
                after_revision_number: None,
                limit: Some(1),
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));

    let stale = create_query_instance(&pool, &seed).await;
    sqlx::query(
        "INSERT INTO workflow_context_revisions
         (context_revision_id, workflow_instance_id, revision_number, previous_revision_id,
          payload, payload_digest, created_by_principal_id)
         VALUES ($1, $2, 2, $3, '{}'::jsonb, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(stale.workflow_instance_id)
    .bind(stale.current_context_revision_id)
    .bind("e".repeat(64))
    .bind(seed.creator)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: seed.owner,
                workflow_instance_id: stale.workflow_instance_id,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));
}
