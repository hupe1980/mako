#![deny(unsafe_code)]

//! `portald` — customer portal read-model gateway (LF role), port `:9480`.
//!
//! `mako_service::run` owns the lifecycle (tracing/OTel, graceful shutdown,
//! health + metrics routes); this only names the daemon.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<portald::server::Portald>().await
}
