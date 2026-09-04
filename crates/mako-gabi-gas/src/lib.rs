//! `mako-gabi-gas` — GaBi Gas process engine for the German gas market
//! (Gasbilanzierung Gas).
//!
//! # Implemented processes
//!
//! | Process | PIDs | Messages |
//! |---|---|---|
//! | Kapazitätsrechnung (capacity billing) | 31010 | INVOIC |
//! | Aggreg. MMM-Rechnung Gas (NB → MGV) | 31007, 31008 | INVOIC |
//! | Allokationsliste Gas (MSCONS data delivery) | 13013 | MSCONS |
//! | Allokation (NB/MGV/BKV, receive-and-record) | 70001–70023 | ALOCAT (DVGW) |
//! | Nominierung (Transportkunde ↔ NB/MGV, both ends) | 70030–70039 | NOMINT / NOMRES (DVGW) |
//! | Mehr-/Mindermengenmeldung (NB → MGV, both ends) | 70095, 70096 | SSQNOT (DVGW) |
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
//! | `dvgw-edi` | EDIFACT parsing — ALOCAT, NOMINT, NOMRES, SSQNOT (parse at transport boundary in `makod`) |
//! | `mako-gabi-gas` | Process engine — Workflow state machines, PID routing, deadline handling |
//!
//! # Domain background
//!
//! **GaBi Gas** (*Gasbilanzierung Gas*) is the German regulatory framework for
//! gas balancing, established by the Bundesnetzagentur (BNetzA) under the
//! Gasnetzzugangsverordnung (GasNZV). The current version, **GaBi Gas 2.1**,
//! entered into force with BNetzA order **BK7-24-01-008**.
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
//! - **§20 Abs. 3 EnWG** — the Festlegungskompetenz for gas network
//!   access and balancing
//! - **BNetzA BK7-24-01-008** — GaBi Gas 2.1 ruling (current)
//! - Note: BK7-06-067 is the original **GeLi Gas** ruling, not GaBi Gas
//! - **DVGW G 685** — technical rules for gas metering and allocation

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic, clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)] // German MaKo terms and BDEW acronyms produce many false positives
#![allow(clippy::too_many_lines)] // process handle() functions are necessarily verbose
#![allow(clippy::match_same_arms)] // sometimes intentional for process-family readability
#![allow(clippy::manual_let_else)] // existing code style; rewrite in follow-up
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::unnested_or_patterns)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::items_after_statements)]

/// Core GaBi Gas domain types — [`GasDay`], [`GasQuantity`], [`GasBeschaffenheit`],
/// [`Bilanzkreis`], [`NominationQuantity`], [`GasImbalanceSaldo`], etc.
pub mod domain;

/// GaBi Gas portfolio balancing — [`GasMarketRole`], [`GasPortfolioBalance`],
/// [`PortfolioPosition`]. BKV portfolio aggregation across Bilanzkreise.
pub mod portfolio;

/// GaBi Gas INVOIC billing workflow — PIDs 31010, 31007, 31008.
pub mod invoic;

/// GaBi Gas Nomination workflow — NOMINT/NOMRES (Transportkunde ↔ NB/MGV, PIDs 70030–70039).
pub mod nomination;

/// GaBi Gas Allocation workflow — ALOCAT receive-and-record (PIDs 70001–70023).
pub mod allocation;

/// GaBi Gas MMM Allokationsliste Gas — Mehr-/Mindermengen data delivery (MSCONS 13013).
pub mod mmma;

/// GaBi Gas Mehr-/Mindermengenmeldung — SSQNOT receive-and-record (PIDs 70095/70096).
pub mod mehr_mindermengen;

// ── Domain re-exports ─────────────────────────────────────────────────────────

pub use domain::{
    AllokationsSerie,
    Bilanzkreis,
    DeliveryPoint,
    DeliveryPointDirection,
    GasBeschaffenheit,
    GasBeschaffenheitValidationError,
    GasDay,
    GasImbalanceSaldo,
    GasQualityClass,
    GasQualityFlag,
    GasQuantity,
    ImbalanceDirection as GasImbalanceDirection,
    NominationQuantity,
    // CloudEvent type constants (de.gabi.*)
    cloud_events as gabi_cloud_events,
};
pub use portfolio::{ConservationViolation, GasMarketRole, GasPortfolioBalance, PortfolioPosition};

