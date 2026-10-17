#![deny(unsafe_code)]

//! `einsd` — the daemon is [`einsd::Einsd`]; `mako_service::run` owns its lifecycle.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<einsd::Einsd>().await
}
