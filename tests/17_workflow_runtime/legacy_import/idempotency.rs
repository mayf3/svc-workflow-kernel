use super::*;

use svc_workflow::application::workflow_instance::import::compute_legacy_import_request_hash;
use svc_workflow::domain::workflow_instance::import::COMMAND_TYPE;

#[tokio::test]
async fn exact_replay_returns_the_stored_fact_identifiers() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let first = run(&fixture).await.unwrap();
    let second = run(&fixture).await.unwrap();
    assert!(!first.replayed);
    assert!(second.replayed);
    assert_eq!(first.command_id, second.command_id);
    assert_eq!(first.workflow_instance_id, second.workflow_instance_id);
    assert_eq!(
        first.current_context_revision_id,
        second.current_context_revision_id
    );
    assert_eq!(first.current_node_visit_id, second.current_node_visit_id);
    assert_eq!(first.event_id, second.event_id);
}

#[tokio::test]
async fn exact_replay_survives_valid_post_import_lifecycle_changes() {
    let fixture = fixture(ImportedNodeKind::Draft).await;
    let first = run(&fixture).await.unwrap();
    let revised = revise_workflow_context(
        &fixture.pool,
        ReviseWorkflowContextCommand {
            principal_id: PrincipalId::from_uuid(fixture.owner),
            idempotency_key: Uuid::new_v4().to_string(),
            command_schema_version: "v1".to_string(),
            workflow_instance_id: WorkflowInstanceId::from_uuid(first.workflow_instance_id),
            expected_workflow_state_version: 1,
            context_payload: serde_json::json!({"requirementId": "revised"}),
        },
    )
    .await
    .unwrap();
    assert_eq!(revised.workflow_state_version, 2);
    let transition: Uuid = sqlx::query_scalar(
        "SELECT primary_advance_transition_id
         FROM workflow_node_definitions WHERE node_id = $1",
    )
    .bind(fixture.node)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    let transitioned = execute_workflow_transition(
        &fixture.pool,
        make_transition_command(
            fixture.owner,
            first.workflow_instance_id,
            2,
            transition,
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(transitioned.workflow_state_version, 3);
    assert_ne!(
        transitioned.current_node_visit_id,
        first.current_node_visit_id
    );
    let replay = run(&fixture).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.command_id, first.command_id);
    assert_eq!(replay.workflow_instance_id, first.workflow_instance_id);
    assert_eq!(
        replay.current_context_revision_id,
        first.current_context_revision_id
    );
    assert_eq!(replay.current_node_visit_id, first.current_node_visit_id);
    assert_eq!(replay.event_id, first.event_id);
    assert_eq!(replay.workflow_state_version, 1);
    assert_eq!(replay.event_sequence, 1);
}

#[tokio::test]
async fn same_fixed_key_with_different_request_conflicts() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    run(&fixture).await.unwrap();
    let mut changed = fixture.command.clone();
    changed.metadata = serde_json::json!({"source": "changed"});
    let error = import_legacy_workflow_instance(&fixture.pool, changed)
        .await
        .unwrap_err();
    assert_eq!(error, LegacyImportError::IdempotencyConflict);
    let instances: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_instances WHERE external_reference=$1")
            .bind(fixture.command.external_reference())
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(instances, 1);
}

