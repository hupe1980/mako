#![deny(unsafe_code)]

//! `edmd` — Energy Data Management daemon.
//!
//! The daemon is [`edmd::Edmd`]; `mako_service::run` owns its lifecycle.
//!
//! Port: `:8380`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<edmd::Edmd>().await
}
