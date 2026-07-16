use super::*;

use svc_workflow::domain::workflow_instance::recovery::{
    AdminEmergencyOperation, AdminRelatedReference, RecoveryError,
};

#[tokio::test]
async fn override_validates_reason_references_and_expected_digest() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let mut empty_reason = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    empty_reason.reason = " \n".to_string();
    assert!(matches!(
        run_override(&pool, empty_reason).await.unwrap_err(),
        RecoveryError::InvalidInput(_)
    ));

    let mut too_many = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    too_many.related_references = (0..21)
        .map(|index| AdminRelatedReference {
            resource_type: "INCIDENT".to_string(),
            resource_id: format!("INC-{index}"),
        })
        .collect();
    assert!(matches!(
        run_override(&pool, too_many).await.unwrap_err(),
        RecoveryError::InvalidInput(_)
    ));

    let mut digest_mismatch = override_command(
        &fixture,
        AdminEmergencyOperation::MoveToNode,
        fixture.normal,
    );
    digest_mismatch.expected_before_snapshot_digest = Some("f".repeat(64));
    assert!(matches!(
        run_override(&pool, digest_mismatch).await.unwrap_err(),
        RecoveryError::BeforeSnapshotDigestMismatch { .. }
    ));
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
}

#[tokio::test]
async fn move_fails_closed_when_target_assignee_is_disabled() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(fixture.creator)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        run_override(
            &pool,
            override_command(
                &fixture,
                AdminEmergencyOperation::MoveToNode,
                fixture.normal,
            )
        )
        .await
        .unwrap_err(),
        RecoveryError::AssigneeResolutionFailed(_)
    ));
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        (1, 1, 0, 1)
    );
}

#[tokio::test]
async fn draft_definition_is_not_an_override_target_status() {
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
    assert!(matches!(
        run_override(
            &pool,
            override_command(
                &fixture,
                AdminEmergencyOperation::MoveToNode,
                fixture.normal,
            )
        )
        .await
        .unwrap_err(),
        RecoveryError::InvalidTarget(_)
    ));
}

#[tokio::test]
async fn orphan_context_visit_and_submission_facts_are_rejected() {
    let pool = create_pool().await;
    let context_fixture = seed_recovery_fixture(&pool).await;
    let payload = serde_json::json!({"orphan": true});
    let payload_digest =
        svc_workflow::domain::definition::digest::compute_json_digest(&payload).unwrap();
    sqlx::query(
        "INSERT INTO workflow_context_revisions
         (context_revision_id, workflow_instance_id, revision_number,
          previous_revision_id, payload, payload_digest, created_by_principal_id)
         VALUES ($1, $2, 2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(context_fixture.instance)
    .bind(context_fixture.initial_context)
    .bind(payload)
    .bind(payload_digest)
    .bind(context_fixture.creator)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        run_rebuild(&pool, rebuild_command(&context_fixture))
            .await
            .unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));

    let visit_fixture = seed_recovery_fixture(&pool).await;
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number,
          assignee_principal_id, entered_by_transition_id)
         VALUES ($1, $2, $3, 1, $4, NULL)",
    )
    .bind(Uuid::new_v4())
    .bind(visit_fixture.instance)
    .bind(visit_fixture.normal)
    .bind(visit_fixture.creator)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        run_rebuild(&pool, rebuild_command(&visit_fixture))
            .await
            .unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));

    let submission_fixture = seed_recovery_fixture(&pool).await;
    let transition: Uuid = sqlx::query_scalar(
        "SELECT transition_id FROM workflow_transition_definitions
         WHERE definition_version_id = $1 AND source_node_id = $2
           AND transition_effect = 'ADVANCE' ORDER BY transition_key LIMIT 1",
    )
    .bind(submission_fixture.version)
    .bind(submission_fixture.draft)
    .fetch_one(&pool)
    .await
    .unwrap();
    let payload = serde_json::json!({"orphan": "submission"});
    let payload_digest =
        svc_workflow::domain::definition::digest::compute_json_digest(&payload).unwrap();
    sqlx::query(
        "INSERT INTO workflow_submissions
         (submission_id, workflow_instance_id, source_node_visit_id,
          context_revision_id, author_principal_id, transition_id,
          payload, payload_digest, schema_version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'v1')",
    )
    .bind(Uuid::new_v4())
    .bind(submission_fixture.instance)
    .bind(submission_fixture.initial_visit)
    .bind(submission_fixture.initial_context)
    .bind(submission_fixture.creator)
    .bind(transition)
    .bind(payload)
    .bind(payload_digest)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        run_rebuild(&pool, rebuild_command(&submission_fixture))
            .await
            .unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));
}

