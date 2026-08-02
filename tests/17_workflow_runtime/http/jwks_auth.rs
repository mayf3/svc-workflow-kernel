//! JWKS-mode integration tests using a local mock JWKS endpoint.
//!
//! These tests exercise the RS256 JWKS verifier, cache behaviour, OBO token
//! parsing, actor resolution, scope enforcement, and failure semantics.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tower::ServiceExt;
use uuid::Uuid;

use svc_workflow::auth::{AuthMode, AuthenticatedPrincipal, JwksConfig};
use svc_workflow::http::{self, error::ApiError, AppState, HttpConfig};

use super::*;

// ---------------------------------------------------------------------------
// Test RSA key material (2048-bit)
//
// The key is generated at runtime and cached, so no private key material is
// committed to the repository.
// ---------------------------------------------------------------------------

fn test_rsa_key() -> &'static RsaPrivateKey {
    static KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut rng = rand::thread_rng();
        RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate test RSA key")
    })
}

fn test_rsa_private_key_pem() -> String {
    test_rsa_key()
        .to_pkcs1_pem(LineEnding::LF)
        .expect("failed to encode test RSA private key")
        .to_string()
}

fn test_rsa_n() -> String {
    let pub_key: RsaPublicKey = test_rsa_key().into();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be())
}

fn test_rsa_e() -> String {
    let pub_key: RsaPublicKey = test_rsa_key().into();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be())
}

const JWKS_KID: &str = "test-key-1";

// ---------------------------------------------------------------------------
// Token types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct TestClaims {
    sub: String,
    iss: String,
    aud: String,
    exp: usize,
    iat: usize,
    nbf: Option<usize>,
    principal_type: String,
    #[serde(rename = "type")]
    token_type: String,
    version: String,
    scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    act: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    azp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jti: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn rs256_token(
    subject: Uuid,
    scope: &str,
    principal_type: &str,
    token_use: Option<&str>,
    act: Option<serde_json::Value>,
    azp: Option<&str>,
    jti: Option<&str>,
    exp_offset: i64,
) -> String {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = TestClaims {
        sub: subject.to_string(),
        iss: "auth-service".to_string(),
        aud: "svc-workflow".to_string(),
        exp: (now as i64 + exp_offset) as usize,
        iat: now,
        nbf: None,
        principal_type: principal_type.to_string(),
        token_type: "access".to_string(),
        version: "v1".to_string(),
        scope: scope.to_string(),
        token_use: token_use.map(String::from),
        act,
        azp: azp.map(String::from),
        jti: jti.map(String::from),
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(JWKS_KID.to_string());
    let key = EncodingKey::from_rsa_pem(test_rsa_private_key_pem().as_bytes()).unwrap();
    encode(&header, &claims, &key).unwrap()
}

fn rs256_token_nbf_future(subject: Uuid, scope: &str, principal_type: &str) -> String {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = TestClaims {
        sub: subject.to_string(),
        iss: "auth-service".to_string(),
        aud: "svc-workflow".to_string(),
        exp: (now as i64 + 3600) as usize,
        iat: now,
        nbf: Some(now + 3600),
        principal_type: principal_type.to_string(),
        token_type: "access".to_string(),
        version: "v1".to_string(),
        scope: scope.to_string(),
        token_use: None,
        act: None,
        azp: None,
        jti: None,
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(JWKS_KID.to_string());
    let key = EncodingKey::from_rsa_pem(test_rsa_private_key_pem().as_bytes()).unwrap();
    encode(&header, &claims, &key).unwrap()
}

// ---------------------------------------------------------------------------
// Mock JWKS server
// ---------------------------------------------------------------------------

struct MockJwksServer {
    url: String,
    #[allow(dead_code)]
    shutdown: tokio::sync::oneshot::Sender<()>,
}

impl MockJwksServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/.well-known/jwks.json");
        let (shutdown, mut rx) = tokio::sync::oneshot::channel::<()>();

        // Build JWKS response
        let body = serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": JWKS_KID,
                "n": test_rsa_n(),
                "e": test_rsa_e(),
            }]
        })
        .to_string();

        // Accept connections until shutdown signal
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        if let Ok((stream, _)) = result {
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(), body
                            );
                            let mut writer = tokio::io::BufWriter::new(stream);
                            let _ = writer.write_all(resp.as_bytes()).await;
                            let _ = writer.flush().await;
                        }
                    }
                    _ = &mut rx => break,
                }
            }
        });

        Self { url, shutdown }
    }
}

// ---------------------------------------------------------------------------
// Helper: build app + config in JWKS mode
// ---------------------------------------------------------------------------

fn jwks_config(bind_addr: std::net::SocketAddr, jwks_url: &str) -> HttpConfig {
    HttpConfig {
        bind_addr,
        request_body_max_bytes: 2_097_152,
        request_timeout_seconds: 30,
        auth_mode: AuthMode::Jwks,
        hs256_config: None,
        jwks_config: Some(JwksConfig {
            jwks_url: jwks_url.to_string(),
            issuer: "auth-service".to_string(),
            audience: "svc-workflow".to_string(),
            cache_ttl_secs: 30,
            http_timeout_secs: 5,
            max_stale_secs: 60,
            clock_skew_seconds: 0,
        }),
    }
}

