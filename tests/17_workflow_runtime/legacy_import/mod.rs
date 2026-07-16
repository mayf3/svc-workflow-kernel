use super::*;

use svc_workflow::application::workflow_instance::import::{
    import_legacy_workflow_instance, ImportLegacyWorkflowInstanceResult,
};
use svc_workflow::domain::workflow_instance::import::{
    ImportLegacyWorkflowInstanceCommand, LegacyAdcImportSnapshotV1, LegacyImportError,
    COMMAND_SCHEMA_VERSION, SNAPSHOT_SCHEMA_VERSION,
};

mod authorization_concurrency;
mod fault_atomicity;
mod idempotency;
mod request_hash_contract;
mod strict_rebuild;
mod success;
mod validation;

#[derive(Clone, Copy)]
pub(crate) enum ImportedNodeKind {
    Draft,
    Normal,
    Terminal,
}

pub(crate) struct ImportFixture {
    pub pool: PgPool,
    pub owner: Uuid,
    pub service: Uuid,
    pub domain: Uuid,
    pub version: Uuid,
    pub node: Uuid,
    pub command: ImportLegacyWorkflowInstanceCommand,
}

async fn seed_service(pool: &PgPool, domain: Uuid) -> Uuid {
    let service = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO principals
         (principal_id, principal_type, display_name, enabled)
         VALUES ($1, 'SERVICE', 'ADC migration', TRUE)",
    )
    .bind(service)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1, $2, $3, 'WORKFLOW_MIGRATION', TRUE)",
    )
    .bind(Uuid::new_v4())
    .bind(domain)
    .bind(service)
    .execute(pool)
    .await
    .unwrap();
    service
}

async fn definition_for(pool: &PgPool, domain: Uuid, kind: ImportedNodeKind) -> (Uuid, Uuid) {
    match kind {
        ImportedNodeKind::Draft => {
            let (_, version) = seed_published_definition_wf_creator(pool, domain).await;
            let node = sqlx::query_scalar(
                "SELECT node_id FROM workflow_node_definitions
                 WHERE definition_version_id = $1 AND node_type = 'DRAFT'",
            )
            .bind(version)
            .fetch_one(pool)
            .await
            .unwrap();
            (version, node)
        }
        ImportedNodeKind::Normal => {
            let (_, version, node) = seed_published_definition_normal_node(pool, domain).await;
            (version, node)
        }
        ImportedNodeKind::Terminal => {
            let (_, version, node) = seed_published_definition_terminal_only(pool, domain).await;
            (version, node)
        }
    }
}

pub(crate) async fn fixture(kind: ImportedNodeKind) -> ImportFixture {
    let pool = create_pool().await;
    let (owner, domain) = seed_principal_domain_with_owner(&pool).await;
    let service = seed_service(&pool, domain).await;
    let (version, node) = definition_for(&pool, domain, kind).await;
    let domain_key: String =
        sqlx::query_scalar("SELECT domain_key FROM domains WHERE domain_id=$1")
            .bind(domain)
            .fetch_one(&pool)
            .await
            .unwrap();
    let node_key: String =
        sqlx::query_scalar("SELECT node_key FROM workflow_node_definitions WHERE node_id=$1")
            .bind(node)
            .fetch_one(&pool)
            .await
            .unwrap();
    let legacy_record = Uuid::new_v4();
    let snapshot = LegacyAdcImportSnapshotV1 {
        schema_version: SNAPSHOT_SCHEMA_VERSION.to_string(),
        requirement_id: legacy_record,
        domain_key,
        workflow_id: "adc-workflow-v1".to_string(),
        workflow_snapshot: serde_json::json!({"id": "adc-workflow-v1", "steps": ["draft", "review", "done"]}),
        current_step: node_key,
        assignee_id: (!matches!(kind, ImportedNodeKind::Terminal)).then_some(owner),
        requester_id: Some(owner),
        state_version: 9,
        updated_at: "2026-07-15T01:02:03Z".to_string(),
        context_payload: serde_json::json!({"requirementId": legacy_record}),
    };
    let digest = snapshot.digest().unwrap();
    let command = ImportLegacyWorkflowInstanceCommand {
        principal_id: PrincipalId::from_uuid(service),
        command_schema_version: COMMAND_SCHEMA_VERSION.to_string(),
        domain_id: DomainId::from_uuid(domain),
        definition_version_id: DefinitionVersionId::from_uuid(version),
        imported_node_id: NodeId::from_uuid(node),
        legacy_record_id: legacy_record,
        legacy_snapshot: snapshot,
        expected_legacy_snapshot_digest: digest,
        legacy_creator_principal_id: Some(PrincipalId::from_uuid(owner)),
        external_url: Some("https://adc.example.test/requirements/1".to_string()),
        metadata: serde_json::json!({"source": "adc-import"}),
    };
    ImportFixture {
        pool,
        owner,
        service,
        domain,
        version,
        node,
        command,
    }
}

pub(crate) async fn run(
    fixture: &ImportFixture,
) -> Result<ImportLegacyWorkflowInstanceResult, LegacyImportError> {
    import_legacy_workflow_instance(&fixture.pool, fixture.command.clone()).await
}
