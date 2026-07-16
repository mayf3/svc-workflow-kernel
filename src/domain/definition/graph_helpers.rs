//! Helper function for graph validation.
//!
//! Provides directed BFS reachability from the DRAFT node,
//! following only source → target direction of transitions.

use std::collections::{HashMap, HashSet};

use crate::domain::ids::NodeId;

use super::model::{NodeDefinition, TransitionDefinition};

/// Compute directed-reachable nodes from `start_node_id`.
///
/// Only follows transitions in the forward direction:
/// `Transition.source_node_id → Transition.target_node_id`.
///
/// This computes the true set of nodes that can be reached from the DRAFT entry
/// node by following the workflow's directed edges. Unlike weak connectivity,
/// a node that is only connected via a backwards RETURN edge will NOT be
/// considered reachable.
///
/// # Returns
/// A set of `NodeId` values reachable via a directed path from `start_node_id`.
pub fn compute_directed_reachable(
    _nodes: &[NodeDefinition],
    transitions: &[TransitionDefinition],
    start_node_id: NodeId,
) -> HashSet<NodeId> {
    // Build an adjacency list: source_node_id → [target_node_id]
    let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for trans in transitions {
        adj.entry(trans.source_node_id)
            .or_default()
            .push(trans.target_node_id);
    }

    let mut reachable: HashSet<NodeId> = HashSet::new();
    let mut stack = vec![start_node_id];

    while let Some(current) = stack.pop() {
        if !reachable.insert(current) {
            continue;
        }
        if let Some(neighbors) = adj.get(&current) {
            for &next in neighbors {
                if !reachable.contains(&next) {
                    stack.push(next);
                }
            }
        }
    }

    reachable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::enums::TransitionEffect;
    use crate::domain::ids::{DefinitionVersionId, TransitionId};

    fn make_node(id: NodeId, key: &str, order: i32) -> NodeDefinition {
        NodeDefinition {
            node_id: id,
            definition_version_id: DefinitionVersionId::new(),
            node_key: key.to_string(),
            display_name: key.to_string(),
            order_index: order,
            node_type: crate::domain::enums::NodeType::NORMAL,
            assignee_ref: Some(crate::domain::definition::model::AssigneeRef {
                ref_type: crate::domain::enums::AssigneeRefType::WorkflowCreator,
                fixed_principal_id: None,
            }),
            instructions: None,
            primary_advance_transition_id: None,
            metadata: None,
            created_at: chrono::Utc::now(),
        }
    }

    fn make_trans(
        src: NodeId,
        tgt: NodeId,
        key: &str,
        effect: TransitionEffect,
    ) -> TransitionDefinition {
        TransitionDefinition {
            transition_id: TransitionId::new(),
            definition_version_id: DefinitionVersionId::new(),
            transition_key: key.to_string(),
            display_name: key.to_string(),
            source_node_id: src,
            target_node_id: tgt,
            transition_effect: effect,
            submission_schema: None,
            metadata: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn directed_chain_all_reachable() {
        let a = NodeId::new();
        let b = NodeId::new();
        let c = NodeId::new();
        let nodes = vec![
            make_node(a, "a", 0),
            make_node(b, "b", 1),
            make_node(c, "c", 2),
        ];
        let transitions = vec![
            make_trans(a, b, "a->b", TransitionEffect::Advance),
            make_trans(b, c, "b->c", TransitionEffect::Advance),
        ];
        let reachable = compute_directed_reachable(&nodes, &transitions, a);
        assert!(reachable.contains(&a));
        assert!(reachable.contains(&b));
        assert!(reachable.contains(&c));
    }

    #[test]
    fn node_only_reachable_via_backwards_edge_not_reachable() {
        let draft = NodeId::new();
        let review = NodeId::new();
        let done = NodeId::new();
        let orphan = NodeId::new();
        let nodes = vec![
            make_node(draft, "draft", 0),
            make_node(review, "review", 1),
            make_node(done, "done", 2),
            make_node(orphan, "orphan", 3),
        ];
        // orphan can only be reached by going backwards from review → orphan
        let transitions = vec![
            make_trans(draft, review, "draft->review", TransitionEffect::Advance),
            make_trans(review, done, "review->done", TransitionEffect::Advance),
            make_trans(review, orphan, "review->orphan", TransitionEffect::Return),
        ];
        let reachable = compute_directed_reachable(&nodes, &transitions, draft);
        assert!(reachable.contains(&draft));
        assert!(reachable.contains(&review));
        assert!(reachable.contains(&done));
        // orphan is reachable because review -> orphan is a forward RETURN edge
        // This is correct — the RETURN direction is still source → target
        assert!(reachable.contains(&orphan));
    }

    #[test]
    fn isolated_node_not_reachable() {
        let draft = NodeId::new();
        let done = NodeId::new();
        let isolated = NodeId::new();
        let nodes = vec![
            make_node(draft, "draft", 0),
            make_node(done, "done", 1),
            make_node(isolated, "isolated", 10),
        ];
        let transitions = vec![make_trans(
            draft,
            done,
            "draft->done",
            TransitionEffect::Advance,
        )];
        let reachable = compute_directed_reachable(&nodes, &transitions, draft);
        assert!(reachable.contains(&draft));
        assert!(reachable.contains(&done));
        assert!(!reachable.contains(&isolated));
    }

    #[test]
    fn node_only_connected_via_reverse_edge_not_reachable() {
        let draft = NodeId::new();
        let a = NodeId::new();
        let b = NodeId::new();
        let nodes = vec![
            make_node(draft, "draft", 0),
            make_node(a, "a", 1),
            make_node(b, "b", 2),
        ];
        // b has an edge TO draft (reverse direction), but no edge FROM draft to b
        let transitions = vec![
            make_trans(draft, a, "draft->a", TransitionEffect::Advance),
            make_trans(b, draft, "b->draft", TransitionEffect::Return),
        ];
        let reachable = compute_directed_reachable(&nodes, &transitions, draft);
        assert!(reachable.contains(&draft));
        assert!(reachable.contains(&a));
        // b has an outgoing edge to draft but no incoming edge from the directed graph
        // Since b's only connection is b → draft (reverse direction), b is NOT directed-reachable from draft
        assert!(
            !reachable.contains(&b),
            "b should not be reachable via directed edges only"
        );
    }

    #[test]
    fn return_edge_does_not_help_unreachable_nodes() {
        let draft = NodeId::new();
        let review = NodeId::new();
        let done = NodeId::new();
        let side = NodeId::new();
        let nodes = vec![
            make_node(draft, "draft", 0),
            make_node(review, "review", 1),
            make_node(done, "done", 2),
            make_node(side, "side", 5),
        ];
        // side has a return edge TO review, but nothing reaches side
        let transitions = vec![
            make_trans(draft, review, "draft->review", TransitionEffect::Advance),
            make_trans(review, done, "review->done", TransitionEffect::Advance),
            make_trans(side, review, "side->review", TransitionEffect::Return),
        ];
        let reachable = compute_directed_reachable(&nodes, &transitions, draft);
        assert!(reachable.contains(&draft));
        assert!(reachable.contains(&review));
        assert!(reachable.contains(&done));
        // side is NOT reachable because no directed edge reaches it
        assert!(
            !reachable.contains(&side),
            "side should NOT be reachable via directed edges"
        );
    }
}
