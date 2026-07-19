//! GaBi Gas MMM Allokationsliste Gas — Mehr-/Mindermengen data delivery (MSCONS).
//!
//! Handles inbound MSCONS messages for Gas Mehr-/Mindermengen (MMM) allocation
//! lists as defined in the GaBi Gas 2.1 framework and BDEW AWH Prozesse
//! Mehr-/Mindermengen Strom/Gas:
//!
//! | PID   | Direction | Description |
//! |-------|-----------|-------------|
//! | 13013 | NB → LF   | Marktlokationsscharfe Allokationsliste Gas (MMMA, Gas-only) |
//!
//! # Related ORDERS/ORDRSP PIDs (informational)
//!
//! | PID   | Direction | Description |
//! |-------|-----------|-------------|
//! | 17110 | LF → NB   | Anforderung der Allokationsliste (Gas-only, ⚡=— in ORDERS AHB 1.0) |
//! | 19110 | NB → LF   | Ablehnung der Anforderung Allokationsliste (Gas-only) |
//!
//! These ORDERS/ORDRSP PIDs are listed here for reference; they are also present
//! in `mako-gpke` `gpke-allokationsliste` from a legacy cross-commodity
//! registration. A future cleanup should remove them from `mako-gpke` and
//! register them here exclusively, since both are Gas-only (⚡=— in AHB 1.0).
//!
//! # Regulatory basis
//!
//! - **BK7-24-01-008** — GaBi Gas 2.1 (Mehr-/Mindermengenbilanzierung Gas)
//! - **BDEW AWH Prozesse Mehr-/Mindermengen Strom/Gas V2.1** — Gas Allokationsliste
//!   format and PID definitions (MSCONS AHB)
//! - **MSCONS G1.x** — EDI@Energy metered gas data format

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow key for the GaBi Gas MMM Allokationsliste data delivery process.
pub const WORKFLOW_NAME: &str = "gabi-gas-mmma";

/// ORDERS PID used when LF requests the Gas Allokationsliste from NB.
///
/// Gas-only (⚡=— in ORDERS AHB 1.0). See module-level doc for routing note.
pub const ORDERS_ANFRAGE_PID: u32 = 17110;

/// ORDRSP rejection PID: NB declines the Gas Allokationsliste request.
///
/// Gas-only (⚡=— in ORDRSP AHB 1.0). See module-level doc for routing note.
pub const ORDRSP_ABLEHNUNG_PID: u32 = 19110;

/// MSCONS Prüfidentifikatoren for Gas MMM Allokationsliste data delivery.
///
/// | PID   | Description |
/// |-------|-------------|
/// | 13013 | Marktlokationsscharfe Allokationsliste Gas (Gas-only MMMA, NB → LF) |
///
/// This constant aliases [`mako_edm::GAS_MMMA_PIDS`] for convenience.
/// The canonical source of truth is `mako-edm`.
pub use mako_edm::GAS_MMMA_PIDS as MMMA_MSCONS_PIDS;
