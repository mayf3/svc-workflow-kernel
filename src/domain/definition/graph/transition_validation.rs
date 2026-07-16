//! Transition validation rules for workflow graph validation.
//!
//! Handles:
//! - Transition reference checks (source/target node existence, self-loops)
//! - Primary trunk rules (primary_targets, acyclicity, terminal reachability)
//! - RETURN rules (target ordering, no-primary constraint)
//! - TERMINATE rules (terminal target, no-primary constraint)
//! - Terminal outgoing transition check

use std::collections::{HashMap, HashSet};

use crate::domain::definition::error::GraphValidationError;
use crate::domain::definition::model::{NodeDefinition, TransitionDefinition, WorkflowGraph};
use crate::domain::enums::{NodeType, TransitionEffect};
use crate::domain::ids::NodeId;

/// Validate transition references (source/target node existence, self-loops).
pub(super) fn validate_transition_references(
    graph: &WorkflowGraph,
    nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    errors: &mut Vec<GraphValidationError>,
) {
    for trans in &graph.transitions {
        if !nodes_by_id.contains_key(&trans.source_node_id) {
            errors.push(GraphValidationError::new(
                "TRANSITION_SOURCE_MISSING",
                format!(
                    "transition '{}' references non-existent source node_id",
                    trans.transition_key
                ),
            ));
        }
        if !nodes_by_id.contains_key(&trans.target_node_id) {
            errors.push(GraphValidationError::new(
                "TRANSITION_TARGET_MISSING",
                format!(
                    "transition '{}' references non-existent target node_id",
                    trans.transition_key
                ),
            ));
        }
    }
    // No self-loops
    for trans in &graph.transitions {
        if trans.source_node_id == trans.target_node_id {
            errors.push(GraphValidationError::new(
                "SELF_LOOP",
                format!("transition '{}' is a self-loop", trans.transition_key),
            ));
        }
    }
}

/// Validate primary trunk rules.
///
/// Builds primary_targets map and validates:
/// - Primary transition originates from its node
/// - Primary effect is ADVANCE (H-3)
/// - Primary target has higher order_index
/// - Primary trunk is acyclic
/// - Primary trunk eventually reaches a terminal node
/// - Non-terminal nodes must have a primary transition
pub(super) fn validate_primary_trunk(
    graph: &WorkflowGraph,
    nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    transitions_by_id: &HashMap<crate::domain::ids::TransitionId, &TransitionDefinition>,
    draft_nodes: &[&NodeDefinition],
    errors: &mut Vec<GraphValidationError>,
) -> (HashMap<NodeId, NodeId>, HashSet<NodeId>) {
    let mut primary_targets: HashMap<NodeId, NodeId> = HashMap::new();
    let mut nodes_with_primary: HashSet<NodeId> = HashSet::new();
    let mut node_order_indices: HashMap<NodeId, i32> = HashMap::new();

    for node in &graph.nodes {
        node_order_indices.insert(node.node_id, node.order_index);
    }

    for node in &graph.nodes {
        if let Some(pt_id) = node.primary_advance_transition_id {
            if let Some(trans) = transitions_by_id.get(&pt_id) {
                if node.node_type != NodeType::TERMINAL {
                    primary_targets.insert(node.node_id, trans.target_node_id);
                    nodes_with_primary.insert(node.node_id);
                }
                // Primary transition must originate from this node
                if trans.source_node_id != node.node_id {
                    errors.push(GraphValidationError::new(
                        "PRIMARY_NOT_FROM_NODE",
                        format!(
                            "primary transition '{}' for node '{}' does not originate from this node",
                            trans.transition_key, node.node_key
                        ),
                    ));
                }
                // H-3: Primary transition effect must be ADVANCE
                if trans.transition_effect != TransitionEffect::Advance {
                    errors.push(GraphValidationError::new(
                        "PRIMARY_NOT_ADVANCE",
                        format!(
                            "primary transition '{}' for node '{}' has effect {:?}, expected ADVANCE",
                            trans.transition_key, node.node_key, trans.transition_effect
                        ),
                    ));
                }
                // Primary target must have higher order_index
                if let Some(target_order) = node_order_indices.get(&trans.target_node_id) {
                    if *target_order <= node.order_index {
                        errors.push(GraphValidationError::new(
                            "PRIMARY_NOT_ADVANCING",
                            format!(
                                "primary transition '{}' from '{}' (order={}) to target (order={}) does not advance",
                                trans.transition_key, node.node_key, node.order_index, target_order
                            ),
                        ));
                    }
                }
            } else if node.node_type != NodeType::TERMINAL {
                errors.push(GraphValidationError::new(
                    "PRIMARY_TRANSITION_MISSING",
                    format!(
                        "node '{}' primary_advance_transition_id {} not found in transitions",
                        node.node_key, pt_id
                    ),
                ));
            } else {
                errors.push(GraphValidationError::new(
                    "TERMINAL_HAS_PRIMARY",
                    format!(
                        "terminal node '{}' should not have a primary_advance_transition_id",
                        node.node_key
                    ),
                ));
            }
        } else if node.node_type != NodeType::TERMINAL {
            errors.push(GraphValidationError::new(
                "MISSING_PRIMARY",
                format!(
                    "non-terminal node '{}' (type={:?}) lacks primary_advance_transition_id",
                    node.node_key, node.node_type
                ),
            ));
        }
    }

    // Primary trunk must be acyclic
    validate_primary_acyclic(nodes_by_id, &primary_targets, errors);

    // Primary trunk must eventually reach a terminal node
    validate_trunk_reaches_terminal(draft_nodes, nodes_by_id, &primary_targets, errors);

    (primary_targets, nodes_with_primary)
}