#[tokio::test]
async fn concurrent_identical_import_creates_one_fact_set() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let pool_a = fixture.pool.clone();
    let pool_b = fixture.pool.clone();
    let command_a = fixture.command.clone();
    let command_b = fixture.command.clone();
    let (left, right) = tokio::join!(
        import_legacy_workflow_instance(&pool_a, command_a),
        import_legacy_workflow_instance(&pool_b, command_b)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.workflow_instance_id, right.workflow_instance_id);
    assert_ne!(left.replayed, right.replayed);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_instances WHERE external_reference=$1")
            .bind(fixture.command.external_reference())
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn processing_receipt_returns_retryable_error() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let hash = compute_legacy_import_request_hash(&fixture.command).unwrap();
    sqlx::query(
        "INSERT INTO workflow_command_receipts
         (command_id, principal_id, idempotency_key, command_type, request_hash, receipt_status)
         VALUES ($1,$2,$3,$4,$5,'PROCESSING')",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.service)
    .bind(fixture.command.idempotency_key())
    .bind(COMMAND_TYPE)
    .bind(hash)
    .execute(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(
        run(&fixture).await.unwrap_err(),
        LegacyImportError::CommandStillProcessing
    );
}

#[tokio::test]
async fn replay_revalidates_current_migration_authorization() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    run(&fixture).await.unwrap();
    sqlx::query(
        "UPDATE domain_role_bindings SET enabled=FALSE, disabled_at=now()
         WHERE domain_id=$1 AND role_key='WORKFLOW_MIGRATION'",
    )
    .bind(fixture.domain)
    .execute(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(
        run(&fixture).await.unwrap_err(),
        LegacyImportError::MigrationBindingInvalid
    );
}

#[tokio::test]
async fn deterministic_failure_completes_receipt_for_stable_replay() {
    let mut fixture = fixture(ImportedNodeKind::Normal).await;
    fixture.command.expected_legacy_snapshot_digest = "0".repeat(64);
    assert!(matches!(
        run(&fixture).await.unwrap_err(),
        LegacyImportError::SnapshotDigestMismatch { .. }
    ));
    let receipt: (String, i32, String) = sqlx::query_as(
        "SELECT receipt_status::text, response_status, response_body->>'error'
         FROM workflow_command_receipts WHERE principal_id=$1 AND idempotency_key=$2",
    )
    .bind(fixture.service)
    .bind(fixture.command.idempotency_key())
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(
        receipt,
        (
            "COMPLETED".to_string(),
            409,
            "snapshot_digest_mismatch".to_string()
        )
    );
    assert!(matches!(
        run(&fixture).await.unwrap_err(),
        LegacyImportError::SnapshotDigestMismatch { .. }
    ));
}

#[tokio::test]
async fn server_derived_external_reference_is_globally_collision_checked() {
    let first = fixture(ImportedNodeKind::Normal).await;
    run(&first).await.unwrap();
    let mut second = fixture(ImportedNodeKind::Normal).await;
    second.command.legacy_record_id = first.command.legacy_record_id;
    second.command.legacy_snapshot.requirement_id = first.command.legacy_record_id;
    second.command.expected_legacy_snapshot_digest =
        second.command.legacy_snapshot.digest().unwrap();
    assert_eq!(
        run(&second).await.unwrap_err(),
        LegacyImportError::ExternalReferenceConflict
    );
}

#[tokio::test]
async fn request_hash_covers_route_parameters_and_complete_body() {
    let fixture = fixture(ImportedNodeKind::Normal).await;
    let original = compute_legacy_import_request_hash(&fixture.command).unwrap();
    let mut variants = Vec::new();
    let mut domain = fixture.command.clone();
    domain.domain_id = DomainId::new();
    variants.push(domain);
    let mut definition = fixture.command.clone();
    definition.definition_version_id = DefinitionVersionId::new();
    variants.push(definition);
    let mut node = fixture.command.clone();
    node.imported_node_id = NodeId::new();
    variants.push(node);
    let mut snapshot = fixture.command.clone();
    snapshot.command_schema_version = "v2".to_string();
    variants.push(snapshot);
    let mut expected = fixture.command.clone();
    expected.expected_legacy_snapshot_digest = "0".repeat(64);
    variants.push(expected);
    let mut creator = fixture.command.clone();
    creator.legacy_creator_principal_id = None;
    variants.push(creator);
    let mut url = fixture.command.clone();
    url.external_url = None;
    variants.push(url);
    let mut metadata = fixture.command.clone();
    metadata.metadata = serde_json::json!({"changed": true});
    variants.push(metadata);
    for variant in variants {
        assert_ne!(
            compute_legacy_import_request_hash(&variant).unwrap(),
            original
        );
    }
}

async fn tamper_success_receipt(
    fixture: &ImportFixture,
    result: &ImportLegacyWorkflowInstanceResult,
    case: &str,
) {
    let mut transaction = fixture.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *transaction)
        .await
        .unwrap();
    let mut body: serde_json::Value = sqlx::query_scalar(
        "SELECT response_body FROM workflow_command_receipts WHERE command_id=$1",
    )
    .bind(result.command_id)
    .fetch_one(&mut *transaction)
    .await
    .unwrap();
    match case {
        "response digest" => {}
        "body fact id" => body["workflowInstanceId"] = Uuid::new_v4().to_string().into(),
        "extra field" => body["unexpected"] = true.into(),
        _ => unreachable!("unknown replay tamper case"),
    }
    let response_digest = if case == "response digest" {
        "0".repeat(64)
    } else {
        svc_workflow::domain::definition::digest::compute_json_digest(&body).unwrap()
    };
    sqlx::query(
        "UPDATE workflow_command_receipts
         SET response_body=$2, response_digest=$3 WHERE command_id=$1",
    )
    .bind(result.command_id)
    .bind(body)
    .bind(response_digest)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
}

#[tokio::test]
async fn exact_replay_rejects_corrupted_success_receipt() {
    for case in ["response digest", "body fact id", "extra field"] {
        let fixture = fixture(ImportedNodeKind::Normal).await;
        let result = run(&fixture).await.unwrap();
        tamper_success_receipt(&fixture, &result, case).await;
        assert!(matches!(
            run(&fixture).await.unwrap_err(),
            LegacyImportError::InternalConsistency(_)
        ));
    }
}
