//! Concrete DVGW message type implementations.
//!
//! Each sub-module is feature-gated. Only the modules corresponding to enabled
//! Cargo features are compiled.

#[cfg(feature = "alocat")]
pub mod alocat;
#[cfg(feature = "delord")]
pub mod delord;
#[cfg(feature = "delres")]
pub mod delres;
#[cfg(feature = "imbnot")]
pub mod imbnot;
#[cfg(feature = "nomint")]
pub mod nomint;
#[cfg(feature = "nomres")]
pub mod nomres;
#[cfg(feature = "schedl")]
pub mod schedl;
#[cfg(feature = "tranot")]
pub mod tranot;
