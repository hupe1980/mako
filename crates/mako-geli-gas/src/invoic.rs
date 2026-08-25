//! GeLi Gas INVOIC billing for AWH Sperrprozesse Gas — PID 31011
//! (Rechnung sonstige Leistung, VNB → LFN/LFA).
//!
//! The gas network operator (GNB/VNB) invoices the supplier (LFN/LFA) for
//! services rendered during the gas disconnection/reconnection process
//! (AWH = Abrechnungswürdige Handlungen from Sperrprozesse Gas).
//!
//! This is a GeLi Gas (BK7-24-01-009) billing process — **not** GaBi Gas.
//! PID 31010 (Kapazitätsrechnung, VNB → BKV) is the GaBi Gas capacity invoice
//! and belongs to `mako_gabi_gas`.
//!
//! The process lives in [`mako_invoic`]; this module declares the family.
//!
//! # Two roles, one workflow
//!
//! A GNB deployment issues this invoice; an LFG deployment receives one. Both
//! sides live here, as they do for the MSB-Rechnung in `mako_wim`: the process
//! is the same conversation seen from opposite ends, and splitting it would put
//! the REMADV correlation key in two places.
//!
//! # Covered Prüfidentifikatoren (INVOIC AHB / FV2025-10-01, BK7-24-01-009)
//!
//! | PID   | Process                                            | Direction     |
//! |-------|----------------------------------------------------|---------------|
//! | 31011 | Rechnung sonstige Leistung (AWH Sperrprozesse Gas) | VNB → LFN/LFA |
//!
//! 31011 is **Sparte-neutral** — GPKE Teil 2 uses it as well as the AWH
//! Sperrprozesse Gas — which is why its home is a matter of registration rather
//! than of commodity. It sits here because the AWH process is where it is
//! driven from.
//!
//! # Regulatory basis
//!
//! - **BK7-24-01-009** — GeLi Gas 3.0 ruling (Beschluss 12.09.2025)
//! - **INVOIC AHB** — EDI@Energy invoice message format
//! - **§20 Abs. 3 EnWG** — Festlegungskompetenz for gas network access

use mako_engine::types::Pruefidentifikator;
use mako_invoic::{InvoicFamily, InvoicWorkflow};

/// GeLi Gas AWH billing PID handled by this workflow (INVOIC AHB).
///
/// | PID   | Name                                                               |
/// |-------|--------------------------------------------------------------------|
/// | 31011 | Rechnung sonstige Leistung (AWH Sperrprozesse Gas, VNB → LFN/LFA) |
pub const SPERRPROZESSE_INVOIC_PID: Pruefidentifikator = Pruefidentifikator::const_new(31011);

/// The PID set, as the family declares it.
pub const SPERRPROZESSE_INVOIC_PIDS: &[u32] = &[31011];

/// Workflow key used for PID router registration.
pub const WORKFLOW_NAME: &str = "geli-gas-sperrprozesse-invoic";

/// REMADV Prüfidentifikatoren that answer a 31011 invoice.
///
/// **33001 is the only Zahlungsbestätigung.** 33002, 33003 and 33004 are all
/// Abweisungen — 33003 and 33004 are the itemised rejections, not partial
/// payments, and treating either as a confirmation books money that never
/// arrived.
pub const SPERRPROZESSE_REMADV_PIDS: &[u32] = mako_invoic::REMADV_PIDS;

/// Deadline label for the GeLi Gas AWH INVOIC settlement response window.
///
/// Register a [`mako_engine::deadline::Deadline`] with this label once the
/// invoice validates, so the workflow can enforce the contractual settlement
/// deadline.
pub const SETTLEMENT_WINDOW_LABEL: &str = "geli-gas-sperrprozesse-invoic-settlement";

/// The GeLi Gas AWH Sperrprozesse billing family.
///
/// Both roles ship, so the issuer side is available; the AWH process publishes
/// no COMDIS leg.
pub struct GeliGasSperrprozesseInvoic;

impl InvoicFamily for GeliGasSperrprozesseInvoic {
    const WORKFLOW_NAME: &'static str = WORKFLOW_NAME;
    const DEADLINE_LABEL: &'static str = SETTLEMENT_WINDOW_LABEL;
    const INVOIC_PIDS: &'static [u32] = SPERRPROZESSE_INVOIC_PIDS;
    const SENDS_INVOIC: bool = true;
    const ANSWERS_COMDIS: bool = false;
}

/// The GeLi Gas AWH Sperrprozesse INVOIC billing workflow (PID 31011).
pub type GeliGasSperrprozesseInvoicWorkflow = InvoicWorkflow<GeliGasSperrprozesseInvoic>;
