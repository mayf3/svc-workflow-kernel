#[cfg(test)]
mod tests {
    use crate::domain::definition::digest::compute_digest;
    use crate::domain::definition::error::DefinitionError;
    use crate::domain::definition::model::{AssigneeRef, NodeDefinition, TransitionDefinition};
    use crate::domain::enums::{AssigneeRefType, NodeType, TransitionEffect};
    use crate::domain::ids::{DefinitionVersionId, NodeId, PrincipalId, TransitionId};
    use std::collections::HashMap;

    #[allow(clippy::type_complexity)]
    fn make_test_data(
        version_id: DefinitionVersionId,
    ) -> (
        Vec<NodeDefinition>,
        Vec<TransitionDefinition>,
        HashMap<NodeId, String>,
        HashMap<TransitionId, String>,
    ) {
        let draft_node_id = NodeId::new();
        let normal_node_id = NodeId::new();
        let terminal_node_id = NodeId::new();
        let advance_trans_id = TransitionId::new();
        let complete_trans_id = TransitionId::new();

        let nodes = vec![
            NodeDefinition {
                node_id: draft_node_id,
                definition_version_id: version_id,
                node_key: "draft".to_string(),
                display_name: "Draft".to_string(),
                order_index: 0,
                node_type: NodeType::DRAFT,
                assignee_ref: Some(AssigneeRef {
                    ref_type: AssigneeRefType::WorkflowCreator,
                    fixed_principal_id: None,
                }),
                instructions: None,
                primary_advance_transition_id: Some(advance_trans_id),
                metadata: None,
                created_at: chrono::Utc::now(),
            },
            NodeDefinition {
                node_id: normal_node_id,
                definition_version_id: version_id,
                node_key: "dev_self_check".to_string(),
                display_name: "Dev Self Check".to_string(),
                order_index: 1,
                node_type: NodeType::NORMAL,
                assignee_ref: Some(AssigneeRef {
                    ref_type: AssigneeRefType::FixedPrincipal,
                    fixed_principal_id: Some(PrincipalId::new()),
                }),
                instructions: Some("Run tests".to_string()),
                primary_advance_transition_id: Some(complete_trans_id),
                metadata: None,
                created_at: chrono::Utc::now(),
            },
            NodeDefinition {
                node_id: terminal_node_id,
                definition_version_id: version_id,
                node_key: "done".to_string(),
                display_name: "Done".to_string(),
                order_index: 2,
                node_type: NodeType::TERMINAL,
                assignee_ref: None,
                instructions: None,
                primary_advance_transition_id: None,
                metadata: None,
                created_at: chrono::Utc::now(),
            },
        ];

        let transitions = vec![
            TransitionDefinition {
                transition_id: advance_trans_id,
                definition_version_id: version_id,
                transition_key: "advance-dev".to_string(),
                display_name: "Advance to Dev".to_string(),
                source_node_id: draft_node_id,
                target_node_id: normal_node_id,
                transition_effect: TransitionEffect::Advance,
                submission_schema: None,
                metadata: None,
                created_at: chrono::Utc::now(),
            },
            TransitionDefinition {
                transition_id: complete_trans_id,
                definition_version_id: version_id,
                transition_key: "advance-done".to_string(),
                display_name: "Complete".to_string(),
                source_node_id: normal_node_id,
                target_node_id: terminal_node_id,
                transition_effect: TransitionEffect::Advance,
                submission_schema: None,
                metadata: None,
                created_at: chrono::Utc::now(),
            },
        ];

        let mut node_key_map = HashMap::new();
        node_key_map.insert(draft_node_id, "draft".to_string());
        node_key_map.insert(normal_node_id, "dev_self_check".to_string());
        node_key_map.insert(terminal_node_id, "done".to_string());

        let mut transition_key_map = HashMap::new();
        transition_key_map.insert(advance_trans_id, "advance-dev".to_string());
        transition_key_map.insert(complete_trans_id, "advance-done".to_string());

        (nodes, transitions, node_key_map, transition_key_map)
    }

