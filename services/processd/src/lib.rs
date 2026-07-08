//! `processd` — Process decision engine for German energy market automation.
//!
//! # Role
//!
//! Consumes `de.mako.process.initiated` CloudEvents from `marktd` and applies
//! role-specific policy to make decisions within regulatory deadlines.
//!
//! ## LF module (`role-lf-strom`, `role-lf-gas`)
//!
//! Handles LFA (alter Lieferant) obligations under LFW24 (BK6-22-024):
//! - **E_0624 auto-response** (PID 55008): within 45 minutes of receipt.
//! - Evaluation from `VersorgungsStatus` without ERP involvement.
//! - Escalates to `approval_queue` when data is insufficient.
//!
//! ## NB module (`role-nb-strom`, `role-nb-gas`)
//!
//! Handles NB (Netzbetreiber) Anmeldung STP decisions:
//! - **GPKE Anmeldung** (PIDs 55001, 55016): 24 wall-clock hour deadline.
//! - **GeLi Gas Anmeldung** (PID 44001): 10 Werktage deadline.
//! - Evaluation via `netz-checker` pure library (6 deterministic checks).
//! - STP target ≥ 95 % (requires `mastr-syncd` for grid record coverage).
//!
//! # Regulatory basis
//!
//! - GPKE: BK6-22-024 §5 (LFW24)
//! - GeLi Gas: BK7-24-01-009
//! - §20 EnWG: `initiator_is_affiliate` recorded for every decision
#![deny(unsafe_code)]
#![allow(clippy::doc_markdown)]

pub mod config;
pub mod handler;
pub mod mcp_server;
pub mod pg;
pub mod server;

#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
pub mod nb_module;

#[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
pub mod lf_module;
