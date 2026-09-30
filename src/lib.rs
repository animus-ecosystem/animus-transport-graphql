//! GraphQL transport backend for Animus.
//!
//! Mounts an Axum app exposing `/graphql` (POST + Playground), `/graphql/ws`
//! (subscriptions over WebSocket), and `/graphql/sdl`. Each resolver opens a
//! short-lived `ControlClient` (from `animus-control-protocol`) against the
//! local daemon control socket and translates the GraphQL operation into the
//! matching control RPC.

pub mod backend;
pub mod config;
pub mod loopback;
pub mod resolvers;
pub mod schema;
pub mod server;

pub use backend::GraphqlTransportBackend;
pub use config::GraphqlConfig;
pub use schema::{build_schema, AnimusSchema};
