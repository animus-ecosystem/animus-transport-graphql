//! Dispatch queue queries and mutations.
//!
//! Shapes mirror `animus_control_protocol`'s `QueueEntry` / `QueueStats`.

use animus_control_protocol::types::{
    QueueDropRequest, QueueEnqueueRequest, QueueEntry as WireQueueEntry, QueueEntryStatus,
    QueueHoldRequest, QueueListRequest, QueueReleaseRequest, QueueReorderPosition,
    QueueReorderRequest, QueueStats as WireQueueStats,
};
use async_graphql::{Context, Enum, InputObject, Object, Result, SimpleObject, ID};

use super::client_from_ctx;

/// Coarse status of a queue entry. Mirrors
/// [`animus_control_protocol::types::QueueEntryStatus`].
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum QueueState {
    Ready,
    Held,
    InFlight,
    Done,
    Dropped,
}

impl From<QueueEntryStatus> for QueueState {
    fn from(s: QueueEntryStatus) -> Self {
        match s {
            QueueEntryStatus::Ready => QueueState::Ready,
            QueueEntryStatus::Held => QueueState::Held,
            QueueEntryStatus::InFlight => QueueState::InFlight,
            QueueEntryStatus::Done => QueueState::Done,
            QueueEntryStatus::Dropped => QueueState::Dropped,
        }
    }
}

impl From<QueueState> for QueueEntryStatus {
    fn from(s: QueueState) -> Self {
        match s {
            QueueState::Ready => QueueEntryStatus::Ready,
            QueueState::Held => QueueEntryStatus::Held,
            QueueState::InFlight => QueueEntryStatus::InFlight,
            QueueState::Done => QueueEntryStatus::Done,
            QueueState::Dropped => QueueEntryStatus::Dropped,
        }
    }
}

/// One dispatch-queue entry. Mirrors
/// [`animus_control_protocol::types::QueueEntry`].
#[derive(SimpleObject)]
pub struct QueueEntry {
    pub id: ID,
    pub task_id: ID,
    /// Priority on a 0..=4 scale (0 = none, 4 = critical).
    pub priority: i32,
    pub state: QueueState,
    pub enqueued_at: String,
    pub held: bool,
    pub hold_reason: Option<String>,
}

impl From<WireQueueEntry> for QueueEntry {
    fn from(entry: WireQueueEntry) -> Self {
        let held = matches!(entry.status, QueueEntryStatus::Held);
        QueueEntry {
            id: ID(entry.id),
            task_id: ID(entry.subject_id.as_str().to_string()),
            priority: entry.priority as i32,
            state: entry.status.into(),
            enqueued_at: entry.enqueued_at.to_rfc3339(),
            held,
            hold_reason: entry.hold_reason,
        }
    }
}

/// Aggregate queue counters. Mirrors
/// [`animus_control_protocol::types::QueueStats`].
#[derive(SimpleObject, Default)]
pub struct QueueStats {
    /// Total live entries (ready + held + in-flight).
    pub total: i32,
    pub ready: i32,
    pub held: i32,
    pub in_flight: i32,
    pub done_recent: i32,
    pub dropped_recent: i32,
}

impl From<WireQueueStats> for QueueStats {
    fn from(s: WireQueueStats) -> Self {
        let total = (s.ready + s.held + s.in_flight) as i32;
        QueueStats {
            total,
            ready: s.ready as i32,
            held: s.held as i32,
            in_flight: s.in_flight as i32,
            done_recent: s.done_recent as i32,
            dropped_recent: s.dropped_recent as i32,
        }
    }
}

#[derive(InputObject)]
pub struct EnqueueInput {
    pub task_id: ID,
    /// Priority on a 0..=4 scale. Defaults to 2 (medium) when omitted.
    pub priority: Option<i32>,
}

#[derive(Default)]
pub struct QueueQuery;

#[Object]
impl QueueQuery {
    /// List queue entries, optionally restricted to a state.
    async fn queue(&self, ctx: &Context<'_>, state: Option<QueueState>) -> Result<Vec<QueueEntry>> {
        let client = client_from_ctx(ctx).await?;
        let request = QueueListRequest {
            status: state.map(QueueEntryStatus::from),
            cursor: None,
            limit: None,
        };
        let response = client
            .queue_list(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("queue/list failed: {e}")))?;
        Ok(response.entries.into_iter().map(QueueEntry::from).collect())
    }

    async fn queue_stats(&self, ctx: &Context<'_>) -> Result<QueueStats> {
        let client = client_from_ctx(ctx).await?;
        let stats = client
            .queue_stats()
            .await
            .map_err(|e| async_graphql::Error::new(format!("queue/stats failed: {e}")))?;
        Ok(stats.into())
    }
}

#[derive(Default)]
pub struct QueueMutation;

#[Object]
impl QueueMutation {
    async fn enqueue(&self, ctx: &Context<'_>, input: EnqueueInput) -> Result<QueueEntry> {
        let client = client_from_ctx(ctx).await?;
        let request = QueueEnqueueRequest {
            task_id: input.task_id.to_string(),
            priority: input.priority.map(|p| p.clamp(0, 4) as u8),
        };
        let entry = client
            .queue_enqueue(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("queue/enqueue failed: {e}")))?;
        Ok(entry.into())
    }

    async fn drop_queue(&self, ctx: &Context<'_>, id: ID) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .queue_drop(QueueDropRequest { id: id.to_string() })
            .await
            .map_err(|e| async_graphql::Error::new(format!("queue/drop failed: {e}")))?;
        Ok(true)
    }

    async fn hold_queue(&self, ctx: &Context<'_>, id: ID, reason: Option<String>) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .queue_hold(QueueHoldRequest {
                id: id.to_string(),
                reason,
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("queue/hold failed: {e}")))?;
        Ok(true)
    }

    async fn release_queue(&self, ctx: &Context<'_>, id: ID) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .queue_release(QueueReleaseRequest { id: id.to_string() })
            .await
            .map_err(|e| async_graphql::Error::new(format!("queue/release failed: {e}")))?;
        Ok(true)
    }

    /// Reorder queue entries: move the given ids as a contiguous group to the
    /// front (default) or back of the queue.
    async fn reorder_queue(
        &self,
        ctx: &Context<'_>,
        ids: Vec<ID>,
        #[graphql(default = true)] front: bool,
    ) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        let request = QueueReorderRequest {
            id: None,
            subject_ids: ids.into_iter().map(|id| id.to_string()).collect(),
            anchor_id: None,
            position: if front {
                QueueReorderPosition::Front
            } else {
                QueueReorderPosition::Back
            },
        };
        client
            .queue_reorder(request)
            .await
            .map_err(|e| async_graphql::Error::new(format!("queue/reorder failed: {e}")))?;
        Ok(true)
    }
}