#[tokio::test]
async fn event_matrix_violation_is_rejected_and_security_audit_omits_reason_text() {
    let pool = create_pool().await;
    let bad = seed_recovery_fixture(&pool).await;
    let data = serde_json::json!({"previous_context_revision_id": bad.initial_context});
    let digest = svc_workflow::domain::definition::digest::compute_json_digest(&data).unwrap();
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          event_type, target_node_visit_id, context_revision_id, event_data,
          event_data_digest, actor_principal_id, old_workflow_state_version,
          new_workflow_state_version)
         VALUES ($1, $2, 2, 'v1', 'CONTEXT_REVISED', $3, $4, $5, $6, $7, 1, 2)",
    )
    .bind(Uuid::new_v4())
    .bind(bad.instance)
    .bind(bad.initial_visit)
    .bind(bad.initial_context)
    .bind(data)
    .bind(digest)
    .bind(bad.creator)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        run_rebuild(&pool, rebuild_command(&bad)).await.unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));

    let good = seed_recovery_fixture(&pool).await;
    let mut command = override_command(
        &good,
        AdminEmergencyOperation::TerminateInstance,
        good.terminal,
    );
    command.reason = "public incident recovery rationale".to_string();
    run_override(&pool, command).await.unwrap();
    let details: serde_json::Value = sqlx::query_scalar(
        "SELECT details FROM workflow_security_audits
         WHERE principal_id = $1 AND resource_id = $2
           AND action = 'ADMIN_EMERGENCY_OVERRIDE_COMMITTED'",
    )
    .bind(good.admin)
    .bind(good.instance.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!details
        .to_string()
        .contains("public incident recovery rationale"));
}

#[tokio::test]
async fn multiple_admins_are_independently_authorized() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let second = seed_second_principal(&pool).await;
    bind_workflow_admin(&pool, fixture.domain, second).await;
    let mut command = rebuild_command(&fixture);
    command.principal_id = PrincipalId::from_uuid(second);
    assert!(run_rebuild(&pool, command).await.is_ok());
}

#[tokio::test]
async fn grandfathered_terminal_assignee_is_history_but_never_current_work() {
    use svc_workflow::application::workflow_instance::query_service::WorkflowQueryService;
    use svc_workflow::application::workflow_instance::query_types::{
        GetWorkflowInstanceDetail, ListAssignedToMe, WorkflowInstanceDetail,
    };

    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let terminated = run_override(
        &pool,
        override_command(
            &fixture,
            AdminEmergencyOperation::TerminateInstance,
            fixture.terminal,
        ),
    )
    .await
    .unwrap();

    // Simulate rows created before 0010. Transactional DDL keeps the public
    // constraint present for all other sessions and restores it before commit.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("ALTER TABLE workflow_node_definitions DROP CONSTRAINT chk_node_assignee_shape")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_node_definitions SET assignee_ref_type = 'WORKFLOW_CREATOR'
         WHERE node_id = $1",
    )
    .bind(fixture.terminal)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_node_visits SET assignee_principal_id = $2
         WHERE node_visit_id = $1",
    )
    .bind(terminated.current_node_visit_id)
    .bind(fixture.outsider)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "ALTER TABLE workflow_node_definitions
         ADD CONSTRAINT chk_node_assignee_shape CHECK (
           (node_type = 'TERMINAL' AND assignee_ref_type IS NULL
             AND fixed_principal_id IS NULL)
           OR (node_type <> 'TERMINAL' AND assignee_ref_type IS NOT NULL AND (
             (assignee_ref_type = 'FIXED_PRINCIPAL' AND fixed_principal_id IS NOT NULL)
             OR (assignee_ref_type IN ('WORKFLOW_CREATOR', 'DOMAIN_OWNER')
                 AND fixed_principal_id IS NULL)))) NOT VALID",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert!(run_rebuild(&pool, rebuild_command(&fixture)).await.is_ok());
    let query = WorkflowQueryService::new(pool.clone());
    let outsider_detail = query
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: fixture.outsider,
            workflow_instance_id: fixture.instance,
        })
        .await
        .unwrap();
    assert!(matches!(
        outsider_detail,
        WorkflowInstanceDetail::HistoricalParticipant(_)
    ));
    let assigned = query
        .list_assigned_to_me(ListAssignedToMe {
            actor_principal_id: fixture.outsider,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(assigned
        .items
        .iter()
        .all(|item| item.detail.instance.workflow_instance_id != fixture.instance));
    let WorkflowInstanceDetail::Full(owner_detail) = query
        .get_workflow_instance_detail(GetWorkflowInstanceDetail {
            actor_principal_id: fixture.creator,
            workflow_instance_id: fixture.instance,
        })
        .await
        .unwrap()
    else {
        panic!("owner should see full legacy visit")
    };
    assert_eq!(
        owner_detail.current_visit.assignee_principal_id,
        Some(fixture.outsider)
    );
}
