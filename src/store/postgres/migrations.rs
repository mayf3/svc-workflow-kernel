//! Programmatic SQL migration runner.
//!
//! Migrations are stored as `.sql` files in the `migrations/` directory
//! and filenames follow the convention `<sequence>_<name>.sql`.

use sqlx::PgPool;

/// Run all pending migrations from the `migrations/` directory.
///
/// Uses sqlx's built-in migrator with raw SQL files.
///
/// # Panics
///
/// Panics if the migrations directory cannot be found or a migration fails.
pub async fn run(pool: &PgPool) {
    let migrator = sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
        .await
        .expect("failed to load migrations from migrations/ directory");

    migrator
        .run(pool)
        .await
        .expect("failed to run database migrations");
}