pub use allocation::{
    ALLOCATION_PIDS, AllocationCommand, AllocationData, AllocationEvent, AllocationState,
    AllocationType, AllocationVersion, FINAL_ALOCAT_DEADLINE_LABEL, GaBiGasAllocationWorkflow,
    WORKFLOW_NAME as ALLOCATION_WORKFLOW_NAME,
};
pub use invoic::{
    COMDIS_RESUME_PATH as INVOIC_COMDIS_RESUME_PATH, GABI_GAS_COMDIS_ABLEHNUNG_PID,
    GABI_GAS_INVOIC_PIDS, GaBiGasInvoic, GaBiGasInvoicWorkflow,
    SETTLEMENT_WINDOW_LABEL as INVOIC_SETTLEMENT_WINDOW_LABEL,
    WORKFLOW_NAME as INVOIC_WORKFLOW_NAME,
};
pub use mehr_mindermengen::{
    GaBiGasMehrMindermengenWorkflow, MEHR_MINDERMENGEN_PIDS, MehrMindermengenCommand,
    MehrMindermengenData, MehrMindermengenEvent, MehrMindermengenState, MmmVerfahren,
    WORKFLOW_NAME as MEHR_MINDERMENGEN_WORKFLOW_NAME,
};
pub use mmma::{
    MMMA_MSCONS_PIDS, ORDERS_ANFRAGE_PID as MMMA_ORDERS_ANFRAGE_PID,
    ORDRSP_ABLEHNUNG_PID as MMMA_ORDRSP_ABLEHNUNG_PID, WORKFLOW_NAME as MMMA_WORKFLOW_NAME,
};
pub use nomination::{
    GaBiGasNominationWorkflow, NOMINATION_PIDS, NOMINT_PIDS, NOMRES_DEADLINE_LABEL, NOMRES_PIDS,
    NominationCommand, NominationCounterparty, NominationData, NominationEvent, NominationMenge,
    NominationPosition, NominationRichtung, NominationState, NomresAcceptance, Renominierung,
    WORKFLOW_NAME as NOMINATION_WORKFLOW_NAME, nomination_process_key, nomint_payload,
    nomres_payload, single_direction_energy,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the GaBi Gas process family.
///
/// Registers all GaBi Gas `Prüfidentifikator` values into the
/// [`mako_engine::pid_router::PidRouter`] at engine startup:
///
/// **INVOIC billing (BDEW / edi-energy):**
/// - PID 31010 → `"gabi-gas-invoic"` ([`GaBiGasInvoicWorkflow`], Kapazitätsrechnung, FNB/VNB → BKV)
/// - PID 31007 → `"gabi-gas-invoic"` (Aggreg. MMM-Rechnung Gas, NB → MGV; Gas-only)
/// - PID 31008 → `"gabi-gas-invoic"` (Aggreg. MMM-selbst ausgest. Rechnung Gas, NB → MGV; Gas-only)
/// - PID 33001 → `"gabi-gas-invoic"` (REMADV Zahlungsavis, invoicer role)
/// - PID 29001 → `"gabi-gas-invoic"` (COMDIS Ablehnung REMADV, payer role)
///
/// **MMM Allokationsliste Gas (MSCONS):**
/// - PID 13013 → `"gabi-gas-mmma"` (Marktlokationsscharfe Allokationsliste Gas, NB → LF; Gas-only)
///
/// **DVGW gas transport (Prüfidentifikatoren from `SG1 RFF+Z13`):**
/// - PIDs 70001–70023 → `"gabi-gas-allocation"` (ALOCAT)
/// - PIDs 70030–70039 → `"gabi-gas-nomination"` (NOMINT / NOMRES)
/// - PIDs 70095–70096 → `"gabi-gas-mehr-mindermengen"` (SSQNOT)
///
/// The DVGW formats this crate does **not** cover — SCHEDL, IMBNOT, TRANOT,
/// DELORD/DELRES, CHACAP, NUEVOR, SLPASP and TSIMSG — have no workflow and no
/// Prüfidentifikator here. `dvgw-edi` cannot parse them, so a workflow for one
/// would be unreachable and its registration would overstate what the router
/// handles.
///
/// Note: PID 31011 (Rechnung sonstige Leistung / AWH Sperrprozesse Gas) is
/// handled by `mako-geli-gas` (`geli-gas-sperrprozesse-invoic`), not here.
pub struct GaBiGasModule;

impl mako_engine::builder::EngineModule for GaBiGasModule {
    fn name(&self) -> &'static str {
        "mako-gabi-gas"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        // Every entry is the owning module's own constant. A literal here can
        // disagree with the name `register_pids` routes to, and the two are
        // checked against each other only at `EngineBuilder::build`.
        &[
            invoic::WORKFLOW_NAME,
            nomination::WORKFLOW_NAME,
            allocation::WORKFLOW_NAME,
            mmma::WORKFLOW_NAME,
            mehr_mindermengen::WORKFLOW_NAME,
        ]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // INVOIC billing PIDs — independent of dvgw-edi.
        for &pid in invoic::GABI_GAS_INVOIC_PIDS {
            router.register(pid, "gabi-gas-invoic");
        }

        // No REMADV registration: every GaBi Gas INVOIC arrives *at* this
        // platform, so the REMADV answering it is one this platform sends. The
        // inbound direction does not exist for the roles modelled here — see
        // the module docs on `invoic`.

        // COMDIS 29001 — inbound Ablehnung REMADV (payer role).
        //
        // The FNB/VNB can reject the BKV's REMADV via COMDIS 29001.
        //
        // Source: COMDIS AHB 1.0, GaBi Gas, BK7.
        router.register(
            invoic::GABI_GAS_COMDIS_ABLEHNUNG_PID.as_u32(),
            "gabi-gas-invoic",
        );

        // NOMINT / NOMRES Prüfidentifikatoren (DVGW, 70030–70039).
        for &pid in nomination::NOMINATION_PIDS {
            router.register(pid, "gabi-gas-nomination");
        }

        // ALOCAT Prüfidentifikatoren (DVGW, 70001–70023).
        for &pid in allocation::ALLOCATION_PIDS {
            router.register(pid, "gabi-gas-allocation");
        }

        // SSQNOT Prüfidentifikatoren (DVGW, 70095/70096) — NB → MGV.
        for &pid in mehr_mindermengen::MEHR_MINDERMENGEN_PIDS {
            router.register(pid, mehr_mindermengen::WORKFLOW_NAME);
        }

        // MMM Allokationsliste Gas — MSCONS 13013 (NB → LF, Gas-only).
        //
        // MGV (Marktgebietsverantwortlicher) and the Gas MMM process are
        // Gas-domain only, so 13013 belongs here and not to `mako-gpke`.
        // PIDs 17110/19110 (ORDERS/ORDRSP) are informational; see `mmma` module doc.
        for &pid in mmma::MMMA_MSCONS_PIDS {
            router.register(pid, "gabi-gas-mmma");
        }
    }
}
