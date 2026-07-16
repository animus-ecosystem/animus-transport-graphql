//! Workflow queries, mutations, and event subscription.
//!
//! Wire shape: defers to `animus-control-protocol` types so the GraphQL
//! schema and the JSON-RPC wire stay in lockstep (canonical contract).

use animus_control_protocol::types::{
    WorkflowCancelRequest, WorkflowEventsRequest, WorkflowExecuteRequest, WorkflowGetRequest,
    WorkflowListRequest, WorkflowPauseRequest, WorkflowResumeRequest, WorkflowRunRequest,
    WorkflowRunSummary, WorkflowStatus as WireStatus,
};
use async_graphql::{Context, Enum, Object, Result, SimpleObject, Subscription, ID};
use futures_util::stream::{self, Stream};

use super::client_from_ctx;

/// Workflow lifecycle status. Mirrors
/// [`animus_control_protocol::types::WorkflowStatus`].
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum WorkflowStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl From<WireStatus> for WorkflowStatus {
    fn from(s: WireStatus) -> Self {
        match s {
            WireStatus::Pending => WorkflowStatus::Pending,
            WireStatus::Running => WorkflowStatus::Running,
            WireStatus::Paused => WorkflowStatus::Paused,
            WireStatus::Completed => WorkflowStatus::Completed,
            WireStatus::Failed => WorkflowStatus::Failed,
            WireStatus::Cancelled => WorkflowStatus::Cancelled,
        }
    }
}

impl From<WorkflowStatus> for WireStatus {
    fn from(s: WorkflowStatus) -> Self {
        match s {
            WorkflowStatus::Pending => WireStatus::Pending,
            WorkflowStatus::Running => WireStatus::Running,
            WorkflowStatus::Paused => WireStatus::Paused,
            WorkflowStatus::Completed => WireStatus::Completed,
            WorkflowStatus::Failed => WireStatus::Failed,
            WorkflowStatus::Cancelled => WireStatus::Cancelled,
        }
    }
}

