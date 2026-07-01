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
//! | Stornierung (multi-domain) | 44022–44024 | GeLi Gas 2.0 + WiM Gas; routed by `WimGasModule` (GeLi Gas role routing: TODO) |
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

pub mod datenabruf;
pub mod lieferbeginn;
pub mod mscons;
pub mod partin;
pub mod sperrung_lf;
pub mod stornierung;

pub use datenabruf::{
    GeliGasDatanabrufCommand, GeliGasDatanabrufEvent, GeliGasDatanabrufState,
    GeliGasDatanabrufWorkflow, ORDERS_ANFRAGE_PIDS as GELI_GAS_DATENABRUF_ORDERS_PIDS,
    ORDRSP_ABLEHNUNG_PIDS as GELI_GAS_DATENABRUF_ORDRSP_PIDS,
    WORKFLOW_NAME as GELI_GAS_DATENABRUF_WORKFLOW_NAME,
};
pub use lieferbeginn::{
    ANFRAGE_PIDS as LIEFERBEGINN_ANFRAGE_PIDS, ANTWORT_PIDS as LIEFERBEGINN_ANTWORT_PIDS,
    APERAK_WINDOW_LABEL as LIEFERBEGINN_APERAK_WINDOW_LABEL, GasProcessVariant,
    GasSupplierChangeCommand, GasSupplierChangeData, GasSupplierChangeEvent,
    GasSupplierChangeProjection, GasSupplierChangeRecord, GasSupplierChangeRecordData,
    GasSupplierChangeState, GeliGasSupplierChangeWorkflow, UTILMD_PIDS, WORKFLOW_NAME,
    response_pid_for,
};
pub use mscons::{
    GasMsconsDatenCommand, GasMsconsDatenEvent, GasMsconsDatenState, GeliGasMsconsWorkflow,
    MSCONS_PIDS as GELI_GAS_MSCONS_PIDS, WORKFLOW_NAME as GAS_MSCONS_WORKFLOW_NAME,
};
pub use partin::{
    GasKommunikationsdatenCommand, GasKommunikationsdatenData, GasKommunikationsdatenEvent,
    GasKommunikationsdatenState, GeliGasPartinWorkflow, PARTIN_GAS_PIDS as GELI_GAS_PARTIN_PIDS,
    WORKFLOW_NAME as GELI_GAS_PARTIN_WORKFLOW_NAME,
};
pub use sperrung_lf::{
    ANTWORT_WINDOW_LABEL as GELI_GAS_SPERRUNG_LF_ANTWORT_WINDOW_LABEL, GasSperrungAuftragData,
    GasSperrungLfCommand, GasSperrungLfEvent, GasSperrungLfState, GeliGasSperrungLfWorkflow,
    ORDRSP_SPERRUNG_PIDS as GELI_GAS_SPERRUNG_LF_ORDRSP_PIDS,
    ORDRSP_STORNO_PIDS as GELI_GAS_SPERRUNG_LF_ORDRSP_STORNO_PIDS,
    SPERRUNG_ANFRAGE_PIDS as GELI_GAS_SPERRUNG_ANFRAGE_PIDS,
    WORKFLOW_NAME as GELI_GAS_SPERRUNG_LF_WORKFLOW_NAME,
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
/// Note: PIDs **44022–44024** are multi-domain (GeLi Gas 2.0 + WiM Gas per BDEW PID
/// 3.3/4.0 xlsx) and currently routed by `WimGasModule` in `mako-wim-gas`.
/// Role-based routing for LFN/LFA contexts (GeLi Gas Stornierung) is a TODO.
pub struct GeliGasModule;

impl mako_engine::builder::EngineModule for GeliGasModule {
    fn name(&self) -> &'static str {
        "geli-gas"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            "geli-gas-supplier-change",
            // "geli-gas-stornierung" is registered here so `assert_dispatch_coverage`
            // enforces its presence in the dispatch table.  PIDs 44022–44024 are
            // currently routed by `WimGasModule` (wim-gas-stornierung) pending
            // role-conditional routing for the LFN/LFA context.  Adding the name
            // here ensures the dispatch arm can never be silently removed.
            stornierung::WORKFLOW_NAME,
            mscons::WORKFLOW_NAME,
            datenabruf::WORKFLOW_NAME,
            sperrung_lf::WORKFLOW_NAME,
            partin::WORKFLOW_NAME,
        ]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // GeLi Gas Lieferantenwechsel Gas (BDEW GeLi Gas AHB — UTILMD G profiles)
        // PIDs 44001–44021 → supplier-change workflow
        for &pid in lieferbeginn::UTILMD_PIDS {
            router.register(pid, "geli-gas-supplier-change");
        }
        // PIDs 44022–44024 are multi-domain (GeLi Gas 2.0 + WiM Gas) and currently
        // registered by WimGasModule only. GeLi Gas role routing is a TODO.

        // Gas MSCONS data delivery PIDs (NB/MSB → LF, GeLi Gas Teil 2).
        //
        // Inbound gas metered values, load profiles, and allocation data.
        // Registered unconditionally for LF deployments.
        for &pid in mscons::MSCONS_PIDS {
            router.register(pid, mscons::WORKFLOW_NAME);
        }

        // Gas Datenabruf — LF/MSB Gas requests Gas-specific metered values.
        //
        // ORDERS 17103 (Anfrage Abrechnungsbrennwert/Zustandszahl) and 17104
        // (MSB Gas Anfrage an NB Strom). Rejections via ORDRSP 19103/19104.
        for &pid in datenabruf::ORDERS_ANFRAGE_PIDS {
            router.register(pid, datenabruf::WORKFLOW_NAME);
        }
        for &pid in datenabruf::ORDRSP_ABLEHNUNG_PIDS {
            router.register(pid, datenabruf::WORKFLOW_NAME);
        }

        // PARTIN Gas Kommunikationsdaten (GeLi Gas, BK7-24-01-009).
        //
        // Gas party GLNs (GNB, gMSB, LF Gas, MGV) differ from Strom party GLNs.
        // Registered here so Gas-only deployments receive Gas PARTIN independently.
        // Strom PARTIN (37000–37006) is handled by mako-gpke gpke-partin.
        for &pid in partin::PARTIN_GAS_PIDS {
            router.register(pid, partin::WORKFLOW_NAME);
        }

        // Gas Sperrung / Entsperrung (LF-side) — PIDs 17115/17117 outbound (LF → GNB),
        // inbound ORDRSP 19116/19117 (Bestätigung/Ablehnung), Storno ORDRSP 19128/19129.
        // PIDs 19116/19117 are shared with GPKE Sperrung Strom; process context is
        // resolved by correlation ID at runtime in mixed Strom+Gas deployments.
        // Regulatory basis: BK7-24-01-009 (GeLi Gas 3.0).
        // APERAK Frist: 10 Werktage.
        for &pid in sperrung_lf::ORDRSP_SPERRUNG_PIDS {
            router.register(pid, sperrung_lf::WORKFLOW_NAME);
        }
        for &pid in sperrung_lf::ORDRSP_STORNO_PIDS {
            router.register(pid, sperrung_lf::WORKFLOW_NAME);
        }
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
            ProfileRequirement {
                message_type: "MSCONS",
                label: "MSCONS Gas Messdaten (13002, 13007–13009, 13013–13014)",
            },
            ProfileRequirement {
                message_type: "ORDERS",
                label: "ORDERS Gas Datenabruf (17103, 17104)",
            },
            ProfileRequirement {
                message_type: "ORDERS",
                label: "ORDERS Gas Sperrung / Entsperrung (17115, 17117)",
            },
            ProfileRequirement {
                message_type: "PARTIN",
                label: "PARTIN Gas Kommunikationsdaten (37008–37014)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        // Verify that all static PID slices are non-empty so a codegen regression
        // is caught at startup before any messages are processed.
        const _: () = assert!(
            !lieferbeginn::UTILMD_PIDS.is_empty(),
            "geli-gas: lieferbeginn::UTILMD_PIDS is empty — at least one PID must be registered"
        );
        const _: () = assert!(
            !stornierung::STORNIERUNG_PIDS.is_empty(),
            "geli-gas: stornierung::STORNIERUNG_PIDS is empty — 44022/44023/44024 must be present"
        );
        const _: () = assert!(
            !mscons::MSCONS_PIDS.is_empty(),
            "geli-gas: mscons::MSCONS_PIDS is empty — at least one Gas MSCONS PID must be registered"
        );
        const _: () = assert!(
            !datenabruf::ORDERS_ANFRAGE_PIDS.is_empty(),
            "geli-gas: datenabruf::ORDERS_ANFRAGE_PIDS is empty"
        );
        const _: () = assert!(
            !sperrung_lf::SPERRUNG_ANFRAGE_PIDS.is_empty(),
            "geli-gas: sperrung_lf::SPERRUNG_ANFRAGE_PIDS is empty — 17115/17117 must be present"
        );
        const _: () = assert!(
            !partin::PARTIN_GAS_PIDS.is_empty(),
            "geli-gas: partin::PARTIN_GAS_PIDS is empty — 37008–37014 must be present"
        );
        Ok(())
    }
}
