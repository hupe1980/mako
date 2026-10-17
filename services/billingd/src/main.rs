#![deny(unsafe_code)]

//! `billingd` — retail billing daemon (LF role), port `:9280`.
//!
//! `mako_service::run` owns the lifecycle; [`billingd::daemon::Billingd`]
//! supplies the migrations, the router and the workers.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<billingd::daemon::Billingd>().await
}
