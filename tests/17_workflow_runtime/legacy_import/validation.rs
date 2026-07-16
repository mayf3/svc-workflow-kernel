use super::*;

fn refresh_digest(command: &mut ImportLegacyWorkflowInstanceCommand) {
    command.expected_legacy_snapshot_digest = command.legacy_snapshot.digest().unwrap();
}

#[tokio::test]
async fn migration_actor_must_be_enabled_service_with_exact_binding() {
    let missing = fixture(ImportedNodeKind::Normal).await;
    sqlx::query(
        "UPDATE domain_role_bindings SET enabled=FALSE, disabled_at=now()
         WHERE domain_id=$1 AND role_key='WORKFLOW_MIGRATION'",
    )
    .bind(missing.domain)
    .execute(&missing.pool)
    .await
    .unwrap();
    assert_eq!(
        run(&missing).await.unwrap_err(),
        LegacyImportError::MigrationBindingInvalid
    );

    let mut wrong_type = fixture(ImportedNodeKind::Normal).await;
    wrong_type.command.principal_id = PrincipalId::from_uuid(wrong_type.owner);
    assert_eq!(
        run(&wrong_type).await.unwrap_err(),
        LegacyImportError::PrincipalTypeNotAllowed
    );

    let disabled = fixture(ImportedNodeKind::Normal).await;
    sqlx::query("UPDATE principals SET enabled=FALSE WHERE principal_id=$1")
        .bind(disabled.service)
        .execute(&disabled.pool)
        .await
        .unwrap();
    assert_eq!(
        run(&disabled).await.unwrap_err(),
        LegacyImportError::PrincipalDisabled
    );
}

#[tokio::test]
async fn multiple_or_actor_mismatched_migration_bindings_are_rejected() {
    let multiple = fixture(ImportedNodeKind::Normal).await;
    let second = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, enabled)
         VALUES ($1, 'SERVICE', 'other migrator', TRUE)",
    )
    .bind(second)
    .execute(&multiple.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1,$2,$3,'WORKFLOW_MIGRATION',TRUE)",
    )
    .bind(Uuid::new_v4())
    .bind(multiple.domain)
    .bind(second)
    .execute(&multiple.pool)
    .await
    .unwrap();
    assert_eq!(
        run(&multiple).await.unwrap_err(),
        LegacyImportError::MigrationBindingInvalid
    );

    let mismatch = fixture(ImportedNodeKind::Normal).await;
    sqlx::query(
        "UPDATE domain_role_bindings SET enabled=FALSE, disabled_at=now()
         WHERE domain_id=$1 AND role_key='WORKFLOW_MIGRATION'",
    )
    .bind(mismatch.domain)
    .execute(&mismatch.pool)
    .await
    .unwrap();
    let other = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals (principal_id, principal_type, display_name, enabled)
         VALUES ($1, 'SERVICE', 'other migrator', TRUE)",
    )
    .bind(other)
    .execute(&mismatch.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1,$2,$3,'WORKFLOW_MIGRATION',TRUE)",
    )
    .bind(Uuid::new_v4())
    .bind(mismatch.domain)
    .bind(other)
    .execute(&mismatch.pool)
    .await
    .unwrap();
    assert_eq!(
        run(&mismatch).await.unwrap_err(),
        LegacyImportError::MigrationBindingInvalid
    );
}

