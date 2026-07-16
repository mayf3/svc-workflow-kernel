//! Stable error types for the workflow definition domain.

use std::fmt;

/// Top-level error type for definition operations.
#[derive(Debug, Clone)]
pub enum DefinitionError {
    /// Principal does not exist.
    PrincipalNotFound,
    /// Principal exists but is disabled.
    PrincipalDisabled,
    /// Domain does not exist.
    DomainNotFound,
    /// Domain exists but is disabled.
    DomainDisabled,
    /// Actor lacks required permission.
    PermissionDenied,
    /// Workflow definition not found.
    DefinitionNotFound,
    /// Workflow definition version not found.
    DefinitionVersionNotFound,
    /// `definition_key` already exists within the domain.
    DefinitionKeyConflict,
    /// The version is not in DRAFT state for the requested operation.
    VersionNotDraft,
    /// The lifecycle transition is not allowed (e.g., REVOKED → PUBLISHED).
    InvalidLifecycleTransition,
    /// Graph validation failed with specific errors.
    GraphValidationFailed(Vec<GraphValidationError>),
    /// JSON Schema validation failed.
    SchemaValidationFailed(String),
    /// Fixed principal reference is invalid (missing, not found, or disabled).
    FixedPrincipalInvalid(String),
    /// Digest computation failed.
    DigestFailure(String),
    /// Concurrent modification detected (optimistic lock).
    ConcurrentModification(String),
    /// Generic storage error (wraps underlying DB error message).
    StorageError(String),
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrincipalNotFound => write!(f, "principal not found"),
            Self::PrincipalDisabled => write!(f, "principal is disabled"),
            Self::DomainNotFound => write!(f, "domain not found"),
            Self::DomainDisabled => write!(f, "domain is disabled"),
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::DefinitionNotFound => write!(f, "workflow definition not found"),
            Self::DefinitionVersionNotFound => write!(f, "definition version not found"),
            Self::DefinitionKeyConflict => write!(f, "definition key already exists in domain"),
            Self::VersionNotDraft => write!(f, "version is not in DRAFT status"),
            Self::InvalidLifecycleTransition => {
                write!(f, "invalid lifecycle status transition")
            }
            Self::GraphValidationFailed(errors) => {
                write!(f, "graph validation failed ({} errors)", errors.len())
            }
            Self::SchemaValidationFailed(detail) => {
                write!(f, "schema validation failed: {}", detail)
            }
            Self::FixedPrincipalInvalid(detail) => {
                write!(f, "fixed principal invalid: {}", detail)
            }
            Self::DigestFailure(detail) => write!(f, "digest failure: {}", detail),
            Self::ConcurrentModification(detail) => {
                write!(f, "concurrent modification: {}", detail)
            }
            Self::StorageError(detail) => write!(f, "storage error: {}", detail),
        }
    }
}

impl std::error::Error for DefinitionError {}

/// A single graph validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphValidationError {
    /// Machine-readable error code.
    pub code: String,
    /// Human-readable error message.
    pub message: String,
}

impl GraphValidationError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
