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

/// GaBi Gas Nomination workflow — NOMINT/NOMRES (BKV ↔ FNB/MGV, PIDs 90011/90012/90021/90022).
pub mod nomination;

/// GaBi Gas Allocation workflow — ALOCAT receive-and-record (PIDs 90001/90002/90003).
pub mod allocation;

/// GaBi Gas SCHEDL workflow — day-ahead transport schedule receive-and-record (PID 90031).
pub mod schedl;

/// GaBi Gas IMBNOT workflow — imbalance notification receive-and-record (PID 90041).
pub mod imbnot;

/// GaBi Gas MMM Allokationsliste Gas — Mehr-/Mindermengen data delivery (MSCONS 13013).
pub mod mmma;

/// GaBi Gas TRANOT workflow — transport notification receive-and-record (PID 90051).
pub mod tranot;

/// GaBi Gas DELORD/DELRES workflow — delivery order and response (PIDs 90061/90062).
pub mod delord;

// ── Domain re-exports ─────────────────────────────────────────────────────────

pub use domain::{
    Bilanzkreis,
    DeliveryPoint,
    DeliveryPointDirection,
    DvgwFormatVersion,
    DvgwMessageType,
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
    // DVGW format versions
    dvgw_versions,
};
pub use portfolio::{ConservationViolation, GasMarketRole, GasPortfolioBalance, PortfolioPosition};

