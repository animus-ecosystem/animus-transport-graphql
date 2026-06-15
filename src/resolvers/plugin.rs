//! Plugin registry queries and lifecycle mutations.
//!
//! Shapes mirror `animus_control_protocol`'s `PluginInfo`,
//! `PluginRegistryEntry`, `PluginPingResponse`, and `PluginUpdateEntry`.

use animus_control_protocol::types::{
    PluginBrowseRequest, PluginInfo as WirePluginInfo, PluginInfoRequest, PluginInstallRequest,
    PluginListRequest, PluginPingRequest, PluginRegistryEntry as WireRegistryEntry,
    PluginSearchRequest, PluginUninstallRequest, PluginUpdateRequest,
};
use async_graphql::{Context, InputObject, Object, Result, SimpleObject, ID};

use super::client_from_ctx;

/// One installed plugin. Mirrors
/// [`animus_control_protocol::types::PluginInfo`].
#[derive(SimpleObject, Default)]
pub struct Plugin {
    /// Plugin name (manifest `name`); also serves as the GraphQL id.
    pub id: ID,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub source: Option<String>,
    pub signature_verified: bool,
    pub description: Option<String>,
    pub binary_path: Option<String>,
}

impl From<WirePluginInfo> for Plugin {
    fn from(p: WirePluginInfo) -> Self {
        Plugin {
            id: ID(p.name.clone()),
            name: p.name,
            version: p.version,
            kind: p.kind,
            source: p.source,
            signature_verified: p.signature_verified,
            description: p.description,
            binary_path: p.binary_path.map(|b| b.display().to_string()),
        }
    }
}

/// A plugin registry entry, returned by search/browse. Mirrors
/// [`animus_control_protocol::types::PluginRegistryEntry`].
#[derive(SimpleObject, Default)]
pub struct PluginRegistryEntry {
    pub id: ID,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub installed: bool,
}

impl From<WireRegistryEntry> for PluginRegistryEntry {
    fn from(e: WireRegistryEntry) -> Self {
        PluginRegistryEntry {
            id: ID(e.id),
            name: e.name,
            version: e.version,
            kind: e.kind,
            description: e.description,
            url: e.url,
            tags: e.tags,
            installed: e.installed,
        }
    }
}

/// Result of pinging a plugin. Mirrors
/// [`animus_control_protocol::types::PluginPingResponse`].
#[derive(SimpleObject, Default)]
pub struct PluginPing {
    pub ok: bool,
    pub latency_ms: Option<i64>,
    pub error: Option<String>,
}

/// One row of a plugin update. Mirrors
/// [`animus_control_protocol::types::PluginUpdateEntry`].
#[derive(SimpleObject, Default)]
pub struct PluginUpdate {
    pub name: String,
    pub from_version: String,
    pub to_version: String,
    pub applied: bool,
}

#[derive(InputObject)]
pub struct InstallPluginInput {
    pub source: String,
    pub version: Option<String>,
    #[graphql(default)]
    pub yes: bool,
    #[graphql(default)]
    pub allow_unsigned: bool,
}

#[derive(Default)]
pub struct PluginQuery;

#[Object]
impl PluginQuery {
    /// List installed plugins, optionally filtered by kind.
    async fn plugin(&self, ctx: &Context<'_>, kind: Option<String>) -> Result<Vec<Plugin>> {
        let client = client_from_ctx(ctx).await?;
        let response = client
            .plugin_list(PluginListRequest {
                include_warnings: false,
                kind,
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/list failed: {e}")))?;
        Ok(response.plugins.into_iter().map(Plugin::from).collect())
    }

    /// Look up a single installed plugin by name.
    async fn plugin_info(&self, ctx: &Context<'_>, name: String) -> Result<Plugin> {
        let client = client_from_ctx(ctx).await?;
        let info = client
            .plugin_info(PluginInfoRequest { name })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/info failed: {e}")))?;
        Ok(info.into())
    }

    /// Search the plugin registry by free-text query.
    async fn plugin_search(
        &self,
        ctx: &Context<'_>,
        query: String,
        kind: Option<String>,
        tag: Option<String>,
    ) -> Result<Vec<PluginRegistryEntry>> {
        let client = client_from_ctx(ctx).await?;
        let response = client
            .plugin_search(PluginSearchRequest { query, kind, tag })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/search failed: {e}")))?;
        Ok(response
            .entries
            .into_iter()
            .map(PluginRegistryEntry::from)
            .collect())
    }

    /// Browse the plugin registry.
    async fn plugin_browse(
        &self,
        ctx: &Context<'_>,
        kind: Option<String>,
        #[graphql(default)] installed: bool,
        #[graphql(default)] available: bool,
    ) -> Result<Vec<PluginRegistryEntry>> {
        let client = client_from_ctx(ctx).await?;
        let response = client
            .plugin_browse(PluginBrowseRequest {
                kind,
                installed,
                available,
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/browse failed: {e}")))?;
        Ok(response
            .entries
            .into_iter()
            .map(PluginRegistryEntry::from)
            .collect())
    }
}

#[derive(Default)]
pub struct PluginMutation;

#[Object]
impl PluginMutation {
    async fn install_plugin(&self, ctx: &Context<'_>, input: InstallPluginInput) -> Result<Plugin> {
        let client = client_from_ctx(ctx).await?;
        let response = client
            .plugin_install(PluginInstallRequest {
                source: input.source,
                version: input.version,
                yes: input.yes,
                allow_unsigned: input.allow_unsigned,
            })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/install failed: {e}")))?;
        Ok(response.plugin.into())
    }

    async fn uninstall_plugin(&self, ctx: &Context<'_>, name: String) -> Result<bool> {
        let client = client_from_ctx(ctx).await?;
        client
            .plugin_uninstall(PluginUninstallRequest { name })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/uninstall failed: {e}")))?;
        Ok(true)
    }

    /// Lifecycle-ping a plugin.
    async fn ping_plugin(&self, ctx: &Context<'_>, name: String) -> Result<PluginPing> {
        let client = client_from_ctx(ctx).await?;
        let resp = client
            .plugin_ping(PluginPingRequest { name })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/ping failed: {e}")))?;
        Ok(PluginPing {
            ok: resp.ok,
            latency_ms: resp.latency_ms.map(|l| l as i64),
            error: resp.error,
        })
    }

    /// Update installed plugins. With `dryRun = true`, lists available
    /// upgrades without applying them.
    async fn update_plugins(
        &self,
        ctx: &Context<'_>,
        name: Option<String>,
        tag: Option<String>,
        #[graphql(default)] dry_run: bool,
    ) -> Result<Vec<PluginUpdate>> {
        let client = client_from_ctx(ctx).await?;
        let response = client
            .plugin_update(PluginUpdateRequest { name, tag, dry_run })
            .await
            .map_err(|e| async_graphql::Error::new(format!("plugin/update failed: {e}")))?;
        Ok(response
            .updates
            .into_iter()
            .map(|u| PluginUpdate {
                name: u.name,
                from_version: u.from_version,
                to_version: u.to_version,
                applied: u.applied,
            })
            .collect())
    }
}
