//! Concrete DVGW message type implementations.
//!
//! Each sub-module is feature-gated. Only the modules corresponding to enabled
//! Cargo features are compiled.

#[cfg(feature = "alocat")]
pub mod alocat;
#[cfg(feature = "nomint")]
pub mod nomint;
#[cfg(feature = "nomres")]
pub mod nomres;
