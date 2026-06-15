//! GraphQL schema assembly.
//!
//! The schema is composed of category-scoped resolver modules under
//! [`crate::resolvers`]. Each module owns its own root-merge object for
//! [`MergedObject`] / [`MergedSubscription`] composition.

use std::sync::Arc;

use async_graphql::{EmptySubscription, MergedObject, MergedSubscription, Schema};

use crate::{
    config::GraphqlConfig,
    resolvers::{
        daemon::{DaemonEventsSubscription, DaemonMutation, DaemonQuery},
        plugin::{PluginMutation, PluginQuery},
        queue::{QueueMutation, QueueQuery},
        subject::{SubjectChangedSubscription, SubjectMutation, SubjectQuery},
        workflows::{WorkflowEventsSubscription, WorkflowMutation, WorkflowQuery},
    },
};

#[derive(MergedObject, Default)]
pub struct QueryRoot(
    WorkflowQuery,
    QueueQuery,
    PluginQuery,
    DaemonQuery,
    SubjectQuery,
);

#[derive(MergedObject, Default)]
pub struct MutationRoot(
    WorkflowMutation,
    QueueMutation,
    PluginMutation,
    DaemonMutation,
    SubjectMutation,
);

#[derive(MergedSubscription, Default)]
pub struct SubscriptionRoot(
    WorkflowEventsSubscription,
    DaemonEventsSubscription,
    SubjectChangedSubscription,
);

pub type AnimusSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub fn build_schema(cfg: Arc<GraphqlConfig>) -> AnimusSchema {
    Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .data(cfg)
    .finish()
}

/// Build a schema with subscriptions stripped — used by introspection tests
/// that don't want to wire a streaming control client.
pub fn build_schema_no_subs(
    cfg: Arc<GraphqlConfig>,
) -> Schema<QueryRoot, MutationRoot, EmptySubscription> {
    Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        EmptySubscription,
    )
    .data(cfg)
    .finish()
}