pub use allocation::{
    ALLOCATION_PIDS, AllocationCommand, AllocationData, AllocationEvent, AllocationState,
    AllocationType, AllocationVersion, GaBiGasAllocationWorkflow,
    WORKFLOW_NAME as ALLOCATION_WORKFLOW_NAME,
};
pub use delord::{
    DELIVERY_ORDER_PIDS, DELORD_PID, DELRES_DEADLINE_LABEL, DELRES_PID, DeliveryOrderCommand,
    DeliveryOrderData, DeliveryOrderEvent, DeliveryOrderState, DelresStatus,
    GaBiGasDeliveryOrderWorkflow, WORKFLOW_NAME as DELIVERY_ORDER_WORKFLOW_NAME,
};
pub use imbnot::{
    GaBiGasImbalanceWorkflow, IMBNOT_PID, IMBNOT_PIDS, ImbalanceCommand, ImbalanceData,
    ImbalanceDirection, ImbalanceEvent, ImbalanceState, WORKFLOW_NAME as IMBNOT_WORKFLOW_NAME,
};
pub use invoic::{
    COMDIS_RESUME_PATH as INVOIC_COMDIS_RESUME_PATH, GABI_GAS_COMDIS_ABLEHNUNG_PID,
    GABI_GAS_INVOIC_PIDS, GABI_GAS_REMADV_PID, GaBiGasInvoicCommand, GaBiGasInvoicData,
    GaBiGasInvoicEvent, GaBiGasInvoicProjection, GaBiGasInvoicRecord, GaBiGasInvoicState,
    GaBiGasInvoicWorkflow, REMADV_RESUME_PATH as INVOIC_REMADV_RESUME_PATH,
    SETTLEMENT_WINDOW_LABEL as INVOIC_SETTLEMENT_WINDOW_LABEL,
    WORKFLOW_NAME as INVOIC_WORKFLOW_NAME,
};
pub use mmma::{
    MMMA_MSCONS_PIDS, ORDERS_ANFRAGE_PID as MMMA_ORDERS_ANFRAGE_PID,
    ORDRSP_ABLEHNUNG_PID as MMMA_ORDRSP_ABLEHNUNG_PID, WORKFLOW_NAME as MMMA_WORKFLOW_NAME,
};
pub use nomination::{
    GaBiGasNominationWorkflow, NOMINATION_PIDS, NOMINT_PIDS, NOMRES_DEADLINE_LABEL, NOMRES_PIDS,
    NominationCommand, NominationCounterparty, NominationData, NominationEvent, NominationState,
    NomresAcceptance, WORKFLOW_NAME as NOMINATION_WORKFLOW_NAME,
};
pub use schedl::{
    GaBiGasSchedlWorkflow, SCHEDL_PID, SCHEDL_PIDS, SchedlCommand, SchedlData, SchedlEvent,
    SchedlState, WORKFLOW_NAME as SCHEDL_WORKFLOW_NAME,
};
pub use tranot::{
    GaBiGasTransportNotificationWorkflow, TRANOT_PID, TRANOT_PIDS, TransportNotificationCommand,
    TransportNotificationData, TransportNotificationEvent, TransportNotificationState,
    TransportNotificationType, WORKFLOW_NAME as TRANOT_WORKFLOW_NAME,
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
/// **Nomination — NOMINT/NOMRES (DVGW synthetic PIDs):**
/// - PID 90011 → `"gabi-gas-nomination"` (NOMINT BKV → FNB)
/// - PID 90012 → `"gabi-gas-nomination"` (NOMINT BKV → MGV)
/// - PID 90021 → `"gabi-gas-nomination"` (NOMRES FNB → BKV)
/// - PID 90022 → `"gabi-gas-nomination"` (NOMRES MGV → BKV)
///
/// **Allocation — ALOCAT (DVGW synthetic PIDs):**
/// - PID 90001 → `"gabi-gas-allocation"` (ALOCAT FNB → BKV daily)
/// - PID 90002 → `"gabi-gas-allocation"` (ALOCAT MGV → BKV monthly)
/// - PID 90003 → `"gabi-gas-allocation"` (ALOCAT VNB → FNB sub-daily)
///
/// **Schedule — SCHEDL (DVGW synthetic PID):**
/// - PID 90031 → `"gabi-gas-schedl"` (SCHEDL transport schedule, receive-and-record)
///
/// **Imbalance notification — IMBNOT (DVGW synthetic PID):**
/// - PID 90041 → `"gabi-gas-imbnot"` (IMBNOT FNB/MGV → BKV)
///
/// **Transport notification — TRANOT (DVGW synthetic PID):**
/// - PID 90051 → `"gabi-gas-tranot"` (TRANOT FNB/VNB → BKV/GH/MGV)
///
/// **Delivery order — DELORD/DELRES (DVGW synthetic PIDs):**
/// - PID 90061 → `"gabi-gas-delivery-order"` (DELORD BKV/GH → FNB/MGV)
/// - PID 90062 → `"gabi-gas-delivery-order"` (DELRES FNB/MGV → BKV/GH)
///
/// Note: PID 31011 (Rechnung sonstige Leistung / AWH Sperrprozesse Gas) is
/// handled by `mako-geli-gas` (`geli-gas-sperrprozesse-invoic`), not here.
pub struct GaBiGasModule;

impl mako_engine::builder::EngineModule for GaBiGasModule {
    fn name(&self) -> &'static str {
        "mako-gabi-gas"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            "gabi-gas-invoic",
            "gabi-gas-nomination",
            "gabi-gas-allocation",
            "gabi-gas-schedl",
            "gabi-gas-imbnot",
            "gabi-gas-tranot",
            "gabi-gas-delivery-order",
            "gabi-gas-mmma",
        ]
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

        // NOMINT / NOMRES synthetic PIDs (DVGW, 90011/90012/90021/90022).
        for &pid in nomination::NOMINATION_PIDS {
            router.register(pid, "gabi-gas-nomination");
        }

        // ALOCAT synthetic PIDs (DVGW, 90001/90002/90003).
        for &pid in allocation::ALLOCATION_PIDS {
            router.register(pid, "gabi-gas-allocation");
        }

        // SCHEDL transport schedule (DVGW, 90031).
        router.register(schedl::SCHEDL_PID, "gabi-gas-schedl");

        // IMBNOT imbalance notification (DVGW, 90041).
        router.register(imbnot::IMBNOT_PID, "gabi-gas-imbnot");

        // TRANOT transport notification (DVGW, 90051).
        router.register(tranot::TRANOT_PID, "gabi-gas-tranot");

        // DELORD / DELRES delivery order cycle (DVGW, 90061/90062).
        for &pid in delord::DELIVERY_ORDER_PIDS {
            router.register(pid, "gabi-gas-delivery-order");
        }

        // MMM Allokationsliste Gas — MSCONS 13013 (NB → LF, Gas-only).
        //
        // PID 13013 was previously misassigned to `mako-gpke` `gpke-allokationsliste`.
        // MGV (Marktgebietsverantwortlicher) and the Gas MMM process are Gas-domain only.
        // PIDs 17110/19110 (ORDERS/ORDRSP) are informational; see `mmma` module doc.
        for &pid in mmma::MMMA_MSCONS_PIDS {
            router.register(pid, "gabi-gas-mmma");
        }
    }
}
