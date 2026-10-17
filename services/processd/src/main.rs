#![deny(unsafe_code)]

//! `processd` — Process decision engine for the German energy market
//! (decision modules, process read model, Sperrauftrag queue).
//!
//! The daemon is [`processd::Processd`]; `mako_service::run` owns its lifecycle.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<processd::Processd>().await
}
