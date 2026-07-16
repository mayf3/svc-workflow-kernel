use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::workflow_instance::import::{
    CreatorResolution, ImportLegacyWorkflowInstanceCommand, LegacyImportError,
    COMMAND_SCHEMA_VERSION, SNAPSHOT_SCHEMA_VERSION,
};

pub(super) struct ValidatedImport {
    pub creator_id: Uuid,
    pub creator_resolution: CreatorResolution,
    pub assignee_id: Option<Uuid>,
    pub snapshot_digest: String,
}

pub(super) struct ValidatedAccess {
    context_schema: Option<serde_json::Value>,
    node: NodeRow,
    snapshot_digest: String,
}

#[derive(sqlx::FromRow)]
struct DefinitionRow {
    status: String,
    domain_id: Uuid,
    context_schema: Option<serde_json::Value>,
}

#[derive(sqlx::FromRow)]
struct NodeRow {
    node_key: String,
    node_type: String,
    assignee_ref_type: Option<String>,
    fixed_principal_id: Option<Uuid>,
}

fn storage(error: sqlx::Error) -> LegacyImportError {
    LegacyImportError::StorageError(error.to_string())
}

fn has_role_user_map(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("roleUserMap") || has_role_user_map(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(has_role_user_map),
        _ => false,
    }
}

fn valid_lower_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn digest_matches(expected: &str, actual: &str) -> bool {
    expected.len() == actual.len()
        && expected
            .bytes()
            .zip(actual.bytes())
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

fn validate_input_shape(
    command: &ImportLegacyWorkflowInstanceCommand,
) -> Result<String, LegacyImportError> {
    let snapshot = &command.legacy_snapshot;
    if command.command_schema_version != COMMAND_SCHEMA_VERSION {
        return Err(LegacyImportError::InvalidInput(
            "command_schema_version must be v1".to_string(),
        ));
    }
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Err(LegacyImportError::InvalidInput(
            "legacy snapshot schemaVersion is unsupported".to_string(),
        ));
    }
    if command.legacy_record_id != snapshot.requirement_id {
        return Err(LegacyImportError::InvalidInput(
            "legacy_record_id must equal snapshot.requirementId".to_string(),
        ));
    }
    if command
        .legacy_creator_principal_id
        .is_some_and(|id| snapshot.requester_id != Some(id.into_uuid()))
    {
        return Err(LegacyImportError::InvalidInput(
            "legacy creator must equal snapshot.requesterId".to_string(),
        ));
    }
    let bounded = |value: &str, max: usize| {
        !value.is_empty()
            && value.len() <= max
            && !value.chars().any(|character| character.is_control())
    };
    if !bounded(&snapshot.domain_key, 128)
        || !bounded(&snapshot.workflow_id, 256)
        || !bounded(&snapshot.current_step, 128)
        || snapshot.state_version < 0
    {
        return Err(LegacyImportError::InvalidInput(
            "snapshot identity fields are missing or invalid".to_string(),
        ));
    }
    if matches!(
        snapshot.current_step.to_ascii_lowercase().as_str(),
        "unassigned"
            | "waiting"
            | "blocked"
            | "archived"
            | "deleted"
            | "abandoned"
            | "rejected"
            | "in_progress"
    ) {
        return Err(LegacyImportError::InvalidInput(
            "legacy pseudo-state cannot be imported as a workflow node".to_string(),
        ));
    }
    if snapshot.workflow_snapshot.is_null() || has_role_user_map(&snapshot.workflow_snapshot) {
        return Err(LegacyImportError::InvalidInput(
            "workflowSnapshot is missing or contains per-instance roleUserMap".to_string(),
        ));
    }
    if snapshot
        .updated_at
        .chars()
        .any(|character| character.is_control())
    {
        return Err(LegacyImportError::InvalidInput(
            "snapshot.updatedAt contains control characters".to_string(),
        ));
    }
    chrono::DateTime::parse_from_rfc3339(&snapshot.updated_at).map_err(|_| {
        LegacyImportError::InvalidInput("snapshot.updatedAt must be RFC3339".to_string())
    })?;
    if !valid_lower_digest(&command.expected_legacy_snapshot_digest) {
        return Err(LegacyImportError::InvalidInput(
            "expected snapshot digest must be 64 lowercase hex characters".to_string(),
        ));
    }
    let size = |value: &serde_json::Value| {
        serde_json::to_vec(value)
            .map(|bytes| bytes.len())
            .map_err(|error| LegacyImportError::StorageError(error.to_string()))
    };
    if size(&snapshot.workflow_snapshot)? > 1024 * 1024 {
        return Err(LegacyImportError::SizeLimitExceeded(
            "workflowSnapshot exceeds 1 MiB".to_string(),
        ));
    }
    if size(&snapshot.context_payload)? > 1024 * 1024 {
        return Err(LegacyImportError::SizeLimitExceeded(
            "contextPayload exceeds 1 MiB".to_string(),
        ));
    }
    if size(&command.metadata)? > 64 * 1024 {
        return Err(LegacyImportError::SizeLimitExceeded(
            "metadata exceeds 64 KiB".to_string(),
        ));
    }
    if command.external_url.as_ref().is_some_and(|url| {
        url.len() > 2048 || url.is_empty() || url.chars().any(|character| character.is_control())
    }) {
        return Err(LegacyImportError::SizeLimitExceeded(
            "externalUrl exceeds 2048 bytes".to_string(),
        ));
    }
    let actual = snapshot.digest()?;
    if !digest_matches(&command.expected_legacy_snapshot_digest, &actual) {
        return Err(LegacyImportError::SnapshotDigestMismatch {
            expected: command.expected_legacy_snapshot_digest.clone(),
            actual,
        });
    }
    Ok(actual)
}

