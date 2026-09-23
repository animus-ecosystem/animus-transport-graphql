use animus_plugin_protocol::PluginInfo;
use animus_plugin_runtime::transport_backend_main;
use animus_transport_graphql::backend::GraphqlTransportBackend;
use animus_transport_protocol::PLUGIN_KIND_TRANSPORT_BACKEND;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    emit_manifest_if_requested();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let backend = GraphqlTransportBackend::default();
    let info = PluginInfo {
        name: env!("CARGO_PKG_NAME").into(),
        version: env!("CARGO_PKG_VERSION").into(),
        plugin_kind: PLUGIN_KIND_TRANSPORT_BACKEND.into(),
        plugin_kinds: Vec::new(),
        description: Some(env!("CARGO_PKG_DESCRIPTION").into()),
    };

    transport_backend_main(info, backend).await
}

fn emit_manifest_if_requested() {
    if !std::env::args()
        .skip(1)
        .any(|arg| arg == "--manifest" || arg == "-m")
    {
        return;
    }

    let manifest = serde_json::json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "plugin_kind": "transport_backend",
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "protocol_version": animus_plugin_protocol::PROTOCOL_VERSION,
        "capabilities": [
            "transport/start",
            "transport/shutdown",
            "transport/schema",
            "health/check"
        ],
        "env_required": []
    });
    println!(
        "{}",
        serde_json::to_string(&manifest).expect("serialize manifest")
    );
    std::process::exit(0);
}
