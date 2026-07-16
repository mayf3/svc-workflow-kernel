use super::*;

use svc_workflow::application::workflow_instance::admin_recovery::{
    AdminEmergencyOverrideResult, RebuildProjectionResult,
};
use svc_workflow::domain::workflow_instance::recovery::{
    AdminEmergencyOperation, AdminEmergencyOverrideCommand, AdminRelatedReference,
    RebuildProjectionCommand,
};

pub(crate) struct RecoveryFixture {
    pub creator: Uuid,
    pub admin: Uuid,
    pub outsider: Uuid,
    pub domain: Uuid,
    pub version: Uuid,
    pub draft: Uuid,
    pub normal: Uuid,
    pub terminal: Uuid,
    pub instance: Uuid,
    pub initial_context: Uuid,
    pub initial_visit: Uuid,
}

pub(crate) async fn bind_workflow_admin(pool: &PgPool, domain: Uuid, actor: Uuid) -> Uuid {
    let binding = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1, $2, $3, 'WORKFLOW_ADMIN', TRUE)",
    )
    .bind(binding)
    .bind(domain)
    .bind(actor)
    .execute(pool)
    .await
    .unwrap();
    binding
}

pub(crate) async fn seed_recovery_fixture(pool: &PgPool) -> RecoveryFixture {
    let (creator, domain) = seed_principal_domain_with_owner(pool).await;
    let admin = seed_second_principal(pool).await;
    let outsider = seed_second_principal(pool).await;
    bind_workflow_admin(pool, domain, admin).await;
    let (_, version, draft, normal, terminal, ..) =
        seed_transition_graph(pool, domain, "WORKFLOW_CREATOR", "WORKFLOW_CREATOR", None).await;
    let created = create_workflow_instance(pool, make_command(creator, domain, version))
        .await
        .unwrap();
    RecoveryFixture {
        creator,
        admin,
        outsider,
        domain,
        version,
        draft,
        normal,
        terminal,
        instance: created.workflow_instance_id,
        initial_context: created.current_context_revision_id,
        initial_visit: created.current_node_visit_id,
    }
}

pub(crate) fn rebuild_command(fixture: &RecoveryFixture) -> RebuildProjectionCommand {
    RebuildProjectionCommand {
        principal_id: PrincipalId::from_uuid(fixture.admin),
        idempotency_key: Uuid::new_v4().to_string(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(fixture.instance),
        expected_before_snapshot_digest: None,
    }
}

pub(crate) fn override_command(
    fixture: &RecoveryFixture,
    operation: AdminEmergencyOperation,
    target: Uuid,
) -> AdminEmergencyOverrideCommand {
    AdminEmergencyOverrideCommand {
        principal_id: PrincipalId::from_uuid(fixture.admin),
        idempotency_key: Uuid::new_v4().to_string(),
        command_schema_version: "v1".to_string(),
        workflow_instance_id: WorkflowInstanceId::from_uuid(fixture.instance),
        expected_workflow_state_version: 1,
        operation,
        target_node_id: NodeId::from_uuid(target),
        reason: "operator-approved emergency recovery".to_string(),
        related_references: vec![AdminRelatedReference {
            resource_type: "INCIDENT".to_string(),
            resource_id: "INC-123".to_string(),
        }],
        expected_before_snapshot_digest: None,
    }
}

pub(crate) async fn count_instance_facts(pool: &PgPool, instance: Uuid) -> (i64, i64, i64, i64) {
    let contexts = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_context_revisions WHERE workflow_instance_id = $1",
    )
    .bind(instance)
    .fetch_one(pool)
    .await
    .unwrap();
    let visits = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_node_visits WHERE workflow_instance_id = $1",
    )
    .bind(instance)
    .fetch_one(pool)
    .await
    .unwrap();
    let submissions = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_submissions WHERE workflow_instance_id = $1",
    )
    .bind(instance)
    .fetch_one(pool)
    .await
    .unwrap();
    let events =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_events WHERE workflow_instance_id = $1")
            .bind(instance)
            .fetch_one(pool)
            .await
            .unwrap();
    (contexts, visits, submissions, events)
}

pub(crate) async fn run_override(
    pool: &PgPool,
    command: AdminEmergencyOverrideCommand,
) -> Result<
    AdminEmergencyOverrideResult,
    svc_workflow::domain::workflow_instance::recovery::RecoveryError,
> {
    svc_workflow::application::workflow_instance::admin_recovery::admin_emergency_override(
        pool, command,
    )
    .await
}

pub(crate) async fn run_rebuild(
    pool: &PgPool,
    command: RebuildProjectionCommand,
) -> Result<RebuildProjectionResult, svc_workflow::domain::workflow_instance::recovery::RecoveryError>
{
    svc_workflow::application::workflow_instance::admin_recovery::rebuild_projection(pool, command)
        .await
}
