//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with a minimal configuration and a journal in a scratch
//! directory, and the runner's infra routes are merged in exactly as at boot —
//! so a route axum refuses, a merge collision with the runner's `/metrics` or
//! `/health/*`, or a `build` that panics fails `cargo test` instead of the
//! container start. `build` completes the MCP handshake with every server a
//! specialist is granted, so one stand-in answers it for all of them.

/// An MCP server with no tools, at `/mcp`.
#[derive(Clone)]
struct NoTools;
impl rmcp::ServerHandler for NoTools {}

async fn mcp() -> String {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let service = StreamableHttpService::new(
        || Ok(NoTools),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let app = axum::Router::new().route_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn the_router_builds_as_the_runner_mounts_it() {
    let mcp = mcp().await;
    let dir = std::env::temp_dir().join(format!("agentd-startup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let journal = dir.join("journal.redb");
    let cfg = toml::from_str(&format!(
        r#"
tenant = "9900357000004"
allow_insecure_no_auth = true
mcp_api_key = "test"
[journal]
backend = "redb"
path = "{}"
[mcp_servers]
accountingd = "{mcp}"
billingd = "{mcp}"
edmd = "{mcp}"
einsd = "{mcp}"
invoicd = "{mcp}"
makod = "{mcp}"
marktd = "{mcp}"
netzbilanzd = "{mcp}"
processd = "{mcp}"
productd = "{mcp}"
vertragd = "{mcp}"
[providers.anthropic]
backend = "anthropic"
api_key = "test"
[bundled_agents]
enable_all = true
"#,
        journal.display(),
    ))
    .expect("the configuration parses");
    let built = mako_service::build_for_test::<agentd::Agentd>(cfg).await;
    std::fs::remove_dir_all(&dir).ok();
    let _app = built.expect("the router builds");
}
