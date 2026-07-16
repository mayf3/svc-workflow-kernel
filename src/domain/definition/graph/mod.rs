//! Workflow graph validation engine.
//!
//! Validates a complete workflow graph before publication.
//! Rules are specified in the architecture document section 14.
#![allow(clippy::needless_borrow)]

mod assignee_validation;
mod transition_validation;
mod validation;

use std::collections::HashMap;

use super::error::GraphValidationError;
use super::model::{NodeDefinition, TransitionDefinition, ValidationResult, WorkflowGraph};
use crate::domain::ids::TransitionId;

/// Validate a complete workflow graph against all publication rules.
///
/// Returns a [`ValidationResult`] summarizing all errors, warnings,
/// and optionally a computed digest.
pub fn validate_graph(graph: &WorkflowGraph) -> ValidationResult {
    let mut errors: Vec<GraphValidationError> = Vec::new();
    let warnings: Vec<String> = Vec::new();

    // Build lookup maps
    let nodes_by_id: HashMap<_, &NodeDefinition> =
        graph.nodes.iter().map(|n| (n.node_id, n)).collect();
    let transitions_by_id: HashMap<TransitionId, &TransitionDefinition> = graph
        .transitions
        .iter()
        .map(|t| (t.transition_id, t))
        .collect();

    // ---
    // 14.1 Node rules
    // ---
    let (draft_nodes, _terminal_nodes) = validation::validate_node_rules(graph, &mut errors);

    // ---
    // H-2: Assignee rules
    // ---
    assignee_validation::validate_assignee_rules(graph, &nodes_by_id, &mut errors);

    // ---
    // Transition uniqueness + reference checks (14.5)
    // ---
    transition_validation::validate_transition_references(graph, &nodes_by_id, &mut errors);

    // ---
    // 14.2 Primary trunk rules
    // ---
    let (_primary_targets, _nodes_with_primary) = transition_validation::validate_primary_trunk(
        graph,
        &nodes_by_id,
        &transitions_by_id,
        &draft_nodes,
        &mut errors,
    );

    // 7. All nodes must be reachable from DRAFT (H-1)
    validation::validate_directed_reachability(graph, &draft_nodes, &nodes_by_id, &mut errors);

    // ---
    // 14.3 RETURN rules
    // ---
    transition_validation::validate_return_rules(graph, &nodes_by_id, &mut errors);

    // ---
    // 14.4 TERMINATE rules
    // ---
    transition_validation::validate_terminate_rules(graph, &nodes_by_id, &mut errors);

    // Terminal nodes must have no outgoing transitions
    transition_validation::validate_terminal_outgoing(graph, &mut errors);

    let valid = errors.is_empty();
    ValidationResult {
        valid,
        errors,
        warnings,
        computed_digest: None,
    }
}
