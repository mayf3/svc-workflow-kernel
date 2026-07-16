//! Definition digest computation using JCS (JSON Canonicalization Scheme).
//!
//! Produces a stable SHA-256 digest of a complete workflow definition version,
//! excluding database-generated fields (timestamps, IDs not part of business identity).
//!
//! Algorithm:
//! 1. Build a canonical document with deterministic field order
//! 2. Sort nodes by node_key, transitions by transition_key
//! 3. JCS-normalize the document
//! 4. SHA-256 the normalized bytes

use serde::Serialize;

use crate::domain::ids::{DefinitionVersionId, WorkflowDefinitionId};

use super::model::{AssigneeRef, NodeDefinition, TransitionDefinition};

/// Canonical document used for digest computation.
///
/// Fields are ordered alphabetically for deterministic output.
/// Node and Transition arrays are sorted by their stable keys.
/// All timestamps and database-generated IDs are excluded.
#[derive(Debug, Clone, Serialize)]
pub struct CanonicalDefinitionDocument {
    pub definition_key: String,
    pub version_number: i32,
    pub json_schema_dialect: Option<String>,
    pub validator_version: Option<String>,
    pub context_schema: Option<serde_json::Value>,
    pub nodes: Vec<CanonicalNode>,
    pub transitions: Vec<CanonicalTransition>,
}

/// Canonical representation of a node.
#[derive(Debug, Clone, Serialize)]
pub struct CanonicalNode {
    pub node_key: String,
    pub display_name: String,
    pub order_index: i32,
    pub node_type: String,
    pub assignee_ref_type: Option<String>,
    pub fixed_principal_id: Option<String>,
    pub instructions: Option<String>,
    pub primary_advance_transition_key: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Canonical representation of a transition.
#[derive(Debug, Clone, Serialize)]
pub struct CanonicalTransition {
    pub transition_key: String,
    pub display_name: String,
    pub source_node_key: String,
    pub target_node_key: String,
    pub transition_effect: String,
    pub submission_schema: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

/// Compute the stable digest for a workflow definition version.
///
/// # Arguments
/// * `definition_key` - The workflow definition key (stable business identifier)
/// * `version_number` - The version number
/// * `json_schema_dialect` - The JSON Schema dialect
/// * `validator_version` - The validator version
/// * `context_schema` - The context JSON Schema
/// * `nodes` - All node definitions
/// * `transitions` - All transition definitions
/// * `node_key_by_id` - Map from node_id to node_key for resolving references
///
/// # Returns
/// SHA-256 hex digest string (64 lowercase hex characters).
#[allow(clippy::too_many_arguments)]
pub fn compute_digest(
    definition_key: &str,
    version_number: i32,
    json_schema_dialect: Option<&str>,
    validator_version: Option<&str>,
    context_schema: Option<&serde_json::Value>,
    nodes: &[NodeDefinition],
    transitions: &[TransitionDefinition],
    node_key_by_id: &std::collections::HashMap<crate::domain::ids::NodeId, String>,
    transition_key_by_id: &std::collections::HashMap<crate::domain::ids::TransitionId, String>,
) -> Result<String, super::error::DefinitionError> {
    // Build sorted canonical nodes
    let mut canonical_nodes: Vec<CanonicalNode> = nodes
        .iter()
        .map(|n| {
            let primary_key = n
                .primary_advance_transition_id
                .and_then(|tid| transition_key_by_id.get(&tid).cloned());

            let fixed_id = n
                .assignee_ref
                .as_ref()
                .and_then(|reference| reference.fixed_principal_id)
                .map(|pid| pid.to_string());

            CanonicalNode {
                node_key: n.node_key.clone(),
                display_name: n.display_name.clone(),
                order_index: n.order_index,
                node_type: n.node_type.to_string(),
                assignee_ref_type: n
                    .assignee_ref
                    .as_ref()
                    .map(|reference| reference.ref_type.to_string()),
                fixed_principal_id: fixed_id,
                instructions: n.instructions.clone(),
                primary_advance_transition_key: primary_key,
                metadata: n.metadata.clone(),
            }
        })
        .collect();

    // Sort nodes by node_key for deterministic order
    canonical_nodes.sort_by(|a, b| a.node_key.cmp(&b.node_key));

    // Build sorted canonical transitions
    let mut canonical_transitions: Vec<CanonicalTransition> = transitions
        .iter()
        .map(|t| {
            let source_key = node_key_by_id
                .get(&t.source_node_id)
                .cloned()
                .unwrap_or_default();
            let target_key = node_key_by_id
                .get(&t.target_node_id)
                .cloned()
                .unwrap_or_default();

            CanonicalTransition {
                transition_key: t.transition_key.clone(),
                display_name: t.display_name.clone(),
                source_node_key: source_key,
                target_node_key: target_key,
                transition_effect: t.transition_effect.to_string(),
                submission_schema: t.submission_schema.clone(),
                metadata: t.metadata.clone(),
            }
        })
        .collect();

    // Sort transitions by transition_key for deterministic order
    canonical_transitions.sort_by(|a, b| a.transition_key.cmp(&b.transition_key));

    let doc = CanonicalDefinitionDocument {
        definition_key: definition_key.to_string(),
        version_number,
        json_schema_dialect: json_schema_dialect.map(|s| s.to_string()),
        validator_version: validator_version.map(|s| s.to_string()),
        context_schema: context_schema.cloned(),
        nodes: canonical_nodes,
        transitions: canonical_transitions,
    };

    // Serialize to JSON then JCS-canonicalize with SHA-256
    // sha256_jcs_hex serializes the value to JSON, JCS-canonicalizes it,
    // and computes SHA-256 in one step.
    let digest = jcs_canonicalize::sha256_jcs_hex(&doc).map_err(|e| {
        super::error::DefinitionError::DigestFailure(format!("JCS canonicalization failed: {}", e))
    })?;

    Ok(digest)
}

/// Compute the JCS + SHA-256 digest of an arbitrary JSON value.
///
/// Used for computing digests of context payloads, event data, response bodies, etc.
/// Returns a 64-character lowercase hex SHA-256 string.
pub fn compute_json_digest(value: &serde_json::Value) -> Result<String, String> {
    jcs_canonicalize::sha256_jcs_hex(value)
        .map_err(|e| format!("JCS canonicalization failed: {}", e))
}

/// Compute SHA-256 hex digest of raw bytes.
///
/// Used for deterministic digest of simple byte content.
pub fn compute_sha256(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
