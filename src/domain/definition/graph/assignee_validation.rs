//! H-2: Assignee rules for workflow graph validation.
//!
//! Enforces contract §3.1.7: Terminal nodes have no assignee.
//! Enforces contract §3.1.8: Non-terminal nodes must have a legal assignee reference.
//!
//! | Node type    | Assignee rule                                           |
//! |--------------|--------------------------------------------------------|
//! | TERMINAL     | No assignee reference                                     |
//! | DRAFT        | ref_type == WORKFLOW_CREATOR; no fixed_principal_id     |
//! | NORMAL       | Depends on ref_type per contract                        |

use std::collections::HashMap;

use crate::domain::definition::error::GraphValidationError;
use crate::domain::definition::model::{NodeDefinition, WorkflowGraph};
use crate::domain::enums::{AssigneeRefType, NodeType};
use crate::domain::ids::NodeId;

/// Validate assignee rules for all nodes in the graph.
///
/// Appends errors to the provided vector.
pub(super) fn validate_assignee_rules(
    graph: &WorkflowGraph,
    _nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    errors: &mut Vec<GraphValidationError>,
) {
    for node in &graph.nodes {
        match node.node_type {
            NodeType::TERMINAL => {
                if node.assignee_ref.is_some() {
                    errors.push(GraphValidationError::new(
                        "TERMINAL_HAS_ASSIGNEE",
                        format!(
                            "terminal node '{}' must not have an assignee reference",
                            node.node_key
                        ),
                    ));
                }
            }
            NodeType::DRAFT => {
                let Some(assignee_ref) = &node.assignee_ref else {
                    errors.push(GraphValidationError::new(
                        "ASSIGNEE_REQUIRED",
                        format!("DRAFT node '{}' requires an assignee", node.node_key),
                    ));
                    continue;
                };
                if assignee_ref.ref_type != AssigneeRefType::WorkflowCreator {
                    errors.push(GraphValidationError::new(
                        "DRAFT_NOT_WORKFLOW_CREATOR",
                        format!(
                            "DRAFT node '{}' has assignee {:?}, expected WORKFLOW_CREATOR",
                            node.node_key, assignee_ref.ref_type
                        ),
                    ));
                }
                if assignee_ref.fixed_principal_id.is_some() {
                    errors.push(GraphValidationError::new(
                        "UNEXPECTED_FIXED_PRINCIPAL",
                        format!(
                            "DRAFT node '{}' has fixed_principal_id but assignee type is {:?}",
                            node.node_key, assignee_ref.ref_type
                        ),
                    ));
                }
            }
            NodeType::NORMAL => {
                let Some(assignee_ref) = &node.assignee_ref else {
                    errors.push(GraphValidationError::new(
                        "ASSIGNEE_REQUIRED",
                        format!("NORMAL node '{}' requires an assignee", node.node_key),
                    ));
                    continue;
                };
                match assignee_ref.ref_type {
                    AssigneeRefType::WorkflowCreator => {
                        if assignee_ref.fixed_principal_id.is_some() {
                            errors.push(GraphValidationError::new(
                                "UNEXPECTED_FIXED_PRINCIPAL",
                                format!(
                                "NORMAL node '{}' is WORKFLOW_CREATOR but has fixed_principal_id",
                                node.node_key
                            ),
                            ));
                        }
                    }
                    AssigneeRefType::DomainOwner => {
                        if assignee_ref.fixed_principal_id.is_some() {
                            errors.push(GraphValidationError::new(
                                "UNEXPECTED_FIXED_PRINCIPAL",
                                format!(
                                    "NORMAL node '{}' is DOMAIN_OWNER but has fixed_principal_id",
                                    node.node_key
                                ),
                            ));
                        }
                    }
                    AssigneeRefType::FixedPrincipal => {
                        if assignee_ref.fixed_principal_id.is_none() {
                            errors.push(GraphValidationError::new(
                                "FIXED_PRINCIPAL_MISSING_ID",
                                format!(
                                "NORMAL node '{}' is FIXED_PRINCIPAL but no principal_id provided",
                                node.node_key
                            ),
                            ));
                        }
                    }
                }
            }
        }
    }
}
