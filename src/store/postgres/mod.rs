//! PostgreSQL storage layer.

pub mod admin_recovery_repository;
pub mod definition_repository;
pub(crate) mod import_receipt_validation;
pub mod legacy_import_repository;
pub mod migrations;
pub mod pool;
pub mod repository_rows;
pub mod workflow_instance_repository;

/// Default database name used in development / CI.
#[allow(dead_code)]
pub const DEFAULT_DB_NAME: &str = "svc_workflow";