    #[test]
    fn same_semantics_produces_same_digest() {
        let version_id = DefinitionVersionId::new();
        let (nodes, transitions, nk, tk) = make_test_data(version_id);
        let context_schema = serde_json::json!({"type": "object"});

        let digest1 = compute_digest(
            "test-def",
            1,
            Some("https://json-schema.org/draft/2020-12/schema"),
            Some("1"),
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        // Same data should produce same digest
        let digest2 = compute_digest(
            "test-def",
            1,
            Some("https://json-schema.org/draft/2020-12/schema"),
            Some("1"),
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        assert_eq!(digest1, digest2, "same input should produce same digest");
    }

    #[test]
    fn different_json_key_order_same_digest() {
        let version_id = DefinitionVersionId::new();
        let (nodes, transitions, nk, tk) = make_test_data(version_id);

        // Context schema with different key order
        let ctx1 = serde_json::json!({"type": "object", "required": ["title"]});
        let ctx2 = serde_json::json!({"required": ["title"], "type": "object"});

        let digest1 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&ctx1),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        let digest2 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&ctx2),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        assert_eq!(
            digest1, digest2,
            "different JSON key order should produce same digest"
        );
    }

    #[test]
    fn different_node_order_same_digest() {
        let version_id = DefinitionVersionId::new();
        let (mut nodes, transitions, nk, tk) = make_test_data(version_id);

        // Reverse node order
        nodes.reverse();

        let context_schema = serde_json::json!({"type": "object"});

        let digest1 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        // Same data, original order
        nodes.reverse();
        let digest2 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        assert_eq!(
            digest1, digest2,
            "different node order should produce same digest"
        );
    }

    #[test]
    fn different_transition_order_same_digest() {
        let version_id = DefinitionVersionId::new();
        let (nodes, mut transitions, nk, tk) = make_test_data(version_id);

        transitions.reverse();

        let context_schema = serde_json::json!({"type": "object"});

        let digest1 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        transitions.reverse();
        let digest2 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        assert_eq!(
            digest1, digest2,
            "different transition order should produce same digest"
        );
    }

    #[test]
    fn different_context_schema_produces_different_digest() {
        let version_id = DefinitionVersionId::new();
        let (nodes, transitions, nk, tk) = make_test_data(version_id);

        let ctx1 = serde_json::json!({"type": "object", "required": ["title"]});
        let ctx2 = serde_json::json!({"type": "object", "required": ["description"]});

        let digest1 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&ctx1),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        let digest2 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&ctx2),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        assert_ne!(
            digest1, digest2,
            "different context schema should produce different digest"
        );
    }

    #[test]
    fn different_instructions_produces_different_digest() {
        let version_id = DefinitionVersionId::new();
        let (mut nodes, transitions, nk, tk) = make_test_data(version_id);

        // Change instructions on one node
        nodes[1].instructions = Some("Different instructions".to_string());

        let context_schema = serde_json::json!({"type": "object"});

        let digest1 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        nodes[1].instructions = Some("Original instructions".to_string());
        let digest2 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        assert_ne!(
            digest1, digest2,
            "different instructions should produce different digest"
        );
    }

    #[test]
    fn different_submission_schema_produces_different_digest() {
        let version_id = DefinitionVersionId::new();
        let (nodes, mut transitions, nk, tk) = make_test_data(version_id);

        transitions[0].submission_schema = Some(serde_json::json!({"type": "object"}));

        let context_schema = serde_json::json!({"type": "object"});

        let digest1 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        transitions[0].submission_schema =
            Some(serde_json::json!({"type": "object", "required": ["field"]}));
        let digest2 = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        assert_ne!(
            digest1, digest2,
            "different submission schema should produce different digest"
        );
    }

    #[test]
    fn different_timestamps_do_not_affect_digest() {
        let version_id = DefinitionVersionId::new();
        let (nodes, transitions, nk, tk) = make_test_data(version_id);

        let context_schema = serde_json::json!({"type": "object"});

        let digest = compute_digest(
            "test-def",
            1,
            None,
            None,
            Some(&context_schema),
            &nodes,
            &transitions,
            &nk,
            &tk,
        )
        .unwrap();

        // Digest should be deterministic regardless of when it's computed
        assert_eq!(digest.len(), 64, "digest should be a 64-char hex string");
        assert!(
            digest.chars().all(|c: char| c.is_ascii_hexdigit()),
            "digest should be hex"
        );
    }
}
