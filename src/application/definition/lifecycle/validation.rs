//! Validation operations for workflow definition versions.
//!
//! Handles:
//! - ValidateDraftVersion application flow
//! - Fixed principal existence and enabled checks
//! - JSON Schema validation (context_schema, submission_schema)
//! - External reference rejection ($ref, $dynamicRef, $recursiveRef)

use crate::domain::definition::error::{DefinitionError, GraphValidationError};
use crate::domain::definition::graph;
use crate::domain::definition::model::{NodeDefinition, ValidationResult, WorkflowGraph};
use crate::domain::enums::{AssigneeRefType, DefinitionVersionStatus};

use super::super::commands::ValidateDraftVersion;
use super::super::repository::DefinitionRepository;
use super::super::service::DefinitionService;

impl<R: DefinitionRepository> DefinitionService<R> {
    /// Validate a DRAFT version without changing state.
    pub async fn validate_draft_version(
        &self,
        cmd: ValidateDraftVersion,
    ) -> Result<ValidationResult, DefinitionError> {
        self.ensure_principal_enabled(cmd.actor_principal_id)
            .await?;

        let version = self.repo.lock_version(cmd.definition_version_id).await?;
        if version.version_status != DefinitionVersionStatus::DRAFT {
            return Err(DefinitionError::VersionNotDraft);
        }

        // H-5 + M-4: Domain authorization and enabled check
        let domain_id = self
            .repo
            .get_definition_domain(version.workflow_definition_id.into_uuid())
            .await?;
        self.ensure_domain_enabled(domain_id).await?;
        self.ensure_domain_owner(cmd.actor_principal_id, domain_id)
            .await?;

        let (nodes, transitions) = self
            .repo
            .get_complete_graph(cmd.definition_version_id)
            .await?;

        let graph = WorkflowGraph {
            nodes,
            transitions,
            context_schema: version.context_schema.clone(),
        };

        let mut result = graph::validate_graph(&graph);

        // Also validate JSON schemas
        let schema_errors = self.validate_json_schemas(&graph).await;
        result.errors.extend(schema_errors);
        result.valid = result.errors.is_empty();

        Ok(result)
    }

