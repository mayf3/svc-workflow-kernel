use super::*;

use svc_workflow::domain::workflow_instance::recovery::RecoveryError;

#[tokio::test]
async fn rebuild_is_fact_preserving_when_projection_is_already_correct() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let before = count_instance_facts(&pool, fixture.instance).await;
    let result = run_rebuild(&pool, rebuild_command(&fixture)).await.unwrap();
    assert!(!result.changed);
    assert!(!result.replayed);
    assert_eq!(result.before_projection, result.after_projection);
    assert_eq!(count_instance_facts(&pool, fixture.instance).await, before);
    let security: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_security_audits
         WHERE principal_id = $1 AND action = 'REBUILD_PROJECTION_COMMITTED'
           AND resource_id = $2",
    )
    .bind(fixture.admin)
    .bind(fixture.instance.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(security, 1);
}

#[tokio::test]
async fn rebuild_repairs_all_three_projection_fields_from_facts() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    sqlx::query(
        "UPDATE workflow_instances
         SET current_context_revision_id = NULL, current_node_visit_id = NULL,
             workflow_state_version = 99
         WHERE workflow_instance_id = $1",
    )
    .bind(fixture.instance)
    .execute(&pool)
    .await
    .unwrap();
    let immutable_before = count_instance_facts(&pool, fixture.instance).await;
    let result = run_rebuild(&pool, rebuild_command(&fixture)).await.unwrap();
    assert!(result.changed);
    assert_eq!(result.before_projection.current_context_revision_id, None);
    assert_eq!(result.before_projection.current_node_visit_id, None);
    assert_eq!(result.before_projection.workflow_state_version, 99);
    assert_eq!(
        result.after_projection.current_context_revision_id,
        Some(fixture.initial_context)
    );
    assert_eq!(
        result.after_projection.current_node_visit_id,
        Some(fixture.initial_visit)
    );
    assert_eq!(result.after_projection.workflow_state_version, 1);
    assert_eq!(
        count_instance_facts(&pool, fixture.instance).await,
        immutable_before
    );
}

#[tokio::test]
async fn digest_mismatch_is_completed_without_projection_update() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let mut command = rebuild_command(&fixture);
    command.expected_before_snapshot_digest = Some("0".repeat(64));
    let key = command.idempotency_key.clone();
    let error = run_rebuild(&pool, command).await.unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::BeforeSnapshotDigestMismatch { .. }
    ));
    let row: (String, i32) = sqlx::query_as(
        "SELECT receipt_status::text, response_status FROM workflow_command_receipts
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(fixture.admin)
    .bind(key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("COMPLETED".to_string(), 409));
}

#[tokio::test]
async fn empty_fact_set_and_event_gap_fail_closed() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let empty_instance = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_instances
         (workflow_instance_id, domain_id, definition_version_id,
          created_by_principal_id, workflow_state_version)
         VALUES ($1, $2, $3, $4, 1)",
    )
    .bind(empty_instance)
    .bind(fixture.domain)
    .bind(fixture.version)
    .bind(fixture.creator)
    .execute(&pool)
    .await
    .unwrap();
    let mut empty = rebuild_command(&fixture);
    empty.workflow_instance_id = WorkflowInstanceId::from_uuid(empty_instance);
    assert!(matches!(
        run_rebuild(&pool, empty).await.unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));

    let data = serde_json::json!({
        "previous_context_revision_id": fixture.initial_context,
        "new_context_revision_id": fixture.initial_context,
    });
    let data_digest = svc_workflow::domain::definition::digest::compute_json_digest(&data).unwrap();
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          event_type, source_node_visit_id, target_node_visit_id,
          context_revision_id, event_data, event_data_digest, actor_principal_id,
          old_workflow_state_version, new_workflow_state_version)
         VALUES ($1, $2, 3, 'v1', 'CONTEXT_REVISED', $3, $3, $4, $5, $6, $7, 2, 3)",
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
async fn documented_creation_alias_is_accepted_and_unknown_type_is_rejected() {
    let pool = create_pool().await;
    let alias_fixture = seed_recovery_fixture(&pool).await;
    let mut alias_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *alias_tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_events SET event_type = 'WORKFLOW_INSTANCE_CREATED'
         WHERE workflow_instance_id = $1",
    )
    .bind(alias_fixture.instance)
    .execute(&mut *alias_tx)
    .await
    .unwrap();
    alias_tx.commit().await.unwrap();
    assert!(run_rebuild(&pool, rebuild_command(&alias_fixture))
        .await
        .is_ok());

    let bad_fixture = seed_recovery_fixture(&pool).await;
    let mut bad_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *bad_tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_events SET event_type = 'UNKNOWN_RECOVERY_EVENT'
         WHERE workflow_instance_id = $1",
    )
    .bind(bad_fixture.instance)
    .execute(&mut *bad_tx)
    .await
    .unwrap();
    bad_tx.commit().await.unwrap();
    assert!(matches!(
        run_rebuild(&pool, rebuild_command(&bad_fixture))
            .await
            .unwrap_err(),
        RecoveryError::InvalidImmutableFacts(_)
    ));
}

