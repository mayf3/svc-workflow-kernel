use uuid::Uuid;

use crate::application::workflow_instance::query_types::*;

use super::query_detail::{build_full, fetch_submissions, validate_submission_rows};
use super::query_rows::{EventRow, InstanceCursorRow, SubmissionRow};
use super::query_visibility::{
    actor_snapshot, load_base, map_storage, validate_all_facts, validate_base,
};

fn worklist_limit(limit: Option<u32>, default: u32, max: u32) -> Result<usize, WorkflowQueryError> {
    let limit = limit.unwrap_or(default);
    if limit == 0 || limit > max {
        return Err(WorkflowQueryError::InvalidPagination(format!(
            "limit must be between 1 and {max}"
        )));
    }
    Ok(limit as usize)
}

async fn recent_return_events(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    instance_id: Uuid,
    limit: usize,
) -> Result<Vec<EventRow>, WorkflowQueryError> {
    sqlx::query_as::<_, EventRow>(
        "SELECT e.event_id, e.workflow_instance_id, e.event_sequence,
                e.event_schema_version, e.command_id, e.causation_id, e.correlation_id,
                e.event_type, e.transition_effect::text, e.source_node_visit_id,
                e.target_node_visit_id, e.context_revision_id, e.submission_id,
                e.event_data, e.event_data_digest, e.actor_principal_id,
                e.from_node_id, e.to_node_id, e.old_workflow_state_version,
                e.new_workflow_state_version, e.created_at, TRUE AS references_consistent
         FROM workflow_events e
         WHERE e.workflow_instance_id = $1 AND e.transition_effect = 'RETURN'
         ORDER BY e.event_sequence DESC LIMIT $2",
    )
    .bind(instance_id)
    .bind(limit as i64)
    .fetch_all(&mut **tx)
    .await
    .map_err(map_storage)
}

pub async fn list_assigned_to_me(
    pool: &sqlx::PgPool,
    query: ListAssignedToMe,
) -> Result<Page<AssignedWorkItem>, WorkflowQueryError> {
    let mut tx = actor_snapshot(pool, query.actor_principal_id, None, "ListAssignedToMe").await?;
    let limit = worklist_limit(query.limit, 20, 20)?;
    let candidates = sqlx::query_as::<_, InstanceCursorRow>(
        "SELECT wi.workflow_instance_id, wi.created_at
         FROM workflow_instances wi
         JOIN workflow_node_visits v ON v.node_visit_id = wi.current_node_visit_id
          AND v.workflow_instance_id = wi.workflow_instance_id
         JOIN workflow_node_definitions n ON n.node_id = v.node_id
          AND n.definition_version_id = wi.definition_version_id
         WHERE v.assignee_principal_id = $1 AND n.node_type <> 'TERMINAL'
           AND ($2::timestamptz IS NULL OR (wi.created_at, wi.workflow_instance_id) < ($2, $3))
         ORDER BY wi.created_at DESC, wi.workflow_instance_id DESC LIMIT $4",
    )
    .bind(query.actor_principal_id)
    .bind(query.before.map(|value| value.created_at))
    .bind(query.before.map(|value| value.id))
    .bind((limit + 1) as i64)
    .fetch_all(&mut *tx)
    .await
    .map_err(map_storage)?;
    let has_more = candidates.len() > limit;
    let selected: Vec<_> = candidates.into_iter().take(limit).collect();
    let mut items = Vec::with_capacity(selected.len());
    for candidate in &selected {
        let base = load_base(&mut tx, candidate.workflow_instance_id)
            .await?
            .ok_or_else(|| {
                WorkflowQueryError::InternalConsistency(
                    "assigned worklist instance disappeared inside snapshot".to_string(),
                )
            })?;
        validate_base(&base)?;
        validate_all_facts(&mut tx, &base).await?;
        if base.current_assignee_principal_id != Some(query.actor_principal_id)
            || base.current_node_type.as_deref() == Some("TERMINAL")
        {
            return Err(WorkflowQueryError::InternalConsistency(
                "assigned worklist predicate disagrees with current projection".to_string(),
            ));
        }
        let detail = build_full(&mut tx, &base, query.actor_principal_id).await?;
        let mut submission_rows = fetch_submissions(
            &mut tx,
            &base,
            query.actor_principal_id,
            true,
            None,
            51,
            true,
        )
        .await?;
        validate_submission_rows(&submission_rows, &base)?;
        let submissions_truncated = submission_rows.len() > 50;
        submission_rows.truncate(50);
        let upstream_submissions = submission_rows
            .into_iter()
            .map(SubmissionRow::into_item)
            .collect();
        let mut event_rows = recent_return_events(&mut tx, base.workflow_instance_id, 51).await?;
        let events_truncated = event_rows.len() > 50;
        event_rows.truncate(50);
        let return_feedback_events = event_rows.into_iter().map(EventRow::into_item).collect();
        items.push(AssignedWorkItem {
            detail,
            upstream_submissions,
            return_feedback_events,
            submissions_truncated,
            return_events_truncated: events_truncated,
        });
    }
    let next_cursor = has_more.then(|| {
        let last = selected.last().expect("non-empty page");
        TimeUuidCursor {
            created_at: last.created_at,
            id: last.workflow_instance_id,
        }
    });
    tx.commit().await.map_err(map_storage)?;
    Ok(Page { items, next_cursor })
}

