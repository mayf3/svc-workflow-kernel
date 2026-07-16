use super::*;

use svc_workflow::application::workflow_instance::admin_recovery::rebuild_projection;
use svc_workflow::domain::workflow_instance::import::CreatorResolution;
use svc_workflow::domain::workflow_instance::recovery::RebuildProjectionCommand;

#[derive(sqlx::FromRow)]
struct ImportedEventRow {
    event_type: String,
    transition_effect: Option<String>,
    source_node_visit_id: Option<Uuid>,
    target_node_visit_id: Uuid,
    context_revision_id: Uuid,
    submission_id: Option<Uuid>,
    from_node_id: Option<Uuid>,
    to_node_id: Option<Uuid>,
    event_data: serde_json::Value,
    actor_principal_id: Uuid,
    old_workflow_state_version: i32,
    new_workflow_state_version: i32,
}

#[tokio::test]
async fn imports_draft_normal_and_terminal_initial_facts() {
    for kind in [
        ImportedNodeKind::Draft,
        ImportedNodeKind::Normal,
        ImportedNodeKind::Terminal,
    ] {
        let fixture = fixture(kind).await;
        let result = run(&fixture).await.unwrap();
        assert_eq!(result.workflow_state_version, 1);
        assert_eq!(result.event_sequence, 1);
        assert_eq!(result.creator_resolution, CreatorResolution::LegacyCreator);
        let row: (Uuid, Uuid, Uuid, i32, String) = sqlx::query_as(
            "SELECT created_by_principal_id, current_context_revision_id,
                    current_node_visit_id, workflow_state_version, external_reference
             FROM workflow_instances WHERE workflow_instance_id = $1",
        )
        .bind(result.workflow_instance_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(row.0, fixture.owner);
        assert_eq!(row.1, result.current_context_revision_id);
        assert_eq!(row.2, result.current_node_visit_id);
        assert_eq!(row.3, 1);
        assert_eq!(row.4, fixture.command.idempotency_key());
        let assignee: Option<Uuid> = sqlx::query_scalar(
            "SELECT assignee_principal_id FROM workflow_node_visits WHERE node_visit_id=$1",
        )
        .bind(result.current_node_visit_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        if matches!(kind, ImportedNodeKind::Terminal) {
            assert_eq!(assignee, None);
        } else {
            assert_eq!(assignee, Some(fixture.owner));
        }
    }
}

#[tokio::test]
async fn import_event_has_exact_shape_and_null_matrix() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let result = run(&fixture).await.unwrap();
    let event: ImportedEventRow = sqlx::query_as(
        "SELECT event_type, transition_effect::text, source_node_visit_id,
                target_node_visit_id, context_revision_id, submission_id,
                from_node_id, to_node_id, event_data, actor_principal_id,
                old_workflow_state_version, new_workflow_state_version
         FROM workflow_events WHERE event_id=$1",
    )
    .bind(result.event_id)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(event.event_type, "WORKFLOW_INSTANCE_IMPORTED");
    assert_eq!(event.transition_effect, None);
    assert_eq!(event.source_node_visit_id, None);
    assert_eq!(event.target_node_visit_id, result.current_node_visit_id);
    assert_eq!(
        event.context_revision_id,
        result.current_context_revision_id
    );
    assert_eq!(event.submission_id, None);
    assert_eq!(event.from_node_id, None);
    assert_eq!(event.to_node_id, None);
    assert_eq!(event.actor_principal_id, fixture.service);
    assert_eq!(
        (
            event.old_workflow_state_version,
            event.new_workflow_state_version
        ),
        (0, 1)
    );
    let object = event.event_data.as_object().unwrap();
    let expected = [
        "legacySystem",
        "legacyRecordId",
        "legacySnapshotDigest",
        "importedNodeId",
        "importedAt",
        "creatorResolution",
    ];
    assert_eq!(object.len(), expected.len());
    assert!(expected.iter().all(|key| object.contains_key(*key)));
    assert_eq!(event.event_data["legacySystem"], "adc");
    assert_eq!(
        event.event_data["legacyRecordId"],
        fixture.command.legacy_record_id.to_string()
    );
    assert_eq!(event.event_data["importedNodeId"], fixture.node.to_string());
    let imported_at = event.event_data["importedAt"].as_str().unwrap();
    assert_eq!(imported_at.len(), 20);
    assert!(chrono::NaiveDateTime::parse_from_str(imported_at, "%Y-%m-%dT%H:%M:%SZ").is_ok());
    let receipt_completed: bool = sqlx::query_scalar(
        "SELECT completed_at IS NOT NULL FROM workflow_command_receipts WHERE command_id=$1",
    )
    .bind(result.command_id)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert!(receipt_completed);
}

#[tokio::test]
async fn owner_fallback_never_uses_service_as_creator() {
    let mut fixture = fixture(ImportedNodeKind::Normal).await;
    fixture.command.legacy_creator_principal_id = None;
    let result = run(&fixture).await.unwrap();
    assert_eq!(
        result.creator_resolution,
        CreatorResolution::DomainOwnerFallback
    );
    let creator: Uuid = sqlx::query_scalar(
        "SELECT created_by_principal_id FROM workflow_instances WHERE workflow_instance_id=$1",
    )
    .bind(result.workflow_instance_id)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(creator, fixture.owner);
    assert_ne!(creator, fixture.service);
}

#[tokio::test]
async fn import_has_no_submission_and_rebuild_accepts_event() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let result = run(&fixture).await.unwrap();
    let submissions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_submissions WHERE workflow_instance_id=$1",
    )
    .bind(result.workflow_instance_id)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(submissions, 0);
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1, $2, $3, 'WORKFLOW_ADMIN', TRUE)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.domain)
    .bind(fixture.owner)
    .execute(&fixture.pool)
    .await
    .unwrap();
    let rebuilt = rebuild_projection(
        &fixture.pool,
        RebuildProjectionCommand {
            principal_id: PrincipalId::from_uuid(fixture.owner),
            idempotency_key: Uuid::new_v4().to_string(),
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(result.workflow_instance_id),
            expected_before_snapshot_digest: None,
        },
    )
    .await
    .unwrap();
    assert!(!rebuilt.changed);
    assert_eq!(rebuilt.after_projection.workflow_state_version, 1);
}
