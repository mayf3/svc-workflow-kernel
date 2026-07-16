use reqwest::{Client, Response};
use serde_json::{json, Value};

use svc_workflow::application::workflow_instance::idempotency::{
    compute_request_hash, compute_transition_request_hash,
};

use super::database::TemporaryDatabase;
use super::server::{token, RunningServer};
use super::*;

struct Scenario {
    pool: PgPool,
    base_url: String,
    principal_id: Uuid,
    domain_id: Uuid,
    version_id: Uuid,
    draft_transition_id: Uuid,
    normal_transition_id: Uuid,
    outsider_id: Uuid,
}

struct ScenarioDatabase {
    pool: PgPool,
}

struct ScenarioServer {
    base_url: String,
}

async fn json_response(response: Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let body = response.json().await.expect("JSON response envelope");
    (status, body)
}

#[tokio::test]
async fn real_tcp_isolated_database_internal_api_contract() {
    let database = TemporaryDatabase::create().await;
    let setup = tokio::spawn(setup_scenario(database.pool.clone())).await;
    let (server, scenario) = match setup {
        Ok(value) => value,
        Err(error) => {
            database.cleanup().await;
            TemporaryDatabase::assert_no_residue().await;
            propagate_task_failure(error)
        }
    };
    let outcome = tokio::spawn(run_scenario(scenario)).await;
    let server_result = server.stop().await;
    database.cleanup().await;
    TemporaryDatabase::assert_no_residue().await;
    server_result.expect("E2E server shutdown");
    if let Err(error) = outcome {
        propagate_task_failure(error);
    }
}

fn propagate_task_failure(error: tokio::task::JoinError) -> ! {
    if error.is_panic() {
        std::panic::resume_unwind(error.into_panic());
    }
    panic!("E2E task was cancelled: {error}");
}

async fn setup_scenario(pool: PgPool) -> (RunningServer, Scenario) {
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, version_id, _) = seed_published_definition_normal_node(&pool, domain_id).await;
    let draft_transition_id: Uuid = sqlx::query_scalar(
        "SELECT transition_id FROM workflow_transition_definitions \
         WHERE definition_version_id = $1 AND transition_key = 'advance-draft'",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("draft transition");
    let normal_transition_id: Uuid = sqlx::query_scalar(
        "SELECT transition_id FROM workflow_transition_definitions \
         WHERE definition_version_id = $1 AND transition_key = 'advance-review'",
    )
    .bind(version_id)
    .fetch_one(&pool)
    .await
    .expect("normal transition");
    let outsider_id = seed_second_principal(&pool).await;
    let server = RunningServer::start(pool.clone(), 2_097_152).await;
    let scenario = Scenario {
        pool,
        base_url: server.base_url.clone(),
        principal_id,
        domain_id,
        version_id,
        draft_transition_id,
        normal_transition_id,
        outsider_id,
    };
    (server, scenario)
}