/// Check primary trunk for cycles.
fn validate_primary_acyclic(
    nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    primary_targets: &HashMap<NodeId, NodeId>,
    errors: &mut Vec<GraphValidationError>,
) {
    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut in_path: HashSet<NodeId> = HashSet::new();
    for &start_node in nodes_by_id.keys() {
        if visited.contains(&start_node) {
            continue;
        }
        let mut current = start_node;
        let mut path: Vec<NodeId> = Vec::new();
        loop {
            if in_path.contains(&current) {
                let cycle_start_idx = path.iter().position(|n| *n == current).unwrap_or(0);
                let cycle_nodes: Vec<String> = path[cycle_start_idx..]
                    .iter()
                    .map(|n| {
                        nodes_by_id
                            .get(n)
                            .map(|nn| nn.node_key.clone())
                            .unwrap_or_else(|| "?".to_string())
                    })
                    .collect();
                errors.push(GraphValidationError::new(
                    "PRIMARY_CYCLE",
                    format!(
                        "primary trunk contains a cycle: {}",
                        cycle_nodes.join(" -> ")
                    ),
                ));
                break;
            }
            if visited.contains(&current) {
                break;
            }
            in_path.insert(current);
            path.push(current);
            if let Some(&next) = primary_targets.get(&current) {
                current = next;
            } else {
                break;
            }
        }
        for n in &path {
            visited.insert(*n);
            in_path.remove(n);
        }
    }
}

/// Check that the primary trunk from DRAFT reaches a terminal node.
fn validate_trunk_reaches_terminal(
    draft_nodes: &[&NodeDefinition],
    nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    primary_targets: &HashMap<NodeId, NodeId>,
    errors: &mut Vec<GraphValidationError>,
) {
    if let Some(draft_node) = draft_nodes.first() {
        let mut current = draft_node.node_id;
        while let Some(node) = nodes_by_id.get(&current) {
            if node.node_type == NodeType::TERMINAL {
                break;
            }
            if let Some(&next) = primary_targets.get(&current) {
                if next == current {
                    break;
                }
                current = next;
            } else {
                errors.push(GraphValidationError::new(
                    "PRIMARY_TRUNK_NO_TERMINAL",
                    format!(
                        "primary trunk from draft node '{}' does not reach a terminal node",
                        draft_node.node_key
                    ),
                ));
                break;
            }
        }
    }
}

