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
//! | EDIFACT | UTILMD G (44001–44018, 44555) | UTILMD G (44039–44053, 44168–44170) |
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

#![deny(missing_docs)]

/// WiM Gas Anmeldung / Abmeldung workflows (PIDs 44042–44053).
pub mod anmeldung;
/// WiM Gas Kündigung MSB Gas workflow (PIDs 44039–44041).
pub mod kuendigung;
/// WiM Gas Verpflichtungsanfrage workflow (PIDs 44168–44170).
pub mod verpflichtungsanfrage;

pub use anmeldung::{
    ANMELDUNG_PIDS, APERAK_WINDOW_LABEL as ANMELDUNG_APERAK_WINDOW_LABEL,
    WORKFLOW_NAME as ANMELDUNG_WORKFLOW_NAME, WimGasAnmeldungCommand, WimGasAnmeldungData,
    WimGasAnmeldungEvent, WimGasAnmeldungProjection, WimGasAnmeldungRecord,
    WimGasAnmeldungRecordData, WimGasAnmeldungState, WimGasAnmeldungWorkflow,
};
pub use kuendigung::{
    APERAK_WINDOW_LABEL as KUENDIGUNG_APERAK_WINDOW_LABEL, KUENDIGUNG_PIDS,
    WORKFLOW_NAME as KUENDIGUNG_WORKFLOW_NAME, WimGasKuendigungCommand, WimGasKuendigungData,
    WimGasKuendigungEvent, WimGasKuendigungProjection, WimGasKuendigungRecord,
    WimGasKuendigungRecordData, WimGasKuendigungState, WimGasKuendigungWorkflow,
};
pub use verpflichtungsanfrage::{
    APERAK_WINDOW_LABEL as VERPFLICHTUNGSANFRAGE_APERAK_WINDOW_LABEL, VERPFLICHTUNGSANFRAGE_PIDS,
    WORKFLOW_NAME as VERPFLICHTUNGSANFRAGE_WORKFLOW_NAME, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageData, WimGasVerpflichtungsanfrageEvent,
    WimGasVerpflichtungsanfrageProjection, WimGasVerpflichtungsanfrageRecord,
    WimGasVerpflichtungsanfrageRecordData, WimGasVerpflichtungsanfrageState,
    WimGasVerpflichtungsanfrageWorkflow,
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
/// - PIDs 44022–44024 → `"geli-gas-stornierung"` (re-exported from `mako-geli-gas`;
///   PID ownership per `docs/pid-reference.md` is WiM Gas, routing pending full migration)
/// - IFTSTA PIDs 21009/21010/21011/21012/21013/21015/21018 → `"wim-gas-device-change"`
///   (informational status messages for WiM Gas MSB-Wechsel)
///
/// Note: GeLi Gas PIDs 44001–44021 belong to `mako-geli-gas`.
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
        // PIDs 44022–44024 (WiM Gas Stornierung per BDEW PID overview).
        // Routing via geli-gas-stornierung workflow until a dedicated WiM Gas
        // Stornierung workflow is implemented.
        for pid in [44022_u32, 44023, 44024] {
            router.register(pid, "geli-gas-stornierung");
        }
        // IFTSTA WiM Gas MSB-Wechsel status messages (informational).
        for pid in [21009_u32, 21010, 21011, 21012, 21013, 21015, 21018] {
            router.register(pid, "wim-gas-device-change");
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
