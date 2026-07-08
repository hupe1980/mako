//! `mako-wim-gas` — WiM Gas process engine for the German gas market
//! (Wechselprozesse im Messwesen Gas).
//!
//! # Domain background
//!
//! **WiM Gas** (*Wechselprozesse im Messwesen Gas*) governs the switching
//! processes for gas metering-point operators (gMSB) in the German gas market.
//! It is regulated by the Bundesnetzagentur (BNetzA) under ruling
//! **BK7-24-01-009** ("GeLi Gas 3.0").
//!
//! The current process specification is **WiM Gas AWH V2.0** (BDEW/VKU/GEODE/
//! FNBGas, published 2025-08-04).
//!
//! # Implemented process families
//!
//! | PID range | Workflow | Status |
//! |---|---|---|
//! | 44039–44041 | Kündigung MSB Gas | ✅ Registered |
//! | 44042–44044 | Anmeldung neuer MSB Gas | ✅ Registered |
//! | 44051–44053 | Ende MSB Gas / Vorläufige Abmeldung | ✅ Registered |
//! | 44168–44170 | Verpflichtungsanfrage | ✅ Registered |
//!
//! # Key boundaries
//!
//! | Aspect | GeLi Gas (`mako-geli-gas`) | WiM Gas (`mako-wim-gas`) |
//! |---|---|---|
//! | Ruling | BK7-24-01-009 | BK7-24-01-009 (same umbrella) |
//! | Scope | Supplier switching (Lieferbeginn/-ende) | MSB change (Anmeldung/Kündigung gMSB) |
//! | EDIFACT | UTILMD G (44001–44021, 44022–44024) | UTILMD G (44039–44053, 44168–44170) |
//! | APERAK Frist | 10 Werktage | 10 Werktage |
//!
//! | Aspect | WiM Strom (`mako-wim`) | WiM Gas (`mako-wim-gas`) |
//! |---|---|---|
//! | APERAK Frist | **5 Werktage** | **10 Werktage** |
//! | Ruling | BK6-24-174 | BK7-24-01-009 |
//! | EDIFACT | UTILMD S2.x | UTILMD G1.x |
//!
//! # AHB profile note
//!
//! WiM Gas PIDs (44039–44053, 44168–44170) are not yet present in the
//! `fv*_gas` UTILMD AHB profile set. Until `cargo xtask import-xml-ahb`
//! imports these profiles, `msg.validate()` returns a vacuous pass for these
//! PIDs. The adapter layer applies the `pid_has_ahb_rules()` guard to prevent
//! false-positive validation.
//!
//! # Regulatory references
//!
//! - **BNetzA BK7-24-01-009** — GeLi Gas 3.0 / WiM Gas ruling,
//!   Beschluss 12.09.2025, abgeschlossen 24.09.2025
//! - **BDEW/VKU/GEODE/FNBGas AWH WiM Gas V2.0** (2025-08-04) —
//!   `docs/pdfs/bdew-mako/BDEW_VKU_GEODE_FNBGas_AWH_WiMGas_V2_0_20250804.pdf`
//! - **UTILMD AHB Gas 1.1 / 1.2** — message specification

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

/// WiM Gas Anmeldung / Abmeldung workflows (PIDs 44042–44053).
pub mod anmeldung;
/// WiM Gas INSRPT Störungsmeldung workflow (PIDs 23005/23009 Gas-only;
/// 23001/23003/23004/23008 shared with WiM Strom).
///
/// Gas-only PIDs 23005 and 23009 are unconditionally registered here.
/// Shared PIDs 23001/23003/23004/23008 are registered with `Sparte::Gas` via
/// `PidRouter::register_with_sparte` so that `route_with_sparte(pid, Sparte::Gas)`
/// resolves to `"wim-gas-insrpt"` (10 WT) in combined Strom+Gas deployments,
/// while `route_with_sparte(pid, Sparte::Strom)` resolves to `"wim-insrpt"` (5 WT)
/// via `WimModule`'s `Sparte::Strom` entry. APERAK Frist: 10 Werktage (BK7-24-01-009).
pub mod insrpt;
/// WiM Gas INVOIC billing stub (PIDs 31003, 31004).
///
/// ⚠️ Stub — settlement workflow pending. Records receipt and emits `tracing::warn!`.
pub mod invoic;
/// WiM Gas Kündigung MSB Gas workflow (PIDs 44039–44041).
pub mod kuendigung;
/// WiM Gas Stornierung workflow (PIDs 44022–44024).
///
/// Per BDEW PID overview (PID 3.3 / PID 4.0), PIDs 44022–44024 belong to the
/// WiM Gas process family. This module is the canonical owner (BK7-24-01-009).
pub mod stornierung;
/// WiM Gas Verpflichtungsanfrage workflow (PIDs 44168–44170).
pub mod verpflichtungsanfrage;

