use std::collections::{HashMap, HashSet};

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::domain::definition::digest;
use crate::domain::workflow_instance::recovery::{
    BeforeSnapshotV1, RecoveryError, WorkflowProjection,
};

use super::event_replay;
use super::import_event;
use super::rows::{ContextFact, EventFact, InstanceRow, SubmissionFact, TransitionFact, VisitFact};

fn storage(error: sqlx::Error) -> RecoveryError {
    RecoveryError::StorageError(error.to_string())
}

pub(super) async fn lock_instance(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: Uuid,
) -> Result<InstanceRow, RecoveryError> {
    sqlx::query_as(
        "SELECT i.workflow_instance_id, i.domain_id, i.definition_version_id,
                i.created_by_principal_id, p.principal_type::text AS created_by_principal_type,
                i.external_reference, i.current_context_revision_id,
                i.current_node_visit_id, i.workflow_state_version
         FROM workflow_instances i JOIN principals p
           ON p.principal_id = i.created_by_principal_id
         WHERE i.workflow_instance_id = $1 FOR UPDATE OF i",
    )
    .bind(instance_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(RecoveryError::InstanceNotFound)
}

pub(super) fn before_snapshot(instance: &InstanceRow) -> BeforeSnapshotV1 {
    BeforeSnapshotV1::new(
        instance.workflow_instance_id,
        instance.domain_id,
        instance.definition_version_id,
        instance.created_by_principal_id,
        &instance.projection(),
    )
}

pub(super) fn verify_expected_digest(
    expected: Option<&str>,
    actual: &str,
) -> Result<(), RecoveryError> {
    if let Some(expected) = expected {
        if expected.len() != 64
            || !expected
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RecoveryError::InvalidInput(
                "expected_before_snapshot_digest must be lowercase SHA-256 hex".to_string(),
            ));
        }
        if expected != actual {
            return Err(RecoveryError::BeforeSnapshotDigestMismatch {
                expected: expected.to_string(),
                actual: actual.to_string(),
            });
        }
    }
    Ok(())
}

