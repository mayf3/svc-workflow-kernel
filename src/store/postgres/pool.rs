//! PostgreSQL connection pool initialization.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Default database URL if `DATABASE_URL` is not set.
const DEFAULT_DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/svc_workflow";

/// Create a new [`PgPool`] from the `DATABASE_URL` environment variable
/// or a sensible development default.
///
/// # Panics
///
/// Panics if the connection string is invalid or the pool cannot be created.
pub async fn create_pool() -> PgPool {
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());

    PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("failed to create PostgreSQL connection pool")
}

/// Create a [`PgPool`] from an explicit database URL (useful in tests).
#[allow(dead_code)]
pub async fn create_pool_from_url(database_url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .expect("failed to create PostgreSQL connection pool from URL")
}
