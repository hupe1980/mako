#![deny(unsafe_code)]

//! `accountingd` — the daemon is [`accountingd::Accountingd`]; `mako_service::run` owns its lifecycle.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<accountingd::Accountingd>().await
}
