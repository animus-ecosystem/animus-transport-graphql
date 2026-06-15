//! Daemon health/status queries, control mutation, and event stream.
//!
//! Shapes mirror `animus_control_protocol`'s `DaemonStatusResponse`,
//! `DaemonHealthResponse`, and `DaemonAgentsResponse`.

use animus_control_protocol::types::{
    AgentInfo as WireAgentInfo, DaemonEventsRequest, DaemonHealthStatus as WireHealthStatus,
    PluginHealth as WirePluginHealth,
};
use async_graphql::{Context, Enum, Object, Result, SimpleObject, Subscription};
use futures_util::stream::{self, Stream};

use super::client_from_ctx;

/// Coarse daemon health verdict. Mirrors
/// [`animus_control_protocol::types::DaemonHealthStatus`].
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Down,
}

impl From<WireHealthStatus> for HealthStatus {
    fn from(s: WireHealthStatus) -> Self {
        match s {
            WireHealthStatus::Healthy => HealthStatus::Healthy,
            WireHealthStatus::Degraded => HealthStatus::Degraded,
            WireHealthStatus::Unhealthy => HealthStatus::Unhealthy,
            WireHealthStatus::Down => HealthStatus::Down,
        }
    }
}

/// Daemon process status. Mirrors
/// [`animus_control_protocol::types::DaemonStatusResponse`].
#[derive(SimpleObject, Default)]
pub struct DaemonStatus {
    pub running: bool,
    pub pid: Option<i32>,
    pub uptime_seconds: Option<i64>,
    pub version: Option<String>,
    pub project_root: Option<String>,
    pub log_path: Option<String>,
}

/// Per-plugin health snapshot. Mirrors
/// [`animus_control_protocol::types::PluginHealth`].
#[derive(SimpleObject)]
pub struct PluginHealth {
    pub name: String,
    pub kind: String,
    pub status: HealthStatus,
    pub uptime_ms: Option<i64>,
    pub last_error: Option<String>,
}

impl From<WirePluginHealth> for PluginHealth {
    fn from(p: WirePluginHealth) -> Self {
        PluginHealth {
            name: p.name,
            kind: p.kind,
            status: p.status.into(),
            uptime_ms: p.uptime_ms.map(|u| u as i64),
            last_error: p.last_error,
        }
    }
}

/// Daemon health report. Mirrors
/// [`animus_control_protocol::types::DaemonHealthResponse`].
#[derive(SimpleObject)]
pub struct DaemonHealth {
    /// Convenience flag: `true` only when `status == HEALTHY`.
    pub healthy: bool,
    pub status: HealthStatus,
    pub plugins: Vec<PluginHealth>,
    pub last_error: Option<String>,
}

/// An active agent session. Mirrors
/// [`animus_control_protocol::types::AgentInfo`].
#[derive(SimpleObject, Default)]
pub struct DaemonAgent {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub workflow_id: Option<String>,
    pub phase_id: Option<String>,
    pub started_at: String,
}

impl From<WireAgentInfo> for DaemonAgent {
    fn from(a: WireAgentInfo) -> Self {
        DaemonAgent {
            session_id: a.session_id,
            provider: a.provider,
            model: a.model,
            workflow_id: a.workflow_id,
            phase_id: a.phase_id,
            started_at: a.started_at.to_rfc3339(),
        }
    }
}

#[derive(SimpleObject, Default)]
pub struct DaemonEvent {
    pub id: String,
    pub kind: String,
    pub payload: String,
    pub at: String,
}

#[derive(Default)]
pub struct DaemonQuery;

#[Object]
impl DaemonQuery {
    async fn daemon(&self, ctx: &Context<'_>) -> Result<DaemonStatus> {
        let client = client_from_ctx(ctx).await?;
        let status = client
            .daemon_status()
            .await
            .map_err(|e| async_graphql::Error::new(format!("daemon/status failed: {e}")))?;
        Ok(DaemonStatus {
            running: status.running,
            pid: status.pid.map(|p| p as i32),
            uptime_seconds: status.uptime_seconds.map(|u| u as i64),
            version: status.version,
            project_root: status.project_root.map(|p| p.display().to_string()),
            log_path: status.log_path.map(|p| p.display().to_string()),
        })
    }

    async fn daemon_health(&self, ctx: &Context<'_>) -> Result<DaemonHealth> {
        let client = client_from_ctx(ctx).await?;
        let health = client
            .daemon_health()
            .await
            .map_err(|e| async_graphql::Error::new(format!("daemon/health failed: {e}")))?;
        Ok(DaemonHealth {
            healthy: matches!(health.status, WireHealthStatus::Healthy),
            status: health.status.into(),
            plugins: health.plugins.into_iter().map(PluginHealth::from).collect(),
            last_error: health.last_error,
        })
    }

    /// Currently active agent sessions.
    async fn daemon_agents(&self, ctx: &Context<'_>) -> Result<Vec<DaemonAgent>> {
        let client = client_from_ctx(ctx).await?;
        let response = client
            .daemon_agents()
            .await
            .map_err(|e| async_graphql::Error::new(format!("daemon/agents failed: {e}")))?;
        Ok(response.agents.into_iter().map(DaemonAgent::from).collect())
    }
}

#[derive(Default)]
pub struct DaemonMutation;

#[Object]
impl DaemonMutation {
    /// Start the daemon. `daemon/stop` and `daemon/restart` are intentionally
    /// not exposed — the kernel forbids them over the control socket.
    async fn start_daemon(&self, ctx: &Context<'_>) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .daemon_start()
            .await
            .map_err(|e| async_graphql::Error::new(format!("daemon/start failed: {e}")))?;
        Ok(true)
    }
}

#[derive(Default)]
pub struct DaemonEventsSubscription;

#[Subscription]
impl DaemonEventsSubscription {
    async fn daemon_events(&self, ctx: &Context<'_>) -> Result<impl Stream<Item = DaemonEvent>> {
        let client = client_from_ctx(ctx).await?;
        let subscription = client
            .daemon_events(DaemonEventsRequest::default())
            .await
            .map_err(|e| async_graphql::Error::new(format!("daemon/events failed: {e}")))?;
        Ok(stream::unfold(
            (subscription, client),
            |(mut sub, client)| async move {
                let event = sub.recv().await?;
                let projected = DaemonEvent {
                    id: event.id,
                    kind: event.kind,
                    payload: event.payload.to_string(),
                    at: event.occurred_at.to_rfc3339(),
                };
                Some((projected, (sub, client)))
            },
        ))
    }
}
