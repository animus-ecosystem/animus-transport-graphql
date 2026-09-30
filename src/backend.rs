//! `TransportBackend` impl that drives the Axum + async-graphql server.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;

use animus_plugin_protocol::{HealthCheckResult, HealthStatus};
use animus_transport_protocol::{
    BackendError, TransportBackend, TransportConfig, TransportInfo, TransportSchema,
};

use crate::{config::GraphqlConfig, server};

#[derive(Default)]
pub struct GraphqlTransportBackend {
    handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

#[async_trait]
impl TransportBackend for GraphqlTransportBackend {
    fn schema(&self) -> TransportSchema {
        TransportSchema {
            kinds: vec!["graphql".into()],
            supports_streaming: true,
            supports_websocket: true,
            default_port: Some(8081),
        }
    }

    async fn start(&self, config: TransportConfig) -> Result<TransportInfo, BackendError> {
        let bind = config
            .bind_addr
            .clone()
            .unwrap_or_else(|| "127.0.0.1:8081".into());

        let cfg = GraphqlConfig {
            bind: bind.clone(),
            control_socket_path: config.control_socket_path,
            auth_token: config
                .config
                .get("auth_token")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            playground_enabled: config
                .config
                .get("playground")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            allowed_hosts: config
                .config
                .get("allowed_hosts")
                .and_then(|v| v.as_array())
                .map(|hosts| {
                    hosts
                        .iter()
                        .filter_map(|h| h.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        };

        // Bind here rather than in the spawned task: the host reports the
        // returned address as where the API lives, so a port that is already
        // taken must fail `transport/start` instead of being reported as ours.
        let listener = tokio::net::TcpListener::bind(&bind)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AddrInUse => BackendError::AddressInUse(bind.clone()),
                std::io::ErrorKind::PermissionDenied => {
                    BackendError::PermissionDenied(bind.clone())
                }
                _ => BackendError::Other(anyhow::anyhow!("failed to bind {bind}: {e}")),
            })?;
        let bound_addr = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| bind.clone());

        let task = tokio::spawn(async move {
            if let Err(err) = server::serve(listener, cfg).await {
                tracing::error!(error = %err, "graphql transport server exited with error");
            }
        });

        let mut guard = self.handle.lock().await;
        if let Some(prev) = guard.take() {
            prev.abort();
        }
        *guard = Some(task);

        Ok(TransportInfo {
            bound_addr,
            started_at: Utc::now(),
        })
    }

    async fn shutdown(&self) -> Result<(), BackendError> {
        let mut guard = self.handle.lock().await;
        if let Some(handle) = guard.take() {
            handle.abort();
        }
        Ok(())
    }

    async fn health(&self) -> Result<HealthCheckResult, BackendError> {
        let running = self
            .handle
            .lock()
            .await
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false);
        Ok(HealthCheckResult {
            status: if running {
                HealthStatus::Healthy
            } else {
                HealthStatus::Degraded
            },
            uptime_ms: None,
            memory_usage_bytes: None,
            last_error: None,
        })
    }
}
