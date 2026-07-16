use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::application::workflow_instance::query_types::WorkflowQueryError;

use super::query_rows::QueryBaseRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryVisibility {
    DomainOwnerFull,
    CurrentAssigneeFull,
    CreatorDraftFull,
    HistoricalParticipantRestricted,
}

impl QueryVisibility {
    pub(crate) fn is_full(self) -> bool {
        !matches!(self, Self::HistoricalParticipantRestricted)
    }
}

pub(crate) struct AuthorizedSnapshot<'a> {
    pub tx: Transaction<'a, Postgres>,
    pub base: QueryBaseRow,
    pub visibility: QueryVisibility,
}

fn storage(error: sqlx::Error) -> WorkflowQueryError {
    WorkflowQueryError::StorageError(error.to_string())
}

pub(crate) async fn begin_snapshot(
    pool: &PgPool,
) -> Result<Transaction<'_, Postgres>, WorkflowQueryError> {
    let mut tx = pool.begin().await.map_err(storage)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
    Ok(tx)
}

async fn audit_security(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    action: &str,
    instance_id: Option<Uuid>,
    query_type: &str,
    reason: &str,
    domain_id: Option<Uuid>,
) -> Result<(), WorkflowQueryError> {
    let details = serde_json::json!({
        "queryType": query_type,
        "reason": reason,
        "domainId": domain_id.map(|value| value.to_string()),
    });
    sqlx::query(
        "INSERT INTO workflow_security_audits
         (audit_id, principal_id, action, resource_type, resource_id, details)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(actor)
    .bind(action)
    .bind(instance_id.map(|_| "WORKFLOW_INSTANCE"))
    .bind(instance_id.map(|id| id.to_string()))
    .bind(details)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

async fn actor_state(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
) -> Result<Option<bool>, WorkflowQueryError> {
    sqlx::query_scalar("SELECT enabled FROM principals WHERE principal_id = $1")
        .bind(actor)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)
}

pub(crate) async fn actor_snapshot<'a>(
    pool: &'a PgPool,
    actor: Uuid,
    instance_id: Option<Uuid>,
    query_type: &str,
) -> Result<Transaction<'a, Postgres>, WorkflowQueryError> {
    let mut tx = begin_snapshot(pool).await?;
    match actor_state(&mut tx, actor).await? {
        None => Err(WorkflowQueryError::PrincipalNotFound),
        Some(true) => Ok(tx),
        Some(false) => {
            audit_security(
                &mut tx,
                actor,
                "DISABLED_PRINCIPAL_READ_ATTEMPT",
                instance_id,
                query_type,
                "PRINCIPAL_DISABLED",
                None,
            )
            .await?;
            tx.commit().await.map_err(storage)?;
            Err(WorkflowQueryError::PrincipalDisabled)
        }
    }
}

pub(crate) async fn load_base(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: Uuid,
) -> Result<Option<QueryBaseRow>, WorkflowQueryError> {
    sqlx::query_as::<_, QueryBaseRow>(
        "SELECT wi.workflow_instance_id, wi.domain_id,
                wd.domain_id AS definition_domain_id, wi.definition_version_id,
                wdv.version_status::text AS definition_version_status,
                wi.created_by_principal_id, wi.current_context_revision_id,
                wi.current_node_visit_id, wi.workflow_state_version,
                wi.external_reference, wi.external_url, wi.metadata,
                wi.created_at AS instance_created_at, d.enabled AS domain_enabled,
                cr.workflow_instance_id AS context_instance_id,
                cr.revision_number AS context_revision_number,
                cr.previous_revision_id AS context_previous_revision_id,
                cr.payload AS context_payload, cr.payload_digest AS context_payload_digest,
                cr.created_by_principal_id AS context_created_by_principal_id,
                cr.created_at AS context_created_at,
                nv.workflow_instance_id AS visit_instance_id,
                nv.node_id AS current_node_id, nv.visit_number,
                nv.assignee_principal_id AS current_assignee_principal_id,
                nv.entered_by_transition_id, nv.created_at AS visit_created_at,
                nd.definition_version_id AS node_definition_version_id,
                nd.node_key AS current_node_key, nd.display_name AS current_node_display_name,
                nd.node_type::text AS current_node_type,
                nd.instructions AS current_node_instructions,
                nd.primary_advance_transition_id AS current_primary_advance_transition_id,
                COALESCE(es.event_count, 0) AS event_count,
                es.min_event_sequence, es.max_event_sequence,
                es.event_references_consistent
         FROM workflow_instances wi
         JOIN domains d ON d.domain_id = wi.domain_id
         JOIN workflow_definition_versions wdv
           ON wdv.definition_version_id = wi.definition_version_id
         JOIN workflow_definitions wd
           ON wd.workflow_definition_id = wdv.workflow_definition_id
         LEFT JOIN workflow_context_revisions cr
           ON cr.context_revision_id = wi.current_context_revision_id
         LEFT JOIN workflow_node_visits nv
           ON nv.node_visit_id = wi.current_node_visit_id
         LEFT JOIN workflow_node_definitions nd ON nd.node_id = nv.node_id
         LEFT JOIN LATERAL (
           SELECT COUNT(*) AS event_count, MIN(event_sequence) AS min_event_sequence,
                  MAX(event_sequence) AS max_event_sequence,
                  BOOL_AND(
                    (e.source_node_visit_id IS NULL OR EXISTS (
                      SELECT 1 FROM workflow_node_visits v
                      WHERE v.node_visit_id = e.source_node_visit_id
                        AND v.workflow_instance_id = e.workflow_instance_id))
                    AND (e.target_node_visit_id IS NULL OR EXISTS (
                      SELECT 1 FROM workflow_node_visits v
                      WHERE v.node_visit_id = e.target_node_visit_id
                        AND v.workflow_instance_id = e.workflow_instance_id))
                    AND (e.context_revision_id IS NULL OR EXISTS (
                      SELECT 1 FROM workflow_context_revisions c
                      WHERE c.context_revision_id = e.context_revision_id
                        AND c.workflow_instance_id = e.workflow_instance_id))
                    AND (e.submission_id IS NULL OR EXISTS (
                      SELECT 1 FROM workflow_submissions s
                      WHERE s.submission_id = e.submission_id
                        AND s.workflow_instance_id = e.workflow_instance_id))
                  ) AS event_references_consistent
           FROM workflow_events e WHERE e.workflow_instance_id = wi.workflow_instance_id
         ) es ON TRUE
         WHERE wi.workflow_instance_id = $1",
    )
    .bind(instance_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)
}

