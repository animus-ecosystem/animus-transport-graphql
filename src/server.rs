//! Axum app exposing the GraphQL endpoint, Playground, and SDL dump.

use std::sync::Arc;

use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use axum::{
    extract::Extension,
    http::{header, HeaderMap, StatusCode},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};

use crate::{
    config::GraphqlConfig,
    loopback,
    schema::{build_schema, AnimusSchema},
};

pub async fn run(config: GraphqlConfig) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    serve(listener, config).await
}

/// Serve on a listener the caller already bound, so bind errors surface to
/// the caller (see `GraphqlTransportBackend::start`).
pub async fn serve(listener: tokio::net::TcpListener, config: GraphqlConfig) -> anyhow::Result<()> {
    let cfg = Arc::new(config);
    let schema = build_schema(cfg.clone());

    let app = router(schema, cfg.clone());

    tracing::info!(addr = %listener.local_addr()?, "graphql transport listening");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn router(schema: AnimusSchema, cfg: Arc<GraphqlConfig>) -> Router {
    let playground_enabled = cfg.playground_enabled;

    let mut graphql_route = post(graphql_handler);
    if playground_enabled {
        graphql_route = graphql_route.get(graphql_playground);
    }

    Router::new()
        .route("/graphql", graphql_route)
        .route_service("/graphql/ws", GraphQLSubscription::new(schema.clone()))
        .route("/graphql/sdl", get(graphql_sdl_handler))
        .route("/healthz", get(healthz))
        .layer(Extension(schema))
        .layer(Extension(cfg.clone()))
        .layer(middleware::from_fn_with_state(cfg, loopback::guard))
}

async fn graphql_handler(
    Extension(schema): Extension<AnimusSchema>,
    Extension(cfg): Extension<Arc<GraphqlConfig>>,
    headers: HeaderMap,
    req: GraphQLRequest,
) -> Response {
    if !authorized(&cfg, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    GraphQLResponse::from(schema.execute(req.into_inner()).await).into_response()
}

fn authorized(cfg: &GraphqlConfig, headers: &HeaderMap) -> bool {
    let Some(expected) = cfg.auth_token.as_deref() else {
        return true;
    };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| constant_time_eq(t.as_bytes(), expected.as_bytes()))
        .unwrap_or(false)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn graphql_playground() -> impl IntoResponse {
    Html(playground_source(
        GraphQLPlaygroundConfig::new("/graphql").subscription_endpoint("/graphql/ws"),
    ))
}

async fn graphql_sdl_handler(
    Extension(schema): Extension<AnimusSchema>,
    Extension(cfg): Extension<Arc<GraphqlConfig>>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&cfg, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    schema.sdl().into_response()
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}