/// A workflow run. Mirrors
/// [`animus_control_protocol::types::WorkflowRunSummary`] plus the opaque
/// `detail` blob from `WorkflowRun` (serialized as JSON when present).
#[derive(SimpleObject)]
pub struct Workflow {
    pub id: ID,
    /// Workflow definition name.
    pub definition: String,
    pub status: WorkflowStatus,
    pub subject_id: Option<ID>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Full run detail as a JSON string (phase history, decisions,
    /// checkpoints). Only populated by `workflow`-by-id lookups.
    pub detail: Option<String>,
}

impl From<WorkflowRunSummary> for Workflow {
    fn from(s: WorkflowRunSummary) -> Self {
        Workflow {
            id: ID(s.id),
            definition: s.definition,
            status: s.status.into(),
            subject_id: s.subject_id.map(|id| ID(id.as_str().to_string())),
            started_at: s.started_at.to_rfc3339(),
            finished_at: s.finished_at.map(|t| t.to_rfc3339()),
            detail: None,
        }
    }
}

/// A page of workflow runs plus the total across all pages (for numbered
/// pagination + a run-count header). Backs the `workflowsPage` query.
#[derive(SimpleObject)]
pub struct WorkflowPage {
    /// Runs in this page (most recent first).
    pub items: Vec<Workflow>,
    /// Total matching runs across all pages (respecting the status/type filter).
    pub total: i32,
}

/// Result of starting a workflow. Mirrors
/// [`animus_control_protocol::types::WorkflowRunStart`].
#[derive(SimpleObject, Default)]
pub struct WorkflowRunStart {
    pub workflow_id: ID,
    pub status: Option<WorkflowStatus>,
    pub started_at: String,
}

#[derive(SimpleObject, Default)]
pub struct WorkflowEvent {
    pub workflow_id: ID,
    pub kind: String,
    pub payload: String,
    pub at: String,
}

#[derive(Default)]
pub struct WorkflowQuery;

#[Object]
impl WorkflowQuery {
    /// List workflow runs, optionally filtered by status.
    async fn workflows(
        &self,
        ctx: &Context<'_>,
        status: Option<WorkflowStatus>,
        #[graphql(
            desc = "Maximum number of runs to return (most recent first). Defaults to 200 to keep the list fast — an unbounded request makes the daemon materialize EVERY journaled run (O(n), seconds for thousands of runs). Clamped to 1000."
        )]
        limit: Option<i32>,
    ) -> Result<Vec<Workflow>> {
        let client = client_from_ctx(ctx).await?;
        // A `None`/absent limit takes the daemon's unbounded list path (builds
        // every run). Default to 200 and clamp so the list uses the bounded page
        // path; callers can override up to 1000.
        let effective_limit = limit.filter(|n| *n > 0).unwrap_or(200).min(1000) as u32;
        let request = WorkflowListRequest {
            status: status.map(WireStatus::from),
            cursor: None,
            limit: Some(effective_limit),
            workflow_ref: None,
        };
        let response = client
            .workflow_list(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/list failed: {e}")))?;
        Ok(response.runs.into_iter().map(Workflow::from).collect())
    }

    /// Paginated workflow list with a total count and an optional type filter,
    /// for the numbered-pagination UI. `offset` is a plain row offset (page N =
    /// N * pageSize); `total` in the result lets the client compute page count.
    async fn workflows_page(
        &self,
        ctx: &Context<'_>,
        status: Option<WorkflowStatus>,
        #[graphql(desc = "Filter to a single workflow definition (the run 'type').")] workflow_ref: Option<String>,
        #[graphql(desc = "Page size. Defaults to 50, clamped to 1000.")] limit: Option<i32>,
        #[graphql(desc = "Row offset from the newest run. Defaults to 0.")] offset: Option<i32>,
    ) -> Result<WorkflowPage> {
        let client = client_from_ctx(ctx).await?;
        let effective_limit = limit.filter(|n| *n > 0).unwrap_or(50).min(1000) as u32;
        let effective_offset = offset.filter(|n| *n > 0).unwrap_or(0);
        let request = WorkflowListRequest {
            status: status.map(WireStatus::from),
            cursor: (effective_offset > 0).then(|| effective_offset.to_string()),
            limit: Some(effective_limit),
            workflow_ref: workflow_ref.filter(|s| !s.trim().is_empty()),
        };
        let response = client
            .workflow_list(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/list failed: {e}")))?;
        Ok(WorkflowPage {
            items: response.runs.into_iter().map(Workflow::from).collect(),
            total: response.total.unwrap_or(0) as i32,
        })
    }

    /// Look up a single workflow run by id, including full run detail.
    async fn workflow(&self, ctx: &Context<'_>, id: ID) -> Result<Workflow> {
        let client = client_from_ctx(ctx).await?;
        let run = client
            .workflow_get(WorkflowGetRequest { id: id.to_string() })
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/get failed: {e}")))?;
        let detail = (!run.detail.is_null()).then(|| run.detail.to_string());
        let mut wf = Workflow::from(run.summary);
        wf.detail = detail;
        Ok(wf)
    }
}

#[derive(Default)]
pub struct WorkflowMutation;

#[Object]
impl WorkflowMutation {
    /// Run a workflow for a subject/task.
    async fn run_workflow(
        &self,
        ctx: &Context<'_>,
        task_id: ID,
        definition: Option<String>,
    ) -> Result<WorkflowRunStart> {
        let client = client_from_ctx(ctx).await?;
        let start = client
            .workflow_run(WorkflowRunRequest {
                task_id: task_id.to_string(),
                definition,
                params: Default::default(),
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/run failed: {e}")))?;
        Ok(WorkflowRunStart {
            workflow_id: ID(start.workflow_id),
            status: Some(start.status.into()),
            started_at: start.started_at.to_rfc3339(),
        })
    }

    /// Execute a workflow definition directly, optionally bound to a subject.
    async fn execute_workflow(
        &self,
        ctx: &Context<'_>,
        definition: String,
        subject_id: Option<ID>,
    ) -> Result<WorkflowRunStart> {
        let client = client_from_ctx(ctx).await?;
        let start = client
            .workflow_execute(WorkflowExecuteRequest {
                definition,
                params: Default::default(),
                subject_id: subject_id
                    .map(|id| animus_subject_protocol::SubjectId::new(id.to_string())),
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/execute failed: {e}")))?;
        Ok(WorkflowRunStart {
            workflow_id: ID(start.workflow_id),
            status: Some(start.status.into()),
            started_at: start.started_at.to_rfc3339(),
        })
    }

    async fn pause_workflow(&self, ctx: &Context<'_>, id: ID) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .workflow_pause(WorkflowPauseRequest { id: id.to_string() })
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/pause failed: {e}")))?;
        Ok(true)
    }

    async fn resume_workflow(
        &self,
        ctx: &Context<'_>,
        id: ID,
        feedback: Option<String>,
    ) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .workflow_resume(WorkflowResumeRequest {
                id: id.to_string(),
                feedback,
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/resume failed: {e}")))?;
        Ok(true)
    }

    async fn cancel_workflow(
        &self,
        ctx: &Context<'_>,
        id: ID,
        reason: Option<String>,
    ) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .workflow_cancel(WorkflowCancelRequest {
                id: id.to_string(),
                reason,
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/cancel failed: {e}")))?;
        Ok(true)
    }
}

#[derive(Default)]
pub struct WorkflowEventsSubscription;

#[Subscription]
impl WorkflowEventsSubscription {
    async fn workflow_events(
        &self,
        ctx: &Context<'_>,
        workflow_id: Option<ID>,
        kinds: Option<Vec<String>>,
    ) -> Result<impl Stream<Item = WorkflowEvent>> {
        let client = client_from_ctx(ctx).await?;
        let request = WorkflowEventsRequest {
            workflow_id: workflow_id.map(|id| id.to_string()),
            kinds,
        };
        let subscription = client
            .workflow_events(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("workflow/events failed: {e}")))?;
        Ok(stream::unfold(
            (subscription, client),
            |(mut sub, client)| async move {
                let event = sub.recv().await?;
                let projected = WorkflowEvent {
                    workflow_id: ID(event.workflow_id),
                    kind: event.kind,
                    payload: event.payload.to_string(),
                    at: event.occurred_at.to_rfc3339(),
                };
                Some((projected, (sub, client)))
            },
        ))
    }
}
