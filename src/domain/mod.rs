//! Domain types for svc-workflow.
//!
//! This module contains strongly-typed ID newtypes, enum types
//! matching the frozen PostgreSQL schema, and the workflow
//! definition domain (models, graph validation, digest).

pub mod definition;
pub mod enums;
pub mod ids;
pub mod workflow_instance;
