//! GPKE Netznutzungsabrechnung / Mehr-Mindermengen — the INVOIC billing family.
//!
//! The process itself lives in [`mako_invoic`]: receive an invoice, validate it
//! against the AHB, settle or dispute it, and — in the issuer role — correlate
//! the payer's REMADV and any COMDIS answering it. Nothing in that is
//! GPKE-specific, so this module declares only what *is*: which PIDs belong to
//! the family, which roles a GPKE deployment plays, and the deadline it answers
//! under.
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB 2.8e / AHB 1.0)
//!
//! | PID   | Process variant                          |
//! |-------|------------------------------------------|
//! | 31001 | Abschlagsrechnung (Netznutzung)          |
//! | 31002 | NN-Rechnung (Netznutzungsabrechnung)     |
//! | 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)   |
//! | 31006 | MMM-Rechnung (selbst ausgestellt)        |
//!
//! Neighbouring PIDs deliberately belong elsewhere, and must not be registered
//! here or they would be double-routed:
//!
//! - **31003** (WiM-Rechnung) and **31009** (MSB-Rechnung, GPKE Teil 3 *and*
//!   WiM Strom Teil 1) → `mako_wim::invoic`.
//! - **31007 / 31008** (Aggreg. MMM-Rechnung Gas, NB → MGV) → `mako_gabi_gas`;
//!   MGV is a Gas-only role.
//! - **31004** (Stornorechnung) is a Sparte-neutral universal Storno
//!   (INVOIC AHB § 3.1.2), checked Sparte-neutrally by `invoicd`.
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE** — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität
//! - **INVOIC AHB 2.8e / 1.0** — the invoice message format
//! - **REMADV AHB 1.0a § 3**, **COMDIS AHB 1.0** — the payment answer and its
//!   refusal; see [`mako_invoic`] for how the two are read.

use mako_engine::types::Pruefidentifikator;
use mako_invoic::{InvoicFamily, InvoicWorkflow};

/// GPKE billing Prüfidentifikatoren (INVOIC-based), FV2025-10-01 onwards.
pub const GPKE_INVOIC_PIDS: &[u32] = &[31001, 31002, 31005, 31006];

/// REMADV Prüfidentifikatoren answering a GPKE invoice.
///
/// The shared [`mako_invoic::REMADV_PIDS`] set: only 33001 confirms payment.
pub const GPKE_REMADV_PIDS: &[u32] = mako_invoic::REMADV_PIDS;

/// COMDIS Prüfidentifikator for an inbound Ablehnung of a REMADV (payer side).
pub const GPKE_COMDIS_ABLEHNUNG_PID: Pruefidentifikator = mako_invoic::COMDIS_ABLEHNUNG_PID;

/// Deadline label for the INVOIC settlement response window.
///
/// The recipient must settle or dispute within the contractual period
/// (typically 5 Werktage from receipt). Register a `Deadline` with this label
/// once the invoice validates.
pub const ABRECHNUNG_WINDOW_LABEL: &str = "invoic-settlement-deadline";

/// Canonical workflow name registered in the process engine.
pub const WORKFLOW_NAME: &str = "gpke-abrechnung";

/// The GPKE billing family.
///
/// A GPKE deployment plays both roles: the NB issues Netznutzungs- and
/// MMM-Rechnungen and receives the LF's REMADV, and the LF receives invoices
/// and may have its own REMADV refused by COMDIS.
pub struct GpkeAbrechnung;

impl InvoicFamily for GpkeAbrechnung {
    const WORKFLOW_NAME: &'static str = WORKFLOW_NAME;
    const DEADLINE_LABEL: &'static str = ABRECHNUNG_WINDOW_LABEL;
    const INVOIC_PIDS: &'static [u32] = GPKE_INVOIC_PIDS;
    const SENDS_INVOIC: bool = true;
    const ANSWERS_COMDIS: bool = true;
}

/// The GPKE Netznutzungsabrechnung workflow.
pub type GpkeAbrechnungWorkflow = InvoicWorkflow<GpkeAbrechnung>;
