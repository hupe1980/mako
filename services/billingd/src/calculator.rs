//! Calculator module — now delegates to the [`energy_billing`] crate.
//!
//! All types and functions are re-exported unchanged. `billingd` uses
//! the same public API; nothing in handlers.rs needs updating.
//!
//! [`energy_billing`]: https://docs.rs/energy-billing
pub use energy_billing::*;
