//! Liveness, readiness, and build metadata endpoints.

use axum::extract::State;
use axum::Json;

use crate::http::dto::{HealthResponse, VersionResponse};
use crate::http::error::ApiError;
use crate::http::{
    AppState, API_CONTRACT_VERSION, EXPECTED_MIGRATION_VERSION, SCHEMA_VERSION, SERVICE_VERSION,
};

pub(crate) async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub(crate) async fn readyz(
    State(state): State<AppState>,
) -> Result<Json<HealthResponse>, ApiError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "readiness database probe failed");
            ApiError::service_unavailable("service_unavailable", "database is not available")
        })?;
    let applied = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(migration_probe_error)?;
    let failed =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations WHERE NOT success")
            .fetch_one(&state.pool)
            .await
            .map_err(migration_probe_error)?;
    let expected = expected_migrations();
    if !migration_ledger_matches(&applied, failed) {
        return Err(ApiError::service_unavailable(
            "migration_version_mismatch",
            "database migration version does not match this build",
        )
        .with_details(serde_json::json!({
            "expected": expected,
            "actual": applied
        })));
    }

    // Check auth verifier readiness.
    if !state.auth_verifier.is_ready().await {
        return Err(ApiError::service_unavailable(
            "auth_verifier_unavailable",
            "authentication verifier is not ready",
        ));
    }

    Ok(Json(HealthResponse { status: "ready" }))
}

fn expected_migrations() -> Vec<i64> {
    (1..=EXPECTED_MIGRATION_VERSION).collect()
}

fn migration_ledger_matches(applied: &[i64], failed: i64) -> bool {
    applied == expected_migrations() && failed == 0
}

fn migration_probe_error(error: sqlx::Error) -> ApiError {
    tracing::error!(error = %error, "readiness migration probe failed");
    ApiError::service_unavailable(
        "migration_version_mismatch",
        "migration state cannot be verified",
    )
}

pub(crate) async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        service: "svc-workflow",
        version: SERVICE_VERSION,
        git_sha: option_env!("GIT_SHA").unwrap_or("unknown"),
        schema_version: SCHEMA_VERSION,
        api_contract_version: API_CONTRACT_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_the_complete_exact_ledger() {
        let expected = expected_migrations();
        assert!(migration_ledger_matches(&expected, 0));

        let mut missing_middle = expected.clone();
        missing_middle.remove(4);
        assert!(!migration_ledger_matches(&missing_middle, 0));

        let mut future = expected.clone();
        future.push(EXPECTED_MIGRATION_VERSION + 1);
        assert!(!migration_ledger_matches(&future, 0));
        assert!(!migration_ledger_matches(&expected, 1));
    }
}
