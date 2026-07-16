use super::*;

use sqlx::Connection;
use svc_workflow::application::workflow_instance::query_service::WorkflowQueryService;

const QUERY_TEST_DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

pub(crate) struct QueryAuditTriggerGuard {
    function_name: String,
    trigger_name: String,
}

impl QueryAuditTriggerGuard {
    pub(crate) async fn install(pool: &PgPool, principal_id: Uuid) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let function_name = format!("query_audit_fail_{suffix}");
        let trigger_name = format!("query_audit_fail_trg_{suffix}");
        sqlx::query(&format!(
            "CREATE FUNCTION {function_name}() RETURNS trigger AS $$ BEGIN
               IF NEW.principal_id = '{principal_id}'::uuid THEN
                 RAISE EXCEPTION 'forced query audit failure';
               END IF;
               RETURN NEW;
             END; $$ LANGUAGE plpgsql"
        ))
        .execute(pool)
        .await
        .expect("create query audit failure function");
        sqlx::query(&format!(
            "CREATE TRIGGER {trigger_name} BEFORE INSERT ON workflow_security_audits
             FOR EACH ROW EXECUTE FUNCTION {function_name}()"
        ))
        .execute(pool)
        .await
        .expect("create query audit failure trigger");
        Self {
            function_name,
            trigger_name,
        }
    }
}

impl Drop for QueryAuditTriggerGuard {
    fn drop(&mut self) {
        let function_name = self.function_name.clone();
        let trigger_name = self.trigger_name.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build query audit cleanup runtime");
            runtime.block_on(async move {
                let Ok(mut connection) = sqlx::PgConnection::connect(QUERY_TEST_DATABASE_URL).await
                else {
                    return;
                };
                let _ = sqlx::query(&format!(
                    "DROP TRIGGER IF EXISTS {trigger_name} ON workflow_security_audits"
                ))
                .execute(&mut connection)
                .await;
                let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS {function_name}()"))
                    .execute(&mut connection)
                    .await;
            });
        })
        .join()
        .ok();
    }
}

pub(crate) struct QueryFixture {
    pub owner: Uuid,
    pub creator: Uuid,
    pub assignee: Uuid,
    pub outsider: Uuid,
    pub domain: Uuid,
    pub version: Uuid,
    pub draft: Uuid,
    pub normal: Uuid,
    pub terminal: Uuid,
    pub draft_advance: Uuid,
    pub extra_advance: Uuid,
    pub normal_advance: Uuid,
    pub return_transition: Uuid,
    pub terminate_transition: Uuid,
}

pub(crate) struct CompletedFixture {
    pub seed: QueryFixture,
    pub instance: Uuid,
    pub creator_submission: Uuid,
    pub feedback_submission: Uuid,
}