async fn classify_visibility(
    tx: &mut Transaction<'_, Postgres>,
    base: &QueryBaseRow,
    actor: Uuid,
) -> Result<Option<QueryVisibility>, WorkflowQueryError> {
    let owner: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM domain_role_bindings
           WHERE domain_id = $1 AND principal_id = $2
             AND role_key = 'DOMAIN_OWNER' AND enabled = TRUE)",
    )
    .bind(base.domain_id)
    .bind(actor)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if owner {
        return Ok(Some(QueryVisibility::DomainOwnerFull));
    }
    if base.current_assignee_principal_id == Some(actor)
        && base.current_node_type.as_deref() != Some("TERMINAL")
        && base.visit_instance_id == Some(base.workflow_instance_id)
        && base.node_definition_version_id == Some(base.definition_version_id)
    {
        return Ok(Some(QueryVisibility::CurrentAssigneeFull));
    }
    if base.created_by_principal_id == actor && base.current_node_type.as_deref() == Some("DRAFT") {
        return Ok(Some(QueryVisibility::CreatorDraftFull));
    }
    let historical: bool = sqlx::query_scalar(
        "SELECT $2 = created_by_principal_id
             OR EXISTS (
               SELECT 1 FROM workflow_node_visits v
               JOIN workflow_node_definitions n ON n.node_id = v.node_id
               WHERE v.workflow_instance_id = $1 AND v.assignee_principal_id = $2
                 AND n.definition_version_id = $3)
             OR EXISTS (
               SELECT 1 FROM workflow_submissions s
               JOIN workflow_node_visits v ON v.node_visit_id = s.source_node_visit_id
                 AND v.workflow_instance_id = s.workflow_instance_id
               JOIN workflow_node_definitions n ON n.node_id = v.node_id
               JOIN workflow_context_revisions c ON c.context_revision_id = s.context_revision_id
                 AND c.workflow_instance_id = s.workflow_instance_id
               JOIN workflow_transition_definitions t ON t.transition_id = s.transition_id
               WHERE s.workflow_instance_id = $1 AND s.author_principal_id = $2
                 AND n.definition_version_id = $3
                 AND t.definition_version_id = $3
                 AND t.source_node_id = v.node_id)
         FROM workflow_instances WHERE workflow_instance_id = $1",
    )
    .bind(base.workflow_instance_id)
    .bind(actor)
    .bind(base.definition_version_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(historical.then_some(QueryVisibility::HistoricalParticipantRestricted))
}

pub(crate) fn validate_base(base: &QueryBaseRow) -> Result<(), WorkflowQueryError> {
    let fail = |detail: &str| WorkflowQueryError::InternalConsistency(detail.to_string());
    if base.domain_id != base.definition_domain_id {
        return Err(fail("instance definition belongs to another domain"));
    }
    let context_id = base
        .current_context_revision_id
        .ok_or_else(|| fail("missing current context pointer"))?;
    if base.context_instance_id != Some(base.workflow_instance_id)
        || base.current_context().is_none()
    {
        return Err(fail("current context pointer does not belong to instance"));
    }
    if base.current_context().map(|item| item.context_revision_id) != Some(context_id) {
        return Err(fail("current context projection is incomplete"));
    }
    let visit_id = base
        .current_node_visit_id
        .ok_or_else(|| fail("missing current visit pointer"))?;
    if base.visit_instance_id != Some(base.workflow_instance_id)
        || base.current_visit(true).is_none()
    {
        return Err(fail("current visit pointer does not belong to instance"));
    }
    if base.current_visit(true).map(|item| item.node_visit_id) != Some(visit_id)
        || base.node_definition_version_id != Some(base.definition_version_id)
    {
        return Err(fail(
            "current visit node does not belong to definition version",
        ));
    }
    if base.workflow_state_version < 1
        || base.event_count != i64::from(base.workflow_state_version)
        || base.min_event_sequence != Some(1)
        || base.max_event_sequence != Some(base.workflow_state_version)
        || base.event_references_consistent != Some(true)
    {
        return Err(fail("event sequence does not match workflow state version"));
    }
    Ok(())
}

