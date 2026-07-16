//! Errors for the atomic ReviseContextAndTransition command.

use std::fmt;

use super::errors::{ExecuteWorkflowTransitionError, ReviseWorkflowContextError};

#[derive(Debug, Clone)]
pub enum ReviseContextAndTransitionError {
    PrincipalNotFound,
    PrincipalDisabled,
    InstanceNotFound,
    CurrentVisitNotFound,
    PrincipalNotCreator,
    PrincipalNotAssignee,
    CurrentNodeNotDraft,
    DefinitionVersionRevoked,
    DefinitionVersionDraft,
    WorkflowStateVersionConflict {
        expected: i32,
        actual: i32,
    },
    TransitionNotApplicable(String),
    ContextValidationFailed(String),
    SubmissionValidationFailed(String),
    SizeLimitExceeded(String),
    AssigneeResolutionFailed(String),
    InternalConsistency(String),
    IdempotencyConflict {
        original_command_id: uuid::Uuid,
        original_request_hash: String,
    },
    CommandStillProcessing,
    StorageError(String),
}

impl fmt::Display for ReviseContextAndTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrincipalNotFound => write!(f, "principal not found"),
            Self::PrincipalDisabled => write!(f, "principal is disabled"),
            Self::InstanceNotFound => write!(f, "workflow instance not found"),
            Self::CurrentVisitNotFound => write!(f, "current node visit not found"),
            Self::PrincipalNotCreator => write!(f, "caller is not the workflow creator"),
            Self::PrincipalNotAssignee => write!(f, "caller is not the current assignee"),
            Self::CurrentNodeNotDraft => write!(f, "current node is not DRAFT"),
            Self::DefinitionVersionRevoked => write!(f, "definition version is REVOKED"),
            Self::DefinitionVersionDraft => write!(f, "definition version is DRAFT"),
            Self::WorkflowStateVersionConflict { expected, actual } => write!(
                f,
                "workflow state version conflict: expected={}, actual={}",
                expected, actual
            ),
            Self::TransitionNotApplicable(detail) => {
                write!(f, "transition not applicable: {}", detail)
            }
            Self::ContextValidationFailed(detail) => {
                write!(f, "context validation failed: {}", detail)
            }
            Self::SubmissionValidationFailed(detail) => {
                write!(f, "submission validation failed: {}", detail)
            }
            Self::SizeLimitExceeded(detail) => write!(f, "size limit exceeded: {}", detail),
            Self::AssigneeResolutionFailed(detail) => {
                write!(f, "assignee resolution failed: {}", detail)
            }
            Self::InternalConsistency(detail) => {
                write!(f, "internal consistency error: {}", detail)
            }
            Self::IdempotencyConflict {
                original_command_id,
                original_request_hash,
            } => write!(
                f,
                "idempotency conflict: original command_id={}, request_hash={}",
                original_command_id, original_request_hash
            ),
            Self::CommandStillProcessing => {
                write!(f, "command with this idempotency key is still processing")
            }
            Self::StorageError(detail) => write!(f, "storage error: {}", detail),
        }
    }
}

impl std::error::Error for ReviseContextAndTransitionError {}

pub fn error_code(error: &ReviseContextAndTransitionError) -> i32 {
    match error {
        ReviseContextAndTransitionError::PrincipalNotFound
        | ReviseContextAndTransitionError::InstanceNotFound
        | ReviseContextAndTransitionError::CurrentVisitNotFound => 404,
        ReviseContextAndTransitionError::PrincipalDisabled
        | ReviseContextAndTransitionError::PrincipalNotCreator
        | ReviseContextAndTransitionError::PrincipalNotAssignee => 403,
        ReviseContextAndTransitionError::CurrentNodeNotDraft
        | ReviseContextAndTransitionError::DefinitionVersionRevoked
        | ReviseContextAndTransitionError::WorkflowStateVersionConflict { .. }
        | ReviseContextAndTransitionError::TransitionNotApplicable(_)
        | ReviseContextAndTransitionError::IdempotencyConflict { .. } => 409,
        ReviseContextAndTransitionError::ContextValidationFailed(_)
        | ReviseContextAndTransitionError::SubmissionValidationFailed(_)
        | ReviseContextAndTransitionError::AssigneeResolutionFailed(_) => 422,
        ReviseContextAndTransitionError::SizeLimitExceeded(_) => 413,
        ReviseContextAndTransitionError::CommandStillProcessing => 425,
        ReviseContextAndTransitionError::DefinitionVersionDraft
        | ReviseContextAndTransitionError::InternalConsistency(_)
        | ReviseContextAndTransitionError::StorageError(_) => 500,
    }
}