#[tokio::test]
async fn digest_record_domain_and_node_mismatches_are_rejected() {
    let mut digest = fixture(ImportedNodeKind::Normal).await;
    digest.command.expected_legacy_snapshot_digest = "0".repeat(64);
    assert!(matches!(
        run(&digest).await.unwrap_err(),
        LegacyImportError::SnapshotDigestMismatch { .. }
    ));

    let mut record = fixture(ImportedNodeKind::Normal).await;
    record.command.legacy_snapshot.requirement_id = Uuid::new_v4();
    refresh_digest(&mut record.command);
    assert!(matches!(
        run(&record).await.unwrap_err(),
        LegacyImportError::InvalidInput(_)
    ));

    let mut domain = fixture(ImportedNodeKind::Normal).await;
    domain.command.legacy_snapshot.domain_key = "other-domain".to_string();
    refresh_digest(&mut domain.command);
    assert!(matches!(
        run(&domain).await.unwrap_err(),
        LegacyImportError::InvalidInput(_)
    ));

    let mut node = fixture(ImportedNodeKind::Normal).await;
    node.command.legacy_snapshot.current_step = "different-step".to_string();
    refresh_digest(&mut node.command);
    assert!(matches!(
        run(&node).await.unwrap_err(),
        LegacyImportError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn unpublished_definition_and_disabled_domain_are_rejected() {
    let unpublished = fixture(ImportedNodeKind::Normal).await;
    let mut transaction = unpublished.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status='REVOKED'
         WHERE definition_version_id=$1",
    )
    .bind(unpublished.version)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(
        run(&unpublished).await.unwrap_err(),
        LegacyImportError::VersionNotPublished
    );

    let disabled = fixture(ImportedNodeKind::Normal).await;
    sqlx::query("UPDATE domains SET enabled=FALSE WHERE domain_id=$1")
        .bind(disabled.domain)
        .execute(&disabled.pool)
        .await
        .unwrap();
    assert_eq!(
        run(&disabled).await.unwrap_err(),
        LegacyImportError::DomainDisabled
    );
}

#[tokio::test]
async fn creator_identity_and_nonterminal_assignee_are_bound_to_snapshot() {
    let mut creator = fixture(ImportedNodeKind::Normal).await;
    creator.command.legacy_snapshot.requester_id = Some(Uuid::new_v4());
    refresh_digest(&mut creator.command);
    assert!(matches!(
        run(&creator).await.unwrap_err(),
        LegacyImportError::InvalidInput(_)
    ));

    let mut assignee = fixture(ImportedNodeKind::Normal).await;
    assignee.command.legacy_snapshot.assignee_id = Some(Uuid::new_v4());
    refresh_digest(&mut assignee.command);
    assert!(matches!(
        run(&assignee).await.unwrap_err(),
        LegacyImportError::AssigneeResolutionFailed(_)
    ));
}

#[tokio::test]
async fn role_map_pseudo_state_and_control_characters_are_rejected() {
    let mut role_map = fixture(ImportedNodeKind::Normal).await;
    role_map.command.legacy_snapshot.workflow_snapshot =
        serde_json::json!({"nested": {"roleUserMap": {"cto": "user"}}});
    refresh_digest(&mut role_map.command);
    assert!(matches!(
        run(&role_map).await.unwrap_err(),
        LegacyImportError::InvalidInput(_)
    ));

    let mut pseudo = fixture(ImportedNodeKind::Normal).await;
    pseudo.command.legacy_snapshot.current_step = "in_progress".to_string();
    refresh_digest(&mut pseudo.command);
    assert!(matches!(
        run(&pseudo).await.unwrap_err(),
        LegacyImportError::InvalidInput(_)
    ));

    let mut control = fixture(ImportedNodeKind::Normal).await;
    control.command.legacy_snapshot.workflow_id = "workflow\0bad".to_string();
    refresh_digest(&mut control.command);
    assert!(matches!(
        run(&control).await.unwrap_err(),
        LegacyImportError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn terminal_assignee_must_be_null() {
    let mut fixture = fixture(ImportedNodeKind::Terminal).await;
    fixture.command.legacy_snapshot.assignee_id = Some(fixture.owner);
    refresh_digest(&mut fixture.command);
    assert!(matches!(
        run(&fixture).await.unwrap_err(),
        LegacyImportError::AssigneeResolutionFailed(_)
    ));
}

#[tokio::test]
async fn context_schema_and_payload_bounds_are_enforced() {
    let invalid_schema = fixture(ImportedNodeKind::Normal).await;
    let mut transaction = invalid_schema.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_definition_versions
         SET context_schema='{\"type\":\"object\",\"required\":[\"approved\"]}'::jsonb
         WHERE definition_version_id=$1",
    )
    .bind(invalid_schema.version)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert!(matches!(
        run(&invalid_schema).await.unwrap_err(),
        LegacyImportError::ContextValidationFailed(_)
    ));

    let mut context = fixture(ImportedNodeKind::Normal).await;
    context.command.legacy_snapshot.context_payload =
        serde_json::json!({"v": "x".repeat(1024 * 1024)});
    refresh_digest(&mut context.command);
    assert!(matches!(
        run(&context).await.unwrap_err(),
        LegacyImportError::SizeLimitExceeded(_)
    ));

    let mut workflow = fixture(ImportedNodeKind::Normal).await;
    workflow.command.legacy_snapshot.workflow_snapshot =
        serde_json::json!({"v": "x".repeat(1024 * 1024)});
    refresh_digest(&mut workflow.command);
    assert!(matches!(
        run(&workflow).await.unwrap_err(),
        LegacyImportError::SizeLimitExceeded(_)
    ));

    let mut metadata = fixture(ImportedNodeKind::Normal).await;
    metadata.command.metadata = serde_json::json!({"v": "x".repeat(64 * 1024)});
    assert!(matches!(
        run(&metadata).await.unwrap_err(),
        LegacyImportError::SizeLimitExceeded(_)
    ));

    let mut url = fixture(ImportedNodeKind::Normal).await;
    url.command.external_url = Some(format!("https://example.test/{}", "x".repeat(2048)));
    assert!(matches!(
        run(&url).await.unwrap_err(),
        LegacyImportError::SizeLimitExceeded(_)
    ));
}

#[tokio::test]
async fn definition_domain_and_node_cross_version_references_are_rejected() {
    let mut node_cross = fixture(ImportedNodeKind::Normal).await;
    let other = fixture(ImportedNodeKind::Normal).await;
    node_cross.command.imported_node_id = NodeId::from_uuid(other.node);
    assert_eq!(
        run(&node_cross).await.unwrap_err(),
        LegacyImportError::ImportedNodeNotFound
    );

    let mut definition_cross = fixture(ImportedNodeKind::Normal).await;
    let other = fixture(ImportedNodeKind::Normal).await;
    definition_cross.command.definition_version_id = DefinitionVersionId::from_uuid(other.version);
    definition_cross.command.imported_node_id = NodeId::from_uuid(other.node);
    assert_eq!(
        run(&definition_cross).await.unwrap_err(),
        LegacyImportError::PermissionDenied
    );
}

#[tokio::test]
async fn missing_actor_principal_creates_no_receipt() {
    let mut fixture = fixture(ImportedNodeKind::Normal).await;
    let missing = Uuid::new_v4();
    fixture.command.principal_id = PrincipalId::from_uuid(missing);
    assert_eq!(
        run(&fixture).await.unwrap_err(),
        LegacyImportError::PrincipalNotFound
    );
    let receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_command_receipts WHERE principal_id=$1")
            .bind(missing)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(receipts, 0);
}

async fn set_node_assignee(fixture: &ImportFixture, assignee_type: &str, fixed: Option<Uuid>) {
    let mut transaction = fixture.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_node_definitions
         SET assignee_ref_type=$2::assignee_ref_type, fixed_principal_id=$3 WHERE node_id=$1",
    )
    .bind(fixture.node)
    .bind(assignee_type)
    .bind(fixed)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
}

#[tokio::test]
async fn domain_owner_and_fixed_principal_assignee_resolvers_succeed() {
    let owner = fixture(ImportedNodeKind::Normal).await;
    set_node_assignee(&owner, "DOMAIN_OWNER", None).await;
    assert!(run(&owner).await.is_ok());

    let mut fixed = fixture(ImportedNodeKind::Normal).await;
    let principal = seed_second_principal(&fixed.pool).await;
    set_node_assignee(&fixed, "FIXED_PRINCIPAL", Some(principal)).await;
    fixed.command.legacy_snapshot.assignee_id = Some(principal);
    refresh_digest(&mut fixed.command);
    let result = run(&fixed).await.unwrap();
    let actual: Uuid = sqlx::query_scalar(
        "SELECT assignee_principal_id FROM workflow_node_visits WHERE node_visit_id=$1",
    )
    .bind(result.current_node_visit_id)
    .fetch_one(&fixed.pool)
    .await
    .unwrap();
    assert_eq!(actual, principal);
}
