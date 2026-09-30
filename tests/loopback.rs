//! The router only answers requests addressed to this machine, and `start`
//! reports bind failures instead of claiming a port it does not hold.

use std::sync::Arc;

use animus_transport_graphql::{
    backend::GraphqlTransportBackend, config::GraphqlConfig, schema::build_schema, server::router,
};
use animus_transport_protocol::{BackendError, TransportBackend, TransportConfig};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn app(cfg: GraphqlConfig) -> axum::Router {
    let cfg = Arc::new(cfg);
    router(build_schema(cfg.clone()), cfg)
}

async fn post_graphql(app: axum::Router, host: &str, origin: Option<&str>) -> StatusCode {
    let mut req = Request::builder()
        .method("POST")
        .uri("/graphql")
        .header("host", host)
        .header("content-type", "application/json");
    if let Some(origin) = origin {
        req = req.header("origin", origin);
    }
    let req = req
        .body(Body::from(r#"{"query":"{ __typename }"}"#))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn local_requests_are_served() {
    let cfg = GraphqlConfig::default;
    assert_eq!(
        post_graphql(app(cfg()), "127.0.0.1:8081", None).await,
        StatusCode::OK
    );
    assert_eq!(
        post_graphql(app(cfg()), "localhost:8081", None).await,
        StatusCode::OK
    );
    assert_eq!(
        post_graphql(app(cfg()), "127.0.0.1:8081", Some("http://127.0.0.1:8082")).await,
        StatusCode::OK
    );
    assert_eq!(
        post_graphql(app(cfg()), "[::1]:8081", Some("http://localhost:5174")).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn cross_site_origin_is_refused() {
    let status = post_graphql(
        app(GraphqlConfig::default()),
        "127.0.0.1:8081",
        Some("https://evil.example"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let status = post_graphql(
        app(GraphqlConfig::default()),
        "127.0.0.1:8081",
        Some("null"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rebound_host_is_refused() {
    // DNS rebinding: the page's own origin, resolved to 127.0.0.1.
    let status = post_graphql(
        app(GraphqlConfig::default()),
        "evil.example:8081",
        Some("http://evil.example:8081"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let status = post_graphql(app(GraphqlConfig::default()), "evil.example:8081", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn websocket_upgrade_from_another_site_is_refused() {
    let req = Request::builder()
        .method("GET")
        .uri("/graphql/ws")
        .header("host", "127.0.0.1:8081")
        .header("origin", "https://evil.example")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-protocol", "graphql-transport-ws")
        .body(Body::empty())
        .unwrap();
    let status = app(GraphqlConfig::default())
        .oneshot(req)
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn allowed_hosts_extends_the_loopback_set() {
    let cfg = || GraphqlConfig {
        allowed_hosts: vec!["devbox.lan".into()],
        ..GraphqlConfig::default()
    };
    assert_eq!(
        post_graphql(
            app(cfg()),
            "devbox.lan:8081",
            Some("http://devbox.lan:8082")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        post_graphql(app(cfg()), "other.lan:8081", None).await,
        StatusCode::FORBIDDEN
    );
}

fn transport_config(bind: &str) -> TransportConfig {
    serde_json::from_value(serde_json::json!({
        "control_socket_path": "/tmp/animus-graphql-test.sock",
        "project_root": "/tmp",
        "bind_addr": bind,
    }))
    .unwrap()
}

#[tokio::test]
async fn start_reports_the_port_it_actually_bound() {
    let backend = GraphqlTransportBackend::default();
    let info = backend
        .start(transport_config("127.0.0.1:0"))
        .await
        .expect("start");
    assert!(info.bound_addr.starts_with("127.0.0.1:"));
    assert_ne!(info.bound_addr, "127.0.0.1:0");
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn start_fails_when_the_port_is_taken() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let backend = GraphqlTransportBackend::default();
    let err = backend
        .start(transport_config(&addr))
        .await
        .expect_err("port is taken");
    assert!(matches!(err, BackendError::AddressInUse(_)), "{err:?}");
}
