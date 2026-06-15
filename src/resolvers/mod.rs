//! Category-scoped resolver modules. Each module exposes (where applicable):
//!
//! - a `*Query` struct mounted on the merged `QueryRoot`,
//! - a `*Mutation` struct mounted on the merged `MutationRoot`,
//! - and zero or more `*Subscription` structs for `SubscriptionRoot`.
//!
//! All resolvers go through `animus_control_protocol::client::ControlClient`
//! — there is no direct daemon-state access from the GraphQL layer.

pub mod daemon;
pub mod plugin;
pub mod queue;
pub mod subject;
pub mod workflows;

use std::sync::Arc;

use animus_control_protocol::client::ControlClient;
use async_graphql::Context;

use crate::config::GraphqlConfig;

pub(crate) async fn client_from_ctx(ctx: &Context<'_>) -> async_graphql::Result<ControlClient> {
    let cfg = ctx.data::<Arc<GraphqlConfig>>().map_err(|_| {
        async_graphql::Error::new("graphql config not injected into schema context")
    })?;
    ControlClient::connect(&cfg.control_socket_path)
        .await
        .map_err(|e| async_graphql::Error::new(format!("failed to connect to control socket: {e}")))
}