pub use anmeldung::{
    ANMELDUNG_PIDS, RESPONSE_WINDOW_LABEL as ANMELDUNG_RESPONSE_WINDOW_LABEL,
    WORKFLOW_NAME as ANMELDUNG_WORKFLOW_NAME, WimGasAnmeldungCommand, WimGasAnmeldungData,
    WimGasAnmeldungEvent, WimGasAnmeldungProjection, WimGasAnmeldungRecord,
    WimGasAnmeldungRecordData, WimGasAnmeldungState, WimGasAnmeldungWorkflow,
};
pub use insrpt::{
    ANTWORT_WINDOW_LABEL as INSRPT_GAS_ANTWORT_WINDOW_LABEL, GasStorungsmeldungCommand,
    GasStorungsmeldungData, GasStorungsmeldungEvent, GasStorungsmeldungState, INSRPT_GAS_ONLY_PIDS,
    INSRPT_SHARED_PIDS, WORKFLOW_NAME as INSRPT_GAS_WORKFLOW_NAME, WimGasInsrptWorkflow,
};
pub use invoic::{
    SETTLEMENT_WINDOW_LABEL as INVOIC_SETTLEMENT_WINDOW_LABEL, WIM_GAS_COMDIS_ABLEHNUNG_PID,
    WIM_GAS_INVOIC_PIDS, WIM_GAS_REMADV_PIDS, WORKFLOW_NAME as INVOIC_WORKFLOW_NAME,
    WimGasInvoicCommand, WimGasInvoicData, WimGasInvoicEvent, WimGasInvoicProjection,
    WimGasInvoicRecord, WimGasInvoicState, WimGasInvoicWorkflow,
};
pub use kuendigung::{
    KUENDIGUNG_PIDS, RESPONSE_WINDOW_LABEL as KUENDIGUNG_RESPONSE_WINDOW_LABEL,
    WORKFLOW_NAME as KUENDIGUNG_WORKFLOW_NAME, WimGasKuendigungCommand, WimGasKuendigungData,
    WimGasKuendigungEvent, WimGasKuendigungProjection, WimGasKuendigungRecord,
    WimGasKuendigungRecordData, WimGasKuendigungState, WimGasKuendigungWorkflow,
};
pub use stornierung::{
    STORNIERUNG_PIDS, STORNIERUNG_RESPONSE_WINDOW_LABEL,
    WORKFLOW_NAME as STORNIERUNG_WORKFLOW_NAME, WimGasStornierungCommand, WimGasStornierungData,
    WimGasStornierungEvent, WimGasStornierungState, WimGasStornierungWorkflow,
};
pub use verpflichtungsanfrage::{
    RESPONSE_WINDOW_LABEL as VERPFLICHTUNGSANFRAGE_RESPONSE_WINDOW_LABEL,
    VERPFLICHTUNGSANFRAGE_PIDS, WORKFLOW_NAME as VERPFLICHTUNGSANFRAGE_WORKFLOW_NAME,
    WimGasVerpflichtungsanfrageCommand, WimGasVerpflichtungsanfrageData,
    WimGasVerpflichtungsanfrageEvent, WimGasVerpflichtungsanfrageProjection,
    WimGasVerpflichtungsanfrageRecord, WimGasVerpflichtungsanfrageRecordData,
    WimGasVerpflichtungsanfrageState, WimGasVerpflichtungsanfrageWorkflow,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the WiM Gas process family.
///
/// Registers all WiM Gas UTILMD G `Prüfidentifikator` values into the
/// [`mako_engine::pid_router::PidRouter`] at engine startup:
///
/// - PIDs 44039–44041 → `"wim-gas-kuendigung"` (`WimGasKuendigungWorkflow`)
/// - PIDs 44042–44053 → `"wim-gas-anmeldung"` (`WimGasAnmeldungWorkflow`)
/// - PIDs 44168–44170 → `"wim-gas-verpflichtungsanfrage"`
/// - PIDs 31003, 31004 → `"wim-gas-invoic"` (INVOIC billing stub; settlement pending)
/// - PIDs 44022–44024 → `"wim-gas-stornierung"` when `DeploymentRoles::all()`, `Msb`, or `Nmsb`
///   (role-conditional; see `WimGasModule` impl of `EngineModule::register_pids_with_roles`)
///
/// IFTSTA PIDs 21009/21010/21011/21012/21013/21015/21018 carry informational
/// WiM Gas MSB-Wechsel status messages. Per WiM Gas AWH V2.0 there is no APERAK
/// obligation for these messages; they are not routed to any workflow.
///
/// Note: GeLi Gas PIDs 44001–44021 belong to `mako-geli-gas`.
/// Note: PIDs 44022–44024 are routed to `"geli-gas-stornierung"` (via `GeliGasModule`) when the
/// deployment is a pure GNB (`Nb`-only), since supply-change stornierung (LFN/LFA → GNB)
/// is the dominant traffic pattern for gas network operators.
pub struct WimGasModule;

impl mako_engine::builder::EngineModule for WimGasModule {
    fn name(&self) -> &'static str {
        "wim-gas"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            "wim-gas-anmeldung",
            "wim-gas-kuendigung",
            "wim-gas-verpflichtungsanfrage",
            "wim-gas-invoic",
            "wim-gas-stornierung",
            insrpt::WORKFLOW_NAME,
        ]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        for &pid in anmeldung::ANMELDUNG_PIDS {
            router.register(pid, "wim-gas-anmeldung");
        }
        for &pid in kuendigung::KUENDIGUNG_PIDS {
            router.register(pid, "wim-gas-kuendigung");
        }
        for &pid in verpflichtungsanfrage::VERPFLICHTUNGSANFRAGE_PIDS {
            router.register(pid, "wim-gas-verpflichtungsanfrage");
        }
        // PIDs 44022–44024: role-conditional — NOT registered here.
        // See register_pids_with_roles() for the gMSB-role guard.
        // IFTSTA PIDs 21009/21010/21011/21012/21013/21015/21018 carry informational
        // WiM Gas MSB-Wechsel status messages. Per WiM Gas AWH V2.0, there is no
        // APERAK obligation for these messages; they are not routed to any workflow.
        // PIDs 31003 (WiM-Rechnung) and 31004 (Stornorechnung) — WiM Gas INVOIC
        // billing.  Routes to the stub workflow until full settlement is implemented.
        for &pid in invoic::WIM_GAS_INVOIC_PIDS {
            router.register(pid, "wim-gas-invoic");
        }

        // REMADV 33001–33002 — inbound payment advice (gMSB invoicer role).
        //
        // After the gMSB sends INVOIC 31003/31004, the NB (payer) sends REMADV
        // back to confirm or dispute payment. The gMSB receives these inbound.
        // Without registration they are silently dropped by the AS4 ingest layer.
        //
        // Source: REMADV AHB 1.0, WiM Gas, BK7-24-01-009.
        for &pid in invoic::WIM_GAS_REMADV_PIDS {
            router.register(pid, "wim-gas-invoic");
        }

        // COMDIS 29001 — inbound Ablehnung REMADV (gMSB rejects NB's REMADV).
        //
        // Source: COMDIS AHB 1.0, WiM Gas, BK7-24-01-009.
        router.register(invoic::WIM_GAS_COMDIS_ABLEHNUNG_PID, "wim-gas-invoic");

        // INSRPT Störungsmeldungen (WiM Gas).
        //
        // APERAK Frist: 10 Werktage per BK7-24-01-009 — applies to ALL Gas INSRPT PIDs.
        //
        // Gas-only PIDs 23005 (Ablehnung Gas) and 23009 (Ergebnisbericht Gas) are never
        // shared with WiM Strom and are always unconditionally owned by this module.
        //
        // Shared PIDs 23001/23003/23004/23008 also appear in the WiM Strom AHB with a
        // shorter 5 WT Frist.  Both WimModule (Strom) and WimGasModule (Gas) register
        // these PIDs using commodity-qualified entries so that `route_with_sparte` selects
        // the correct workflow at ingest time:
        //
        //   route_with_sparte(23001, Sparte::Gas)   → "wim-gas-insrpt"   (10 WT)  ← this module
        //   route_with_sparte(23001, Sparte::Strom) → "wim-insrpt"       (5 WT)   ← WimModule
        //
        // In a Gas-standalone deployment (no WimModule) we also set the unambiguous
        // fallback entry so that plain `route(pid)` resolves to "wim-gas-insrpt".
        for &pid in insrpt::INSRPT_GAS_ONLY_PIDS {
            router.register(pid, insrpt::WORKFLOW_NAME);
        }
        for &pid in insrpt::INSRPT_SHARED_PIDS {
            // Unambiguous entry — Gas-standalone fallback; overwritten by WimModule in
            // combined deployments (last-write-wins), but commodity entry always wins for
            // callers that supply Sparte::Gas.
            router.register(pid, insrpt::WORKFLOW_NAME);
            router.register_with_sparte(
                pid,
                mako_engine::types::Sparte::Gas,
                insrpt::WORKFLOW_NAME,
            );
        }
    }

    fn register_pids_with_roles(
        &self,
        router: &mut mako_engine::pid_router::PidRouter,
        roles: &mako_engine::marktrolle::DeploymentRoles,
    ) {
        // Register all unconditional WiM Gas PIDs first.
        self.register_pids(router);

        // PIDs 44022–44024: Stornierung — WiM Gas context (gMSB cancels MSB-change).
        //
        // Routing decision:
        //   - `all()` — backward-compatible default; `wim-gas-stornierung` is the canonical
        //     owner for combined deployments where all roles are present.
        //   - `Msb` or `Nmsb` — the gMSB receives 44022 inbound (Anfrage nach Stornierung
        //     of a WiM Gas Anmeldung/Kündigung) and sends 44023/44024 responses outbound.
        //
        // NOT registered when the deployment is a pure GNB (`Nb`-only, without `Msb`/`Nmsb`):
        //   In that case, `GeliGasModule::register_pids_with_roles` registers 44022–44024 as
        //   `"geli-gas-stornierung"` — appropriate for supply-change stornierung from LFN/LFA.
        //
        // Combined `Nb + Msb` deployments are not supported: both modules would conflict
        //   on 44022–44024; the operator must run separate GNB and gMSB engine instances.
        if roles.is_all()
            || roles.contains(mako_engine::marktrolle::Marktrolle::Msb)
            || roles.contains(mako_engine::marktrolle::Marktrolle::Nmsb)
        {
            for &pid in stornierung::STORNIERUNG_PIDS {
                router.register_with_module(pid, stornierung::WORKFLOW_NAME, "wim-gas");
            }
        }
    }

    fn profile_requirements(&self) -> &'static [mako_engine::profile::ProfileRequirement] {
        use mako_engine::profile::ProfileRequirement;
        &[
            ProfileRequirement {
                message_type: "UTILMD",
                label: "UTILMD Gas (WiM Gas)",
            },
            ProfileRequirement {
                message_type: "APERAK",
                label: "APERAK (WiM Gas)",
            },
            ProfileRequirement {
                message_type: "INVOIC",
                label: "INVOIC WiM Gas (31003/31004)",
            },
            ProfileRequirement {
                message_type: "REMADV",
                label: "REMADV Zahlungsavis (WiM Gas 33001/33002)",
            },
            ProfileRequirement {
                message_type: "COMDIS",
                label: "COMDIS Ablehnung REMADV (WiM Gas 29001)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        const _: () = assert!(
            !anmeldung::ANMELDUNG_PIDS.is_empty(),
            "wim-gas: anmeldung::ANMELDUNG_PIDS is empty"
        );
        const _: () = assert!(
            !kuendigung::KUENDIGUNG_PIDS.is_empty(),
            "wim-gas: kuendigung::KUENDIGUNG_PIDS is empty"
        );
        const _: () = assert!(
            !verpflichtungsanfrage::VERPFLICHTUNGSANFRAGE_PIDS.is_empty(),
            "wim-gas: verpflichtungsanfrage::VERPFLICHTUNGSANFRAGE_PIDS is empty"
        );
        Ok(())
    }
}
