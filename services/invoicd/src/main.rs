#![deny(unsafe_code)]

//! `invoicd` — INVOIC plausibility-check daemon (LF role), port `:8280`.
//!
//! `mako_service::run` owns the lifecycle (tracing/OTel, tuned pool, migrations,
//! real DB-ping readiness, health + metrics routes, graceful shutdown); this
//! only names the daemon.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<invoicd::server::Invoicd>().await
}
