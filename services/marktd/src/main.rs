#![deny(unsafe_code)]

//! `marktd` — the market data hub, port `:8180`.
//!
//! `mako_service::run` owns the lifecycle; [`marktd::daemon::Marktd`]
//! supplies the migrations, the router and the workers.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<marktd::daemon::Marktd>().await
}