    pub(super) async fn validate_fixed_principals(
        &self,
        nodes: &[NodeDefinition],
    ) -> Result<(), DefinitionError> {
        for node in nodes {
            if let Some(assignee_ref) = &node.assignee_ref {
                if assignee_ref.ref_type == AssigneeRefType::FixedPrincipal {
                    if let Some(fixed_id) = assignee_ref.fixed_principal_id {
                        let exists = self
                            .repo
                            .check_principal_exists(fixed_id.into_uuid())
                            .await?;
                        if !exists {
                            return Err(DefinitionError::FixedPrincipalInvalid(format!(
                                "fixed principal {} for node '{}' not found",
                                fixed_id, node.node_key
                            )));
                        }

                        let enabled = self
                            .repo
                            .check_principal_enabled(fixed_id.into_uuid())
                            .await?;
                        if !enabled {
                            return Err(DefinitionError::FixedPrincipalInvalid(format!(
                                "fixed principal {} for node '{}' is disabled",
                                fixed_id, node.node_key
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn validate_json_schemas(
        &self,
        graph: &WorkflowGraph,
    ) -> Vec<GraphValidationError> {
        let mut errors: Vec<GraphValidationError> = Vec::new();

        // Validate context_schema
        if let Some(schema) = &graph.context_schema {
            if let Err(e) = validate_json_schema(schema) {
                errors.push(GraphValidationError::new(
                    "INVALID_CONTEXT_SCHEMA",
                    format!("context_schema: {}", e),
                ));
            }
        }

        // Validate each transition's submission_schema
        for trans in &graph.transitions {
            if let Some(schema) = &trans.submission_schema {
                if let Err(e) = validate_json_schema(schema) {
                    errors.push(GraphValidationError::new(
                        "INVALID_SUBMISSION_SCHEMA",
                        format!(
                            "transition '{}' submission_schema: {}",
                            trans.transition_key, e
                        ),
                    ));
                }
            }
        }

        errors
    }
}

/// Validate that a JSON value is a valid JSON Schema.
///
/// Performs two checks:
/// 1. Recursively inspects all `$ref`, `$dynamicRef`, and `$recursiveRef` values
///    to reject external references (http://, https://, file://, relative paths).
///    Only local fragment references starting with `#` are allowed.
/// 2. Compiles the schema with `jsonschema::validator_for`, propagating any
///    compilation error (invalid keywords, unresolved local refs, etc.).
fn validate_json_schema(schema: &serde_json::Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err("schema must be a JSON object".to_string());
    }

    // Recursively check for external references
    check_external_refs(schema)?;

    // Actually compile the schema to verify it's structurally valid
    jsonschema::validator_for(schema).map_err(|e| format!("schema failed to compile: {}", e))?;

    Ok(())
}

/// Recursively check a schema for external `$ref`, `$dynamicRef`, `$recursiveRef` values.
///
/// Only local fragment references starting with `#/` or bare `#` are allowed.
/// Rejects:
/// - `http://` / `https://`
/// - `file://`
/// - Relative paths (not starting with `#`)
fn check_external_refs(value: &serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::Object(map) => {
            for ref_key in ["$ref", "$dynamicRef", "$recursiveRef"] {
                if let Some(ref_val) = map.get(ref_key) {
                    if let Some(ref_str) = ref_val.as_str() {
                        if !ref_str.starts_with('#') {
                            return Err(format!(
                                "external {} '{}' is not allowed; only local fragment refs (#/...) are permitted",
                                ref_key, ref_str
                            ));
                        }
                    }
                }
            }
            for val in map.values() {
                check_external_refs(val)?;
            }
            Ok(())
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                check_external_refs(val)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_json_schema_valid_object() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["title"],
            "properties": {
                "title": {"type": "string"}
            }
        });
        assert!(validate_json_schema(&schema).is_ok());
    }

    #[test]
    fn validate_json_schema_invalid_type() {
        let schema = serde_json::json!("string_schema");
        assert!(validate_json_schema(&schema).is_err());
    }

    #[test]
    fn validate_json_schema_valid_with_dialect() {
        let schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object"
        });
        assert!(validate_json_schema(&schema).is_ok());
    }

    #[test]
    fn validate_json_schema_rejects_https_ref() {
        let schema = serde_json::json!({
            "$ref": "https://example.com/schema.json"
        });
        let err = validate_json_schema(&schema).unwrap_err();
        assert!(err.contains("https://"), "got: {}", err);
        assert!(err.contains("external"), "got: {}", err);
    }

    #[test]
    fn validate_json_schema_rejects_file_ref() {
        let schema = serde_json::json!({
            "$ref": "file:///etc/passwd"
        });
        let err = validate_json_schema(&schema).unwrap_err();
        assert!(err.contains("file://"), "got: {}", err);
    }

    #[test]
    fn validate_json_schema_rejects_relative_ref() {
        let schema = serde_json::json!({
            "$ref": "../other/schema.json"
        });
        let err = validate_json_schema(&schema).unwrap_err();
        assert!(err.contains("../other"), "got: {}", err);
    }

    #[test]
    fn validate_json_schema_allows_local_fragment() {
        let schema = serde_json::json!({
            "$defs": {
                "User": {"type": "object"}
            },
            "$ref": "#/$defs/User"
        });
        assert!(
            validate_json_schema(&schema).is_ok(),
            "local fragment should pass"
        );
    }

    #[test]
    fn validate_json_schema_allows_bare_hash() {
        let schema = serde_json::json!({
            "$ref": "#"
        });
        assert!(validate_json_schema(&schema).is_ok(), "bare # should pass");
    }

    #[test]
    fn validate_json_schema_rejects_nested_external_ref() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "user": {"$ref": "https://example.com/user.json"}
            }
        });
        let err = validate_json_schema(&schema).unwrap_err();
        assert!(err.contains("https://"), "got: {}", err);
    }

    #[test]
    fn validate_json_schema_rejects_dynamic_ref_external() {
        let schema = serde_json::json!({
            "$dynamicRef": "https://example.com/dynamic"
        });
        let err = validate_json_schema(&schema).unwrap_err();
        assert!(err.contains("https://"), "got: {}", err);
    }

    #[test]
    fn validate_json_schema_rejects_invalid_keyword_structure() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": "not-an-object"
        });
        let err = validate_json_schema(&schema);
        assert!(err.is_err(), "invalid keyword structure should fail");
    }

    #[test]
    fn check_external_refs_empty_object() {
        let val = serde_json::json!({});
        assert!(check_external_refs(&val).is_ok());
    }

    #[test]
    fn check_external_refs_nested_local_ref() {
        let val = serde_json::json!({
            "properties": {
                "user": {"$ref": "#/$defs/User"}
            },
            "$defs": {
                "User": {"type": "object"}
            }
        });
        assert!(check_external_refs(&val).is_ok());
    }
}
