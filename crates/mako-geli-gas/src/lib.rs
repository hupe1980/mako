//! `mako-geli-gas` — GeLi Gas (Geschäftsprozesse Lieferantenwechsel Gas)
//! process engine for German gas market communication (BDEW MaKo).
//!
//! ## Process family
//!
//! GeLi Gas governs the supplier switching processes for the German gas
//! market, regulated by the BDEW GeLi Gas process documentation and
//! BNetzA rulings:
//!
//! | Process | PID |
//! |---|---|
//! | Lieferbeginn Gas (Anfrage LFN → NB) | 44001 |
//! | Abmeldung NN / Lieferende Gas (Anfrage LFN → NB) | 44004 |
//! | Bestätigung Anmeldung NN | 44002 |
//! | Ablehnung Anmeldung NN | 44003 |
//! | Bestätigung Abmeldung NN | 44005 |
//! | Ablehnung Abmeldung NN | 44006 |
//! | Abmeldung NN vom NB | 44007–44009 |
//! | Abmeldungsanfrage des NB | 44010–44012 |
//! | Anmeldung/Abmeldung EoG | 44013–44015 |
//! | Kündigung beim alten Lieferanten | 44016 |
//! | Bestätigung / Ablehnung Kündigung (LFA → LFN) | 44017–44018 |
//! | Bestandsliste / Änderungsmeldung | 44019–44021 |
//!
//! Stornierung splits by role. The LF side sends 44022 (ERP-initiated) and
//! receives 44023 / 44024 into `geli-gas-stornierung-lf`; a `Nb`-only deployment
//! takes all three into `geli-gas-stornierung` instead.
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
//! ## „GeLi Gas 2.0" and „GeLi Gas 3.0" are both current, and both correct
//!
//! The two names belong to different documents and neither supersedes the other:
//!
//! | Name | Document | Stand |
//! |---|---|---|
//! | **GeLi Gas 3.0** | the BNetzA Anlage zu BK7-06-067 in der Fassung **BK7-24-01-009** — the Festlegung itself | Beschluss 12.09.2025, Tenor ab 01.01.2026 |
//! | **GeLi Gas 2.0** | the BDEW/VKU/GEODE/FNB Gas **Anwendungshilfe** (V1.2, 26.03.2026), which still carries the BK7-19-001 title | gültig ab 01.04.2026 |
//!
//! The Anwendungsübersicht Prüfidentifikatoren names „GeLi Gas 2.0" in its
//! Festlegungs-Spalte for every 44xxx row, because it indexes the AWH. So a
//! module citing „GeLi Gas 2.0" for a Prüfidentifikator and „GeLi Gas 3.0" for a
//! Frist is not inconsistent — it is citing the two documents that state each.
//!
//! ## Key differences from the electricity processes
//!
//! Both Sparten switch suppliers at the **Marktlokation** and both are driven
//! by UTILMD — the differences that actually change code are these:
//!
//! | Aspect | GPKE (Strom) | GeLi Gas |
//! |---|---|---|
//! | Festlegung | BK6-24-174 (Teil 1–3), BK6-22-024 Anlage 1d (Teil 4) | **BK7-24-01-009** (GeLi Gas 3.0, Tenor ab 01.01.2026) |
//! | Antwortfrist shape | a wall-clock instant on the 1. WT nach dem ÜT — 07:00 / 09:00 / 11:00 / 12:00 | **Ablauf des 4. / 3. / 2. Werktags** nach Eingang |
//! | Zuordnungszeitpunkt | 00:00 Uhr | **06:00 Uhr** — the Gastag runs 06:00–06:00 |
//! | Vorlauffrist des LF | — | **10 WT** Anmeldung, **7 WT** Abmeldung, bei Lieferantenwechsel |
//! | Entscheidungsbäume | `E_06xx` | **`E_30xx`** |
//! | APERAK | Anerkennungs- *und* Verarbeitbarkeitsfehlermeldung; 45 min für UTILMD/ORDERS | **nur Verarbeitbarkeitsfehlermeldung**; nächster WT 12:00 (Folgeprozess) / 3 WT (Initialprozess) |
//! | CONTRL | nur auf eine syntaktisch defekte APERAK | auf **jede** APERAK |
//! | Grid operator | Netzbetreiber (NB) | Gasnetzbetreiber (GNB) |
//!
//! Every Frist above lives in [`mako_fristen`], never in a literal here: the
//! two families disagree on shape as well as on number, and a helper chosen by
//! Sparte rather than by Prüfidentifikator is how the 10-Werktage Vorlauffrist
//! ended up sized as an answer window across this crate.
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
//!                         .and_then(|t| t.marktlokation()).unwrap_or_default(),
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

