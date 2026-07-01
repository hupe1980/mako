//! `mako-gabi-gas` — GaBi Gas process engine for the German gas market
//! (Gasbilanzierung Gas).
//!
//! # Implemented processes
//!
//! | Process | PIDs | Messages |
//! |---|---|---|
//! | Kapazitätsrechnung (capacity billing) | 31010 | INVOIC |
//!
//! # Note on PID 31011
//!
//! PID 31011 (Rechnung sonstige Leistung / AWH Sperrprozesse Gas, VNB → LFN/LFA)
//! belongs to the **GeLi Gas** domain (BK7-24-01-009) and is implemented in
//! `mako-geli-gas` (`geli-gas-sperrprozesse-invoic` workflow). It is NOT a GaBi
//! Gas (balancing/capacity) process; the direction NB → LF (not NB → BKV)
//! confirms the GeLi Gas context.
//!
//! # Two-crate architecture for GaBi Gas
//!
//! | Crate | Responsibility |
//! |---|---|
//! | `dvgw-edi` | EDIFACT parsing — ALOCAT, NOMINT, NOMRES (parse at transport boundary in `makod`) |
//! | `mako-gabi-gas` | Process engine — Workflow state machines, PID routing, deadline handling |
//!
//! # Domain background
//!
//! **GaBi Gas** (*Gasbilanzierung Gas*) is the German regulatory framework for
//! gas balancing, established by the Bundesnetzagentur (BNetzA) under the
//! Gasnetzzugangsverordnung (GasNZV). The current version, **GaBi Gas 2.0**,
//! entered into force with BNetzA order **BK7-14-020**.
//!
//! The framework governs the exchange of gas quantity data between balance
//! responsible parties (BKV), network operators (FNB/VNB), and market area
//! managers (MGV) via standardised EDIFACT messages.
//!
//! # Market roles
//!
//! | Role | Abbrev. | Description |
//! |------|---------|-------------|
//! | Fernleitungsnetzbetreiber | FNB | Gas transmission system operator |
//! | Verteilnetzbetreiber | VNB | Gas distribution system operator |
//! | Bilanzkreisverantwortlicher | BKV | Balance responsible party |
//! | Marktgebietsverantwortlicher | MGV | Market area manager |
//! | Großhändler / Produzent | GH | Gas wholesaler / producer |
//!
//! # Regulatory references
//!
//! - **GasNZV** (Gasnetzzugangsverordnung) — statutory basis for gas network
//!   access and balancing
//! - **BNetzA BK7-14-020** — GaBi Gas 2.0 ruling (current)
//! - Note: BK7-06-067 is the original **GeLi Gas** ruling, not GaBi Gas
//! - **DVGW G 685** — technical rules for gas metering and allocation

#![deny(unsafe_code)]
#![deny(missing_docs)]

/// GaBi Gas INVOIC billing workflow — PIDs 31010 and 31011.
pub mod invoic;

pub use invoic::{
    GABI_GAS_COMDIS_ABLEHNUNG_PID, GABI_GAS_INVOIC_PIDS, GABI_GAS_REMADV_PID, GaBiGasInvoicCommand,
    GaBiGasInvoicData, GaBiGasInvoicEvent, GaBiGasInvoicProjection, GaBiGasInvoicRecord,
    GaBiGasInvoicState, GaBiGasInvoicWorkflow,
    SETTLEMENT_WINDOW_LABEL as INVOIC_SETTLEMENT_WINDOW_LABEL,
    WORKFLOW_NAME as INVOIC_WORKFLOW_NAME,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the GaBi Gas process family.
///
/// Registers all GaBi Gas INVOIC `Prüfidentifikator` values into the
/// [`mako_engine::pid_router::PidRouter`] at engine startup:
///
/// - PID 31010 → `"gabi-gas-invoic"` ([`GaBiGasInvoicWorkflow`], Kapazitätsrechnung)
/// - PID 33001 → `"gabi-gas-invoic"` (REMADV Zahlungsavis, invoicer role)
/// - PID 29001 → `"gabi-gas-invoic"` (COMDIS Ablehnung REMADV, payer role)
///
/// Note: PID 31011 (Rechnung sonstige Leistung / AWH Sperrprozesse Gas) is
/// handled by `mako-geli-gas` (`geli-gas-sperrprozesse-invoic`), not here.
pub struct GaBiGasModule;

impl mako_engine::builder::EngineModule for GaBiGasModule {
    fn name(&self) -> &'static str {
        "mako-gabi-gas"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &["gabi-gas-invoic"]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // INVOIC billing PIDs — independent of dvgw-edi.
        for &pid in invoic::GABI_GAS_INVOIC_PIDS {
            router.register(pid, "gabi-gas-invoic");
        }

        // REMADV 33001 — inbound payment confirmation (invoicer role).
        //
        // After the FNB/VNB sends INVOIC 31010, the BKV sends REMADV
        // 33001 (Zahlungsavis Bestätigung vollständige Zahlung) to confirm
        // payment. Without this registration, REMADV is silently dropped.
        //
        // Source: REMADV AHB 1.0, GaBi Gas, BK7.
        router.register(invoic::GABI_GAS_REMADV_PID, "gabi-gas-invoic");

        // COMDIS 29001 — inbound Ablehnung REMADV (payer role).
        //
        // The FNB/VNB can reject the BKV's REMADV via COMDIS 29001.
        //
        // Source: COMDIS AHB 1.0, GaBi Gas, BK7.
        router.register(invoic::GABI_GAS_COMDIS_ABLEHNUNG_PID, "gabi-gas-invoic");
    }
}