#[tokio::test]
async fn imported_alias_initial_shape_is_explicitly_supported() {
    let pool = create_pool().await;
    let fixture = seed_recovery_fixture(&pool).await;
    let imported_instance = Uuid::new_v4();
    let legacy_record = Uuid::new_v4();
    let migration_service = Uuid::new_v4();
    let context = Uuid::new_v4();
    let visit = Uuid::new_v4();
    let event = Uuid::new_v4();
    let command = Uuid::new_v4();
    let external_reference = format!("migration:adc:{legacy_record}:v1");
    let payload = serde_json::json!({"legacy": true});
    let payload_digest =
        svc_workflow::domain::definition::digest::compute_json_digest(&payload).unwrap();
    let event_data = serde_json::json!({
        "legacySystem": "adc",
        "legacyRecordId": legacy_record,
        "legacySnapshotDigest": "a".repeat(64),
        "importedNodeId": fixture.draft,
        "importedAt": "2026-07-15T00:00:00Z",
        "creatorResolution": "LEGACY_CREATOR",
    });
    let event_digest =
        svc_workflow::domain::definition::digest::compute_json_digest(&event_data).unwrap();
    let response_body = serde_json::json!({
        "commandId": command,
        "workflowInstanceId": imported_instance,
        "currentContextRevisionId": context,
        "currentNodeVisitId": visit,
        "eventId": event,
        "workflowStateVersion": 1,
        "eventSequence": 1,
        "legacySnapshotDigest": "a".repeat(64),
        "creatorResolution": "LEGACY_CREATOR",
        "replayed": false,
    });
    let response_digest =
        svc_workflow::domain::definition::digest::compute_json_digest(&response_body).unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, enabled)
         VALUES ($1, 'SERVICE', 'Historical migration service', TRUE)",
    )
    .bind(migration_service)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_instances
         (workflow_instance_id, domain_id, definition_version_id,
          created_by_principal_id, workflow_state_version, external_reference)
         VALUES ($1, $2, $3, $4, 1, $5)",
    )
    .bind(imported_instance)
    .bind(fixture.domain)
    .bind(fixture.version)
    .bind(fixture.creator)
    .bind(&external_reference)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_context_revisions
         (context_revision_id, workflow_instance_id, revision_number,
          payload, payload_digest, created_by_principal_id)
         VALUES ($1, $2, 1, $3, $4, $5)",
    )
    .bind(context)
    .bind(imported_instance)
    .bind(payload)
    .bind(payload_digest)
    .bind(fixture.creator)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_visits
         (node_visit_id, workflow_instance_id, node_id, visit_number,
          assignee_principal_id)
         VALUES ($1, $2, $3, 1, $4)",
    )
    .bind(visit)
    .bind(imported_instance)
    .bind(fixture.draft)
    .bind(fixture.creator)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_command_receipts
         (command_id, principal_id, idempotency_key, command_type, request_hash,
          receipt_status, response_status, response_body, response_digest)
         VALUES ($1, $2, $3, 'IMPORT_LEGACY_WORKFLOW_INSTANCE', $4,
                 'COMPLETED', 200, $5, $6)",
    )
    .bind(command)
    .bind(migration_service)
    .bind(&external_reference)
    .bind("b".repeat(64))
    .bind(response_body)
    .bind(response_digest)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_events
         (event_id, workflow_instance_id, event_sequence, event_schema_version,
          command_id, event_type, target_node_visit_id, context_revision_id, event_data,
          event_data_digest, actor_principal_id, old_workflow_state_version,
          new_workflow_state_version)
         VALUES ($1, $2, 1, 'v1', $3, 'WORKFLOW_INSTANCE_IMPORTED', $4, $5,
                 $6, $7, $8, 0, 1)",
    )
    .bind(event)
    .bind(imported_instance)
    .bind(command)
    .bind(visit)
    .bind(context)
    .bind(event_data)
    .bind(event_digest)
    .bind(migration_service)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_instances SET current_context_revision_id = $2,
             current_node_visit_id = $3 WHERE workflow_instance_id = $1",
    )
    .bind(imported_instance)
    .bind(context)
    .bind(visit)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut command = rebuild_command(&fixture);
    command.workflow_instance_id = WorkflowInstanceId::from_uuid(imported_instance);
    assert!(run_rebuild(&pool, command).await.is_ok());
}

