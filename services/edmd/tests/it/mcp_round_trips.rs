//! The MCP round trips every mako server is held to, against this service's
//! `/mcp` as the runner mounts it: the declared revisions on `server/discover`,
//! requests on 2026-07-28 and on 2025-11-25 without a session, `Mcp-Name`
//! read, and a revision outside the declared set refused
//! (`mako_service::mcp::conformance::round_trips`). This daemon's `build`
//! opens a store of its own from `[database] url`, so it starts against a
//! real PostgreSQL.

use testcontainers::{ImageExt as _, runners::AsyncRunner as _};
use testcontainers_modules::postgres::Postgres;

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_mcp_server_passes_the_round_trips() {
    let Ok(pg) = Postgres::default().with_tag("17-alpine").start().await else {
        return;
    };
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres?sslmode=disable");
    #[expect(
        clippy::disallowed_methods,
        reason = "the test connects straight to its throwaway container; pool sizing is a deployment concern"
    )]
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");

    let mut cfg: toml::Value = toml::from_str(include_str!("../../../../dev/config/edmd.toml"))
        .expect("the dev configuration parses");
    cfg["database"]["url"] = toml::Value::String(url);
    if let Some(db) = cfg["database"].as_table_mut() {
        db.remove("migration_url");
    }
    let cfg = cfg
        .try_into()
        .expect("the dev configuration is an edmd one");
    let app = mako_service::build_for_test_on::<edmd::Edmd>(cfg, pool)
        .await
        .expect("the router builds");
    mako_service::mcp::conformance::round_trips(app, "/mcp", "get_policy").await;
}
