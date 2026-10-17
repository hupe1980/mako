#![deny(unsafe_code)]

//! `productd` — the daemon is [`productd::Productd`]; `mako_service::run` owns its lifecycle.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<productd::Productd>().await
}
