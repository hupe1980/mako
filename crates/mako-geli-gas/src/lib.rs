//! `mako-geli-gas` — GeLi Gas (Geschäftsprozesse Lieferantenwechsel Gas)
//! process engine for German gas market communication (BDEW MaKo).
//!
//! ## Process family
//!
//! GeLi Gas governs the supplier switching processes for the German gas
//! market, regulated by the BDEW GeLi Gas process documentation and
//! BNetzA rulings:
//!
//! | Process | PID | Status |
//! |---|---|---|
//! | Lieferbeginn Gas (Anfrage LFN → NB) | 44001 | ✅ Registered |
//! | Lieferende Gas (Anfrage LFN → NB) | 44002 | ✅ Registered |
//! | Bestätigung Lieferbeginn Gas | 44003 | ✅ Registered |
//! | Ablehnung Lieferbeginn Gas | 44004 | ✅ Registered |
//! | Bestätigung Lieferende Gas | 44005 | ✅ Registered |
//! | Ablehnung Lieferende Gas | 44006 | ✅ Registered |
//! | Abmeldung NN vom NB | 44007–44009 | ✅ Registered |
//! | Abmeldungsanfrage des NB | 44010–44012 | ✅ Registered |
//! | Anmeldung/Abmeldung EoG | 44013–44015 | ✅ Registered |
//! | Kündigung beim alten Lieferanten | 44016 | ✅ Registered |
//! | Kündigung Lieferbeginn Gas (LFN ↔ LFA) | 44017–44018 | ✅ Registered |
//! | Bestandsliste / Änderungsmeldung | 44019–44021 | ✅ Registered |
//! | WiM Gas Stornierung | 44022–44024 | ⚠️ Belongs to `mako-wim-gas` per BDEW PID overview |
//!
//! ## Architecture
//!
//! Each BDEW process variant is a separate [`mako_engine::workflow::Workflow`]
//! implementation. This crate contains **only pure domain logic** — no I/O,
//! no EDIFACT parsing, no network calls.
//!
//! Parsing and validation of raw EDIFACT bytes must happen at the transport
//! boundary (AS4 reception layer), **before** constructing a domain command.
//! The workflow `handle()` function receives pre-extracted domain values:
//!
//! ```text
//! AS4 transport layer
//!   └── parse raw bytes          (edi-energy)
//!       └── validate             (edi-energy)
//!           └── extract fields   (application code)
//!               └── GasSupplierChangeCommand { pid, malo_id, … }
//!                   └── Process::execute(cmd)  ← pure domain logic here
//! ```
//!
//! ## Key differences from electricity processes
//!
//! | Aspect | GPKE / WiM (Strom) | GeLi Gas |
//! |---|---|---|
//! | Market | Electricity | **Gas** |
//! | Location object | Messlokation (MeLo) | **Marktlokation (MaLo)** |
//! | Grid operator | Netzbetreiber (NB) | **Gasnetzbetreiber (GNB)** |
//! | APERAK Frist | 24 h (GPKE) / 5 Werktage (WiM) | **10 Werktage** |
//! | Frist helper | `add_hours(24)` / `add_werktage(5, …)` | **`add_werktage(10, BdewMaKo)`** |
//!
//! ## Command construction example
//!
//! ```rust,ignore
//! use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
//! use mako_geli_gas::lieferbeginn::{GeliGasSupplierChangeWorkflow, GasSupplierChangeCommand};
//!
//! let msg    = Platform::with_all_profiles().parse(&raw_bytes)?;
//! let report = msg.validate()?;
//! let AnyMessage::Utilmd(u) = &msg else { anyhow::bail!("not UTILMD") };
//!
//! let cmd = GasSupplierChangeCommand::ReceiveUtilmd {
//!     pid:               msg.detect_pruefidentifikator()?,
//!     sender:            u.sender().and_then(|n| n.party_id.clone()).unwrap_or_default(),
//!     receiver:          u.receiver().and_then(|n| n.party_id.clone()).unwrap_or_default(),
//!     malo_id:           u.transactions().first()
//!                         .and_then(|t| t.ide.object_id.clone()).unwrap_or_default(),
//!     document_date:     u.dtm().iter().find(|d| d.is_document_date())
//!                         .and_then(|d| d.value.clone()).unwrap_or_default(),
//!     message_ref:       msg.message_ref().to_owned(),
//!     validation_passed: report.is_valid(),
//!     validation_errors: report.errors().iter()
//!                         .map(|i| format!("{i}")).collect(),
//! };
//!
//! process.execute(cmd).await?;
//! ```

#![deny(missing_docs)]

pub mod lieferbeginn;
pub mod stornierung;

pub use lieferbeginn::{
    APERAK_WINDOW_LABEL as LIEFERBEGINN_APERAK_WINDOW_LABEL, GasSupplierChangeCommand,
    GasSupplierChangeData, GasSupplierChangeEvent, GasSupplierChangeProjection,
    GasSupplierChangeRecord, GasSupplierChangeRecordData, GasSupplierChangeState,
    GeliGasSupplierChangeWorkflow, UTILMD_PIDS, WORKFLOW_NAME,
};
pub use stornierung::{
    GeliGasStornierungCommand, GeliGasStornierungData, GeliGasStornierungEvent,
    GeliGasStornierungState, GeliGasStornierungWorkflow, STORNIERUNG_APERAK_WINDOW_LABEL,
    STORNIERUNG_PIDS, WORKFLOW_NAME as STORNIERUNG_WORKFLOW_NAME,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the GeLi Gas process family.
///
/// Registers all GeLi Gas UTILMD G `Prüfidentifikator` values into the
/// [`mako_engine::pid_router::PidRouter`] at engine startup:
///
/// - PIDs 44001–44021 → `"geli-gas-supplier-change"`
///   (`GeliGasSupplierChangeWorkflow`)
///
/// Note: PIDs **44022–44024** (WiM Gas Stornierung) are registered by
/// `WimGasModule` in `mako-wim-gas`, not here (per `docs/pid-reference.md`).
///
/// Note: the Gas Sperr-/Entsperrprozess (PIDs 17115/17116/17117, ORDERS
/// format) is not yet implemented; see `docs/pid-reference.md`.
pub struct GeliGasModule;

impl mako_engine::builder::EngineModule for GeliGasModule {
    fn name(&self) -> &'static str {
        "geli-gas"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &["geli-gas-supplier-change"]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // GeLi Gas Lieferantenwechsel Gas (BDEW GeLi Gas AHB — UTILMD G profiles)
        // PIDs 44001–44021 → supplier-change workflow
        for &pid in lieferbeginn::UTILMD_PIDS {
            router.register(pid, "geli-gas-supplier-change");
        }
        // PIDs 44022–44024 (WiM Gas Stornierung) are registered by WimGasModule.
    }

    fn profile_requirements(&self) -> &'static [mako_engine::profile::ProfileRequirement] {
        use mako_engine::profile::ProfileRequirement;
        &[
            ProfileRequirement {
                message_type: "UTILMD",
                label: "UTILMD Gas (GeLi Gas Lieferbeginn)",
            },
            ProfileRequirement {
                message_type: "APERAK",
                label: "APERAK (GeLi Gas)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        // Verify that all static PID slices referenced by register_pids() are
        // non-empty so a codegen regression is caught at startup.
        const _: () = assert!(
            !lieferbeginn::UTILMD_PIDS.is_empty(),
            "geli-gas: lieferbeginn::UTILMD_PIDS is empty — at least one PID must be registered"
        );
        Ok(())
    }
}
