use super::*;

use svc_workflow::application::workflow_instance::query_types::*;

#[tokio::test]
async fn audit_write_failure_is_storage_error_and_never_allows_the_read() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let _guard = QueryAuditTriggerGuard::install(&pool, seed.outsider).await;

    let error = query_service(&pool)
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: seed.outsider,
            workflow_instance_id: created.workflow_instance_id,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, WorkflowQueryError::StorageError(_)));
    let audit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_security_audits WHERE principal_id = $1")
            .bind(seed.outsider)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audit_count, 0);
}

#[tokio::test]
async fn successful_queries_are_read_only_and_rejections_only_add_security_audit() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let before: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_node_visits WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_submissions WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_command_receipts WHERE principal_id = $2)",
    )
    .bind(created.workflow_instance_id)
    .bind(seed.creator)
    .fetch_one(&pool)
    .await
    .unwrap();
    let service = query_service(&pool);
    service
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: seed.owner,
            workflow_instance_id: created.workflow_instance_id,
        })
        .await
        .unwrap();
    service
        .list_workflow_timeline(ListWorkflowTimeline {
            actor_principal_id: seed.owner,
            workflow_instance_id: created.workflow_instance_id,
            after_event_sequence: None,
            limit: None,
        })
        .await
        .unwrap();
    service
        .list_context_revisions(ListContextRevisions {
            actor_principal_id: seed.owner,
            workflow_instance_id: created.workflow_instance_id,
            after_revision_number: None,
            limit: None,
        })
        .await
        .unwrap();
    service
        .list_node_visits(ListNodeVisits {
            actor_principal_id: seed.owner,
            workflow_instance_id: created.workflow_instance_id,
            after: None,
            limit: None,
        })
        .await
        .unwrap();
    service
        .list_submission_history(ListSubmissionHistory {
            actor_principal_id: seed.owner,
            workflow_instance_id: created.workflow_instance_id,
            after: None,
            limit: None,
        })
        .await
        .unwrap();
    service
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    service
        .list_creator_owned_drafts(ListCreatorOwnedDrafts {
            actor_principal_id: seed.creator,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    let after: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_node_visits WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_submissions WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1),
           (SELECT COUNT(*) FROM workflow_command_receipts WHERE principal_id = $2)",
    )
    .bind(created.workflow_instance_id)
    .bind(seed.creator)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);

    let audit_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_security_audits
         WHERE principal_id = $1 AND resource_id = $2
           AND details->>'queryType' = 'ListNodeVisits'",
    )
    .bind(seed.outsider)
    .bind(created.workflow_instance_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        service
            .list_node_visits(ListNodeVisits {
                actor_principal_id: seed.outsider,
                workflow_instance_id: created.workflow_instance_id,
                after: None,
                limit: None,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible
    );
    let audit_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_security_audits
         WHERE principal_id = $1 AND resource_id = $2
           AND details->>'queryType' = 'ListNodeVisits'",
    )
    .bind(seed.outsider)
    .bind(created.workflow_instance_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit_after, audit_before + 1);
}

#[tokio::test]
async fn disabled_and_missing_principal_guards_apply_to_worklists_and_invalid_limits() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let service = query_service(&pool);
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(seed.creator)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        service
            .list_assigned_to_me(ListAssignedToMe {
                actor_principal_id: seed.creator,
                before: None,
                limit: None,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::PrincipalDisabled
    );
    assert_eq!(
        service
            .list_creator_owned_drafts(ListCreatorOwnedDrafts {
                actor_principal_id: Uuid::new_v4(),
                before: None,
                limit: None,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::PrincipalNotFound
    );
    assert!(matches!(
        service
            .list_assigned_to_me(ListAssignedToMe {
                actor_principal_id: seed.owner,
                before: None,
                limit: Some(21),
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InvalidPagination(_)
    ));
    assert!(matches!(
        service
            .list_creator_owned_drafts(ListCreatorOwnedDrafts {
                actor_principal_id: seed.owner,
                before: None,
                limit: Some(51),
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InvalidPagination(_)
    ));
}

#[tokio::test]
async fn state_and_visit_definition_corruption_fail_before_partial_dto() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let service = query_service(&pool);
    sqlx::query(
        "UPDATE workflow_instances SET workflow_state_version = workflow_state_version + 1
         WHERE workflow_instance_id = $1",
    )
    .bind(created.workflow_instance_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: seed.owner,
                workflow_instance_id: created.workflow_instance_id,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));
    sqlx::query(
        "UPDATE workflow_instances SET workflow_state_version = workflow_state_version - 1
         WHERE workflow_instance_id = $1",
    )
    .bind(created.workflow_instance_id)
    .execute(&pool)
    .await
    .unwrap();

    let other = seed_query_fixture(&pool).await;
    let bad_visit = Uuid::new_v4();
    let mut corruption = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *corruption)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number, assignee_principal_id)
         VALUES ($1, $2, $3, 99, $4)",
    )
    .bind(bad_visit)
    .bind(created.workflow_instance_id)
    .bind(other.draft)
    .bind(seed.creator)
    .execute(&mut *corruption)
    .await
    .unwrap();
    corruption.commit().await.unwrap();
    sqlx::query(
        "UPDATE workflow_instances SET current_node_visit_id = $1 WHERE workflow_instance_id = $2",
    )
    .bind(bad_visit)
    .bind(created.workflow_instance_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        service
            .get_workflow_instance_detail(GetWorkflowInstanceDetail {
                actor_principal_id: seed.owner,
                workflow_instance_id: created.workflow_instance_id,
            })
            .await
            .unwrap_err(),
        WorkflowQueryError::InternalConsistency(_)
    ));
}
