use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

use svc_workflow::auth::{AuthMode, Hs256Config};
use svc_workflow::http::{self, AppState, HttpConfig};

use super::*;

const JWT_SECRET: &str = "http-smoke-secret-at-least-32-bytes";

#[derive(Serialize)]
struct Claims {
    sub: String,
    iss: &'static str,
    aud: &'static str,
    exp: usize,
    iat: usize,
    principal_type: &'static str,
    #[serde(rename = "type")]
    token_type: &'static str,
    version: &'static str,
    scope: String,
}

fn token(principal_id: Uuid, scope: &str) -> String {
    let now = chrono::Utc::now().timestamp() as usize;
    encode(
        &Header::new(Algorithm::HS256),
        &Claims {
            sub: principal_id.to_string(),
            iss: "auth-service",
            aud: "svc-workflow",
            exp: now + 300,
            iat: now,
            principal_type: "agent",
            token_type: "access",
            version: "v1",
            scope: scope.to_string(),
        },
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

fn app(pool: sqlx::PgPool) -> axum::Router {
    let hs256 = Hs256Config {
        secret: JWT_SECRET.to_string(),
        issuer: "auth-service".to_string(),
        audience: "svc-workflow".to_string(),
        clock_skew_seconds: 0,
    };
    let config = HttpConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        request_body_max_bytes: 2_097_152,
        request_timeout_seconds: 30,
        auth_mode: AuthMode::TestHs256,
        hs256_config: Some(hs256),
        jwks_config: None,
    };
    http::router(AppState::new(pool, &config), &config)
}

fn request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn internal_api_create_detail_transition_timeline_and_security() {
    let pool = create_pool().await;
    let (principal_id, domain_id) = seed_principal_domain_with_owner(&pool).await;
    let (_, definition_version_id, _) =
        seed_published_definition_normal_node(&pool, domain_id).await;
    let transition_id: Uuid = sqlx::query_scalar(
        "SELECT t.transition_id
         FROM workflow_transition_definitions t
         JOIN workflow_node_definitions n ON n.node_id = t.source_node_id
         WHERE t.definition_version_id = $1 AND n.node_key = 'draft'",
    )
    .bind(definition_version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_principal = seed_second_principal(&pool).await;
    let app = app(pool);
    let full_token = token(principal_id, "workflow.execute workflow.read");

    let health = app
        .clone()
        .oneshot(request("GET", "/healthz", None, None, None))
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(json_body(health).await["status"], "ok");

    let ready = app
        .clone()
        .oneshot(request("GET", "/readyz", None, None, None))
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(json_body(ready).await["status"], "ready");

    let version = app
        .clone()
        .oneshot(request("GET", "/version", None, None, None))
        .await
        .unwrap();
    assert_eq!(version.status(), StatusCode::OK);
    let version_body = json_body(version).await;
    assert_eq!(version_body["service"], "svc-workflow");
    assert_eq!(version_body["schemaVersion"], "0010");
    assert_eq!(version_body["apiContractVersion"], "internal-v0");

    let no_auth = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/internal/v1/workflow-instances/{principal_id}"),
            None,
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(no_auth.status(), StatusCode::UNAUTHORIZED);

    let forged_actor = app
        .clone()
        .oneshot(request(
            "POST",
            "/internal/v1/workflow-instances",
            Some(&full_token),
            Some("actor-forgery"),
            Some(json!({
                "domainId": domain_id,
                "definitionVersionId": definition_version_id,
                "metadata": {},
                "contextPayload": {},
                "principalId": Uuid::new_v4()
            })),
        ))
        .await
        .unwrap();
    assert_eq!(forged_actor.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(forged_actor).await["error"]["code"],
        "unknown_field"
    );

    let create_body = json!({
        "domainId": domain_id,
        "definitionVersionId": definition_version_id,
        "externalReference": format!("http-smoke-{}", Uuid::new_v4()),
        "metadata": { "source": "http-smoke" },
        "contextPayload": { "title": "smoke" }
    });
    let create_request = || {
        request(
            "POST",
            "/internal/v1/workflow-instances",
            Some(&full_token),
            Some("create-http-smoke"),
            Some(create_body.clone()),
        )
    };
    let created = app.clone().oneshot(create_request()).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body = json_body(created).await;
    assert_eq!(created_body["workflowStateVersion"], 1);
    let workflow_instance_id = created_body["workflowInstanceId"].as_str().unwrap();

    let replayed = app.clone().oneshot(create_request()).await.unwrap();
    assert_eq!(replayed.status(), StatusCode::CREATED);
    assert_eq!(json_body(replayed).await, created_body);

    let conflicting = app
        .clone()
        .oneshot(request(
            "POST",
            "/internal/v1/workflow-instances",
            Some(&full_token),
            Some("create-http-smoke"),
            Some(json!({
                "domainId": domain_id,
                "definitionVersionId": definition_version_id,
                "metadata": {},
                "contextPayload": { "title": "different" }
            })),
        ))
        .await
        .unwrap();
    assert_eq!(conflicting.status(), StatusCode::CONFLICT);
    let conflict_body = json_body(conflicting).await;
    assert_eq!(conflict_body["error"]["code"], "idempotency_conflict");
    assert!(conflict_body["error"].get("details").is_none());

    let detail = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/internal/v1/workflow-instances/{workflow_instance_id}"),
            Some(&full_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json_body(detail).await["visibility"], "full");

    let read_only = token(principal_id, "workflow.read");
    let forbidden = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/internal/v1/workflow-instances/{workflow_instance_id}/transitions"),
            Some(&read_only),
            Some("forbidden-transition"),
            Some(json!({
                "transitionDefinitionId": transition_id,
                "expectedWorkflowStateVersion": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let other_token = token(other_principal, "workflow.execute");
    let not_assignee = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/internal/v1/workflow-instances/{workflow_instance_id}/transitions"),
            Some(&other_token),
            Some("not-assignee-transition"),
            Some(json!({
                "transitionDefinitionId": transition_id,
                "expectedWorkflowStateVersion": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(not_assignee.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(not_assignee).await["error"]["code"],
        "principal_not_assignee"
    );

    let transitioned = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/internal/v1/workflow-instances/{workflow_instance_id}/transitions"),
            Some(&full_token),
            Some("transition-http-smoke"),
            Some(json!({
                "transitionDefinitionId": transition_id,
                "expectedWorkflowStateVersion": 1
            })),
        ))
        .await
        .unwrap();
    assert_eq!(transitioned.status(), StatusCode::OK);
    assert_eq!(json_body(transitioned).await["workflowStateVersion"], 2);

    let timeline = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/internal/v1/workflow-instances/{workflow_instance_id}/timeline?limit=50"),
            Some(&full_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(timeline.status(), StatusCode::OK);
    let timeline_body = json_body(timeline).await;
    assert!(timeline_body["items"].as_array().unwrap().len() >= 2);

    let malformed_timeline = app
        .oneshot(request(
            "GET",
            &format!(
                "/internal/v1/workflow-instances/{workflow_instance_id}/timeline?limit=not-a-number"
            ),
            Some(&full_token),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        malformed_timeline.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        json_body(malformed_timeline).await["error"]["code"],
        "invalid_pagination"
    );
}