pub(super) async fn reconstruct_projection(
    tx: &mut Transaction<'_, Postgres>,
    instance: &InstanceRow,
) -> Result<WorkflowProjection, RecoveryError> {
    let contexts = load_contexts(tx, instance.workflow_instance_id).await?;
    let visits = load_visits(tx, instance.workflow_instance_id).await?;
    let submissions = load_submissions(tx, instance.workflow_instance_id).await?;
    let transitions = load_transitions(tx, instance.definition_version_id).await?;
    let events = load_events(tx, instance.workflow_instance_id).await?;
    let definition_digest: Option<String> = sqlx::query_scalar(
        "SELECT definition_digest FROM workflow_definition_versions
         WHERE definition_version_id = $1",
    )
    .bind(instance.definition_version_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .flatten();
    validate_contexts(instance, &contexts)?;
    validate_visits(instance, &visits, &transitions)?;
    validate_submissions(instance, &contexts, &visits, &submissions, &transitions)?;
    // An imported instance must additionally prove its initial event is anchored
    // to exactly one completed import receipt before replay (audit High 2).
    if let Some(initial) = events.first() {
        if initial.event_type == "WORKFLOW_INSTANCE_IMPORTED" {
            let context = contexts
                .iter()
                .find(|fact| Some(fact.context_revision_id) == initial.context_revision_id)
                .ok_or_else(|| invalid("imported initial event references a missing context"))?;
            let visit = visits
                .iter()
                .find(|fact| Some(fact.node_visit_id) == initial.target_node_visit_id)
                .ok_or_else(|| invalid("imported initial event references a missing visit"))?;
            import_event::validate_receipt_linkage(tx, initial, instance, context, visit).await?;
        }
    }
    event_replay::replay(
        instance,
        definition_digest.as_deref(),
        &contexts,
        &visits,
        &submissions,
        &transitions,
        &events,
    )
}

async fn load_contexts(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: Uuid,
) -> Result<Vec<ContextFact>, RecoveryError> {
    sqlx::query_as(
        "SELECT context_revision_id, workflow_instance_id, revision_number,
                previous_revision_id, payload, payload_digest, created_by_principal_id
         FROM workflow_context_revisions WHERE workflow_instance_id = $1
         ORDER BY revision_number",
    )
    .bind(instance_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)
}

async fn load_visits(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: Uuid,
) -> Result<Vec<VisitFact>, RecoveryError> {
    sqlx::query_as(
        "SELECT v.node_visit_id, v.workflow_instance_id, v.node_id, v.visit_number,
                v.assignee_principal_id, v.entered_by_transition_id,
                n.definition_version_id, n.node_type::text,
                n.assignee_ref_type::text
         FROM workflow_node_visits v
         JOIN workflow_node_definitions n ON n.node_id = v.node_id
         WHERE v.workflow_instance_id = $1
         ORDER BY v.created_at, v.node_visit_id",
    )
    .bind(instance_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)
}

async fn load_submissions(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: Uuid,
) -> Result<Vec<SubmissionFact>, RecoveryError> {
    sqlx::query_as(
        "SELECT s.submission_id, s.workflow_instance_id, s.source_node_visit_id,
                s.context_revision_id, s.transition_id, s.payload, s.payload_digest
         FROM workflow_submissions s
         WHERE s.workflow_instance_id = $1 ORDER BY s.created_at, s.submission_id",
    )
    .bind(instance_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)
}

async fn load_transitions(
    tx: &mut Transaction<'_, Postgres>,
    definition_version_id: Uuid,
) -> Result<Vec<TransitionFact>, RecoveryError> {
    sqlx::query_as(
        "SELECT transition_id, definition_version_id, transition_key,
                source_node_id, target_node_id, transition_effect::text
         FROM workflow_transition_definitions WHERE definition_version_id = $1",
    )
    .bind(definition_version_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)
}

async fn load_events(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: Uuid,
) -> Result<Vec<EventFact>, RecoveryError> {
    sqlx::query_as(
        "SELECT e.event_id, e.workflow_instance_id, e.event_sequence, e.event_schema_version, e.event_type,
                e.transition_effect::text, e.source_node_visit_id, e.target_node_visit_id,
                e.context_revision_id, e.submission_id, e.event_data, e.event_data_digest,
                e.command_id, e.actor_principal_id, p.principal_type::text AS actor_principal_type,
                e.from_node_id, e.to_node_id, e.old_workflow_state_version,
                e.new_workflow_state_version
         FROM workflow_events e JOIN principals p ON p.principal_id = e.actor_principal_id
         WHERE e.workflow_instance_id = $1 ORDER BY e.event_sequence",
    )
    .bind(instance_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)
}

fn invalid(detail: impl Into<String>) -> RecoveryError {
    RecoveryError::InvalidImmutableFacts(detail.into())
}

fn validate_contexts(instance: &InstanceRow, facts: &[ContextFact]) -> Result<(), RecoveryError> {
    if facts.is_empty() {
        return Err(invalid("context fact chain is empty"));
    }
    for (index, fact) in facts.iter().enumerate() {
        let expected_number = index as i32 + 1;
        if fact.workflow_instance_id != instance.workflow_instance_id
            || fact.revision_number != expected_number
            || (index == 0 && fact.previous_revision_id.is_some())
            || (index > 0
                && fact.previous_revision_id != Some(facts[index - 1].context_revision_id))
        {
            return Err(invalid("context revision chain is not contiguous"));
        }
        let actual =
            digest::compute_json_digest(&fact.payload).map_err(RecoveryError::StorageError)?;
        if actual != fact.payload_digest {
            return Err(invalid("context payload digest mismatch"));
        }
    }
    Ok(())
}

fn validate_visits(
    instance: &InstanceRow,
    facts: &[VisitFact],
    transitions: &[TransitionFact],
) -> Result<(), RecoveryError> {
    if facts.is_empty() {
        return Err(invalid("node visit fact set is empty"));
    }
    for visit in facts {
        if visit.workflow_instance_id != instance.workflow_instance_id
            || visit.definition_version_id != instance.definition_version_id
            || visit.visit_number < 1
        {
            return Err(invalid("node visit escapes instance definition"));
        }
        if visit.node_type != "TERMINAL" && visit.assignee_principal_id.is_none() {
            return Err(invalid("non-terminal node visit has no assignee"));
        }
        if let Some(transition_id) = visit.entered_by_transition_id {
            let valid = transitions.iter().any(|transition| {
                transition.transition_id == transition_id
                    && transition.definition_version_id == instance.definition_version_id
                    && transition.target_node_id == visit.node_id
            });
            if !valid {
                return Err(invalid(
                    "node visit entered transition relationship is invalid",
                ));
            }
        }
    }
    Ok(())
}

fn validate_submissions(
    instance: &InstanceRow,
    contexts: &[ContextFact],
    visits: &[VisitFact],
    facts: &[SubmissionFact],
    transitions: &[TransitionFact],
) -> Result<(), RecoveryError> {
    let context_ids: HashSet<_> = contexts
        .iter()
        .map(|fact| fact.context_revision_id)
        .collect();
    let visit_by_id: HashMap<_, _> = visits
        .iter()
        .map(|fact| (fact.node_visit_id, fact))
        .collect();
    for fact in facts {
        let source = visit_by_id.get(&fact.source_node_visit_id);
        let transition = transitions
            .iter()
            .find(|transition| transition.transition_id == fact.transition_id);
        if fact.workflow_instance_id != instance.workflow_instance_id
            || !context_ids.contains(&fact.context_revision_id)
            || transition.map(|value| value.definition_version_id)
                != Some(instance.definition_version_id)
            || source.map(|visit| visit.node_id) != transition.map(|value| value.source_node_id)
        {
            return Err(invalid("submission relationship is invalid"));
        }
        let actual =
            digest::compute_json_digest(&fact.payload).map_err(RecoveryError::StorageError)?;
        if actual != fact.payload_digest {
            return Err(invalid("submission payload digest mismatch"));
        }
    }
    Ok(())
}
