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
//! | Stornierung Anfrage (LF → GNB) | 44022 | ✅ Outbound (LF-side), ERP-initiated |
//! | Stornierung Bestätigung (GNB → LF) | 44023 | ✅ Inbound (LF-side), `geli-gas-stornierung-lf` |
//! | Stornierung Ablehnung (GNB → LF) | 44024 | ✅ Inbound (LF-side), `geli-gas-stornierung-lf` |
//! | Stornierung (GNB-side) | 44022–44024 | ✅ Registered (`geli-gas-stornierung`, `Nb`-only deployments) |
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
//!     bilanzierungsmethode: None,
//!     fallgruppe: None,
//! };
//!
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

pub mod datenabruf;
pub mod gas_quality;
pub mod invoic;
pub mod lf_anmeldung;
pub mod lf_stornierung;
pub mod lieferbeginn;
pub mod mscons;
pub mod partin;
pub mod sperrung_lf;
pub mod sperrung_nb;
pub mod stornierung;

pub use datenabruf::{
    GeliGasDatanabrufCommand, GeliGasDatanabrufEvent, GeliGasDatanabrufState,
    GeliGasDatanabrufWorkflow, ORDERS_ANFRAGE_PIDS as GELI_GAS_DATENABRUF_ORDERS_PIDS,
    ORDRSP_ABLEHNUNG_PIDS as GELI_GAS_DATENABRUF_ORDRSP_PIDS,
    WORKFLOW_NAME as GELI_GAS_DATENABRUF_WORKFLOW_NAME,
};
pub use invoic::{
    GeliGasSperrprozesseInvoicCommand, GeliGasSperrprozesseInvoicData,
    GeliGasSperrprozesseInvoicEvent, GeliGasSperrprozesseInvoicProjection,
    GeliGasSperrprozesseInvoicRecord, GeliGasSperrprozesseInvoicState,
    GeliGasSperrprozesseInvoicWorkflow,
    SETTLEMENT_WINDOW_LABEL as SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL, SPERRPROZESSE_INVOIC_PID,
    WORKFLOW_NAME as GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
};
pub use lf_anmeldung::{
    ANFRAGE_PIDS_LF as LF_ANMELDUNG_ANFRAGE_PIDS, ANTWORT_PIDS_LF as LF_ANMELDUNG_ANTWORT_PIDS,
    GNB_RESPONSE_WINDOW_LABEL as LF_ANMELDUNG_RESPONSE_WINDOW_LABEL, GeliGasLfAnmeldungCommand,
    GeliGasLfAnmeldungData, GeliGasLfAnmeldungEvent, GeliGasLfAnmeldungState,
    GeliGasLfAnmeldungWorkflow, WORKFLOW_NAME as LF_ANMELDUNG_WORKFLOW_NAME,
};
pub use lf_stornierung::{
    ANFRAGE_PID_LF as STORNIERUNG_ANFRAGE_PID_LF, ANTWORT_PIDS_LF as STORNIERUNG_ANTWORT_PIDS_LF,
    GNB_RESPONSE_WINDOW_LABEL as STORNIERUNG_LF_RESPONSE_WINDOW_LABEL,
    GeliGasLfStornierungWorkflow, LfStornierungCommand, LfStornierungData, LfStornierungEvent,
    LfStornierungState, WORKFLOW_NAME as STORNIERUNG_LF_WORKFLOW_NAME,
};
pub use lieferbeginn::{
    ANFRAGE_PIDS as LIEFERBEGINN_ANFRAGE_PIDS, ANTWORT_PIDS as LIEFERBEGINN_ANTWORT_PIDS,
    GasProcessVariant, GasSupplierChangeCommand, GasSupplierChangeData, GasSupplierChangeEvent,
    GasSupplierChangeProjection, GasSupplierChangeRecord, GasSupplierChangeRecordData,
    GasSupplierChangeState, GeliGasSupplierChangeWorkflow,
    RESPONSE_WINDOW_LABEL as LIEFERBEGINN_RESPONSE_WINDOW_LABEL, UTILMD_PIDS, WORKFLOW_NAME,
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
pub use sperrung_nb::{
    ANTWORT_WINDOW_LABEL as GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL, GasSperrungNbCommand,
    GasSperrungNbData, GasSperrungNbEvent, GasSperrungNbState, GeliGasSperrungNbWorkflow,
    MSB_ANTWORT_PIDS as GELI_GAS_SPERRUNG_NB_MSB_ANTWORT_PIDS,
    ORDCHG_STORNIERUNG_PIDS as GELI_GAS_SPERRUNG_NB_ORDCHG_PIDS,
    SPERRUNG_PIDS as GELI_GAS_SPERRUNG_NB_PIDS,
    WORKFLOW_NAME as GELI_GAS_SPERRUNG_NB_WORKFLOW_NAME,
};
pub use stornierung::{
    GeliGasStornierungCommand, GeliGasStornierungData, GeliGasStornierungEvent,
    GeliGasStornierungState, GeliGasStornierungWorkflow, STORNIERUNG_PIDS,
    STORNIERUNG_RESPONSE_WINDOW_LABEL, WORKFLOW_NAME as STORNIERUNG_WORKFLOW_NAME,
};

pub use gas_quality::{GasQualitaet, normalize_gasqualitaet};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the GeLi Gas process family.
///
/// Registers all GeLi Gas `Prüfidentifikator` values into the
/// [`mako_engine::pid_router::PidRouter`] at engine startup:
///
/// - PIDs 44001–44021 → `"geli-gas-supplier-change"` (`GeliGasSupplierChangeWorkflow`)
/// - PID 31011 → `"geli-gas-sperrprozesse-invoic"`
///   (`GeliGasSperrprozesseInvoicWorkflow`, Rechnung sonstige Leistung AWH, VNB → LFN/LFA)
/// - PIDs 44022–44024 → `"geli-gas-stornierung"` when `Nb`-only deployment
///   (`GeliGasStornierungWorkflow`; GNB receives Anfrage, sends Bestätigung/Ablehnung)
/// - PIDs 44023–44024 → `"geli-gas-stornierung-lf"` when `Lf`-only deployment (no `Msb`/`Nmsb`)
///   (`GeliGasLfStornierungWorkflow`; LF receives GNB response to outbound 44022)
///
/// ## Stornierung PIDs 44022–44024 — multi-domain routing
///
/// PIDs 44022–44024 are multi-domain (GeLi Gas 2.0 + WiM Gas per BDEW PID 3.3/4.0 xlsx).
/// Routing is role-conditional via `register_pids_with_roles`:
///
/// | Role | Registered PIDs | Workflow |
/// |---|---|---|
/// | `Nb`-only | 44022, 44023, 44024 | `geli-gas-stornierung` (GNB-side) |
/// | `Lf`-only (no `Msb`/`Nmsb`) | 44023, 44024 (inbound responses) | `geli-gas-stornierung-lf` (LF-side) |
/// | `Nb + Lf` | 44022 → GNB-side; 44023/44024 → LF-side | both workflows |
/// | `Msb`/`Nmsb` / `all()` | handled by `WimGasModule` | `wim-gas-stornierung` |
pub struct GeliGasModule;

impl mako_engine::builder::EngineModule for GeliGasModule {
    fn name(&self) -> &'static str {
        "geli-gas"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            "geli-gas-supplier-change",
            // GNB-side: receives 44022 from LFN/LFA, sends 44023/44024 response.
            // Registered when Nb-only (no Msb/Nmsb). For all() and gMSB, WimGasModule owns these.
            stornierung::WORKFLOW_NAME,
            // LF-side: LFN/LFA sends 44022 outbound, receives 44023/44024 inbound.
            // Registered when Lf-only (no Msb/Nmsb). Outbound 44022 is ERP-initiated.
            lf_stornierung::WORKFLOW_NAME,
            mscons::WORKFLOW_NAME,
            datenabruf::WORKFLOW_NAME,
            sperrung_lf::WORKFLOW_NAME,
            sperrung_nb::WORKFLOW_NAME,
            partin::WORKFLOW_NAME,
            // PID 31011 — Rechnung sonstige Leistung / AWH Sperrprozesse Gas (VNB → LFN/LFA).
            // GeLi Gas (BK7-24-01-009) billing for disconnection services; NOT GaBi Gas.
            invoic::WORKFLOW_NAME,
        ]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // GeLi Gas Lieferantenwechsel Gas (BDEW GeLi Gas AHB — UTILMD G profiles)
        // PIDs 44001–44021 → supplier-change workflow
        for &pid in lieferbeginn::UTILMD_PIDS {
            router.register(pid, "geli-gas-supplier-change");
        }
        // PIDs 44022–44024: role-conditional — NOT registered here.
        // See register_pids_with_roles() for the Nb-role guard.

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
        // GNB execution window: 10 Werktage (BK7-24-01-009). APERAK sending Frist: nächster Werktag 12 Uhr (APERAK AHB 1.0 §2.3.1).
        for &pid in sperrung_lf::ORDRSP_SPERRUNG_PIDS {
            router.register(pid, sperrung_lf::WORKFLOW_NAME);
        }
        for &pid in sperrung_lf::ORDRSP_STORNO_PIDS {
            router.register(pid, sperrung_lf::WORKFLOW_NAME);
        }

        // Gas Sperrung / Entsperrung (GNB-side / NB-role) — inbound ORDERS 17115/17117
        // from LF, plus ORDCHG 39000/39001 (Stornierung) and ORDRSP 19118/19119 from gMSB.
        // PIDs 17115/17116/17117 are shared with GPKE Sperrung Strom (NB-role); process
        // context is resolved by commodity (Gas vs. Strom) at runtime.
        // Regulatory basis: BK7-24-01-009 (AWH Sperrprozesse Gas).
        // GNB execution window: 10 Werktage (BK7-24-01-009). APERAK sending Frist: nächster Werktag 12 Uhr (APERAK AHB 1.0 §2.3.1).
        for &pid in sperrung_nb::SPERRUNG_PIDS {
            router.register(pid, sperrung_nb::WORKFLOW_NAME);
        }
        for &pid in sperrung_nb::ORDCHG_STORNIERUNG_PIDS {
            router.register(pid, sperrung_nb::WORKFLOW_NAME);
        }
        for &pid in sperrung_nb::MSB_ANTWORT_PIDS {
            router.register(pid, sperrung_nb::WORKFLOW_NAME);
        }

        // INVOIC 31011 — Rechnung sonstige Leistung / AWH Sperrprozesse Gas (VNB → LFN/LFA).
        // The GNB/VNB bills the LFN/LFA for performing disconnection/reconnection services
        // (Abrechnungswürdige Handlungen from the gas Sperrprozess).
        // Regulatory basis: BK7-24-01-009 (GeLi Gas 3.0, same ruling as Sperrprozesse).
        // This is NOT GaBi Gas (BK7-14-020); direction is NB → LF, not NB → BKV.
        router.register(invoic::SPERRPROZESSE_INVOIC_PID, invoic::WORKFLOW_NAME);
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
                label: "MSCONS Gas Messdaten (13002, 13007–13009)",
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

    fn register_pids_with_roles(
        &self,
        router: &mut mako_engine::pid_router::PidRouter,
        roles: &mako_engine::marktrolle::DeploymentRoles,
    ) {
        // Register all unconditional GeLi Gas PIDs first.
        self.register_pids(router);

        // PIDs 44022–44024: Stornierung — GeLi Gas context (LFN/LFA cancels supply change).
        //
        // Routing decision:
        // PIDs 44022–44024: Stornierung — role-conditional routing.
        //
        // See GeliGasModule doc-comment for the full routing table.
        //
        // GNB-side (`Nb`-only, no `Msb`/`Nmsb`):
        //   Register all three PIDs so the GNB correlates inbound 44022 and can route
        //   44023/44024 outbound responses back via process ID.
        //
        // LF-side (`Lf` set, no `Msb`/`Nmsb`):
        //   Register only 44023/44024 (inbound GNB responses). PID 44022 is ERP-initiated
        //   outbound and does not need PID-router registration.
        //   Combined Nb+Lf deployments work without conflict: different PIDs, different workflows.
        //
        // NOT registered when `Msb`/`Nmsb` or `all()`: WimGasModule owns 44022–44024 in those cases.
        use mako_engine::marktrolle::Marktrolle;

        let has_nb_only = !roles.is_all()
            && roles.contains(Marktrolle::Nb)
            && !roles.contains(Marktrolle::Msb)
            && !roles.contains(Marktrolle::Nmsb);
        // LF role (lf-only OR integrated): register LFN-side response PIDs.
        // On integrated deployments, the LF *receives* 44003/44004 from GNB;
        // the NB only ever *sends* them, so routing to lf-anmeldung is correct.
        let has_lf_role = roles.contains(Marktrolle::Lf) || roles.is_all();
        // LF stornierung: lf-only only (not all()), to avoid conflict with WimGas.
        let has_lf = !roles.is_all()
            && roles.contains(Marktrolle::Lf)
            && !roles.contains(Marktrolle::Msb)
            && !roles.contains(Marktrolle::Nmsb);

        if has_nb_only {
            // Only 44022 is inbound on the GNB side — 44023/44024 are outbound responses
            // dispatched via the outbox and do not need PID-router registration.
            router.register_with_module(
                stornierung::ANFRAGE_PID,
                stornierung::WORKFLOW_NAME,
                "geli-gas",
            );
        }
        if has_lf_role {
            // LF (or integrated): 44003/44004 are inbound GNB confirmations/rejections
            // to an outbound 44001/44002 the LF previously sent. Route to the LFN-side
            // workflow, overriding the unconditional geli-gas-supplier-change registration.
            for &pid in lf_anmeldung::ANTWORT_PIDS_LF {
                // Use register() (silently replaces) — the GNB-side workflow never
                // receives these PIDs inbound, so this override is safe.
                router.register(pid, lf_anmeldung::WORKFLOW_NAME);
            }
        }
        if has_lf {
            for &pid in lf_stornierung::ANTWORT_PIDS_LF {
                router.register_with_module(pid, lf_stornierung::WORKFLOW_NAME, "geli-gas");
            }
        }
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
            !lf_stornierung::ANTWORT_PIDS_LF.is_empty(),
            "geli-gas: lf_stornierung::ANTWORT_PIDS_LF is empty — 44023/44024 must be present"
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
