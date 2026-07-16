//! Application layer for svc-workflow.
//!
//! Contains use-case-level services that orchestrate domain logic
//! and storage operations. This layer does not depend on HTTP or
//! external frameworks.

pub mod definition;
pub mod workflow_instance;
