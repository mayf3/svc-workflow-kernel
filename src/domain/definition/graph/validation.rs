//! Core graph validation rules.
//!
//! Handles:
//! - Node structural rules (minimum nodes, DRAFT count, TERMINAL count, order_index)
//! - Directed reachability from DRAFT node (H-1)

use std::collections::{HashMap, HashSet};

use crate::domain::definition::error::GraphValidationError;
use crate::domain::definition::graph_helpers::compute_directed_reachable;
use crate::domain::definition::model::{NodeDefinition, WorkflowGraph};
use crate::domain::enums::NodeType;
use crate::domain::ids::NodeId;

/// Validate node structural rules.
///
/// Rules checked:
/// 1. At least 2 nodes
/// 2. Exactly one DRAFT node
/// 3. At least one TERMINAL node
/// 4. order_index unique within version
/// 5. node_key unique within version (via hashmap construction)
pub(super) fn validate_node_rules<'a>(
    graph: &'a WorkflowGraph,
    errors: &mut Vec<GraphValidationError>,
) -> (Vec<&'a NodeDefinition>, Vec<&'a NodeDefinition>) {
    // 1. At least two nodes
    if graph.nodes.len() < 2 {
        errors.push(GraphValidationError::new(
            "MIN_NODES",
            "graph must have at least 2 nodes",
        ));
    }

    // 2 & 3. Exactly one DRAFT node
    let draft_nodes: Vec<&NodeDefinition> = graph
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::DRAFT)
        .collect();
    if draft_nodes.is_empty() {
        errors.push(GraphValidationError::new(
            "NO_DRAFT_NODE",
            "graph must have exactly one DRAFT node",
        ));
    } else if draft_nodes.len() > 1 {
        errors.push(GraphValidationError::new(
            "MULTIPLE_DRAFT_NODES",
            format!(
                "graph has {} DRAFT nodes, expected exactly one",
                draft_nodes.len()
            ),
        ));
    }

    // 4. At least one TERMINAL node
    let terminal_nodes: Vec<&NodeDefinition> = graph
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::TERMINAL)
        .collect();
    if terminal_nodes.is_empty() {
        errors.push(GraphValidationError::new(
            "NO_TERMINAL_NODE",
            "graph must have at least one TERMINAL node",
        ));
    }

    // 5. order_index unique within version
    let mut seen_order_indices: HashSet<i32> = HashSet::new();
    for node in &graph.nodes {
        if !seen_order_indices.insert(node.order_index) {
            errors.push(GraphValidationError::new(
                "DUPLICATE_ORDER_INDEX",
                format!(
                    "duplicate order_index {} (node_key={})",
                    node.order_index, node.node_key
                ),
            ));
        }
    }

    (draft_nodes, terminal_nodes)
}

/// Validate that all nodes are reachable from DRAFT via directed edges (H-1).
pub(super) fn validate_directed_reachability(
    graph: &WorkflowGraph,
    draft_nodes: &[&NodeDefinition],
    _nodes_by_id: &HashMap<NodeId, &NodeDefinition>,
    errors: &mut Vec<GraphValidationError>,
) {
    if let Some(draft) = draft_nodes.first() {
        let directed_reachable =
            compute_directed_reachable(&graph.nodes, &graph.transitions, draft.node_id);
        for node in &graph.nodes {
            if !directed_reachable.contains(&node.node_id) {
                errors.push(GraphValidationError::new(
                    "NODE_NOT_REACHABLE",
                    format!(
                        "node '{}' is not reachable from draft node via any path",
                        node.node_key
                    ),
                ));
            }
        }
    }
}