pub async fn list_creator_owned_drafts(
    pool: &sqlx::PgPool,
    query: ListCreatorOwnedDrafts,
) -> Result<Page<CreatorDraftItem>, WorkflowQueryError> {
    let mut tx = actor_snapshot(
        pool,
        query.actor_principal_id,
        None,
        "ListCreatorOwnedDrafts",
    )
    .await?;
    let limit = worklist_limit(query.limit, 20, 50)?;
    let candidates = sqlx::query_as::<_, InstanceCursorRow>(
        "SELECT wi.workflow_instance_id, wi.created_at
         FROM workflow_instances wi
         JOIN workflow_node_visits v ON v.node_visit_id = wi.current_node_visit_id
          AND v.workflow_instance_id = wi.workflow_instance_id
         JOIN workflow_node_definitions n ON n.node_id = v.node_id
          AND n.definition_version_id = wi.definition_version_id
         WHERE wi.created_by_principal_id = $1 AND n.node_type = 'DRAFT'
           AND ($2::timestamptz IS NULL OR (wi.created_at, wi.workflow_instance_id) < ($2, $3))
         ORDER BY wi.created_at DESC, wi.workflow_instance_id DESC LIMIT $4",
    )
    .bind(query.actor_principal_id)
    .bind(query.before.map(|value| value.created_at))
    .bind(query.before.map(|value| value.id))
    .bind((limit + 1) as i64)
    .fetch_all(&mut *tx)
    .await
    .map_err(map_storage)?;
    let has_more = candidates.len() > limit;
    let selected: Vec<_> = candidates.into_iter().take(limit).collect();
    let mut items = Vec::with_capacity(selected.len());
    for candidate in &selected {
        let base = load_base(&mut tx, candidate.workflow_instance_id)
            .await?
            .ok_or_else(|| {
                WorkflowQueryError::InternalConsistency(
                    "creator draft instance disappeared inside snapshot".to_string(),
                )
            })?;
        validate_base(&base)?;
        validate_all_facts(&mut tx, &base).await?;
        if base.created_by_principal_id != query.actor_principal_id
            || base.current_node_type.as_deref() != Some("DRAFT")
        {
            return Err(WorkflowQueryError::InternalConsistency(
                "creator draft predicate disagrees with current projection".to_string(),
            ));
        }
        let detail = build_full(&mut tx, &base, query.actor_principal_id).await?;
        let context_editable = matches!(
            base.definition_version_status.as_str(),
            "PUBLISHED" | "DEPRECATED"
        );
        let combined_executable = context_editable
            && base.current_assignee_principal_id == Some(query.actor_principal_id)
            && detail.outgoing_transitions.iter().any(|transition| {
                transition.transition_effect == "ADVANCE"
                    && Some(transition.transition_id) == base.current_primary_advance_transition_id
                    && transition.executable_for_actor
            });
        items.push(CreatorDraftItem {
            detail,
            context_editable,
            combined_executable,
        });
    }
    let next_cursor = has_more.then(|| {
        let last = selected.last().expect("non-empty page");
        TimeUuidCursor {
            created_at: last.created_at,
            id: last.workflow_instance_id,
        }
    });
    tx.commit().await.map_err(map_storage)?;
    Ok(Page { items, next_cursor })
}
