#![deny(unsafe_code)]

//! `invoicd` — INVOIC plausibility-check daemon (LF role).
//!
//! `mako_service::run` owns the lifecycle (tracing, tuned pool with
//! `application_name`, migrations, real DB-ping readiness, graceful shutdown);
//! this only supplies the migrations and the domain router + background workers
//! via [`invoicd::server::build`].

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use mako_service::{Daemon, ServiceContext};

use invoicd::config;

/// The `invoicd` daemon.
struct Invoicd;

impl Daemon for Invoicd {
    type Config = config::Config;
    const NAME: &'static str = "invoicd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run invoicd migrations")?;
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        Ok(())
    }

    async fn build(cfg: Arc<config::Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
        invoicd::server::build(cfg, ctx).await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Invoicd>().await
}
