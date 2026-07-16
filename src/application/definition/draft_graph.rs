use std::collections::HashMap;

use crate::domain::definition::error::{DefinitionError, GraphValidationError};
use crate::domain::definition::graph;
use crate::domain::definition::model::{NodeDefinition, TransitionDefinition, WorkflowGraph};
use crate::domain::enums::{DefinitionVersionStatus, NodeType, TransitionEffect};
use crate::domain::ids::{DefinitionVersionId, NodeId, TransitionId};

use super::commands::ReplaceDraftGraph;
use super::repository::DefinitionRepository;
use super::service::DefinitionService;

impl<R: DefinitionRepository> DefinitionService<R> {
    // -----------------------------------------------------------------------
    // 12.3 ReplaceDraftGraph
    // -----------------------------------------------------------------------

    /// Atomically replace the graph of a DRAFT version.
    ///
    /// B-1: The repository's replace_draft_graph method handles locking
    /// and DRAFT verification inside its own transaction, serializing
    /// with atomic_publish.  We do NOT call lock_version here; the
    /// repository method does that inside its BEGIN/COMMIT.
    pub async fn replace_draft_graph(&self, cmd: ReplaceDraftGraph) -> Result<(), DefinitionError> {
        self.ensure_principal_enabled(cmd.actor_principal_id)
            .await?;

        // Get definition and version info for domain / version check
        let version = self.repo.get_version(cmd.definition_version_id).await?;
        if version.version_status != DefinitionVersionStatus::DRAFT {
            return Err(DefinitionError::VersionNotDraft);
        }

        // Domain checks (these run before the tx; in practice the
        // domain owner is a rare-change entity, so the race window
        // is negligible.  The version lock inside the tx serializes
        // the critical path with publish.)
        let domain_id = self
            .repo
            .get_definition_domain(version.workflow_definition_id.into_uuid())
            .await?;
        self.ensure_domain_enabled(domain_id).await?;
        self.ensure_domain_owner(cmd.actor_principal_id, domain_id)
            .await?;

        // Resolve node keys -> IDs
        let mut node_id_by_key: HashMap<String, NodeId> = HashMap::new();
        let mut node_defs: Vec<NodeDefinition> = Vec::new();
        let version_id = cmd.definition_version_id;

        for raw_node in &cmd.nodes {
            let node_id = NodeId::new();
            node_id_by_key.insert(raw_node.node_key.clone(), node_id);

            let node_type = raw_node.node_type.parse::<NodeType>().map_err(|_| {
                DefinitionError::StorageError(format!("invalid node_type: {}", raw_node.node_type))
            })?;

            let assignee_ref = match (&raw_node.assignee_ref_type, node_type) {
                (None, NodeType::TERMINAL) if raw_node.fixed_principal_id.is_none() => None,
                (Some(_), NodeType::TERMINAL) | (None, NodeType::TERMINAL) => {
                    return Err(DefinitionError::GraphValidationFailed(vec![
                        GraphValidationError::new(
                            "TERMINAL_HAS_ASSIGNEE",
                            format!(
                                "terminal node '{}' must not define an assignee",
                                raw_node.node_key
                            ),
                        ),
                    ]));
                }
                (Some(ref_type), _) => Some(Self::parse_assignee_ref(
                    ref_type,
                    raw_node.fixed_principal_id,
                )?),
                (None, _) => {
                    return Err(DefinitionError::GraphValidationFailed(vec![
                        GraphValidationError::new(
                            "ASSIGNEE_REQUIRED",
                            format!(
                                "non-terminal node '{}' requires an assignee",
                                raw_node.node_key
                            ),
                        ),
                    ]));
                }
            };

            node_defs.push(NodeDefinition {
                node_id,
                definition_version_id: DefinitionVersionId::from_uuid(version_id),
                node_key: raw_node.node_key.clone(),
                display_name: raw_node.display_name.clone(),
                order_index: raw_node.order_index,
                node_type,
                assignee_ref,
                instructions: raw_node.instructions.clone(),
                primary_advance_transition_id: None, // resolved after transitions
                metadata: raw_node.metadata.clone(),
                created_at: chrono::Utc::now(),
            });
        }

        // Build transition definitions
        let mut transition_defs: Vec<TransitionDefinition> = Vec::new();
        let mut transition_key_to_id: HashMap<String, TransitionId> = HashMap::new();

        for raw_trans in &cmd.transitions {
            let trans_id = TransitionId::new();
            transition_key_to_id.insert(raw_trans.transition_key.clone(), trans_id);

            let source_id = node_id_by_key
                .get(&raw_trans.source_node_key)
                .ok_or_else(|| {
                    DefinitionError::GraphValidationFailed(vec![GraphValidationError::new(
                        "TRANSITION_SOURCE_MISSING",
                        format!(
                            "transition '{}' references unknown source node '{}'",
                            raw_trans.transition_key, raw_trans.source_node_key
                        ),
                    )])
                })?;

            let target_id = node_id_by_key
                .get(&raw_trans.target_node_key)
                .ok_or_else(|| {
                    DefinitionError::GraphValidationFailed(vec![GraphValidationError::new(
                        "TRANSITION_TARGET_MISSING",
                        format!(
                            "transition '{}' references unknown target node '{}'",
                            raw_trans.transition_key, raw_trans.target_node_key
                        ),
                    )])
                })?;

            let effect = raw_trans
                .transition_effect
                .parse::<TransitionEffect>()
                .map_err(|_| {
                    DefinitionError::StorageError(format!(
                        "invalid transition_effect: {}",
                        raw_trans.transition_effect
                    ))
                })?;

            transition_defs.push(TransitionDefinition {
                transition_id: trans_id,
                definition_version_id: DefinitionVersionId::from_uuid(version_id),
                transition_key: raw_trans.transition_key.clone(),
                display_name: raw_trans.display_name.clone(),
                source_node_id: *source_id,
                target_node_id: *target_id,
                transition_effect: effect,
                submission_schema: raw_trans.submission_schema.clone(),
                metadata: raw_trans.metadata.clone(),
                created_at: chrono::Utc::now(),
            });
        }

        // Resolve primary advance transition keys to IDs
        for node_def in &mut node_defs {
            // Find the raw node with matching key
            if let Some(raw_node) = cmd.nodes.iter().find(|n| n.node_key == node_def.node_key) {
                if let Some(pt_key) = &raw_node.primary_advance_transition_key {
                    if let Some(pt_id) = transition_key_to_id.get(pt_key) {
                        node_def.primary_advance_transition_id = Some(*pt_id);
                    }
                }
            }
        }

        // Validate the graph against the effective schema after applying the
        // three-state patch: None keeps the stored value, JSON null clears it,
        // and any other JSON value replaces it.
        let effective_context_schema = match cmd.context_schema.as_ref() {
            None => version.context_schema.clone(),
            Some(schema) if schema.is_null() => None,
            Some(schema) => Some(schema.clone()),
        };

        // Build graph model for validation
        let graph = WorkflowGraph {
            nodes: node_defs.clone(),
            transitions: transition_defs.clone(),
            context_schema: effective_context_schema,
        };

        // Validate the graph
        let validation_result = graph::validate_graph(&graph);

        // Also validate JSON schemas
        let schema_errors = self.validate_json_schemas(&graph).await;
        let mut all_errors = validation_result.errors.clone();
        all_errors.extend(schema_errors);

        if !all_errors.is_empty() {
            return Err(DefinitionError::GraphValidationFailed(all_errors));
        }

        // Replace graph atomically
        self.repo
            .replace_draft_graph(
                version_id,
                cmd.context_schema.as_ref(),
                &node_defs,
                &transition_defs,
            )
            .await
    }
}
