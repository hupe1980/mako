//! The process read model, under the one processd test binary.

mod rebuild;
mod startup_posture;
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
mod webhook_fanout;
mod wire_shape;
mod worker_integration;