pub(crate) async fn validate_all_facts(
    tx: &mut Transaction<'_, Postgres>,
    base: &QueryBaseRow,
) -> Result<(), WorkflowQueryError> {
    let context_consistent: bool = sqlx::query_scalar(
        "SELECT COALESCE((
           SELECT MIN(revision_number) = 1
              AND COUNT(*) = MAX(revision_number)::bigint
              AND MAX(revision_number) = $2
              AND BOOL_AND(
                (revision_number = 1 AND previous_revision_id IS NULL AND prior_id IS NULL)
                OR (revision_number > 1
                    AND previous_revision_id IS NOT DISTINCT FROM prior_id))
           FROM (
             SELECT revision_number, previous_revision_id,
                    LAG(context_revision_id) OVER (ORDER BY revision_number) AS prior_id
             FROM workflow_context_revisions WHERE workflow_instance_id = $1
           ) revisions
         ), FALSE)",
    )
    .bind(base.workflow_instance_id)
    .bind(base.context_revision_number)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;

    let visits_consistent: bool = sqlx::query_scalar(
        "SELECT COALESCE(BOOL_AND(COALESCE(n.definition_version_id = $2, FALSE)), TRUE)
         FROM workflow_node_visits v
         LEFT JOIN workflow_node_definitions n ON n.node_id = v.node_id
         WHERE v.workflow_instance_id = $1",
    )
    .bind(base.workflow_instance_id)
    .bind(base.definition_version_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;

    let submissions_consistent: bool = sqlx::query_scalar(
        "SELECT COALESCE(BOOL_AND(COALESCE(
           v.workflow_instance_id = s.workflow_instance_id
           AND n.definition_version_id = $2
           AND c.workflow_instance_id = s.workflow_instance_id
           AND t.definition_version_id = $2
           AND t.source_node_id = v.node_id, FALSE)), TRUE)
         FROM workflow_submissions s
         LEFT JOIN workflow_node_visits v ON v.node_visit_id = s.source_node_visit_id
         LEFT JOIN workflow_node_definitions n ON n.node_id = v.node_id
         LEFT JOIN workflow_context_revisions c ON c.context_revision_id = s.context_revision_id
         LEFT JOIN workflow_transition_definitions t ON t.transition_id = s.transition_id
         WHERE s.workflow_instance_id = $1",
    )
    .bind(base.workflow_instance_id)
    .bind(base.definition_version_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;

    if !context_consistent {
        return Err(WorkflowQueryError::InternalConsistency(
            "context revision chain is not contiguous or current pointer is stale".to_string(),
        ));
    }
    if !visits_consistent {
        return Err(WorkflowQueryError::InternalConsistency(
            "historical node visit escapes definition version".to_string(),
        ));
    }
    if !submissions_consistent {
        return Err(WorkflowQueryError::InternalConsistency(
            "historical submission relationship escapes instance or definition version".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn authorized_snapshot<'a>(
    pool: &'a PgPool,
    actor: Uuid,
    instance_id: Uuid,
    query_type: &str,
) -> Result<AuthorizedSnapshot<'a>, WorkflowQueryError> {
    let mut tx = actor_snapshot(pool, actor, Some(instance_id), query_type).await?;
    let Some(base) = load_base(&mut tx, instance_id).await? else {
        return Err(WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible);
    };
    let Some(visibility) = classify_visibility(&mut tx, &base, actor).await? else {
        audit_security(
            &mut tx,
            actor,
            "UNAUTHORIZED_WORKFLOW_READ",
            Some(instance_id),
            query_type,
            "NO_VISIBILITY",
            Some(base.domain_id),
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        return Err(WorkflowQueryError::WorkflowInstanceNotFoundOrNotVisible);
    };
    validate_base(&base)?;
    validate_all_facts(&mut tx, &base).await?;
    Ok(AuthorizedSnapshot {
        tx,
        base,
        visibility,
    })
}

pub(crate) async fn reject_restricted_history<T>(
    mut snapshot: AuthorizedSnapshot<'_>,
    actor: Uuid,
    query_type: &str,
) -> Result<T, WorkflowQueryError> {
    audit_security(
        &mut snapshot.tx,
        actor,
        "UNAUTHORIZED_WORKFLOW_READ",
        Some(snapshot.base.workflow_instance_id),
        query_type,
        "RESTRICTED_SCOPE",
        Some(snapshot.base.domain_id),
    )
    .await?;
    snapshot.tx.commit().await.map_err(storage)?;
    Err(WorkflowQueryError::RestrictedHistoryNotVisible)
}

pub(crate) fn map_storage(error: sqlx::Error) -> WorkflowQueryError {
    storage(error)
}
