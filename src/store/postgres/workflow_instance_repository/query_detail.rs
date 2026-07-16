use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::application::workflow_instance::query_types::*;

use super::query_rows::{ContextRow, EventRow, OutgoingRow, QueryBaseRow, SubmissionRow, VisitRow};
use super::query_visibility::{
    authorized_snapshot, map_storage, reject_restricted_history, AuthorizedSnapshot,
};

fn internal(detail: &str) -> WorkflowQueryError {
    WorkflowQueryError::InternalConsistency(detail.to_string())
}

fn page_limit(limit: Option<u32>, default: u32, max: u32) -> Result<usize, WorkflowQueryError> {
    let limit = limit.unwrap_or(default);
    if limit == 0 || limit > max {
        return Err(WorkflowQueryError::InvalidPagination(format!(
            "limit must be between 1 and {max}"
        )));
    }
    Ok(limit as usize)
}

async fn load_outgoing(
    tx: &mut Transaction<'_, Postgres>,
    base: &QueryBaseRow,
    actor: Uuid,
) -> Result<Vec<OutgoingTransitionItem>, WorkflowQueryError> {
    let rows = sqlx::query_as::<_, OutgoingRow>(
        "SELECT t.transition_id, t.transition_key, t.display_name,
                t.transition_effect::text, t.submission_schema,
                t.definition_version_id AS transition_definition_version_id,
                t.source_node_id, target.node_id AS target_node_id,
                target.definition_version_id AS target_definition_version_id,
                target.node_key AS target_node_key,
                target.display_name AS target_display_name,
                target.node_type::text AS target_node_type,
                target.assignee_ref_type::text AS target_assignee_ref_type,
                target.fixed_principal_id AS target_fixed_principal_id
         FROM workflow_transition_definitions t
         JOIN workflow_node_definitions target ON target.node_id = t.target_node_id
         WHERE t.source_node_id = $1
         ORDER BY t.transition_key, t.transition_id",
    )
    .bind(
        base.current_node_id
            .ok_or_else(|| internal("missing current node"))?,
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(map_storage)?;

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        if row.transition_definition_version_id != base.definition_version_id
            || row.target_definition_version_id != base.definition_version_id
            || Some(row.source_node_id) != base.current_node_id
        {
            return Err(internal("outgoing transition escapes definition version"));
        }
        let target_assignee = match row.target_assignee_ref_type.as_deref() {
            None if row.target_node_type == "TERMINAL" => None,
            Some("WORKFLOW_CREATOR") => Some(base.created_by_principal_id),
            Some("DOMAIN_OWNER") => sqlx::query_scalar(
                "SELECT p.principal_id FROM domain_role_bindings b
                     JOIN principals p ON p.principal_id = b.principal_id AND p.enabled = TRUE
                     WHERE b.domain_id = $1 AND b.role_key = 'DOMAIN_OWNER' AND b.enabled = TRUE",
            )
            .bind(base.domain_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(map_storage)?,
            Some("FIXED_PRINCIPAL") => row.target_fixed_principal_id,
            _ => return Err(internal("unknown target assignee reference type")),
        };
        let target_available = if row.target_node_type == "TERMINAL" {
            true
        } else if let Some(target) = target_assignee {
            sqlx::query_scalar::<_, bool>(
                "SELECT COALESCE((SELECT enabled FROM principals WHERE principal_id = $1), FALSE)",
            )
            .bind(target)
            .fetch_one(&mut **tx)
            .await
            .map_err(map_storage)?
        } else {
            false
        };
        let blocked_reason = if base.current_node_type.as_deref() == Some("TERMINAL") {
            Some(TransitionBlockedReason::CurrentNodeTerminal)
        } else if base.definition_version_status == "REVOKED" {
            Some(TransitionBlockedReason::DefinitionVersionRevoked)
        } else if base.definition_version_status == "DRAFT" {
            Some(TransitionBlockedReason::DefinitionVersionDraft)
        } else if row.transition_effect == "ADVANCE"
            && Some(row.transition_id) != base.current_primary_advance_transition_id
        {
            Some(TransitionBlockedReason::AdvanceNotPrimary)
        } else if base.current_assignee_principal_id != Some(actor) {
            Some(TransitionBlockedReason::ActorNotCurrentAssignee)
        } else if !target_available {
            Some(TransitionBlockedReason::TargetAssigneeUnavailable)
        } else {
            None
        };
        items.push(OutgoingTransitionItem {
            transition_id: row.transition_id,
            transition_key: row.transition_key,
            display_name: row.display_name,
            transition_effect: row.transition_effect,
            target_node: PublicNodeSummary {
                node_id: row.target_node_id,
                node_key: row.target_node_key,
                display_name: row.target_display_name,
                node_type: row.target_node_type,
            },
            submission_schema: row.submission_schema,
            executable_for_actor: blocked_reason.is_none(),
            blocked_reason,
        });
    }
    Ok(items)
}

pub(crate) async fn build_full(
    tx: &mut Transaction<'_, Postgres>,
    base: &QueryBaseRow,
    actor: Uuid,
) -> Result<FullWorkflowInstanceDetail, WorkflowQueryError> {
    Ok(FullWorkflowInstanceDetail {
        instance: base
            .summary()
            .ok_or_else(|| internal("incomplete instance summary"))?,
        current_context_revision_id: base
            .current_context_revision_id
            .ok_or_else(|| internal("missing current context pointer"))?,
        current_node_visit_id: base
            .current_node_visit_id
            .ok_or_else(|| internal("missing current visit pointer"))?,
        current_context: base
            .current_context()
            .ok_or_else(|| internal("incomplete current context"))?,
        current_visit: base
            .current_visit(true)
            .ok_or_else(|| internal("incomplete current visit"))?,
        outgoing_transitions: load_outgoing(tx, base, actor).await?,
    })
}

pub async fn get_workflow_instance_detail(
    pool: &sqlx::PgPool,
    query: GetWorkflowInstanceDetail,
) -> Result<WorkflowInstanceDetail, WorkflowQueryError> {
    let mut snapshot = authorized_snapshot(
        pool,
        query.actor_principal_id,
        query.workflow_instance_id,
        "GetWorkflowInstanceDetail",
    )
    .await?;
    let detail = if snapshot.visibility.is_full() {
        WorkflowInstanceDetail::Full(Box::new(
            build_full(&mut snapshot.tx, &snapshot.base, query.actor_principal_id).await?,
        ))
    } else {
        WorkflowInstanceDetail::HistoricalParticipant(ParticipantWorkflowInstanceDetail {
            instance: snapshot
                .base
                .participant_summary()
                .ok_or_else(|| internal("incomplete instance summary"))?,
        })
    };
    snapshot.tx.commit().await.map_err(map_storage)?;
    Ok(detail)
}

pub async fn list_workflow_timeline(
    pool: &sqlx::PgPool,
    query: ListWorkflowTimeline,
) -> Result<Page<WorkflowEventItem, i32>, WorkflowQueryError> {
    let mut snapshot = authorized_snapshot(
        pool,
        query.actor_principal_id,
        query.workflow_instance_id,
        "ListWorkflowTimeline",
    )
    .await?;
    let limit = page_limit(query.limit, 50, 100)?;
    if query.after_event_sequence.is_some_and(|value| value < 0) {
        return Err(WorkflowQueryError::InvalidPagination(
            "after_event_sequence must be non-negative".to_string(),
        ));
    }
    let full = snapshot.visibility.is_full();
    let rows = sqlx::query_as::<_, EventRow>(
        "SELECT e.event_id, e.workflow_instance_id, e.event_sequence,
                e.event_schema_version, e.command_id, e.causation_id, e.correlation_id,
                e.event_type, e.transition_effect::text, e.source_node_visit_id,
                e.target_node_visit_id, e.context_revision_id, e.submission_id,
                e.event_data, e.event_data_digest, e.actor_principal_id,
                e.from_node_id, e.to_node_id, e.old_workflow_state_version,
                e.new_workflow_state_version, e.created_at, TRUE AS references_consistent
         FROM workflow_events e
         WHERE e.workflow_instance_id = $1 AND e.event_sequence > $2
           AND ($3 OR EXISTS (
                 SELECT 1 FROM workflow_submissions own
                 WHERE own.submission_id = e.submission_id AND own.author_principal_id = $4)
                OR (e.transition_effect = 'RETURN' AND EXISTS (
                  SELECT 1 FROM workflow_submissions feedback,
                       LATERAL jsonb_array_elements_text(
                         CASE WHEN jsonb_typeof(feedback.payload->'relatedSubmissionIds') = 'array'
                              THEN feedback.payload->'relatedSubmissionIds' ELSE '[]'::jsonb END
                       ) related(value)
                  JOIN workflow_submissions own
                    ON own.submission_id::text = related.value
                   AND own.workflow_instance_id = e.workflow_instance_id
                   AND own.author_principal_id = $4
                  WHERE feedback.submission_id = e.submission_id
                    AND feedback.workflow_instance_id = e.workflow_instance_id))
                OR e.transition_effect = 'TERMINATE'
                OR EXISTS (SELECT 1 FROM workflow_node_visits tv
                           JOIN workflow_node_definitions tn ON tn.node_id = tv.node_id
                           WHERE tv.node_visit_id = e.target_node_visit_id
                             AND tv.workflow_instance_id = e.workflow_instance_id
                             AND tn.node_type = 'TERMINAL'))
         ORDER BY e.event_sequence ASC LIMIT $5",
    )
    .bind(query.workflow_instance_id)
    .bind(query.after_event_sequence.unwrap_or(0))
    .bind(full)
    .bind(query.actor_principal_id)
    .bind((limit + 1) as i64)
    .fetch_all(&mut *snapshot.tx)
    .await
    .map_err(map_storage)?;
    let has_more = rows.len() > limit;
    let items: Vec<_> = rows
        .into_iter()
        .take(limit)
        .map(|mut row| {
            if !full && row.event_type == "ADMIN_EMERGENCY_OVERRIDE_COMMITTED" {
                row.event_data = None;
                row.event_data_digest = None;
            }
            row.into_item()
        })
        .collect();
    let next_cursor = has_more.then(|| items.last().expect("non-empty page").event_sequence);
    snapshot.tx.commit().await.map_err(map_storage)?;
    Ok(Page { items, next_cursor })
}

pub async fn list_context_revisions(
    pool: &sqlx::PgPool,
    query: ListContextRevisions,
) -> Result<Page<ContextRevisionItem, i32>, WorkflowQueryError> {
    let mut snapshot = authorized_snapshot(
        pool,
        query.actor_principal_id,
        query.workflow_instance_id,
        "ListContextRevisions",
    )
    .await?;
    if !snapshot.visibility.is_full() {
        return reject_restricted_history(
            snapshot,
            query.actor_principal_id,
            "ListContextRevisions",
        )
        .await;
    }
    let limit = page_limit(query.limit, 50, 100)?;
    if query.after_revision_number.is_some_and(|value| value < 0) {
        return Err(WorkflowQueryError::InvalidPagination(
            "after_revision_number must be non-negative".to_string(),
        ));
    }
    let rows = sqlx::query_as::<_, ContextRow>(
        "SELECT context_revision_id, workflow_instance_id, revision_number,
                previous_revision_id, payload, payload_digest,
                created_by_principal_id, created_at
         FROM workflow_context_revisions
         WHERE workflow_instance_id = $1 AND revision_number > $2
         ORDER BY revision_number ASC LIMIT $3",
    )
    .bind(query.workflow_instance_id)
    .bind(query.after_revision_number.unwrap_or(0))
    .bind((limit + 1) as i64)
    .fetch_all(&mut *snapshot.tx)
    .await
    .map_err(map_storage)?;
    let has_more = rows.len() > limit;
    let items: Vec<_> = rows
        .into_iter()
        .take(limit)
        .map(ContextRow::into_item)
        .collect();
    let next_cursor = has_more.then(|| items.last().expect("non-empty page").revision_number);
    snapshot.tx.commit().await.map_err(map_storage)?;
    Ok(Page { items, next_cursor })
}

pub async fn list_node_visits(
    pool: &sqlx::PgPool,
    query: ListNodeVisits,
) -> Result<Page<NodeVisitItem>, WorkflowQueryError> {
    let mut snapshot = authorized_snapshot(
        pool,
        query.actor_principal_id,
        query.workflow_instance_id,
        "ListNodeVisits",
    )
    .await?;
    let limit = page_limit(query.limit, 50, 100)?;
    let full = snapshot.visibility.is_full();
    let rows = sqlx::query_as::<_, VisitRow>(
        "SELECT v.node_visit_id, v.workflow_instance_id, v.node_id,
                n.definition_version_id AS node_definition_version_id,
                n.node_key, n.display_name, n.node_type::text, v.visit_number,
                v.assignee_principal_id, v.entered_by_transition_id,
                n.instructions, v.created_at
         FROM workflow_node_visits v JOIN workflow_node_definitions n ON n.node_id = v.node_id
         WHERE v.workflow_instance_id = $1 AND ($2 OR v.assignee_principal_id = $3)
           AND ($4::timestamptz IS NULL OR (v.created_at, v.node_visit_id) > ($4, $5))
         ORDER BY v.created_at ASC, v.node_visit_id ASC LIMIT $6",
    )
    .bind(query.workflow_instance_id)
    .bind(full)
    .bind(query.actor_principal_id)
    .bind(query.after.map(|value| value.created_at))
    .bind(query.after.map(|value| value.id))
    .bind((limit + 1) as i64)
    .fetch_all(&mut *snapshot.tx)
    .await
    .map_err(map_storage)?;
    if rows.iter().any(|row| {
        row.workflow_instance_id != query.workflow_instance_id
            || row.node_definition_version_id != snapshot.base.definition_version_id
    }) {
        return Err(internal(
            "node visit escapes instance or definition version",
        ));
    }
    let has_more = rows.len() > limit;
    let items: Vec<_> = rows
        .into_iter()
        .take(limit)
        .map(|row| row.into_item(full))
        .collect();
    let next_cursor = has_more.then(|| {
        let last = items.last().expect("non-empty page");
        TimeUuidCursor {
            created_at: last.created_at,
            id: last.node_visit_id,
        }
    });
    snapshot.tx.commit().await.map_err(map_storage)?;
    Ok(Page { items, next_cursor })
}

pub(crate) async fn fetch_submissions(
    tx: &mut Transaction<'_, Postgres>,
    base: &QueryBaseRow,
    actor: Uuid,
    full: bool,
    after: Option<TimeUuidCursor>,
    limit: usize,
    descending: bool,
) -> Result<Vec<SubmissionRow>, WorkflowQueryError> {
    let order = if descending { "DESC" } else { "ASC" };
    let comparison = if descending { "<" } else { ">" };
    let sql = format!(
        "SELECT s.submission_id, s.workflow_instance_id, s.source_node_visit_id,
                sv.workflow_instance_id AS source_visit_instance_id, sv.node_id AS source_node_id,
                sn.definition_version_id AS source_node_definition_version_id,
                sn.node_key AS source_node_key, sn.display_name AS source_node_display_name,
                sn.node_type::text AS source_node_type, s.context_revision_id,
                cr.workflow_instance_id AS context_instance_id, s.author_principal_id,
                s.transition_id, t.definition_version_id AS transition_definition_version_id,
                t.transition_effect::text, s.payload, s.payload_digest, s.schema_version, s.created_at
         FROM workflow_submissions s
         JOIN workflow_node_visits sv ON sv.node_visit_id = s.source_node_visit_id
         JOIN workflow_node_definitions sn ON sn.node_id = sv.node_id
         JOIN workflow_context_revisions cr ON cr.context_revision_id = s.context_revision_id
         JOIN workflow_transition_definitions t ON t.transition_id = s.transition_id
         WHERE s.workflow_instance_id = $1 AND ($2 OR s.author_principal_id = $3 OR
           (t.transition_effect = 'RETURN' AND EXISTS (
             SELECT 1 FROM jsonb_array_elements_text(
               CASE WHEN jsonb_typeof(s.payload->'relatedSubmissionIds') = 'array'
                    THEN s.payload->'relatedSubmissionIds' ELSE '[]'::jsonb END
             ) related(value)
             JOIN workflow_submissions own ON own.submission_id::text = related.value
              AND own.workflow_instance_id = s.workflow_instance_id
              AND own.author_principal_id = $3)))
           AND ($4::timestamptz IS NULL OR (s.created_at, s.submission_id) {comparison} ($4, $5))
         ORDER BY s.created_at {order}, s.submission_id {order} LIMIT $6"
    );
    sqlx::query_as::<_, SubmissionRow>(&sql)
        .bind(base.workflow_instance_id)
        .bind(full)
        .bind(actor)
        .bind(after.map(|value| value.created_at))
        .bind(after.map(|value| value.id))
        .bind(limit as i64)
        .fetch_all(&mut **tx)
        .await
        .map_err(map_storage)
}

pub(crate) fn validate_submission_rows(
    rows: &[SubmissionRow],
    base: &QueryBaseRow,
) -> Result<(), WorkflowQueryError> {
    if rows.iter().any(|row| {
        row.workflow_instance_id != base.workflow_instance_id
            || row.source_visit_instance_id != base.workflow_instance_id
            || row.context_instance_id != base.workflow_instance_id
            || row.source_node_definition_version_id != base.definition_version_id
            || row.transition_definition_version_id != base.definition_version_id
    }) {
        return Err(internal(
            "submission relationship escapes instance or definition version",
        ));
    }
    Ok(())
}

pub async fn list_submission_history(
    pool: &sqlx::PgPool,
    query: ListSubmissionHistory,
) -> Result<Page<SubmissionHistoryItem>, WorkflowQueryError> {
    let mut snapshot = authorized_snapshot(
        pool,
        query.actor_principal_id,
        query.workflow_instance_id,
        "ListSubmissionHistory",
    )
    .await?;
    let limit = page_limit(query.limit, 50, 100)?;
    let full = snapshot.visibility.is_full();
    let rows = fetch_submissions(
        &mut snapshot.tx,
        &snapshot.base,
        query.actor_principal_id,
        full,
        query.after,
        limit + 1,
        false,
    )
    .await?;
    validate_submission_rows(&rows, &snapshot.base)?;
    let has_more = rows.len() > limit;
    let items: Vec<_> = rows
        .into_iter()
        .take(limit)
        .map(SubmissionRow::into_item)
        .collect();
    let next_cursor = has_more.then(|| {
        let last = items.last().expect("non-empty page");
        TimeUuidCursor {
            created_at: last.created_at,
            id: last.submission_id,
        }
    });
    snapshot.tx.commit().await.map_err(map_storage)?;
    Ok(Page { items, next_cursor })
}
