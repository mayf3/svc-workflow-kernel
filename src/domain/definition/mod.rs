//! Workflow Definition domain.
//!
//! This module contains the domain model, graph validation engine,
//! digest computation, and error types for workflow definitions
//! and their version lifecycle.

pub mod digest;
pub mod error;
pub mod graph;
pub mod graph_helpers;
pub mod model;

#[cfg(test)]
mod digest_tests;

#[cfg(test)]
mod graph_tests;
