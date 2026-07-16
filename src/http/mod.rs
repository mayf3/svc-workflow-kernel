//! Axum adapter for the internal workflow API.

pub mod dto;
pub mod error;
mod handlers;
mod state;

use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderName, StatusCode};
use axum::routing::{get, post};
use axum::{BoxError, Router};
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

pub use state::{AppState, HttpConfig};

pub const API_CONTRACT_VERSION: &str = "internal-v0";
pub const SERVICE_VERSION: &str = "0.3.1";
pub const SCHEMA_VERSION: &str = "0010";
pub const EXPECTED_MIGRATION_VERSION: i64 = 10;

pub fn router(state: AppState, config: &HttpConfig) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/readyz", get(handlers::health::readyz))
        .route("/version", get(handlers::health::version))
        .route(
            "/internal/v1/workflow-instances",
            post(handlers::instances::create),
        )
        .route(
            "/internal/v1/workflow-instances/{workflowInstanceId}",
            get(handlers::instances::detail),
        )
        .route(
            "/internal/v1/workflow-instances/{workflowInstanceId}/transitions",
            post(handlers::transitions::execute),
        )
        .route(
            "/internal/v1/workflow-instances/{workflowInstanceId}/timeline",
            get(handlers::timeline::list),
        )
        .fallback(|| async {
            error::ApiError::new(StatusCode::NOT_FOUND, "route_not_found", "route not found")
        })
        .method_not_allowed_fallback(|| async {
            error::ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "method not allowed",
            )
        })
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_service_error))
                .timeout(Duration::from_secs(config.request_timeout_seconds)),
        )
        .layer(DefaultBodyLimit::max(config.request_body_max_bytes))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn handle_service_error(error: BoxError) -> error::ApiError {
    if error.is::<tower::timeout::error::Elapsed>() {
        error::ApiError::new(
            StatusCode::REQUEST_TIMEOUT,
            "request_timeout",
            "request timed out",
        )
    } else {
        tracing::error!(error = %error, "unhandled HTTP service error");
        error::ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal service error",
        )
    }
}