async fn validate_actor_and_domain(
    tx: &mut Transaction<'_, Postgres>,
    command: &ImportLegacyWorkflowInstanceCommand,
) -> Result<(), LegacyImportError> {
    let actor = command.principal_id.into_uuid();
    let domain: Option<(bool, String)> =
        sqlx::query_as("SELECT enabled, domain_key FROM domains WHERE domain_id = $1 FOR UPDATE")
            .bind(command.domain_id.into_uuid())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?;
    let (enabled, domain_key) = domain.ok_or(LegacyImportError::DomainNotFound)?;
    if !enabled {
        return Err(LegacyImportError::DomainDisabled);
    }
    if domain_key != command.legacy_snapshot.domain_key {
        return Err(LegacyImportError::InvalidInput(
            "snapshot.domainKey does not match target domain".to_string(),
        ));
    }
    let bindings: Vec<(Uuid, Uuid, String, bool)> = sqlx::query_as(
        "SELECT binding_id, principal_id, role_key, enabled
         FROM domain_role_bindings WHERE domain_id = $1
         ORDER BY binding_id FOR UPDATE",
    )
    .bind(command.domain_id.into_uuid())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    let mut principal_ids: Vec<Uuid> = bindings.iter().map(|row| row.1).collect();
    principal_ids.push(actor);
    principal_ids.sort_unstable();
    principal_ids.dedup();
    let principals: Vec<(Uuid, bool, String)> = sqlx::query_as(
        "SELECT principal_id, enabled, principal_type::text FROM principals
         WHERE principal_id = ANY($1) ORDER BY principal_id FOR UPDATE",
    )
    .bind(&principal_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    let actor_row = principals
        .iter()
        .find(|row| row.0 == actor)
        .ok_or(LegacyImportError::PrincipalNotFound)?;
    if !actor_row.1 {
        return Err(LegacyImportError::PrincipalDisabled);
    }
    if actor_row.2 != "SERVICE" {
        return Err(LegacyImportError::PrincipalTypeNotAllowed);
    }
    let migration: Vec<_> = bindings
        .iter()
        .filter(|row| row.2 == "WORKFLOW_MIGRATION" && row.3)
        .collect();
    if migration.len() != 1 || migration[0].1 != actor {
        return Err(LegacyImportError::MigrationBindingInvalid);
    }
    Ok(())
}

async fn read_definition_and_node(
    tx: &mut Transaction<'_, Postgres>,
    command: &ImportLegacyWorkflowInstanceCommand,
) -> Result<(DefinitionRow, NodeRow), LegacyImportError> {
    let definition: DefinitionRow = sqlx::query_as(
        "SELECT v.version_status::text AS status, d.domain_id, v.context_schema
         FROM workflow_definition_versions v JOIN workflow_definitions d
           ON d.workflow_definition_id = v.workflow_definition_id
         WHERE v.definition_version_id = $1 FOR UPDATE OF v",
    )
    .bind(command.definition_version_id.into_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(LegacyImportError::DefinitionVersionNotFound)?;
    if definition.domain_id != command.domain_id.into_uuid() {
        return Err(LegacyImportError::PermissionDenied);
    }
    if definition.status != "PUBLISHED" {
        return Err(LegacyImportError::VersionNotPublished);
    }
    let node: NodeRow = sqlx::query_as(
        "SELECT node_key, node_type::text, assignee_ref_type::text, fixed_principal_id
         FROM workflow_node_definitions WHERE node_id = $1 AND definition_version_id = $2
         FOR SHARE",
    )
    .bind(command.imported_node_id.into_uuid())
    .bind(command.definition_version_id.into_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(LegacyImportError::ImportedNodeNotFound)?;
    if node.node_key != command.legacy_snapshot.current_step {
        return Err(LegacyImportError::InvalidInput(
            "snapshot.currentStep does not match imported node".to_string(),
        ));
    }
    Ok((definition, node))
}

async fn enabled_owner(
    tx: &mut Transaction<'_, Postgres>,
    domain_id: Uuid,
) -> Result<Uuid, LegacyImportError> {
    let owners: Vec<(Uuid, bool, String)> = sqlx::query_as(
        "SELECT b.principal_id, p.enabled, p.principal_type::text
         FROM domain_role_bindings b JOIN principals p ON p.principal_id = b.principal_id
         WHERE b.domain_id = $1 AND b.role_key = 'DOMAIN_OWNER' AND b.enabled = TRUE
         FOR SHARE OF b, p",
    )
    .bind(domain_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    match owners.as_slice() {
        [(id, true, kind)] if kind == "HUMAN" || kind == "AGENT" => Ok(*id),
        _ => Err(LegacyImportError::CreatorResolutionFailed(
            "exactly one enabled HUMAN/AGENT DOMAIN_OWNER is required".to_string(),
        )),
    }
}

async fn resolve_creator(
    tx: &mut Transaction<'_, Postgres>,
    command: &ImportLegacyWorkflowInstanceCommand,
) -> Result<(Uuid, CreatorResolution), LegacyImportError> {
    if let Some(candidate) = command.legacy_creator_principal_id {
        let row: Option<(bool, String)> = sqlx::query_as(
            "SELECT enabled, principal_type::text FROM principals WHERE principal_id = $1 FOR SHARE",
        )
        .bind(candidate.into_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?;
        if row.is_some_and(|(enabled, kind)| enabled && (kind == "HUMAN" || kind == "AGENT")) {
            return Ok((candidate.into_uuid(), CreatorResolution::LegacyCreator));
        }
    }
    Ok((
        enabled_owner(tx, command.domain_id.into_uuid()).await?,
        CreatorResolution::DomainOwnerFallback,
    ))
}

async fn validate_enabled_principal(
    tx: &mut Transaction<'_, Postgres>,
    principal_id: Uuid,
) -> Result<Uuid, LegacyImportError> {
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM principals WHERE principal_id = $1 FOR SHARE")
            .bind(principal_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?;
    enabled
        .filter(|value| *value)
        .map(|_| principal_id)
        .ok_or_else(|| {
            LegacyImportError::AssigneeResolutionFailed(
                "resolved assignee is unavailable".to_string(),
            )
        })
}

async fn resolve_assignee(
    tx: &mut Transaction<'_, Postgres>,
    command: &ImportLegacyWorkflowInstanceCommand,
    node: &NodeRow,
    creator_id: Uuid,
) -> Result<Option<Uuid>, LegacyImportError> {
    if node.node_type == "TERMINAL" {
        if command.legacy_snapshot.assignee_id.is_some() {
            return Err(LegacyImportError::AssigneeResolutionFailed(
                "terminal import must not have an assignee".to_string(),
            ));
        }
        return Ok(None);
    }
    let expected = match node.assignee_ref_type.as_deref() {
        Some("WORKFLOW_CREATOR") => creator_id,
        Some("DOMAIN_OWNER") => enabled_owner(tx, command.domain_id.into_uuid()).await?,
        Some("FIXED_PRINCIPAL") => node.fixed_principal_id.ok_or_else(|| {
            LegacyImportError::AssigneeResolutionFailed(
                "fixed principal node has no configured principal".to_string(),
            )
        })?,
        _ => {
            return Err(LegacyImportError::AssigneeResolutionFailed(
                "non-terminal node has no valid assignee resolver".to_string(),
            ))
        }
    };
    validate_enabled_principal(tx, expected).await?;
    if command.legacy_snapshot.assignee_id != Some(expected) {
        return Err(LegacyImportError::AssigneeResolutionFailed(
            "normalized assignee does not match definition resolution".to_string(),
        ));
    }
    Ok(Some(expected))
}

pub(super) async fn validate_access(
    tx: &mut Transaction<'_, Postgres>,
    command: &ImportLegacyWorkflowInstanceCommand,
) -> Result<ValidatedAccess, LegacyImportError> {
    let snapshot_digest = validate_input_shape(command)?;
    validate_actor_and_domain(tx, command).await?;
    let (definition, node) = read_definition_and_node(tx, command).await?;
    Ok(ValidatedAccess {
        context_schema: definition.context_schema,
        node,
        snapshot_digest,
    })
}

pub(super) async fn validate_owned(
    tx: &mut Transaction<'_, Postgres>,
    command: &ImportLegacyWorkflowInstanceCommand,
    access: ValidatedAccess,
) -> Result<ValidatedImport, LegacyImportError> {
    if let Some(schema) = access.context_schema {
        jsonschema::validator_for(&schema)
            .map_err(|error| LegacyImportError::ContextValidationFailed(error.to_string()))?
            .validate(&command.legacy_snapshot.context_payload)
            .map_err(|error| LegacyImportError::ContextValidationFailed(error.to_string()))?;
    }
    let (creator_id, creator_resolution) = resolve_creator(tx, command).await?;
    let assignee_id = resolve_assignee(tx, command, &access.node, creator_id).await?;
    Ok(ValidatedImport {
        creator_id,
        creator_resolution,
        assignee_id,
        snapshot_digest: access.snapshot_digest,
    })
}

pub(super) async fn validate_external_reference_absent(
    tx: &mut Transaction<'_, Postgres>,
    external_reference: &str,
) -> Result<(), LegacyImportError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(external_reference)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflow_instances WHERE external_reference = $1)",
    )
    .bind(external_reference)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if exists {
        Err(LegacyImportError::ExternalReferenceConflict)
    } else {
        Ok(())
    }
}