/// Validate RETURN transition rules.
pub(super) fn validate_return_rules(
    graph: &WorkflowGraph,
    nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    errors: &mut Vec<GraphValidationError>,
) {
    let mut node_order_indices: HashMap<NodeId, i32> = HashMap::new();
    for node in &graph.nodes {
        node_order_indices.insert(node.node_id, node.order_index);
    }

    for trans in &graph.transitions {
        if trans.transition_effect == TransitionEffect::Return {
            let target_order = node_order_indices.get(&trans.target_node_id).copied();
            let source_order = node_order_indices.get(&trans.source_node_id).copied();

            // Target must be a non-terminal node
            if let Some(target_node) = nodes_by_id.get(&trans.target_node_id) {
                if target_node.node_type == NodeType::TERMINAL {
                    errors.push(GraphValidationError::new(
                        "RETURN_TO_TERMINAL",
                        format!(
                            "RETURN transition '{}' targets a TERMINAL node (should use TERMINATE)",
                            trans.transition_key
                        ),
                    ));
                }
            }

            // Target order_index must be less than source order_index
            if let (Some(src_order), Some(tgt_order)) = (source_order, target_order) {
                if tgt_order >= src_order {
                    errors.push(GraphValidationError::new(
                        "RETURN_NOT_BACKWARD",
                        format!(
                            "RETURN transition '{}' goes from order {} to {} (must go to lower order)",
                            trans.transition_key, src_order, tgt_order
                        ),
                    ));
                }
            }

            // Must not be primary_advance_transition_id of source node
            if let Some(source_node) = nodes_by_id.get(&trans.source_node_id) {
                if Some(trans.transition_id) == source_node.primary_advance_transition_id {
                    errors.push(GraphValidationError::new(
                        "RETURN_IS_PRIMARY",
                        format!(
                            "RETURN transition '{}' is also the primary_advance_transition_id of its source node",
                            trans.transition_key
                        ),
                    ));
                }
            }
        }
    }
}

/// Validate TERMINATE transition rules.
pub(super) fn validate_terminate_rules(
    graph: &WorkflowGraph,
    nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    errors: &mut Vec<GraphValidationError>,
) {
    for trans in &graph.transitions {
        if trans.transition_effect == TransitionEffect::Terminate {
            // Must not be primary_advance_transition_id
            if let Some(source_node) = nodes_by_id.get(&trans.source_node_id) {
                if Some(trans.transition_id) == source_node.primary_advance_transition_id {
                    errors.push(GraphValidationError::new(
                        "TERMINATE_IS_PRIMARY",
                        format!(
                            "TERMINATE transition '{}' is also the primary_advance_transition_id",
                            trans.transition_key
                        ),
                    ));
                }
            }
            // Target must be a terminal node
            if let Some(target_node) = nodes_by_id.get(&trans.target_node_id) {
                if target_node.node_type != NodeType::TERMINAL {
                    errors.push(GraphValidationError::new(
                        "TERMINATE_TO_NON_TERMINAL",
                        format!(
                            "TERMINATE transition '{}' targets a non-terminal node (should use RETURN)",
                            trans.transition_key
                        ),
                    ));
                }
            }
        }
    }
}

/// Validate that terminal nodes have no outgoing transitions.
pub(super) fn validate_terminal_outgoing(
    graph: &WorkflowGraph,
    errors: &mut Vec<GraphValidationError>,
) {
    for node in &graph.nodes {
        if node.node_type == NodeType::TERMINAL {
            let outgoing: Vec<&TransitionDefinition> = graph
                .transitions
                .iter()
                .filter(|t| t.source_node_id == node.node_id)
                .collect();
            if !outgoing.is_empty() {
                let keys: Vec<&str> = outgoing.iter().map(|t| t.transition_key.as_str()).collect();
                errors.push(GraphValidationError::new(
                    "TERMINAL_HAS_OUTGOING",
                    format!(
                        "terminal node '{}' has {} outgoing transition(s): {}",
                        node.node_key,
                        outgoing.len(),
                        keys.join(", ")
                    ),
                ));
            }
        }
    }
}
