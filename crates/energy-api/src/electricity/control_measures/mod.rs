//! Grid control commands (Steuerungshandlungen) — `controlMeasuresV1.yaml`.
//!
//! Covers the universal order process for smart-meter-based load control
//! commands between distribution network operators (NB), suppliers (LF) and
//! metering point operators (MSB).
//!
//! ## Role matrix
//!
//! | Role | Sends                                    | Receives                              |
//! |------|------------------------------------------|---------------------------------------|
//! | NB/LF| `send_konfiguration`, `send_initial_zustand` | preliminary/final responses, info |
//! | MSB  | preliminary/final responses, info       | commands from NB/LF                   |
//!
//! Enable features `client` / `server` for the respective implementations.

pub use crate::types::electricity::{
    CommandControl, CommandRegular, LocationId, MaximumPowerValue, NeloId,
    PreliminaryStatePositive, ReasonNegative, SrId, StateNegative, StatePositive, StateUnknown,
    TransactionId, InitialTransactionId, ReferenceId,
};

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::ControlMeasuresClient;

#[cfg(feature = "server")]
pub mod server;