pub(crate) async fn seed_query_fixture(pool: &PgPool) -> QueryFixture {
    let (owner, domain) = seed_principal_domain_with_owner(pool).await;
    let creator = seed_second_principal(pool).await;
    let assignee = seed_second_principal(pool).await;
    let outsider = seed_second_principal(pool).await;
    sqlx::query(
        "INSERT INTO domain_role_bindings
         (binding_id, domain_id, principal_id, role_key, enabled)
         VALUES ($1, $2, $3, 'MEMBER', TRUE)",
    )
    .bind(Uuid::new_v4())
    .bind(domain)
    .bind(creator)
    .execute(pool)
    .await
    .unwrap();

    let definition = Uuid::new_v4();
    let version = Uuid::new_v4();
    let key = format!("query-{}", &Uuid::new_v4().to_string()[..8]);
    sqlx::query(
        "INSERT INTO workflow_definitions
         (workflow_definition_id, domain_id, definition_key, display_name)
         VALUES ($1, $2, $3, 'Query Definition')",
    )
    .bind(definition)
    .bind(domain)
    .bind(key)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_definition_versions
         (definition_version_id, workflow_definition_id, version_number,
          version_status, context_schema)
         VALUES ($1, $2, 1, 'DRAFT', '{\"type\":\"object\"}'::jsonb)",
    )
    .bind(version)
    .bind(definition)
    .execute(pool)
    .await
    .unwrap();

    let draft = Uuid::new_v4();
    let normal = Uuid::new_v4();
    let terminal = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type, instructions)
         VALUES ($1, $2, 'draft', 'Draft', 0, 'DRAFT', 'WORKFLOW_CREATOR',
                 'Draft instructions')",
    )
    .bind(draft)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type, fixed_principal_id, instructions)
         VALUES ($1, $2, 'review', 'Review', 1, 'NORMAL', 'FIXED_PRINCIPAL', $3,
                 'Review instructions')",
    )
    .bind(normal)
    .bind(version)
    .bind(assignee)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow_node_definitions
         (node_id, definition_version_id, node_key, display_name, order_index,
          node_type, assignee_ref_type)
         VALUES ($1, $2, 'done', 'Done', 2, 'TERMINAL', NULL)",
    )
    .bind(terminal)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();

    let draft_advance = Uuid::new_v4();
    let extra_advance = Uuid::new_v4();
    let normal_advance = Uuid::new_v4();
    let return_transition = Uuid::new_v4();
    let terminate_transition = Uuid::new_v4();
    for (id, key, name, source, target, effect, schema) in [
        (
            draft_advance,
            "advance-review",
            "Advance",
            draft,
            normal,
            "ADVANCE",
            serde_json::json!({"type": "object"}),
        ),
        (
            normal_advance,
            "advance-done",
            "Complete",
            normal,
            terminal,
            "ADVANCE",
            serde_json::json!({"type": "object"}),
        ),
        (
            extra_advance,
            "advance-extra",
            "Extra Advance",
            draft,
            terminal,
            "ADVANCE",
            serde_json::json!({"type": "object"}),
        ),
        (
            return_transition,
            "return-draft",
            "Return",
            normal,
            draft,
            "RETURN",
            serde_json::json!({
                "type": "object",
                "required": ["reasonCode", "reason"],
                "properties": {
                    "reasonCode": {"type": "string"},
                    "reason": {"type": "string"},
                    "rootCauseNodeVisitId": {"type": "string"},
                    "relatedSubmissionIds": {"type": "array", "items": {"type": "string"}}
                }
            }),
        ),
        (
            terminate_transition,
            "terminate",
            "Terminate",
            normal,
            terminal,
            "TERMINATE",
            serde_json::json!({
                "type": "object",
                "required": ["reasonCode", "reason"],
                "properties": {
                    "reasonCode": {"type": "string"},
                    "reason": {"type": "string"}
                }
            }),
        ),
    ] {
        sqlx::query(
            "INSERT INTO workflow_transition_definitions
             (transition_id, definition_version_id, transition_key, display_name,
              source_node_id, target_node_id, transition_effect, submission_schema)
             VALUES ($1, $2, $3, $4, $5, $6, $7::transition_effect, $8)",
        )
        .bind(id)
        .bind(version)
        .bind(key)
        .bind(name)
        .bind(source)
        .bind(target)
        .bind(effect)
        .bind(schema)
        .execute(pool)
        .await
        .unwrap();
    }
    for (node, transition) in [(draft, draft_advance), (normal, normal_advance)] {
        sqlx::query(
            "UPDATE workflow_node_definitions SET primary_advance_transition_id = $1
             WHERE node_id = $2",
        )
        .bind(transition)
        .bind(node)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE workflow_definition_versions SET version_status = 'PUBLISHED'
         WHERE definition_version_id = $1",
    )
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
    QueryFixture {
        owner,
        creator,
        assignee,
        outsider,
        domain,
        version,
        draft,
        normal,
        terminal,
        draft_advance,
        extra_advance,
        normal_advance,
        return_transition,
        terminate_transition,
    }
}

pub(crate) async fn create_query_instance(
    pool: &PgPool,
    seed: &QueryFixture,
) -> CreateWorkflowInstanceResult {
    let mut command = make_command(seed.creator, seed.domain, seed.version);
    command.external_reference = Some("QUERY-42".to_string());
    command.external_url = Some("https://example.test/work/42".to_string());
    command.metadata = serde_json::json!({"source": "query-test"});
    command.context_payload = serde_json::json!({"title": "initial"});
    create_workflow_instance(pool, command).await.unwrap()
}

pub(crate) async fn complete_query_instance(pool: &PgPool) -> CompletedFixture {
    let seed = seed_query_fixture(pool).await;
    let created = create_query_instance(pool, &seed).await;
    let first = execute_workflow_transition(
        pool,
        make_transition_command(
            seed.creator,
            created.workflow_instance_id,
            1,
            seed.draft_advance,
            Some(serde_json::json!({"work": "creator-one"})),
        ),
    )
    .await
    .unwrap();
    let creator_submission = first.submission_id.unwrap();
    let feedback = execute_workflow_transition(
        pool,
        make_transition_command(
            seed.assignee,
            created.workflow_instance_id,
            2,
            seed.return_transition,
            Some(serde_json::json!({
                "reasonCode": "FIX",
                "reason": "please revise",
                "rootCauseNodeVisitId": first.current_node_visit_id.to_string(),
                "relatedSubmissionIds": [creator_submission.to_string()]
            })),
        ),
    )
    .await
    .unwrap();
    let feedback_submission = feedback.submission_id.unwrap();
    execute_workflow_transition(
        pool,
        make_transition_command(
            seed.creator,
            created.workflow_instance_id,
            3,
            seed.draft_advance,
            Some(serde_json::json!({"work": "creator-two"})),
        ),
    )
    .await
    .unwrap();
    execute_workflow_transition(
        pool,
        make_transition_command(
            seed.assignee,
            created.workflow_instance_id,
            4,
            seed.terminate_transition,
            Some(serde_json::json!({"reasonCode": "STOP", "reason": "done"})),
        ),
    )
    .await
    .unwrap();
    CompletedFixture {
        seed,
        instance: created.workflow_instance_id,
        creator_submission,
        feedback_submission,
    }
}

pub(crate) fn query_service(pool: &PgPool) -> WorkflowQueryService {
    WorkflowQueryService::new(pool.clone())
}