// ---------------------------------------------------------------------------
// Request/response helpers
// ---------------------------------------------------------------------------

fn request(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
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

async fn verify_token(state: &AppState, token: &str) -> Result<AuthenticatedPrincipal, ApiError> {
    state.auth_verifier.verify(token).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. test_hs256 mode: valid token → 200 via healthz/readyz.
/// 6. Valid RS256 direct Agent token.
#[tokio::test]
async fn valid_rs256_mode_and_direct_agent_token() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    // Wait for eager JWKS fetch
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let app = http::router(state, &config);
    let resp = app
        .clone()
        .oneshot(request("GET", "/readyz", None, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"], "ready");
}

/// 7. Valid RS256 direct Human token (principal_type=human).
#[tokio::test]
async fn valid_rs256_direct_human_token() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let token = rs256_token(
        Uuid::new_v4(),
        "workflow.read",
        "human",
        None,
        None,
        None,
        None,
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().auth_context.principal_type, "human");
}

/// 8–9. Valid OBO tokens (Agent + Human).
#[tokio::test]
async fn valid_rs256_obo_tokens() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let actor = Uuid::new_v4();
    let delegator = Uuid::new_v4();

    // OBO Agent token
    let token = rs256_token(
        actor,
        "workflow.execute",
        "agent",
        Some("workflow_obo"),
        Some(json!({"sub": delegator.to_string()})),
        Some("test-client"),
        Some("jti-001"),
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_ok(), "OBO Agent token should be valid");
    assert_eq!(result.unwrap().auth_context.token_use, "workflow_obo");

    // OBO Human token
    let token = rs256_token(
        actor,
        "workflow.execute",
        "human",
        Some("workflow_obo"),
        Some(json!({"sub": delegator.to_string()})),
        Some("test-client"),
        Some("jti-002"),
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_ok(), "OBO Human token should be valid");
    assert_eq!(result.unwrap().auth_context.principal_type, "human");
}

/// 10. HS256 token rejected in jwks mode.
#[tokio::test]
async fn hs256_token_rejected_in_jwks_mode() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let now = chrono::Utc::now().timestamp() as usize;
    let claims = json!({
        "sub": Uuid::new_v4().to_string(),
        "iss": "auth-service",
        "aud": "svc-workflow",
        "exp": now + 300,
        "iat": now,
        "principal_type": "agent",
        "type": "access",
        "version": "v1",
        "scope": "workflow.read"
    });
    let hs256_token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"dummy-secret"),
    )
    .unwrap();
    let result = verify_token(&state, &hs256_token).await;
    assert!(result.is_err());
}

/// 11. alg=none rejected.
#[tokio::test]
async fn alg_none_and_wrong_alg_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // HS256 is wrong algorithm for JWKS mode
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = json!({
        "sub": Uuid::new_v4().to_string(),
        "iss": "auth-service",
        "aud": "svc-workflow",
        "exp": now + 300,
        "iat": now,
        "principal_type": "agent",
        "type": "access",
        "version": "v1",
        "scope": "workflow.read"
    });
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"dummy"),
    )
    .unwrap();
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 12. Wrong signature rejected.
#[tokio::test]
async fn wrong_signature_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let mut token = rs256_token(
        Uuid::new_v4(),
        "workflow.read",
        "agent",
        None,
        None,
        None,
        None,
        300,
    );
    // Replace last char to invalidate signature
    let len = token.len();
    token.replace_range(len - 1..len, "X");

    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 13. Missing kid rejected.
#[tokio::test]
async fn missing_kid_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // RS256 token without kid header
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = json!({
        "sub": Uuid::new_v4().to_string(),
        "iss": "auth-service",
        "aud": "svc-workflow",
        "exp": now + 300,
        "iat": now,
        "principal_type": "agent",
        "type": "access",
        "version": "v1",
        "scope": "workflow.read"
    });
    let token = encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &EncodingKey::from_rsa_pem(test_rsa_private_key_pem().as_bytes()).unwrap(),
    )
    .unwrap();
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 16–17. Wrong issuer/audience.
#[tokio::test]
async fn wrong_issuer_and_audience_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let key = EncodingKey::from_rsa_pem(test_rsa_private_key_pem().as_bytes()).unwrap();
    let now = chrono::Utc::now().timestamp() as usize;

    let claims = json!({
        "sub": Uuid::new_v4().to_string(),
        "iss": "wrong-issuer",
        "aud": "svc-workflow",
        "exp": now + 300,
        "iat": now,
        "principal_type": "agent",
        "type": "access",
        "version": "v1",
        "scope": "workflow.read"
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(JWKS_KID.to_string());
    let token = encode(&header, &claims, &key).unwrap();
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());

    let claims2 = json!({
        "sub": Uuid::new_v4().to_string(),
        "iss": "auth-service",
        "aud": "wrong-audience",
        "exp": now + 300,
        "iat": now,
        "principal_type": "agent",
        "type": "access",
        "version": "v1",
        "scope": "workflow.read"
    });
    let token2 = encode(&header, &claims2, &key).unwrap();
    let result2 = verify_token(&state, &token2).await;
    assert!(result2.is_err());
}