#[tokio::test]
async fn rebuild_accepts_revise_combined_transition_and_admin_event_matrices() {
    let pool = create_pool().await;
    let revise_fixture = seed_recovery_fixture(&pool).await;
    revise_workflow_context(
        &pool,
        ReviseWorkflowContextCommand {
            principal_id: PrincipalId::from_uuid(revise_fixture.creator),
            idempotency_key: Uuid::new_v4().to_string(),
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(revise_fixture.instance),
            expected_workflow_state_version: 1,
            context_payload: serde_json::json!({"revised": true}),
        },
    )
    .await
    .unwrap();
    assert!(run_rebuild(&pool, rebuild_command(&revise_fixture))
        .await
        .is_ok());

    let combined_fixture = seed_recovery_fixture(&pool).await;
    let draft_advance: Uuid = sqlx::query_scalar(
        "SELECT primary_advance_transition_id FROM workflow_node_definitions WHERE node_id = $1",
    )
    .bind(combined_fixture.draft)
    .fetch_one(&pool)
    .await
    .unwrap();
    revise_context_and_transition(
        &pool,
        make_combined_command(
            combined_fixture.creator,
            combined_fixture.instance,
            1,
            draft_advance,
        ),
    )
    .await
    .unwrap();
    assert!(run_rebuild(&pool, rebuild_command(&combined_fixture))
        .await
        .is_ok());

    let transition_fixture = seed_recovery_fixture(&pool).await;
    run_override(
        &pool,
        override_command(
            &transition_fixture,
            svc_workflow::domain::workflow_instance::recovery::AdminEmergencyOperation::MoveToNode,
            transition_fixture.normal,
        ),
    )
    .await
    .unwrap();
    let normal_advance: Uuid = sqlx::query_scalar(
        "SELECT primary_advance_transition_id FROM workflow_node_definitions WHERE node_id = $1",
    )
    .bind(transition_fixture.normal)
    .fetch_one(&pool)
    .await
    .unwrap();
    execute_workflow_transition(
        &pool,
        make_transition_command(
            transition_fixture.creator,
            transition_fixture.instance,
            2,
            normal_advance,
            None,
        ),
    )
    .await
    .unwrap();
    assert!(run_rebuild(&pool, rebuild_command(&transition_fixture))
        .await
        .is_ok());
}
