use super::*;

use svc_workflow::application::workflow_instance::query_types::*;

fn detail_query(actor: Uuid, instance: Uuid) -> GetWorkflowInstanceDetail {
    GetWorkflowInstanceDetail {
        actor_principal_id: actor,
        workflow_instance_id: instance,
    }
}

#[tokio::test]
async fn detail_visibility_priority_and_full_dto_are_authoritative() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let service = query_service(&pool);

    for actor in [seed.owner, seed.creator] {
        let WorkflowInstanceDetail::Full(detail) = service
            .get_workflow_instance_detail(detail_query(actor, created.workflow_instance_id))
            .await
            .unwrap()
        else {
            panic!("owner and creator-on-DRAFT must receive full detail")
        };
        assert_eq!(
            detail.instance.workflow_instance_id,
            created.workflow_instance_id
        );
        assert_eq!(detail.instance.domain_id, seed.domain);
        assert_eq!(detail.instance.definition_version_id, seed.version);
        assert_eq!(detail.instance.definition_version_status, "PUBLISHED");
        assert_eq!(detail.instance.created_by_principal_id, seed.creator);
        assert_eq!(detail.instance.workflow_state_version, 1);
        assert_eq!(
            detail.instance.external_reference.as_deref(),
            Some("QUERY-42")
        );
        assert_eq!(
            detail.instance.metadata,
            Some(serde_json::json!({"source": "query-test"}))
        );
        assert_eq!(
            detail.current_context_revision_id,
            created.current_context_revision_id
        );
        assert_eq!(detail.current_node_visit_id, created.current_node_visit_id);
        assert_eq!(detail.current_context.revision_number, 1);
        assert_eq!(
            detail.current_context.payload,
            serde_json::json!({"title": "initial"})
        );
        assert_eq!(detail.current_context.payload_digest.len(), 64);
        assert_eq!(detail.current_visit.node.node_id, seed.draft);
        assert_eq!(
            detail.current_visit.assignee_principal_id,
            Some(seed.creator)
        );
        assert_eq!(
            detail.current_visit.instructions.as_deref(),
            Some("Draft instructions")
        );
        assert_eq!(detail.outgoing_transitions.len(), 2);
        let outgoing = detail
            .outgoing_transitions
            .iter()
            .find(|transition| transition.transition_id == seed.draft_advance)
            .unwrap();
        assert_eq!(outgoing.transition_id, seed.draft_advance);
        assert_eq!(outgoing.target_node.node_id, seed.normal);
        assert_eq!(outgoing.transition_effect, "ADVANCE");
        assert!(outgoing.submission_schema.is_some());
        assert_eq!(outgoing.executable_for_actor, actor == seed.creator);
        let extra = detail
            .outgoing_transitions
            .iter()
            .find(|transition| transition.transition_id == seed.extra_advance)
            .unwrap();
        assert!(!extra.executable_for_actor);
        assert_eq!(
            extra.blocked_reason,
            Some(TransitionBlockedReason::AdvanceNotPrimary)
        );
    }

    let non_primary_error = execute_workflow_transition(
        &pool,
        make_transition_command(
            seed.creator,
            created.workflow_instance_id,
            1,
            seed.extra_advance,
            Some(serde_json::json!({})),
        ),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        non_primary_error,
        ExecuteWorkflowTransitionError::TransitionNotApplicable(_)
    ));

    execute_workflow_transition(
        &pool,
        make_transition_command(
            seed.creator,
            created.workflow_instance_id,
            1,
            seed.draft_advance,
            Some(serde_json::json!({"work": "done"})),
        ),
    )
    .await
    .unwrap();
    let WorkflowInstanceDetail::Full(assigned) = service
        .get_workflow_instance_detail(detail_query(seed.assignee, created.workflow_instance_id))
        .await
        .unwrap()
    else {
        panic!("current assignee without membership must receive full detail")
    };
    assert_eq!(assigned.current_visit.node.node_id, seed.normal);
    assert_eq!(
        assigned.current_visit.instructions.as_deref(),
        Some("Review instructions")
    );
}

#[tokio::test]
async fn historical_participant_is_restricted_and_domain_disabled_does_not_block_reads() {
    let pool = create_pool().await;
    let completed = complete_query_instance(&pool).await;
    let service = query_service(&pool);

    let WorkflowInstanceDetail::HistoricalParticipant(detail) = service
        .get_workflow_instance_detail(detail_query(completed.seed.creator, completed.instance))
        .await
        .unwrap()
    else {
        panic!("terminal creator is only a historical participant")
    };
    assert_eq!(
        detail.instance.current_node.node_id,
        completed.seed.terminal
    );
    assert_eq!(detail.instance.current_node.node_type, "TERMINAL");
    assert!(detail.instance.is_terminal);
    let restricted_json = serde_json::to_value(&detail).unwrap();
    for forbidden in [
        "created_by_principal_id",
        "current_context_revision_id",
        "current_node_visit_id",
        "external_reference",
        "external_url",
        "metadata",
        "assignee_principal_id",
        "instructions",
    ] {
        assert!(restricted_json["instance"].get(forbidden).is_none());
    }

    sqlx::query("UPDATE domains SET enabled = FALSE WHERE domain_id = $1")
        .bind(completed.seed.domain)
        .execute(&pool)
        .await
        .unwrap();
    let WorkflowInstanceDetail::Full(owner) = service
        .get_workflow_instance_detail(detail_query(completed.seed.owner, completed.instance))
        .await
        .unwrap()
    else {
        panic!("domain owner remains fully visible when domain is disabled")
    };
    assert!(!owner.instance.domain_enabled);
}

