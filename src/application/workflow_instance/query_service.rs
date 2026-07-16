//! Read-only application service over the authoritative PostgreSQL projection and facts.

use sqlx::PgPool;

use super::query_types::*;
use crate::store::postgres::workflow_instance_repository::{query_detail, query_worklists};

#[derive(Clone)]
pub struct WorkflowQueryService {
    pool: PgPool,
}

impl WorkflowQueryService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get_workflow_instance_detail(
        &self,
        query: GetWorkflowInstanceDetail,
    ) -> Result<WorkflowInstanceDetail, WorkflowQueryError> {
        query_detail::get_workflow_instance_detail(&self.pool, query).await
    }

    pub async fn list_workflow_timeline(
        &self,
        query: ListWorkflowTimeline,
    ) -> Result<Page<WorkflowEventItem, i32>, WorkflowQueryError> {
        query_detail::list_workflow_timeline(&self.pool, query).await
    }

    pub async fn list_context_revisions(
        &self,
        query: ListContextRevisions,
    ) -> Result<Page<ContextRevisionItem, i32>, WorkflowQueryError> {
        query_detail::list_context_revisions(&self.pool, query).await
    }

    pub async fn list_node_visits(
        &self,
        query: ListNodeVisits,
    ) -> Result<Page<NodeVisitItem>, WorkflowQueryError> {
        query_detail::list_node_visits(&self.pool, query).await
    }

    pub async fn list_submission_history(
        &self,
        query: ListSubmissionHistory,
    ) -> Result<Page<SubmissionHistoryItem>, WorkflowQueryError> {
        query_detail::list_submission_history(&self.pool, query).await
    }

    pub async fn list_assigned_to_me(
        &self,
        query: ListAssignedToMe,
    ) -> Result<Page<AssignedWorkItem>, WorkflowQueryError> {
        query_worklists::list_assigned_to_me(&self.pool, query).await
    }

    pub async fn list_creator_owned_drafts(
        &self,
        query: ListCreatorOwnedDrafts,
    ) -> Result<Page<CreatorDraftItem>, WorkflowQueryError> {
        query_worklists::list_creator_owned_drafts(&self.pool, query).await
    }
}
