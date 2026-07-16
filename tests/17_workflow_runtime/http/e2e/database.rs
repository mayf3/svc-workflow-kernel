use sqlx::{Connection, Executor, PgConnection, PgPool};

const ADMIN_URL: &str = "postgres://postgres:postgres@localhost:5432/postgres";

pub(super) struct TemporaryDatabase {
    pub(super) pool: PgPool,
    name: String,
}

impl TemporaryDatabase {
    pub(super) async fn create() -> Self {
        let name = format!("svc_workflow_e2e_{}", uuid::Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(ADMIN_URL)
            .await
            .expect("connect to PostgreSQL administration database");
        admin
            .execute(format!("CREATE DATABASE {name}").as_str())
            .await
            .expect("create isolated E2E database");
        match Self::initialize(&name).await {
            Ok(pool) => Self { pool, name },
            Err(error) => {
                Self::drop_named(&name)
                    .await
                    .expect("drop E2E database after setup failure");
                panic!("initialize isolated E2E database: {error}");
            }
        }
    }

    pub(super) async fn cleanup(self) {
        self.pool.close().await;
        Self::drop_named(&self.name)
            .await
            .expect("drop isolated E2E database");
    }

    async fn initialize(name: &str) -> Result<PgPool, String> {
        let url = format!("postgres://postgres:postgres@localhost:5432/{name}");
        let pool = PgPool::connect(&url)
            .await
            .map_err(|error| format!("connect: {error}"))?;
        let migrator = sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
            .await
            .map_err(|error| format!("load migrations: {error}"))?;
        migrator
            .run(&pool)
            .await
            .map_err(|error| format!("run migrations: {error}"))?;
        Ok(pool)
    }

    async fn drop_named(name: &str) -> Result<(), String> {
        let mut admin = PgConnection::connect(ADMIN_URL)
            .await
            .map_err(|error| format!("connect for drop: {error}"))?;
        admin
            .execute(format!("DROP DATABASE {name} WITH (FORCE)").as_str())
            .await
            .map_err(|error| format!("drop database: {error}"))?;
        Ok(())
    }

    pub(super) async fn assert_no_residue() {
        let mut admin = PgConnection::connect(ADMIN_URL)
            .await
            .expect("connect for E2E residue check");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_database WHERE datname LIKE 'svc_workflow_e2e_%'",
        )
        .fetch_one(&mut admin)
        .await
        .expect("query E2E database residue");
        assert_eq!(count, 0, "temporary E2E databases must not remain");
    }
}
