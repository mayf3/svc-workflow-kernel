//! Definition lifecycle use cases.
//!
//! Modules:
//! - `publish`: PublishVersion workflow
//! - `status_changes`: DeprecateVersion / RevokeVersion
//! - `reads`: definition/version query operations
//! - `validation`: ValidateDraftVersion, schema validation, fixed principal checks

mod publish;
mod reads;
mod status_changes;
mod validation;
