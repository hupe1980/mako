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
