//! `mako-mabis` — MABIS (Marktprozesse für Bilanzkreis- und
//! Aggregationsverantwortliche) process engine for German electricity market
//! balance group accounting (BDEW MaKo).
//!
//! ## Process family
//!
//! MABIS governs the billing and settlement processes for
//! Bilanzkreisverantwortliche (BKV) in the German **electricity** balancing
//! market. There is no gas component in MABIS.
//!
//! | Process | PID | Status |
//! |---|---|---|
//! | Bilanzkreisabrechnung Strom | **13003** | ✅ Implemented |
//! | Clearingliste DZR (BIKO → NB/ÜNB) | **55069** | ✅ Implemented |
//! | Clearingliste BAS (BIKO → BKV) | **55070** | ✅ Implemented |
//! | Lieferantenclearingliste (NB → LF) | **55065** | ✅ Implemented |
//! | UTILTS aggregation (ÜNB → BIKO) | — | ✅ `utilts_aggregation` |
//!
//! > **PID 13003** is the MSCONS Summenzeitreihe PID (`"Summenzeitreihen und
//! > Ausfallarbeitssummen"`), confirmed in MSCONS AHB 2.4c/2.5 (all FV versions)
//! > and the BDEW MSCONS AHB 3.1g PDF §5 page 14. The `edi-energy` MSCONS
//! > profiles contain full segment rules for PID 13003 from FV2024-04-01 onward.
//!
//! ## PIDs 13002–13028 belong to Messwesen, not MABIS
//!
//! The `edi-energy` MSCONS AHB profiles contain PIDs 13002–13028. These are
//! **Messwerten-PIDs** (meter data exchange) — e.g. 13002 "Messwerte
//! Zählerstand Gas", 13017 "Messwerte Zählerstand Strom" — and belong to the
//! Messwesen process family, not to MABIS billing. They must not be registered
//! under any `"mabis-billing"` workflow identifier.
//!
//! ## Architecture
//!
//! Each BDEW process variant is a separate [`mako_engine::workflow::Workflow`]
//! implementation. This crate contains **only pure domain logic** — no I/O,
//! no EDIFACT parsing, no network calls.
//!
//! ## Key difference from supplier-switch processes
//!
//! | Aspect | GPKE / WiM / GeLi Gas | MABIS |
//! |---|---|---|
//! | Trigger | Single inbound EDIFACT | **Abrechnungssummenzeitreihe from BIKO** |
//! | Location scope | Single MeLo / MaLo | **Many MaLo streams per Bilanzkreis** |
//! | Message types | UTILMD + APERAK | **MSCONS Summenzeitreihen + Prüfmitteilung** |
//! | Counterparty | NB / LFA | **BIKO (Bilanzkoordinator)** |
//! | Response Frist | 24 h / 5 Wkt / 10 Wkt | **1 Werktag (Prüfmitteilung, BK6-24-174 §13.8)** |
//!
//! ## Multi-stream aggregation note
//!
//! In a full production implementation, the BIKO's Abrechnungssummenzeitreihe
//! already contains the pre-aggregated billing totals for the billing period.
//! No client-side aggregation is required before issuing `ReceiveSummenzeitreihe`.
//! Any per-MaLo MSCONS meter data streams are managed separately outside this
//! workflow (e.g., via `ProjectionRunner::run_all_streams` in a read-model).
//!
//! ## Command construction example
//!
//! ```rust,ignore
//! use mako_mabis::{MabisBillingWorkflow, BillingCommand, BillingVersion};
//! use mako_engine::types::{BikoId, BillingPeriod, BkvId, MessageRef, Pruefidentifikator};
//!
//! // Called from the EDIFACT adapter when an inbound MSCONS Summenzeitreihe
//! // from the BIKO is validated and decoded.
//! let cmd = BillingCommand::ReceiveSummenzeitreihe {
//!     pid: Pruefidentifikator::new(13003).expect("13003 is valid"),
//!     billing_period: BillingPeriod::new("2025-09"),
//!     bkv_id: BkvId::new("4033872000022"),
//!     biko_id: BikoId::new("10YDE-VE-TRANSMIX"),
//!     version: BillingVersion::Vorlaeufig,
//!     message_ref: MessageRef::new("MSCONS-BKA-2025-09-001"),
//! };
//! process.execute(cmd).await?;
//! ```

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

pub mod bilanzkreisabrechnung;
pub mod clearingliste;
pub mod utilts_aggregation;

