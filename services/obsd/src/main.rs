#![deny(unsafe_code)]

//! `obsd` — Business-process observability daemon.
//!
//! `mako_service::run` owns the lifecycle (tracing, tuned pool, real DB-ping
//! readiness, graceful shutdown); this only supplies the migrations and the
//! domain router (process projection, BNetzA KPI reports, `de.obs.*` sweep
//! producers, MCP server) via [`obsd::server::build_router`].

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use mako_service::{Daemon, ServiceContext};
use obsd::{config, server};

struct Obsd;

impl Daemon for Obsd {
    type Config = config::Config;
    const NAME: &'static str = "obsd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run obsd migrations")?;
        Ok(())
    }

    async fn build(cfg: Arc<config::Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
        server::build_router(cfg, ctx).await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Obsd>().await
}

/// The router builds the way `mako_service::run` mounts it.
///
/// `build` runs with a minimal configuration and a lazy pool, and the runner's
/// infra routes are merged in exactly as at boot — so a route axum refuses, a
/// merge collision with the runner's `/metrics` or `/health/*`, or a `build`
/// that panics fails `cargo test` instead of the container start.
#[cfg(test)]
mod startup_test {
    /// A peer that answers every request `200` — the subscription `build`
    /// registers, if it registers one.
    async fn peer() -> String {
        let app = axum::Router::new().fallback(|| async { axum::http::StatusCode::OK });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn the_router_builds_as_the_runner_mounts_it() {
        let peer = peer().await;
        let cfg = serde_json::from_value(serde_json::json!({
            "http": {}, "database": {"url": "postgres://startup-test/obsd"},
            "identity": {"tenant": "9900357000004"},
            "marktd": {"url": peer, "api_key": "test"},
            "webhook": {"inbound_secret": "s3cret"},
            "allow_insecure_no_auth": true,
        }))
        .expect("the configuration parses");
        let _app = mako_service::build_for_test::<super::Obsd>(cfg)
            .await
            .expect("the router builds");
    }
}