pub fn error_label(error: &ReviseContextAndTransitionError) -> &'static str {
    match error {
        ReviseContextAndTransitionError::PrincipalNotFound => "principal_not_found",
        ReviseContextAndTransitionError::PrincipalDisabled => "principal_disabled",
        ReviseContextAndTransitionError::InstanceNotFound => "instance_not_found",
        ReviseContextAndTransitionError::CurrentVisitNotFound => "current_visit_not_found",
        ReviseContextAndTransitionError::PrincipalNotCreator => "principal_not_creator",
        ReviseContextAndTransitionError::PrincipalNotAssignee => "principal_not_assignee",
        ReviseContextAndTransitionError::CurrentNodeNotDraft => "current_node_not_draft",
        ReviseContextAndTransitionError::DefinitionVersionRevoked => "definition_version_revoked",
        ReviseContextAndTransitionError::DefinitionVersionDraft => "definition_version_draft",
        ReviseContextAndTransitionError::WorkflowStateVersionConflict { .. } => {
            "workflow_state_version_conflict"
        }
        ReviseContextAndTransitionError::TransitionNotApplicable(_) => "transition_not_applicable",
        ReviseContextAndTransitionError::ContextValidationFailed(_) => "context_validation_failed",
        ReviseContextAndTransitionError::SubmissionValidationFailed(_) => {
            "submission_validation_failed"
        }
        ReviseContextAndTransitionError::SizeLimitExceeded(_) => "size_limit_exceeded",
        ReviseContextAndTransitionError::AssigneeResolutionFailed(_) => {
            "assignee_resolution_failed"
        }
        ReviseContextAndTransitionError::InternalConsistency(_) => "internal_consistency_error",
        ReviseContextAndTransitionError::IdempotencyConflict { .. } => "idempotency_conflict",
        ReviseContextAndTransitionError::CommandStillProcessing => "command_still_processing",
        ReviseContextAndTransitionError::StorageError(_) => "storage_error",
    }
}

impl From<ExecuteWorkflowTransitionError> for ReviseContextAndTransitionError {
    fn from(error: ExecuteWorkflowTransitionError) -> Self {
        match error {
            ExecuteWorkflowTransitionError::PrincipalNotFound => Self::PrincipalNotFound,
            ExecuteWorkflowTransitionError::PrincipalDisabled => Self::PrincipalDisabled,
            ExecuteWorkflowTransitionError::InstanceNotFound => Self::InstanceNotFound,
            ExecuteWorkflowTransitionError::CurrentVisitNotFound => Self::CurrentVisitNotFound,
            ExecuteWorkflowTransitionError::PrincipalNotAssignee => Self::PrincipalNotAssignee,
            ExecuteWorkflowTransitionError::SourceNodeTerminal => Self::CurrentNodeNotDraft,
            ExecuteWorkflowTransitionError::DefinitionVersionRevoked => {
                Self::DefinitionVersionRevoked
            }
            ExecuteWorkflowTransitionError::DefinitionVersionDraft => Self::DefinitionVersionDraft,
            ExecuteWorkflowTransitionError::DefinitionVersionDeprecated => {
                Self::InternalConsistency("unexpected deprecated marker".to_string())
            }
            ExecuteWorkflowTransitionError::WorkflowStateVersionConflict { expected, actual } => {
                Self::WorkflowStateVersionConflict { expected, actual }
            }
            ExecuteWorkflowTransitionError::TransitionNotApplicable(detail) => {
                Self::TransitionNotApplicable(detail)
            }
            ExecuteWorkflowTransitionError::SubmissionRequired => {
                Self::SubmissionValidationFailed("submission payload is required".to_string())
            }
            ExecuteWorkflowTransitionError::SubmissionValidationFailed(detail) => {
                Self::SubmissionValidationFailed(detail)
            }
            ExecuteWorkflowTransitionError::SizeLimitExceeded(detail) => {
                Self::SizeLimitExceeded(detail)
            }
            ExecuteWorkflowTransitionError::InvalidReturnReferences(detail) => {
                Self::TransitionNotApplicable(detail)
            }
            ExecuteWorkflowTransitionError::AssigneeResolutionFailed(detail) => {
                Self::AssigneeResolutionFailed(detail)
            }
            ExecuteWorkflowTransitionError::InternalConsistency(detail) => {
                Self::InternalConsistency(detail)
            }
            ExecuteWorkflowTransitionError::IdempotencyConflict {
                original_command_id,
                original_request_hash,
            } => Self::IdempotencyConflict {
                original_command_id,
                original_request_hash,
            },
            ExecuteWorkflowTransitionError::CommandStillProcessing => Self::CommandStillProcessing,
            ExecuteWorkflowTransitionError::StorageError(detail) => Self::StorageError(detail),
        }
    }
}

impl From<ReviseWorkflowContextError> for ReviseContextAndTransitionError {
    fn from(error: ReviseWorkflowContextError) -> Self {
        match error {
            ReviseWorkflowContextError::PrincipalNotFound => Self::PrincipalNotFound,
            ReviseWorkflowContextError::PrincipalDisabled => Self::PrincipalDisabled,
            ReviseWorkflowContextError::InstanceNotFound => Self::InstanceNotFound,
            ReviseWorkflowContextError::CurrentVisitNotFound => Self::CurrentVisitNotFound,
            ReviseWorkflowContextError::CurrentNodeNotDraft => Self::CurrentNodeNotDraft,
            ReviseWorkflowContextError::DefinitionVersionRevoked => Self::DefinitionVersionRevoked,
            ReviseWorkflowContextError::DefinitionVersionDraft => Self::DefinitionVersionDraft,
            ReviseWorkflowContextError::WorkflowStateVersionConflict { expected, actual } => {
                Self::WorkflowStateVersionConflict { expected, actual }
            }
            ReviseWorkflowContextError::ContextValidationFailed(detail) => {
                Self::ContextValidationFailed(detail)
            }
            ReviseWorkflowContextError::SizeLimitExceeded(detail) => {
                Self::SizeLimitExceeded(detail)
            }
            ReviseWorkflowContextError::InternalConsistency(detail) => {
                Self::InternalConsistency(detail)
            }
            ReviseWorkflowContextError::IdempotencyConflict {
                original_command_id,
                original_request_hash,
            } => Self::IdempotencyConflict {
                original_command_id,
                original_request_hash,
            },
            ReviseWorkflowContextError::CommandStillProcessing => Self::CommandStillProcessing,
            ReviseWorkflowContextError::StorageError(detail) => Self::StorageError(detail),
        }
    }
}