async fn run_scenario(scenario: Scenario) {
    let Scenario {
        pool,
        base_url,
        principal_id,
        domain_id,
        version_id,
        draft_transition_id,
        normal_transition_id,
        outsider_id,
    } = scenario;
    let database = ScenarioDatabase { pool };
    let server = ScenarioServer { base_url };
    let client = Client::new();
    let actor_token = token(principal_id, "workflow.execute workflow.read");
    let outsider_token = token(outsider_id, "workflow.read");

    let (ready_status, ready_body) = json_response(
        client
            .get(format!("{}/readyz", server.base_url))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (ready_status, ready_body["status"].as_str()),
        (200, Some("ready"))
    );

    let create_key = format!("tcp-create-{}", Uuid::new_v4());
    let external_reference = format!("tcp-e2e-{}", Uuid::new_v4());
    let metadata = json!({"source": "real-tcp"});
    let context = json!({"title": "isolated"});
    let create_body = json!({
        "domainId": domain_id,
        "definitionVersionId": version_id,
        "externalReference": external_reference,
        "metadata": metadata,
        "contextPayload": context
    });
    let (create_status, created) = json_response(
        client
            .post(format!(
                "{}/internal/v1/workflow-instances",
                server.base_url
            ))
            .bearer_auth(&actor_token)
            .header("idempotency-key", &create_key)
            .json(&create_body)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(create_status, 201);
    let instance_id = Uuid::parse_str(created["workflowInstanceId"].as_str().unwrap()).unwrap();

    let expected_hash = compute_request_hash(
        "v1",
        &create_key,
        &PrincipalId::from_uuid(principal_id),
        &DomainId::from_uuid(domain_id),
        &DefinitionVersionId::from_uuid(version_id),
        &context,
        &metadata,
        &Some(external_reference),
        &None,
    )
    .unwrap();
    let stored_hash: String = sqlx::query_scalar(
        "SELECT request_hash FROM workflow_command_receipts \
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(&create_key)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        stored_hash, expected_hash,
        "HTTP DTO must preserve requestHash inputs"
    );

    let detail_url = format!(
        "{}/internal/v1/workflow-instances/{instance_id}",
        server.base_url
    );
    let (detail_status, detail) = json_response(
        client
            .get(&detail_url)
            .bearer_auth(&actor_token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(detail_status, 200);
    assert_eq!(detail["visibility"], "full");

    let transition_url = format!("{detail_url}/transitions");
    let transition_key = format!("tcp-transition-{}", Uuid::new_v4());
    let transition_body = json!({
        "transitionDefinitionId": draft_transition_id,
        "expectedWorkflowStateVersion": 1,
        "submissionPayload": {"evidence": "tcp"}
    });
    let send_transition = || {
        client
            .post(&transition_url)
            .bearer_auth(&actor_token)
            .header("idempotency-key", &transition_key)
            .json(&transition_body)
            .send()
    };
    let first_transition = json_response(send_transition().await.unwrap()).await;
    let replayed_transition = json_response(send_transition().await.unwrap()).await;
    assert_eq!(first_transition.0, 200);
    assert_eq!(replayed_transition, first_transition);
    let transition_hash = compute_transition_request_hash(
        "v1",
        &transition_key,
        &PrincipalId::from_uuid(principal_id),
        &WorkflowInstanceId::from_uuid(instance_id),
        1,
        &TransitionId::from_uuid(draft_transition_id),
        &Some(json!({"evidence": "tcp"})),
    )
    .unwrap();
    let stored_transition_hash: String = sqlx::query_scalar(
        "SELECT request_hash FROM workflow_command_receipts \
         WHERE principal_id = $1 AND idempotency_key = $2",
    )
    .bind(principal_id)
    .bind(&transition_key)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(stored_transition_hash, transition_hash);

    let (conflict_status, conflict) = json_response(
        client
            .post(&transition_url)
            .bearer_auth(&actor_token)
            .header("idempotency-key", &transition_key)
            .json(&json!({
                "transitionDefinitionId": draft_transition_id,
                "expectedWorkflowStateVersion": 2,
                "submissionPayload": {"evidence": "different"}
            }))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(conflict_status, 409);
    assert_eq!(conflict["error"]["code"], "idempotency_conflict");
    assert!(conflict["error"].get("details").is_none());

    let (timeline_status, timeline) = json_response(
        client
            .get(format!("{detail_url}/timeline"))
            .bearer_auth(&actor_token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(timeline_status, 200);
    assert_eq!(timeline["items"].as_array().unwrap().len(), 2);

    let metadata_key = format!("metadata-limit-{}", Uuid::new_v4());
    let metadata_body = json!({
        "domainId": domain_id, "definitionVersionId": version_id,
        "metadata": {"data": "m".repeat(64 * 1024 + 1)}, "contextPayload": {}
    });
    let send_metadata_failure = || {
        client
            .post(format!(
                "{}/internal/v1/workflow-instances",
                server.base_url
            ))
            .bearer_auth(&actor_token)
            .header("idempotency-key", &metadata_key)
            .json(&metadata_body)
            .send()
    };
    let first_metadata_failure = json_response(send_metadata_failure().await.unwrap()).await;
    let replayed_metadata_failure = json_response(send_metadata_failure().await.unwrap()).await;
    assert_eq!(first_metadata_failure.0, 413);
    assert_eq!(replayed_metadata_failure, first_metadata_failure);
    assert_eq!(
        first_metadata_failure.1["error"]["details"]["field"],
        "metadata"
    );

    let submission_key = format!("submission-limit-{}", Uuid::new_v4());
    let submission_body = json!({
        "transitionDefinitionId": normal_transition_id,
        "expectedWorkflowStateVersion": 2,
        "submissionPayload": {"data": "s".repeat(1024 * 1024 + 1)}
    });
    let send_submission_failure = || {
        client
            .post(&transition_url)
            .bearer_auth(&actor_token)
            .header("idempotency-key", &submission_key)
            .json(&submission_body)
            .send()
    };
    let first_submission_failure = json_response(send_submission_failure().await.unwrap()).await;
    let replayed_submission_failure = json_response(send_submission_failure().await.unwrap()).await;
    assert_eq!(first_submission_failure.0, 413);
    assert_eq!(replayed_submission_failure, first_submission_failure);
    assert_eq!(
        first_submission_failure.1["error"]["details"]["field"],
        "submissionPayload"
    );

    let not_visible = json_response(
        client
            .get(&detail_url)
            .bearer_auth(&outsider_token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let not_found = json_response(
        client
            .get(format!(
                "{}/internal/v1/workflow-instances/{}",
                server.base_url,
                Uuid::new_v4()
            ))
            .bearer_auth(&outsider_token)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(not_visible, not_found);
    assert_eq!(not_visible.0, 404);

    let (reference_status, reference_error) = json_response(
        client
            .post(format!(
                "{}/internal/v1/workflow-instances",
                server.base_url
            ))
            .bearer_auth(&actor_token)
            .header("idempotency-key", format!("long-ref-{}", Uuid::new_v4()))
            .json(&json!({
                "domainId": domain_id, "definitionVersionId": version_id,
                "externalReference": "r".repeat(513), "metadata": {}, "contextPayload": {}
            }))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(reference_status, 422);
    assert_eq!(reference_error["error"]["code"], "invalid_input");

    let oversized = format!(
        "{{\"domainId\":\"{domain_id}\",\"definitionVersionId\":\"{version_id}\",\"metadata\":{{\"data\":\"{}\"}},\"contextPayload\":{{}}}}",
        "x".repeat(2_100_000)
    );
    let (size_status, size_error) = json_response(
        client
            .post(format!(
                "{}/internal/v1/workflow-instances",
                server.base_url
            ))
            .bearer_auth(&actor_token)
            .header("idempotency-key", format!("oversized-{}", Uuid::new_v4()))
            .header("content-type", "application/json")
            .body(oversized)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(size_status, 413);
    assert_eq!(size_error["error"]["code"], "size_limit_exceeded");
    assert_eq!(size_error["error"]["details"]["field"], "request_body");

    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 10")
        .execute(&database.pool)
        .await
        .unwrap();
    let (mismatch_status, mismatch) = json_response(
        client
            .get(format!("{}/readyz", server.base_url))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(mismatch_status, 503);
    assert_eq!(mismatch["error"]["code"], "migration_version_mismatch");
}
