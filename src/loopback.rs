//! Refuse requests that were not addressed to this machine.
//!
//! The transport has no login, so anything that can reach the port can run
//! mutations. Binding to `127.0.0.1` keeps other machines out, but a web page
//! open in the user's browser can still reach it:
//!
//! - a cross-site `fetch` or WebSocket from another page carries that page's
//!   `Origin`;
//! - a DNS-rebinding page reaches `127.0.0.1` under its own host name, which
//!   shows up in `Host`.
//!
//! [`guard`] rejects both with `403`: `Host` must name a loopback address, and
//! `Origin`, when the browser sends one, must too. Requests without an
//! `Origin` (curl, the CLI, the web UI's server-side proxy) are allowed.
//! Extra host names can be allowed through `allowed_hosts` for deployments
//! that bind a non-loopback address on purpose.

use std::net::IpAddr;
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::config::GraphqlConfig;

pub async fn guard(State(cfg): State<Arc<GraphqlConfig>>, req: Request, next: Next) -> Response {
    let allowed = &cfg.allowed_hosts;

    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()));
    if let Some(host) = host {
        if !host_allowed(host_name(&host), allowed) {
            tracing::warn!(%host, "rejected request with a non-local Host");
            return (StatusCode::FORBIDDEN, "forbidden: Host is not local").into_response();
        }
    }

    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let ok = origin
            .to_str()
            .ok()
            .and_then(origin_host)
            .is_some_and(|h| host_allowed(&h, allowed));
        if !ok {
            tracing::warn!(?origin, "rejected request from a non-local Origin");
            return (StatusCode::FORBIDDEN, "forbidden: Origin is not local").into_response();
        }
    }

    next.run(req).await
}

/// Host name part of a `Host` value: drops the port and IPv6 brackets.
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// Host name of an `Origin` value such as `http://127.0.0.1:8082`. `null` and
/// anything that is not an http(s) origin yield `None`.
fn origin_host(origin: &str) -> Option<String> {
    let uri: Uri = origin.parse().ok()?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return None;
    }
    let host = uri.host()?;
    Some(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
    )
}

fn host_allowed(name: &str, allowed: &[String]) -> bool {
    let name = name.trim_end_matches('.');
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return true;
    }
    allowed.iter().any(|a| a.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_name_strips_port_and_brackets() {
        assert_eq!(host_name("127.0.0.1:8081"), "127.0.0.1");
        assert_eq!(host_name("localhost"), "localhost");
        assert_eq!(host_name("[::1]:8081"), "::1");
        assert_eq!(host_name("[::1]"), "::1");
    }

    #[test]
    fn loopback_names_are_allowed() {
        for name in [
            "localhost",
            "LOCALHOST",
            "localhost.",
            "127.0.0.1",
            "127.1.2.3",
            "::1",
        ] {
            assert!(host_allowed(name, &[]), "{name}");
        }
    }

    #[test]
    fn other_names_are_refused_unless_listed() {
        for name in [
            "evil.example",
            "localhost.evil.example",
            "10.0.0.5",
            "0.0.0.0",
        ] {
            assert!(!host_allowed(name, &[]), "{name}");
        }
        assert!(host_allowed("devbox.lan", &["devbox.lan".to_string()]));
    }

    #[test]
    fn origin_host_parses_http_origins_only() {
        assert_eq!(
            origin_host("http://127.0.0.1:8082").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            origin_host("https://localhost").as_deref(),
            Some("localhost")
        );
        assert_eq!(origin_host("http://[::1]:5174").as_deref(), Some("::1"));
        assert_eq!(origin_host("null"), None);
        assert_eq!(origin_host("file://"), None);
    }
}
