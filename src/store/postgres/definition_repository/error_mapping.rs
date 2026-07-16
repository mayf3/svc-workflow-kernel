//! Database error mapping for the PostgreSQL definition repository.
//!
//! Maps PostgreSQL errors to typed [`DefinitionError`] variants.
//! Handles:
//! - `23505` unique violations → `DefinitionKeyConflict` / `ConcurrentModification`
//! - Trigger errors with `graph_immutable:` prefix → `VersionNotDraft`
//! - Trigger errors with `status_transition:` prefix → `InvalidLifecycleTransition`
//! - All other errors → `StorageError(raw)`

use crate::domain::definition::error::DefinitionError;

/// Map a database error to a typed [`DefinitionError`].
pub(super) fn map_db_error(e: sqlx::Error) -> DefinitionError {
    if let sqlx::Error::Database(ref db_err) = e {
        if let Some(code) = db_err.code() {
            if code == "23505" {
                let msg = db_err.message();
                if msg.contains("definition_key") {
                    return DefinitionError::DefinitionKeyConflict;
                }
                if msg.contains("version_number") {
                    return DefinitionError::ConcurrentModification(
                        "duplicate version number".to_string(),
                    );
                }
            }
            let msg = db_err.message();
            if msg.contains("graph_immutable:") {
                return DefinitionError::VersionNotDraft;
            }
            if msg.contains("status_transition:") {
                return DefinitionError::InvalidLifecycleTransition;
            }
        }
    }
    DefinitionError::StorageError(e.to_string())
}