pub use bilanzkreisabrechnung::{
    BillingCommand, BillingData, BillingEvent, BillingProjection, BillingRecord, BillingRecordData,
    BillingState, BillingVersion, DataStatus, IFTSTA_DATENSTATUS_PID, IFTSTA_PIDS,
    MabisBillingWorkflow, PRUEFMITTEILUNG_DEADLINE_LABEL, WORKFLOW_NAME as BILLING_WORKFLOW_NAME,
};
pub use clearingliste::{
    CLEARINGLISTE_PIDS, ClearinglisteCommand, ClearinglisteData, ClearinglisteEvent,
    ClearinglisteKind, ClearinglisteState, MabisClearinglisteWorkflow,
    WORKFLOW_NAME as CLEARINGLISTE_WORKFLOW_NAME,
};
// Re-export canonical topology IDs from mako-edm (single source of truth).
// Previously these were also defined in `utilts_aggregation` — that was removed.
pub use mako_edm::{BilanzierungsgebietId, BilanzkreisId};
pub use utilts_aggregation::{SumInterval, Summenzeitreihe, SummenzeitreiheBuilder};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the MABIS process family.
///
/// Registers:
/// - PID 13003, 13010–13012 (MSCONS Summenzeitreihe — Bilanzkreisabrechnung Strom)
/// - PIDs 55065, 55069, 55070 (UTILMD Clearinglisten)
/// - PIDs 21000–21005 (IFTSTA MaBiS Statusmeldungen)
pub struct MabisModule;

impl mako_engine::builder::EngineModule for MabisModule {
    fn name(&self) -> &'static str {
        "mabis"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &["mabis-billing", "mabis-clearingliste"]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // PID 13003 — Bilanzkreisabrechnung Strom (MABIS electricity billing).
        //
        // MSCONS AHB 2.4c/2.5 §5: "Summenzeitreihen und Ausfallarbeitssummen".
        // Confirmed absent: PID 13001 does not exist in any MSCONS AHB version.
        // Gas/WiM meter PIDs (13002, 13005–13009, 13013–13019, 13020–13028) are
        // Messwesen or Redispatch PIDs and are registered in their respective crates.
        // Exception: PIDs 13010/13011/13012 (normiertes Profil / Profilschar / TEP)
        // are BK-Treue/MaBiS settlement profile data and are registered here.
        router.register(13003, "mabis-billing");
        router.register(13010, "mabis-billing");
        router.register(13011, "mabis-billing");
        router.register(13012, "mabis-billing");

        // IFTSTA MaBiS PIDs 21000–21005.
        //
        // All MaBiS IFTSTA status messages are routed to the same
        // `mabis-billing` workflow so they can be correlated with their
        // billing stream by conversation ID (CI tag).
        //
        // PID 21004 ("Statusmeldung vom BIKO an BKV/NB") is the Datenstatus
        // confirmation that drives the PruefmitteilungSent → Settled transition.
        // All other MaBiS IFTSTA PIDs are informational status notifications.
        //
        // Note: PID 21006 does not exist. PID 21007 is WiM Strom Teil 1 /
        // WiM Gas and is registered in `mako-wim` (`wim-device-change`).
        for &pid in bilanzkreisabrechnung::IFTSTA_PIDS {
            router.register(pid, "mabis-billing");
        }

        // UTILMD Clearingliste PIDs (55065, 55069, 55070).
        //
        // PIDs 55065 (Lieferantenclearingliste, NB → LF),
        //      55069 (Clearingliste DZR, BIKO → NB/ÜNB),
        //      55070 (Clearingliste BAS, BIKO → BKV)
        // are all part of the MaBiS Clearingverfahren (BK6-24-174 Anlage 3).
        for &pid in clearingliste::CLEARINGLISTE_PIDS {
            router.register(pid, "mabis-clearingliste");
        }
    }

    fn profile_requirements(&self) -> &'static [mako_engine::profile::ProfileRequirement] {
        use mako_engine::profile::ProfileRequirement;
        &[
            ProfileRequirement {
                message_type: "MSCONS",
                label: "MSCONS Summenzeitreihen (MABIS)",
            },
            ProfileRequirement {
                message_type: "IFTSTA",
                label: "IFTSTA Statusmeldung (MaBiS 21000–21005)",
            },
            ProfileRequirement {
                message_type: "UTILMD",
                label: "UTILMD Clearingliste (MaBiS 55065 / 55069 / 55070)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        Ok(())
    }
}