#[tokio::test]
async fn masked_reads_and_disabled_principals_write_non_sensitive_security_audits() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let service = query_service(&pool);

    let error = service
        .get_workflow_instance_detail(detail_query(seed.outsider, created.workflow_instance_id))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible
    );
    let audit: (String, String, serde_json::Value) = sqlx::query_as(
        "SELECT action, resource_id, details FROM workflow_security_audits
         WHERE principal_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(seed.outsider)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit.0, "UNAUTHORIZED_WORKFLOW_READ");
    assert_eq!(audit.1, created.workflow_instance_id.to_string());
    assert_eq!(audit.2["reason"], "NO_VISIBILITY");
    let serialized = audit.2.to_string();
    assert!(!serialized.contains("initial"));
    assert!(!serialized.contains("submission_schema"));

    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_security_audits WHERE principal_id = $1")
            .bind(seed.outsider)
            .fetch_one(&pool)
            .await
            .unwrap();
    let missing = service
        .get_workflow_instance_detail(detail_query(seed.outsider, Uuid::new_v4()))
        .await
        .unwrap_err();
    assert_eq!(
        missing,
        WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible
    );
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_security_audits WHERE principal_id = $1")
            .bind(seed.outsider)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, after, "missing instances are not audited");

    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(seed.outsider)
        .execute(&pool)
        .await
        .unwrap();
    let disabled = service
        .get_workflow_instance_detail(detail_query(seed.outsider, created.workflow_instance_id))
        .await
        .unwrap_err();
    assert_eq!(disabled, WorkflowQueryError::PrincipalDisabled);
    let action: String = sqlx::query_scalar(
        "SELECT action FROM workflow_security_audits WHERE principal_id = $1
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(seed.outsider)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(action, "DISABLED_PRINCIPAL_READ_ATTEMPT");
}

#[tokio::test]
async fn owner_replacement_and_projection_corruption_are_observed_immediately() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let service = query_service(&pool);

    sqlx::query(
        "UPDATE domain_role_bindings SET enabled = FALSE, disabled_at = now()
         WHERE domain_id = $1 AND role_key = 'DOMAIN_OWNER' AND enabled = TRUE",
    )
    .bind(seed.domain)
    .execute(&pool)
    .await
    .unwrap();
    seed_domain_owner(&pool, seed.domain, seed.outsider).await;
    assert_eq!(
        service
            .get_workflow_instance_detail(detail_query(seed.owner, created.workflow_instance_id))
            .await
            .unwrap_err(),
        WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible
    );
    assert!(matches!(
        service
            .get_workflow_instance_detail(detail_query(seed.outsider, created.workflow_instance_id))
            .await
            .unwrap(),
        WorkflowInstanceDetail::Full(_)
    ));

    sqlx::query(
        "UPDATE workflow_instances SET current_context_revision_id = NULL
         WHERE workflow_instance_id = $1",
    )
    .bind(created.workflow_instance_id)
    .execute(&pool)
    .await
    .unwrap();
    let error = service
        .get_workflow_instance_detail(detail_query(seed.outsider, created.workflow_instance_id))
        .await
        .unwrap_err();
    assert!(matches!(error, WorkflowQueryError::InternalConsistency(_)));
}

#[tokio::test]
async fn stable_transition_blocking_covers_status_actor_and_target_availability() {
    let pool = create_pool().await;
    let seed = seed_query_fixture(&pool).await;
    let created = create_query_instance(&pool, &seed).await;
    let service = query_service(&pool);

    let WorkflowInstanceDetail::Full(owner) = service
        .get_workflow_instance_detail(detail_query(seed.owner, created.workflow_instance_id))
        .await
        .unwrap()
    else {
        panic!()
    };
    let owner_primary = owner
        .outgoing_transitions
        .iter()
        .find(|transition| transition.transition_id == seed.draft_advance)
        .unwrap();
    assert_eq!(
        owner_primary.blocked_reason,
        Some(TransitionBlockedReason::ActorNotCurrentAssignee)
    );

    sqlx::query("UPDATE principals SET enabled = FALSE WHERE principal_id = $1")
        .bind(seed.assignee)
        .execute(&pool)
        .await
        .unwrap();
    let WorkflowInstanceDetail::Full(creator) = service
        .get_workflow_instance_detail(detail_query(seed.creator, created.workflow_instance_id))
        .await
        .unwrap()
    else {
        panic!()
    };
    let creator_primary = creator
        .outgoing_transitions
        .iter()
        .find(|transition| transition.transition_id == seed.draft_advance)
        .unwrap();
    assert_eq!(
        creator_primary.blocked_reason,
        Some(TransitionBlockedReason::TargetAssigneeUnavailable)
    );

    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'REVOKED'
         WHERE definition_version_id = $1",
    )
    .bind(seed.version)
    .execute(&pool)
    .await
    .unwrap();
    let WorkflowInstanceDetail::Full(revoked) = service
        .get_workflow_instance_detail(detail_query(seed.creator, created.workflow_instance_id))
        .await
        .unwrap()
    else {
        panic!()
    };
    let revoked_primary = revoked
        .outgoing_transitions
        .iter()
        .find(|transition| transition.transition_id == seed.draft_advance)
        .unwrap();
    assert_eq!(
        revoked_primary.blocked_reason,
        Some(TransitionBlockedReason::DefinitionVersionRevoked)
    );
}