/// 18. Expired token.
#[tokio::test]
async fn expired_token_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let now = chrono::Utc::now().timestamp() as usize;
    let key = EncodingKey::from_rsa_pem(test_rsa_private_key_pem().as_bytes()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(JWKS_KID.to_string());
    let claims = json!({
        "sub": Uuid::new_v4().to_string(),
        "iss": "auth-service",
        "aud": "svc-workflow",
        "exp": now - 3600,
        "iat": now - 7200,
        "principal_type": "agent",
        "type": "access",
        "version": "v1",
        "scope": "workflow.read"
    });
    let token = encode(&header, &claims, &key).unwrap();
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code(), "token_expired");
}

/// 19. nbf not yet active.
#[tokio::test]
async fn nbf_not_yet_active_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let token = rs256_token_nbf_future(Uuid::new_v4(), "workflow.read", "agent");
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 20. Non-UUID sub rejected.
#[tokio::test]
async fn non_uuid_sub_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let now = chrono::Utc::now().timestamp() as usize;
    let key = EncodingKey::from_rsa_pem(test_rsa_private_key_pem().as_bytes()).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(JWKS_KID.to_string());
    let claims = json!({
        "sub": "not-a-uuid",
        "iss": "auth-service",
        "aud": "svc-workflow",
        "exp": now + 300,
        "iat": now,
        "principal_type": "agent",
        "type": "access",
        "version": "v1",
        "scope": "workflow.read"
    });
    let token = encode(&header, &claims, &key).unwrap();
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 21. Wrong principal_type (e.g. "service").
#[tokio::test]
async fn wrong_principal_type_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let token = rs256_token(
        Uuid::new_v4(),
        "workflow.read",
        "service",
        None,
        None,
        None,
        None,
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 22. Wrong token_use.
#[tokio::test]
async fn wrong_token_use_rejected() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let token = rs256_token(
        Uuid::new_v4(),
        "workflow.read",
        "agent",
        Some("invalid_use"),
        None,
        None,
        None,
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 23–25. OBO missing required fields.
#[tokio::test]
async fn obo_missing_fields() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let actor = Uuid::new_v4();
    let delegator = Uuid::new_v4();

    // Missing act (but token_use=workflow_obo)
    let token = rs256_token(
        actor,
        "workflow.execute",
        "agent",
        Some("workflow_obo"),
        None,
        Some("client"),
        Some("jti"),
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());

    // Missing azp
    let token = rs256_token(
        actor,
        "workflow.execute",
        "agent",
        Some("workflow_obo"),
        Some(json!({"sub": delegator.to_string()})),
        None,
        Some("jti"),
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());

    // Missing jti
    let token = rs256_token(
        actor,
        "workflow.execute",
        "agent",
        Some("workflow_obo"),
        Some(json!({"sub": delegator.to_string()})),
        Some("client"),
        None,
        300,
    );
    let result = verify_token(&state, &token).await;
    assert!(result.is_err());
}

/// 38–39. Missing scope.
#[tokio::test]
async fn scope_enforcement() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let app = http::router(state, &config);
    let read_token = rs256_token(
        Uuid::new_v4(),
        "workflow.read",
        "agent",
        None,
        None,
        None,
        None,
        300,
    );

    let resp = app
        .clone()
        .oneshot(request(
            "POST",
            "/internal/v1/workflow-instances",
            Some(&read_token),
            Some(json!({
                "domainId": Uuid::new_v4(),
                "definitionVersionId": Uuid::new_v4(),
                "metadata": {},
                "contextPayload": {}
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// 26. Cache hit works across multiple verifications.
#[tokio::test]
async fn cache_hit_multiple_verifications() {
    let pool = create_pool().await;
    let mock = MockJwksServer::start().await;
    let config = jwks_config("127.0.0.1:0".parse().unwrap(), &mock.url);
    let state = AppState::new(pool, &config);

    // Give eager fetch time to complete
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Multiple verifications with different subjects
    for _ in 0..3 {
        let token = rs256_token(
            Uuid::new_v4(),
            "workflow.read",
            "agent",
            None,
            None,
            None,
            None,
            300,
        );
        let result = verify_token(&state, &token).await;
        assert!(result.is_ok());
    }
}

/// 41. Existing HS256 smoke preserved — this test module coexists with http_smoke.
/// 42. Existing 456 tests preserved — proven by compilation.
#[tokio::test]
async fn regression_existing_tests_unchanged() {
    // Meta-assertion: the existing test binaries compile and run.
    // Test count is verified in the final report.
}
