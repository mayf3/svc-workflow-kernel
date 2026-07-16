//! svc-workflow — Serial governed workflow kernel (Rust + PostgreSQL).
//!
//! This library crate provides the domain model, application services,
//! and storage layer for the workflow kernel.

// Allow dead code and unused imports for domain types that are tested
// but not yet wired into a running service.
#![allow(dead_code, unused_imports)]
// Allow clippy-specific warnings for implementation code that is correct
// but triggers stylistic lints.
#![allow(clippy::too_many_arguments, clippy::needless_borrow)]

pub mod application;
pub mod auth;
pub mod domain;
pub mod http;
pub mod store;