pub mod antwortfrist;
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
pub mod stammdatenaenderung;
pub mod stornierung;
pub mod zuordnungsmeldung;

pub use stammdatenaenderung::{
    ANFRAGE_PIDS as GAS_STAMMDATEN_ANFRAGE_PIDS, GasAntwort, GasStammdatenCommand,
    GasStammdatenData, GasStammdatenEvent, GasStammdatenState, GeliGasStammdatenaenderungWorkflow,
    STAMMDATEN_PAIRS as GAS_STAMMDATEN_PAIRS, WORKFLOW_NAME as GAS_STAMMDATEN_WORKFLOW_NAME,
    antwort_for as gas_stammdaten_antwort_for, is_aenderung_pid as gas_is_stammdaten_aenderung_pid,
};

pub use datenabruf::{
    GeliGasDatanabrufCommand, GeliGasDatanabrufEvent, GeliGasDatanabrufState,
    GeliGasDatanabrufWorkflow, ORDERS_ANFRAGE_PIDS as GELI_GAS_DATENABRUF_ORDERS_PIDS,
    ORDRSP_ABLEHNUNG_PIDS as GELI_GAS_DATENABRUF_ORDRSP_PIDS,
    WORKFLOW_NAME as GELI_GAS_DATENABRUF_WORKFLOW_NAME,
};
pub use invoic::{
    GeliGasSperrprozesseInvoic, GeliGasSperrprozesseInvoicWorkflow,
    SETTLEMENT_WINDOW_LABEL as SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL, SPERRPROZESSE_INVOIC_PID,
    SPERRPROZESSE_REMADV_PIDS, WORKFLOW_NAME as GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
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

pub use gas_quality::normalize_gasqualitaet;
pub use zuordnungsmeldung::{
    AUFHEBUNG_PID as GAS_AUFHEBUNG_PID, BEENDIGUNG_PID as GAS_BEENDIGUNG_PID,
    GeliGasZuordnungsmeldungWorkflow, INFORMATION_PID as GAS_INFORMATION_PID,
    WORKFLOW_NAME as GAS_ZUORDNUNGSMELDUNG_WORKFLOW_NAME,
    ZUORDNUNGSMELDUNG_PIDS as GAS_ZUORDNUNGSMELDUNG_PIDS,
};

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
/// | `Msb`/`Nmsb` alone | nothing — neither side of the exchange | — |
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
            stammdatenaenderung::WORKFLOW_NAME,
            partin::WORKFLOW_NAME,
            // PID 31011 — Rechnung sonstige Leistung / AWH Sperrprozesse Gas (VNB → LFN/LFA).
            // GeLi Gas (BK7-24-01-009) billing for disconnection services; NOT GaBi Gas.
            invoic::WORKFLOW_NAME,
            zuordnungsmeldung::WORKFLOW_NAME,
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

        // PIDs 44036/44037/44038 (Informationsmeldungen, NB→LFN/LFA/LFZ) —
        // AWH GeLi Gas V1.2 Kap. 2.5.2 SD Lieferbeginn Nr. 2 / 6 / 7. One-way:
        // „eine Nachricht, für die keine Antwort vorgesehen ist" (UTILMD AHB Gas
        // Kap. 5.8), so nothing but this registration keeps an inbound one out
        // of the dead-letter queue.
        for &pid in zuordnungsmeldung::ZUORDNUNGSMELDUNG_PIDS {
            router.register(pid, zuordnungsmeldung::WORKFLOW_NAME);
        }

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

        // GeLi Gas Stammdatenänderung (44109–44182). Change families (G1–G7):
        // both the Änderung PIDs and their shared Antwort PIDs. Anfrage families
        // (G8–G10) are registered too, or they dead-letter. Excludes
        // 44168–44170 (WiM Gas Verpflichtungsanfrage, WimGasModule) and 44183.
        for &(aenderung_pid, antwort_pid, _) in stammdatenaenderung::STAMMDATEN_PAIRS {
            router.register(aenderung_pid, stammdatenaenderung::WORKFLOW_NAME);
            // Antwort PIDs are shared across directions — re-registering to the
            // same workflow is an idempotent overwrite.
            router.register(antwort_pid, stammdatenaenderung::WORKFLOW_NAME);
        }
        for &pid in stammdatenaenderung::ANFRAGE_PIDS {
            router.register(pid, stammdatenaenderung::WORKFLOW_NAME);
        }

        // Gas Sperrung / Entsperrung (LF-side) — PIDs 17115/17117 outbound (LF → GNB),
        // inbound ORDRSP 19116/19117 (Bestätigung/Ablehnung), Storno ORDRSP 19128/19129.
        // PIDs 19116/19117 are shared with GPKE Sperrung Strom; process context is
        // resolved by correlation ID at runtime in mixed Strom+Gas deployments.
        // Regulatory basis: BK7-24-01-009 (GeLi Gas 3.0).
        // Business answer window: per-PID via `mako_fristen::antwort`. APERAK sending Frist: nächster Werktag 12 Uhr (APERAK AHB 1.0 §2.3.1).
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
        // Business answer window: per-PID via `mako_fristen::antwort`. APERAK sending Frist: nächster Werktag 12 Uhr (APERAK AHB 1.0 §2.3.1).
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
        // This is NOT GaBi Gas (BK7-24-01-008); direction is NB → LF, not NB → BKV.
        router.register(
            invoic::SPERRPROZESSE_INVOIC_PID.as_u32(),
            invoic::WORKFLOW_NAME,
        );
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
        // The Stornierung workflow itself is Use-Case-agnostic: it resolves the
        // Ursprungsprozess from `RFF+ACW`, so one owner serves the GeLi Gas
        // Lieferbeginn/-ende *and* the WiM Gas MSB-Wechsel.
        use mako_engine::marktrolle::Marktrolle;

        // 44022 is inbound wherever this deployment can be the *recipient* of a
        // Stornierungsanfrage, which is every NB: GeLi Gas and WiM Gas both send
        // it to the same party („Beteiligte wie bei der Ursprungsnachricht",
        // PID-Übersicht 4.0 rows 37030/39000). The `Msb` role adds nothing —
        // §41 MsbG puts the gMSB inside the NB's own legal entity in most grids,
        // and a combined GNB/gMSB receives the Anfrage exactly once.
        let has_nb = roles.is_all() || roles.contains(Marktrolle::Nb);
        // LF role (lf-only OR integrated): register LFN-side response PIDs.
        // On integrated deployments, the LF *receives* 44002/44003 and 44005/44006 from GNB;
        // the NB only ever *sends* them, so routing to lf-anmeldung is correct.
        let has_lf_role = roles.contains(Marktrolle::Lf) || roles.is_all();
        // LF stornierung: the LF side receives 44023/44024 as answers to a 44022
        // it sent. Disjoint from the NB side's 44022, so no exclusion is needed.
        let has_lf = roles.contains(Marktrolle::Lf);

        if has_nb {
            // Only 44022 is inbound on the GNB side — 44023/44024 are outbound responses
            // dispatched via the outbox and do not need PID-router registration.
            router.register_with_module(
                stornierung::ANFRAGE_PID.as_u32(),
                stornierung::WORKFLOW_NAME,
                "geli-gas",
            );
        }
        if has_lf_role {
            // LF (or integrated): 44002/44003 and 44005/44006 are the GNB's
            // confirmations/rejections of an outbound 44001/44004 this LF sent.
            // Route them to the LFN-side workflow, overriding the unconditional
            // geli-gas-supplier-change registration.
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
