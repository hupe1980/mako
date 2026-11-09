//! The MCP round trips every mako server is held to, against this service's
//! `/mcp` as the runner mounts it: the declared revisions on `server/discover`,
//! requests on 2026-07-28 and on 2025-11-25 without a session, `Mcp-Name`
//! read, and a revision outside the declared set refused
//! (`mako_service::mcp::conformance::round_trips`).

use mako_service::Daemon as _;

type Served = productd::Productd;

#[tokio::test]
async fn the_mcp_server_passes_the_round_trips() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../dev/config")
        .join(format!("{}.toml", Served::NAME));
    let cfg = mako_service::load_config_file::<<Served as mako_service::Daemon>::Config>(
        Served::NAME,
        &path,
    )
    .expect("the dev configuration loads");
    let app = mako_service::build_for_test::<Served>(cfg)
        .await
        .expect("the router builds");
    mako_service::mcp::conformance::round_trips(app, "/mcp", "get_policy").await;
}
