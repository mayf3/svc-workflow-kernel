//! Workflow Instance application service.
//!
//! Provides the CreateWorkflowInstance use case with full idempotency,
//! authorization, and atomic consistency guarantees.

pub mod admin_recovery;
pub mod create;
pub mod execute_transition;
pub mod idempotency;
pub mod import;
pub mod query_service;
pub mod query_types;
pub mod revise;
pub mod revise_and_transition;
