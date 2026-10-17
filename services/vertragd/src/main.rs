//! `vertragd` — B2C and B2B contract & customer management (LF role), `:9780`.
//!
//! See the crate documentation in `lib.rs` for the data model and the two
//! durability rails. This binary owns only the wiring: fail-closed
//! authentication checks, the router, and the background workers.

#![deny(unsafe_code)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<vertragd::Vertragd>().await
}
