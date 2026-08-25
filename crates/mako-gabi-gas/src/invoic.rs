//! GaBi Gas Rechnung — the INVOIC billing family of gas balancing.
//!
//! The process lives in [`mako_invoic`]; this module declares which PIDs belong
//! to GaBi Gas and which roles a GaBi Gas deployment plays.
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB, BK7-24-01-008 GaBi Gas 2.1)
//!
//! | PID   | Process                                                          | Direction   |
//! |-------|------------------------------------------------------------------|-------------|
//! | 31010 | Kapazitätsrechnung                                                | FNB/VNB → BKV |
//! | 31007 | Aggreg. MMM-Rechnung Gas                                          | NB → MGV    |
//! | 31008 | Aggreg. MMM-Rechnung Gas, selbst ausgestellt                      | NB → MGV    |
//!
//! PIDs 31007/31008 belong here and not to `mako-gpke`: the MGV
//! (Marktgebietsverantwortlicher) is a Gas-only market role with no Strom
//! counterpart.
//!
//! # This family receives invoices; it does not issue them
//!
//! All three PIDs arrive *at* the role this platform plays — the BKV receives
//! the Kapazitätsrechnung, the MGV the aggregated MMM-Rechnung — and nothing
//! here renders one. [`mako_invoic::InvoicFamily::SENDS_INVOIC`] is therefore
//! `false`, so `SendInvoic` and `ReceiveRemadv` are refused: after *receiving*
//! an invoice this platform is the one that **sends** the REMADV, and no
//! REMADV PID routes here.
//!
//! COMDIS 29001 does: it is the invoicer refusing *our* REMADV, which is
//! genuinely inbound for a payer.
//!
//! # Regulatory basis
//!
//! - **BK7-24-01-008** — GaBi Gas 2.1
//! - **INVOIC AHB 1.0** — the invoice message format
//! - **COMDIS AHB 1.0** — Ablehnung eines Zahlungsavis

use mako_engine::types::Pruefidentifikator;
use mako_invoic::{InvoicFamily, InvoicWorkflow};

/// GaBi Gas billing Prüfidentifikatoren.
pub const GABI_GAS_INVOIC_PIDS: &[u32] = &[
    31010, // Kapazitätsrechnung (FNB/VNB → BKV)
    31007, // Aggreg. MMM-Rechnung Gas (NB → MGV)
    31008, // Aggreg. MMM-Rechnung Gas, selbst ausgestellt (NB → MGV)
];

/// COMDIS Prüfidentifikator for an inbound Ablehnung of our REMADV (payer role).
pub const GABI_GAS_COMDIS_ABLEHNUNG_PID: Pruefidentifikator = mako_invoic::COMDIS_ABLEHNUNG_PID;

/// Workflow key used for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-invoic";

/// Resume-path workflow name for COMDIS messages, used in startup adapter
/// validation.
pub const COMDIS_RESUME_PATH: &str = "gabi-gas-invoic";

/// Deadline label for the settlement response window.
pub const SETTLEMENT_WINDOW_LABEL: &str = "gabi-gas-invoic-settlement-deadline";

/// The GaBi Gas billing family — payer side only.
pub struct GaBiGasInvoic;

impl InvoicFamily for GaBiGasInvoic {
    const WORKFLOW_NAME: &'static str = WORKFLOW_NAME;
    const DEADLINE_LABEL: &'static str = SETTLEMENT_WINDOW_LABEL;
    const INVOIC_PIDS: &'static [u32] = GABI_GAS_INVOIC_PIDS;
    const SENDS_INVOIC: bool = false;
    const ANSWERS_COMDIS: bool = true;
}

/// The GaBi Gas billing workflow (PIDs 31010, 31007, 31008).
pub type GaBiGasInvoicWorkflow = InvoicWorkflow<GaBiGasInvoic>;
