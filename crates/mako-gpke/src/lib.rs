//! `mako-gpke` — GPKE (Geschäftsprozesse Kundenlieferantenwechsel und
//! Netznutzungsabrechnung) process engine for German electricity market
//! communication (BDEW MaKo).
//!
//! ## Process family
//!
//! GPKE governs the standard market processes for supplier switching,
//! grid connection management, and billing reconciliation in the German
//! electricity market:
//!
//! ### UTILMD-based supplier-switching and feed-in processes (LFW24, S2.1/S2.2)
//!
//! #### Inbound ANFRAGE — routed to `gpke-supplier-change`
//!
//! | PID   | Process name (AHB)                                        | Status |
//! |-------|-----------------------------------------------------------|--------|
//! | 55001 | Anfrage Lieferbeginn Strom (LFN → NB)                     | ✅ Implemented |
//! | 55002 | Anfrage Lieferende Strom (LFN → NB)                       | ✅ Implemented |
//! | 55016 | Kündigung Lieferbeginn (LFN → LFA)                       | ✅ Implemented |
//!
//! #### Outbound ANTWORT — derived by `GpkeSupplierChangeWorkflow`, NOT routed (NB role)
//!
//! | PID   | Process name (AHB)                              | Derived from   |
//! |-------|-------------------------------------------------|----------------|
//! | 55003 | Bestätigung Lieferbeginn (NB → LFN)             | 55001 accepted |
//! | 55004 | Ablehnung Lieferbeginn (NB → LFN)               | 55001 rejected |
//! | 55005 | Bestätigung Lieferende (NB → LFN)               | 55002 accepted |
//! | 55006 | Ablehnung Lieferende (NB → LFN)                 | 55002 rejected |
//! | 55017 | Bestätigung Kündigung Lieferbeginn (LFA → LFN)  | 55016 accept  |
//! | 55018 | Ablehnung Kündigung Lieferbeginn (LFA → LFN)    | 55016 reject  |
//!
//! #### Inbound ANTWORT — routed to `gpke-lf-anmeldung` (LF role)
//!
//! When `makod` acts as **Lieferant**, it sends the outbound ANFRAGE and
//! subsequently receives the NB/LFA response via AS4.  These PIDs are
//! registered to route back to [`GpkeLfAnmeldungWorkflow`].
//!
//! | PID   | Process name (AHB)                              | Initiated by   |
//! |-------|-------------------------------------------------|----------------|
//! | 55003 | Bestätigung Lieferbeginn (NB → LFN)             | 55001 Anfrage  |
//! | 55004 | Ablehnung Lieferbeginn (NB → LFN)               | 55001 Anfrage  |
//! | 55005 | Bestätigung Lieferende (NB → LFN)               | 55002 Anfrage  |
//! | 55006 | Ablehnung Lieferende (NB → LFN)                 | 55002 Anfrage  |
//! | 55017 | Bestätigung Kündigung Lieferbeginn (LFA → LFN)  | 55016 Kündigung|
//! | 55018 | Ablehnung Kündigung Lieferbeginn (LFA → LFN)    | 55016 Kündigung|
//! | 55078 | Bestätigung Anmeldung erz. MaLo (NB → LFN)      | 55077 Anfrage  |
//! | 55080 | Ablehnung Anmeldung erz. MaLo (NB → LFN)        | 55077 Anfrage  |
//!
//! #### Sperrung / Entsperrung — routed to `gpke-sperrung`
//!
//! | PID   | Process name (AWH)              | Status         |
//! |-------|-------------------------------|----------------|
//! | 17115 | Sperrauftrag (NB → LFN)        | ✅ Implemented |
//! | 17116 | Anfrage Sperrung (NB → LFN)    | ✅ Implemented |
//! | 17117 | Entsperrauftrag (NB → LFN)     | ✅ Implemented |
//!
//! #### Stornierung — routed to `gpke-stornierung`
//!
//! | PID   | Process name (AHB)                      | Status |
//! |-------|-----------------------------------------|--------|
//! | 55022 | Anfrage nach Stornierung (LFN → NB)     | ✅ Implemented |
//! | 55023 | Bestätigung Stornierung (NB → LFN)      | ✅ Implemented |
//! | 55024 | Ablehnung Stornierung (NB → LFN)        | ✅ Implemented |
//!
//! ### Neuanlage — routed to `gpke-neuanlage`
//!
//! | PID   | Process name (AHB)                               | Status |
//! |-------|--------------------------------------------------|--------|
//! | 55600 | Anmeldung neue verb. MaLo (LF → NB)             | ✅ Implemented |
//! | 55601 | Anmeldung neue erz. MaLo (LF → NB)              | ✅ Implemented |
//!
//! ### NB-initiated Lieferende — routed to `gpke-lf-abmeldung`
//!
//! | PID   | Process name (AHB)                                   | Status |
//! |-------|------------------------------------------------------|--------|
//! | 55007 | Ankündigung NB-seitiges Lieferende (NB → LFN)        | ✅ Implemented |
//!
//! ### Ankündigung Zuordnung LF — routed to `gpke-ankuendigung-zuordnung-lf`
//!
//! | PID   | Process name (AHB)                               | Status |
//! |-------|--------------------------------------------------|--------|
//! | 55607 | Ankündigung Zuordnung LF (NB → LFN)              | ✅ Implemented |
//!
//! PIDs 55608 (Bestätigung) and 55609 (Ablehnung) are outbound responses derived
//! by `GpkeAnkuendigungZuordnungLfWorkflow` and never routed as inbound.
//!
//! PIDs 55008 (Bestätigung) and 55009 (Ablehnung) are outbound responses derived
//! by `GpkeLfAbmeldungWorkflow` and never routed as inbound.
//!
//! **Note on AHB coverage:** PIDs 55010–55015 (NB-initiated processes) are confirmed
//! present in UTILMD AHB Strom 2.1 (FV2025-10-01) but require separate NB-side
//! workflows that are not yet implemented.
//!
//! The 3 inbound ANFRAGE PIDs share [`GpkeSupplierChangeWorkflow`] (workflow name:
//! `"gpke-supplier-change"`). The `pruefidentifikator` stored in
//! [`wechselprozesse::InitiatedData`] lets read-models distinguish variants.
//! The derived ANTWORT PIDs (55003–55006, 55017, 55018) are recorded in the
//! `AntwortGesendet` event but are not routed as inbound messages.
//!
//! ### INVOIC-based billing processes (GPKE Netznutzungsabrechnung / MMM Strom)
//!
//! | PID   | Process name                             | Status |
//! |-------|------------------------------------------|--------|
//! | 31001 | Abschlagsrechnung (Netznutzung)              | ✅ Implemented |
//! | 31002 | NN-Rechnung (Netznutzungsabrechnung)          | ✅ Implemented |
//! | 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)        | ✅ Implemented |
//! | 31006 | MMM-Rechnung (selbst ausgestellt)            | ✅ Implemented |
//! | 31007 | Aggregierte Mehr-/Mindermenge Rechnung       | ✅ Implemented |
//! | 31008 | Aggregierte Mehr-/Mindermenge Rechnung (SA)  | ✅ Implemented |
//!
//! All 6 PIDs use [`GpkeAbrechnungWorkflow`] (workflow name:
//! `"gpke-abrechnung"`). The `pruefidentifikator` stored in
//! [`abrechnung::AbrechnungData`] lets read-models distinguish variants.
//! PID 31003 (WiM-Rechnung) belongs to `mako-wim`. PID 31004 (Stornorechnung
//! WiM Gas) belongs to `mako-wim-gas` (BK7-24-01-009) — not registered here.
//! PID 31009 (MSB-Rechnung, multi-domain: GPKE Teil 3 / WiM Strom Teil 1) is
//! registered by `mako-wim` (`wim-rechnung`) to avoid double-registration;
//! see `crates/mako-wim/src/rechnung.rs`.
//!
//! ## Architecture
//!
//! Each BDEW process **group** maps to a single parameterised workflow.
//! The PID value, stored in domain state, distinguishes process variants
//! within a group without requiring duplicate workflow implementations.
//!
//! This crate contains **only pure domain logic** — no I/O, no EDIFACT
//! parsing, no network calls. Parsing and validation of raw EDIFACT bytes
//! happen at the transport boundary (AS4 reception layer), **before**
//! constructing a domain command.
//!
//! ## Command construction example (UTILMD)
//!
//! ```rust,ignore
//! use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
//! use mako_gpke::wechselprozesse::{GpkeSupplierChangeWorkflow, SupplierChangeCommand};
//!
//! let msg    = Platform::with_all_profiles().parse(&raw_bytes)?;
//! let report = msg.validate()?;
//! let AnyMessage::Utilmd(u) = &msg else { anyhow::bail!("not UTILMD") };
//!
//! let cmd = SupplierChangeCommand::ReceiveUtilmd {
//!     pid:               msg.detect_pruefidentifikator()?,
//!     sender:            u.sender().and_then(|n| n.party_id.clone()).unwrap_or_default(),
//!     receiver:          u.receiver().and_then(|n| n.party_id.clone()).unwrap_or_default(),
//!     location_id:       u.transactions().first()
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

pub mod abrechnung;
pub mod allokationsliste;
pub mod anfrage_bestellung;
pub mod ankuendigung_zuordnung_lf;
pub mod datenabruf;
pub mod konfiguration;
pub mod konfiguration_aenderung;
pub mod lf_abmeldung;
pub mod lf_anmeldung;
pub mod messwerte;
pub mod neuanlage;
pub mod partin;
pub mod post_acceptance;
pub mod sperrung;
pub mod sperrung_lf;
pub mod stornierung;
pub mod utilts;
pub mod wechselprozesse;

pub use abrechnung::{
    ABRECHNUNG_WINDOW_LABEL, AbrechnungCommand, AbrechnungData, AbrechnungEvent,
    AbrechnungProjection, AbrechnungRecord, AbrechnungState, COMDIS_ABLEHNUNG_REMADV_PID,
    GpkeAbrechnungWorkflow, INVOIC_PIDS, REMADV_PIDS,
};
pub use allokationsliste::{
    AllokationslisteCommand, AllokationslisteEvent, AllokationslisteState, AnforderungData,
    GpkeAllokationslisteWorkflow, MSCONS_RESPONSE_PIDS as ALLOKATIONSLISTE_MSCONS_PIDS,
    ORDERS_ANFRAGE_PIDS as ALLOKATIONSLISTE_ORDERS_PIDS,
    ORDRSP_ABLEHNUNG_PIDS as ALLOKATIONSLISTE_ORDRSP_PIDS,
    WORKFLOW_NAME as ALLOKATIONSLISTE_WORKFLOW_NAME,
};
pub use anfrage_bestellung::{
    ANFRAGE_PID as ANFRAGE_BESTELLUNG_PID, ANFRAGE_WINDOW_LABEL, AnfrageBestellungCommand,
    AnfrageBestellungEvent, AnfrageBestellungState, AnfrageData, GpkeAnfrageBestellungWorkflow,
    WORKFLOW_NAME as ANFRAGE_BESTELLUNG_WORKFLOW_NAME,
};
pub use ankuendigung_zuordnung_lf::{
    ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL, ANKUENDIGUNG_ZUORDNUNG_PIDS,
    AnkuendigungZuordnungLfCommand, AnkuendigungZuordnungLfData, AnkuendigungZuordnungLfEvent,
    AnkuendigungZuordnungLfState, GpkeAnkuendigungZuordnungLfWorkflow,
    WORKFLOW_NAME as ANKUENDIGUNG_ZUORDNUNG_LF_WORKFLOW_NAME,
};
pub use datenabruf::{
    DatanabrufCommand, DatanabrufEvent, DatanabrufState, GpkeDatanabrufWorkflow,
    ORDERS_ANFRAGE_PIDS as DATENABRUF_ORDERS_PIDS, ORDRSP_ABLEHNUNG_PIDS as DATENABRUF_ORDRSP_PIDS,
    WORKFLOW_NAME as DATENABRUF_WORKFLOW_NAME,
};
pub use konfiguration::{
    BeauftragungData, GpkeKonfigurationWorkflow, KONFIGURATION_WINDOW_LABEL, KonfigurationCommand,
    KonfigurationEvent, KonfigurationProjection, KonfigurationRecord, KonfigurationState,
    ORDERS_PIDS, ORDRSP_PIDS,
};
pub use konfiguration_aenderung::{
    GpkeKonfigurationAenderungWorkflow, IFTSTA_PIDS as KONFIGURATION_AENDERUNG_IFTSTA_PIDS,
    KonfigurationAenderungCommand, KonfigurationAenderungEvent, KonfigurationAenderungState,
    ORDERS_ANFRAGE_PIDS as KONFIGURATION_AENDERUNG_ORDERS_PIDS,
    ORDRSP_PIDS as KONFIGURATION_AENDERUNG_ORDRSP_PIDS,
    WORKFLOW_NAME as KONFIGURATION_AENDERUNG_WORKFLOW_NAME,
};
pub use lf_abmeldung::{
    GpkeLfAbmeldungWorkflow, LF_ABMELDUNG_APERAK_WINDOW_LABEL, LF_ABMELDUNG_PIDS,
    LfAbmeldungCommand, LfAbmeldungData, LfAbmeldungEvent, LfAbmeldungState,
};
pub use lf_anmeldung::{
    ANFRAGE_PIDS_LF, ANTWORT_PIDS_LF, GpkeLfAnmeldungWorkflow, LfAnmeldungCommand, LfAnmeldungData,
    LfAnmeldungEvent, LfAnmeldungState, NB_RESPONSE_WINDOW_LABEL,
};
pub use messwerte::{
    GpkeMesswerteLieferungWorkflow, MSCONS_PIDS, MesswerteLieferungCommand, MesswerteLieferungData,
    MesswerteLieferungEvent, MesswerteLieferungState,
};
pub use neuanlage::{
    GpkeNeuanlageWorkflow, NEUANLAGE_APERAK_WINDOW_LABEL, NEUANLAGE_PIDS, NeuanlageCommand,
    NeuanlageData, NeuanlageEvent, NeuanlageState,
};
pub use partin::{
    GpkePartinWorkflow, KommunikationsdatenCommand, KommunikationsdatenData,
    KommunikationsdatenEvent, KommunikationsdatenState, PARTIN_STROM_PIDS,
};
pub use sperrung::{
    GpkeSperrungWorkflow, MSB_ANTWORT_PIDS, ORDCHG_STORNIERUNG_PIDS, SPERRUNG_PIDS,
    SPERRUNG_WINDOW_LABEL, SperrungCommand, SperrungData, SperrungEvent, SperrungState,
};
pub use sperrung_lf::{
    ANTWORT_WINDOW_LABEL as SPERRUNG_LF_ANTWORT_WINDOW_LABEL, GpkeSperrungLfWorkflow,
    IFTSTA_SPERRUNG_PID, ORDRSP_SPERRUNG_PIDS, ORDRSP_STORNO_PIDS,
    SPERRUNG_ANFRAGE_PIDS as SPERRUNG_LF_ANFRAGE_PIDS, SperrungAuftragData, SperrungLfCommand,
    SperrungLfEvent, SperrungLfState,
};
pub use stornierung::{
    GpkeStornierungCommand, GpkeStornierungData, GpkeStornierungEvent, GpkeStornierungState,
    GpkeStornierungWorkflow,
    STORNIERUNG_APERAK_WINDOW_LABEL as STORNIERUNG_GPKE_APERAK_WINDOW_LABEL,
    STORNIERUNG_PIDS as STORNIERUNG_GPKE_PIDS,
};
pub use utilts::{
    GpkeUtiltsWorkflow, UTILTS_PIDS, UtiltsKonfigCommand, UtiltsKonfigData, UtiltsKonfigEvent,
    UtiltsKonfigState,
};
pub use wechselprozesse::{
    APERAK_WINDOW_LABEL, GpkeSupplierChangeWorkflow, IFTSTA_PIDS as IFTSTA_VOLLZUGS_PIDS,
    InitiatedData, InitiatedDetails, SupplierChangeCommand, SupplierChangeEvent,
    SupplierChangeProjection, SupplierChangeRecord, SupplierChangeState, UTILMD_PIDS,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the GPKE process family.
///
/// Registers all GPKE Prüfidentifikator values:
/// - PIDs 55001–55002, 55016 (inbound ANFRAGE, UTILMD) → `"gpke-supplier-change"`
/// - PIDs 55022, 55023, 55024 (Stornierung Anfrage + Antwort, UTILMD) → `"gpke-stornierung"`
/// - PIDs 55600, 55601 (Neuanlage ANFRAGE, UTILMD) → `"gpke-neuanlage"`
/// - PID 55007 (NB-seitiges Lieferende, UTILMD) → `"gpke-lf-abmeldung"`
/// - PID 55607 (Ankündigung Zuordnung LF, UTILMD) → `"gpke-ankuendigung-zuordnung-lf"`
/// - PIDs 17115/17116/17117 (Sperrung/Entsperrung, ORDERS) → `"gpke-sperrung"`
/// - **PID 55555** (Anfrage Daten der individuellen Bestellung, UTILMD) → `"gpke-anfrage-bestellung"`
/// - PIDs 31001, 31002, 31005–31009 (billing, INVOIC) → `"gpke-abrechnung"`
///   _(31003 → `mako-wim`; 31004 → `mako-wim-gas`)_
/// - PIDs 19001, 19002 (inbound ORDRSP, NB role only) → `"gpke-konfiguration"`
///
/// **Role-conditional PIDs (ORDRSP 19001/19002):**
///
/// PIDs 19001 (`Bestellbestätigung`) and 19002 (`Ablehnung der Bestellung`)
/// are registered **only when [`DeploymentRoles`] contains [`Marktrolle::Nb`]**.
///
/// In the GPKE Konfiguration workflow:
/// - NB sends outbound ORDERS 17134/17135 (via outbox) to the designated MSB.
/// - The MSB responds with inbound ORDRSP 19001/19002, which must route back to
///   `gpke-konfiguration` for the NB-role makod instance.
///
/// On a **nMSB** (Herausforderer-MSB) instance, the same PIDs 19001/19002 are
/// the response to WiM Geräteübernahme ORDERS 17001 sent by the nMSB to the NB.
/// They route to `wim-geraeteubernahme` instead. Set explicit [`DeploymentRoles`]
/// to prevent both modules from claiming the same PIDs.
///
/// **Not registered (outbound-only):**
/// - PIDs 55003–55006, 55017, 55018 are outbound ANTWORT messages derived by
///   `GpkeSupplierChangeWorkflow::handle`. They are never routed as inbound.
/// - PIDs 17134, 17135 are outbound ORDERS messages dispatched via the outbox
///   by `GpkeKonfigurationWorkflow`. They are never routed as inbound.
///
/// PIDs 55007–55009 (NB-seitiges Lieferende) are handled by `GpkeLfAbmeldungWorkflow`.
/// PID 55010 (pre-LFW24 Stornierung) is not present in AHB Strom 2.1 and is CONTRL-rejected.
///
/// [`DeploymentRoles`]: mako_engine::marktrolle::DeploymentRoles
/// [`Marktrolle::Nb`]: mako_engine::marktrolle::Marktrolle::Nb
///
/// Use with [`mako_engine::builder::EngineBuilder::register`]:
///
/// ```rust,ignore
/// use mako_gpke::GpkeModule;
/// use mako_engine::builder::EngineBuilder;
/// use mako_engine::marktrolle::DeploymentRoles;
///
/// let ctx = EngineBuilder::new()
///     .with_event_store(store)
///     .with_deployment_roles(DeploymentRoles::nb())
///     .register(Box::new(GpkeModule))
///     .build();
/// ```
pub struct GpkeModule;

impl mako_engine::builder::EngineModule for GpkeModule {
    fn name(&self) -> &'static str {
        "gpke"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            "gpke-supplier-change",
            lf_anmeldung::WORKFLOW_NAME,
            sperrung::WORKFLOW_NAME,
            sperrung_lf::WORKFLOW_NAME,
            anfrage_bestellung::WORKFLOW_NAME,
            "gpke-abrechnung",
            "gpke-konfiguration",
            "gpke-neuanlage",
            "gpke-lf-abmeldung",
            ankuendigung_zuordnung_lf::WORKFLOW_NAME,
            stornierung::WORKFLOW_NAME,
            messwerte::WORKFLOW_NAME,
            partin::WORKFLOW_NAME,
            utilts::WORKFLOW_NAME,
            konfiguration_aenderung::WORKFLOW_NAME,
            datenabruf::WORKFLOW_NAME,
            allokationsliste::WORKFLOW_NAME,
        ]
    }

    fn register_pids_with_roles(
        &self,
        router: &mut mako_engine::pid_router::PidRouter,
        roles: &mako_engine::marktrolle::DeploymentRoles,
    ) {
        // UTILMD inbound ANFRAGE PIDs — routed to gpke-supplier-change.
        // Only inbound request PIDs are registered. The outbound ANTWORT PIDs
        // (55003–55006, 55017, 55018) are derived internally and never routed as inbound.
        for &pid in UTILMD_PIDS {
            router.register(pid, "gpke-supplier-change");
        }

        // PIDs 55600/55601 (Neuanlage neue Marktlokation) — BK6-24-174 Anlage 1b.
        for &pid in NEUANLAGE_PIDS {
            router.register(pid, "gpke-neuanlage");
        }

        // PID 55007 (NB-seitiges Lieferende, NB→LF) — GPKE Teil 2 §2.5.
        // LF-role makod receives PID 55007 and responds with 55008/55009.
        for &pid in LF_ABMELDUNG_PIDS {
            router.register(pid, "gpke-lf-abmeldung");
        }

        // PID 55607 (Ankündigung Zuordnung LF, NB→LFN) — GPKE Teil 2 §2.2, BK6-24-174.
        // LF-role makod receives PID 55607 and responds with 55608/55609.
        for &pid in ANKUENDIGUNG_ZUORDNUNG_PIDS {
            router.register(pid, ankuendigung_zuordnung_lf::WORKFLOW_NAME);
        }

        // ORDERS PIDs 17115/17116/17117 (Sperrung/Entsperrung) — NB-role workflow.
        // Per BDEW PID overview: "AWH Sperrprozesse" applies to both Strom and Gas.
        // Direction: LF → NB (17115 Sperrauftrag, 17117 Entsperrauftrag);
        //            NB → MSB (17116 Anfrage Sperrung).
        for &pid in SPERRUNG_PIDS {
            router.register(pid, sperrung::WORKFLOW_NAME);
        }

        // ORDCHG 39000/39001 (Stornierung Sperr-/Entsperrauftrag).
        // 39000: LF → NB (LF cancels a pending Sperrauftrag).
        // 39001: NB → MSB (Weiterleitung der Stornierung — NB forwards LF cancellation to MSB).
        for &pid in ORDCHG_STORNIERUNG_PIDS {
            router.register(pid, sperrung::WORKFLOW_NAME);
        }

        // ORDRSP 19118/19119 (MSB → NB: MSB's response to Anfrage Sperrung 17116).
        // Only relevant when running in NB role (NB sends Anfrage to MSB and waits).
        for &pid in MSB_ANTWORT_PIDS {
            router.register(pid, sperrung::WORKFLOW_NAME);
        }

        // ORDRSP 19116/19117/19128/19129 (NB → LF: NB's response to Sperrauftrag/Stornierung).
        // Registered for the LF-role `gpke-sperrung-lf` workflow so LF receives NB's answer.
        for &pid in ORDRSP_SPERRUNG_PIDS {
            router.register(pid, sperrung_lf::WORKFLOW_NAME);
        }
        for &pid in ORDRSP_STORNO_PIDS {
            router.register(pid, sperrung_lf::WORKFLOW_NAME);
        }

        // IFTSTA 21039 (Auftragsstatus Sperren, NB → LF).
        // LF receives the execution status from NB after the Sperrung is carried out.
        router.register(IFTSTA_SPERRUNG_PID, sperrung_lf::WORKFLOW_NAME);

        // INVOIC-based: all 6 billing PIDs use `GpkeAbrechnungWorkflow`.
        for &pid in INVOIC_PIDS {
            router.register(pid, "gpke-abrechnung");
        }

        // REMADV 33001–33004 — inbound payment advice from payer to invoicer.
        //
        // After the NB/MSB sends an INVOIC (billing invoice), the payer (LF/NB)
        // sends back a REMADV to confirm or dispute the payment. These PIDs must
        // be routed to `gpke-abrechnung` so the `ReceiveRemadv` command can
        // correlate the REMADV with the correct INVOIC process stream.
        //
        // Without this registration, all inbound REMADV messages are silently
        // dead-lettered by the AS4 ingest layer (MessageStatus::UnknownPid),
        // breaking the billing cycle entirely.
        //
        // Source: REMADV AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
        for &pid in REMADV_PIDS {
            router.register(pid, "gpke-abrechnung");
        }

        // COMDIS 29001 — inbound Ablehnung REMADV (invoicer rejects payer's REMADV).
        //
        // After the payer sends a REMADV, the invoicer (NB/MSB) may reject it
        // via COMDIS 29001. This is a different PID from APERAK 29001 (which is
        // an outbound Verarbeitbarkeitsfehler acknowledgement). COMDIS 29001 is
        // inbound from the invoicer and belongs to the billing cycle.
        //
        // Source: COMDIS AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
        router.register(COMDIS_ABLEHNUNG_REMADV_PID, "gpke-abrechnung");

        // ORDRSP inbound PIDs for Konfigurationseinrichtung (19001/19002).
        //
        // NB role only: the NB sends ORDERS 17134/17135 outbound (via outbox)
        // to the designated MSB and receives ORDRSP 19001/19002 back.
        // On nMSB instances these same PIDs are WiM Geräteübernahme responses
        // and route to `wim-geraeteubernahme` — controlled via DeploymentRoles.
        if roles.contains(mako_engine::marktrolle::Marktrolle::Nb) {
            for &pid in ORDRSP_PIDS {
                // register_with_module enforces the documented guarantee: if both NB
                // and nMSB roles are active simultaneously, build() panics before any
                // message is processed instead of silently routing to the wrong workflow.
                router.register_with_module(pid, "gpke-konfiguration", "gpke");
            }
        }

        // LF-side Anmeldung: inbound NB/LFA response PIDs (55003–55006, 55017, 55018, 55078, 55080).
        // Registered so the AS4 inbound layer can route them by conversation ID
        // to the correct GpkeLfAnmeldungWorkflow instance (makod acting as LF).
        // 55078 = Bestätigung Anmeldung erz. MaLo (NB → LFN)
        // 55080 = Ablehnung Anmeldung erz. MaLo  (NB → LFN); PID 55079 unassigned
        for &pid in ANTWORT_PIDS_LF {
            router.register(pid, lf_anmeldung::WORKFLOW_NAME);
        }

        // IFTSTA GPKE Vollzugsmeldungen (PIDs 21024–21028, 21033).
        //
        // PIDs 21024–21028 are "GPKE / Vollzugsmeldung" per the IFTSTA AHB.
        // PID 21033 is "GPKE / Statusmeldung Kündigung" (Ablehnung GPKE Teil 3).
        // Previously, 21024–21028 were incorrectly attributed to WiM Gas in
        // docs/pid-reference.md; the AHB profile is authoritative.
        // PID 21039 (Auftragsstatus Sperren) is registered to `gpke-sperrung-lf` above.
        for &pid in wechselprozesse::IFTSTA_PIDS {
            router.register(pid, "gpke-supplier-change");
        }

        // MSCONS data delivery PIDs (NB/MSB → LF, GPKE Teil 2/4, WiM Strom Teil 2).
        //
        // These are inbound MSCONS messages containing metered energy data that the
        // NB or MSB sends to the LF. Essential for LF billing reconciliation.
        // Registered unconditionally (both LF and NB deployments receive MSCONS).
        for &pid in messwerte::MSCONS_PIDS {
            router.register(pid, messwerte::WORKFLOW_NAME);
        }

        // PARTIN Kommunikationsdaten Strom (GPKE Teil 4).
        //
        // PIDs 37000–37006 exchange Strom market participant communication data
        // (AS4 endpoints, GLNs, contact details) between LF, NB, MSB, and ÜNB.
        // Gas PARTIN (PIDs 37008–37014) is handled by mako-geli-gas (geli-gas-partin).
        for &pid in partin::PARTIN_STROM_PIDS {
            router.register(pid, partin::WORKFLOW_NAME);
        }

        // UTILTS Konfigurationsdaten (GPKE Teil 3, WiM Strom/Gas Teil 2).
        //
        // UTILTS messages convey metering configuration definitions
        // (Zählzeit-, Schaltzeit-, Leistungskurvendefinitionen) from NB/MSB to LF.
        for &pid in utilts::UTILTS_PIDS {
            router.register(pid, utilts::WORKFLOW_NAME);
        }

        // GPKE Stornierung PIDs 55022/55023/55024 (Anfrage + Antwort).
        // NB role: NB receives 55022 inbound and dispatches 55023/55024.
        // All three are registered so routing works for both inbound legs.
        for &pid in stornierung::STORNIERUNG_PIDS {
            router.register(pid, stornierung::WORKFLOW_NAME);
        }

        // PID 55555 — Anfrage Daten der individuellen Bestellung (GPKE Teil 4).
        // LFN queries NB for data about a specific order. NB must respond
        // within 24 wall-clock hours (BK6-22-024 §5). Governed by BK6-24-174.
        router.register(
            anfrage_bestellung::ANFRAGE_PID,
            anfrage_bestellung::WORKFLOW_NAME,
        );

        // GPKE Teil 3 Konfigurationsänderung — LF-initiated config change requests.
        //
        // LF sends ORDERS 17120/17122/17123/17128–17131/17133 to NB or MSB.
        // NB/MSB responds with ORDRSP 19120–19133 (various confirmation/rejection/status PIDs).
        // IFTSTA 21043/21044 are informational status/completion messages for this process.
        // All routed to `gpke-konfiguration-aenderung`.
        for &pid in konfiguration_aenderung::ORDERS_ANFRAGE_PIDS {
            router.register(pid, konfiguration_aenderung::WORKFLOW_NAME);
        }
        for &pid in konfiguration_aenderung::ORDRSP_PIDS {
            router.register(pid, konfiguration_aenderung::WORKFLOW_NAME);
        }
        for &pid in konfiguration_aenderung::IFTSTA_PIDS {
            router.register(pid, konfiguration_aenderung::WORKFLOW_NAME);
        }

        // GPKE Datenabruf — LF-initiated data-value requests and reclamations.
        //
        // LF sends ORDERS 17102/17113 (Anfrage/Reklamation von Werten) to NB/MSB.
        // NB/MSB rejects with ORDRSP 19101/19102/19114 (positive response via MSCONS).
        for &pid in datenabruf::ORDERS_ANFRAGE_PIDS {
            router.register(pid, datenabruf::WORKFLOW_NAME);
        }
        for &pid in datenabruf::ORDRSP_ABLEHNUNG_PIDS {
            router.register(pid, datenabruf::WORKFLOW_NAME);
        }

        // GPKE Allokationsliste — LF requests allocation lists (MMM Strom/Gas).
        //
        // LF sends ORDERS 17110/17114, NB rejects with ORDRSP 19110/19115.
        // Positive response comes via MSCONS (13013/13014) — MMM Strom/Gas PIDs,
        // NOT GeLi Gas. Routed here per BK6-22-024 §8 / MMM AHB.
        for &pid in allokationsliste::ORDERS_ANFRAGE_PIDS {
            router.register(pid, allokationsliste::WORKFLOW_NAME);
        }
        for &pid in allokationsliste::ORDRSP_ABLEHNUNG_PIDS {
            router.register(pid, allokationsliste::WORKFLOW_NAME);
        }
        for &pid in allokationsliste::MSCONS_RESPONSE_PIDS {
            router.register(pid, allokationsliste::WORKFLOW_NAME);
        }

        // EnFG IFTSTA PIDs (21045, 21047) and Rückmeldung (21035) are included
        // in wechselprozesse::IFTSTA_PIDS and registered above under gpke-supplier-change.
        // IFTSTA Konfigurationsbestellungsantworten (21043, 21044) are registered above
        // under gpke-konfiguration-aenderung.
        // PID 21042 (Bestellung WiM, WiM Strom Teil 2) has no GPKE crate assignment
        // per docs/pid-reference.md and is correctly dead-lettered.
    }

    fn profile_requirements(&self) -> &'static [mako_engine::profile::ProfileRequirement] {
        use mako_engine::profile::ProfileRequirement;
        &[
            ProfileRequirement {
                message_type: "UTILMD",
                label: "UTILMD Strom (GPKE Lieferantenwechsel)",
            },
            ProfileRequirement {
                message_type: "INVOIC",
                label: "INVOIC Abrechnung (GPKE)",
            },
            ProfileRequirement {
                message_type: "REMADV",
                label: "REMADV Zahlungsavis (GPKE 33001–33004)",
            },
            ProfileRequirement {
                message_type: "COMDIS",
                label: "COMDIS Ablehnung REMADV (GPKE 29001)",
            },
            ProfileRequirement {
                message_type: "IFTSTA",
                label: "IFTSTA Vollzugsmeldung (GPKE 21033) + Auftragsstatus Sperren (21039)",
            },
            ProfileRequirement {
                message_type: "ORDRSP",
                label: "ORDRSP Sperrung (19116/19117/19118/19119/19128/19129) + Konfiguration (19001/19002)",
            },
            ProfileRequirement {
                message_type: "ORDCHG",
                label: "ORDCHG Stornierung Sperrauftrag (39000/39001)",
            },
            ProfileRequirement {
                message_type: "MSCONS",
                label: "MSCONS Messdatenlieferung NB/MSB → LF (13015–13027)",
            },
            ProfileRequirement {
                message_type: "PARTIN",
                label: "PARTIN Kommunikationsdaten Strom (37000–37006)",
            },
            ProfileRequirement {
                message_type: "UTILTS",
                label: "UTILTS Konfigurationsdaten GPKE Teil 3 (25001, 25004–25010)",
            },
            ProfileRequirement {
                message_type: "ORDERS",
                label: "ORDERS Konfigurationsänderung/Datenabruf/Allokationsliste (17102–17133)",
            },
            ProfileRequirement {
                message_type: "IFTSTA",
                label: "IFTSTA Vollzugsmeldung/Statusmeldung (GPKE 21024-21028, 21033, 21035, 21045, 21047) + Auftragsstatus Sperren (21039) + Konfiguration (21043, 21044)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        // Verify that all static PID slices are non-empty.  An empty slice
        // would mean the module registers no routes, which is always a bug
        // (e.g. an accidental empty const, a codegen regression, or a stale
        // feature flag).  Discovered at startup rather than on first inbound
        // message.
        let named: &[(&str, &[u32])] = &[
            ("UTILMD_PIDS", UTILMD_PIDS),
            ("SPERRUNG_PIDS", SPERRUNG_PIDS),
            ("ORDCHG_STORNIERUNG_PIDS", ORDCHG_STORNIERUNG_PIDS),
            ("ORDRSP_SPERRUNG_PIDS", ORDRSP_SPERRUNG_PIDS),
            ("ORDRSP_STORNO_PIDS", ORDRSP_STORNO_PIDS),
            ("MSB_ANTWORT_PIDS", MSB_ANTWORT_PIDS),
            ("INVOIC_PIDS", INVOIC_PIDS),
            ("REMADV_PIDS", REMADV_PIDS),
            ("ORDRSP_PIDS", ORDRSP_PIDS),
            ("ANTWORT_PIDS_LF", ANTWORT_PIDS_LF),
            ("wechselprozesse::IFTSTA_PIDS", wechselprozesse::IFTSTA_PIDS),
            ("NEUANLAGE_PIDS", NEUANLAGE_PIDS),
            ("LF_ABMELDUNG_PIDS", LF_ABMELDUNG_PIDS),
            (
                "stornierung::STORNIERUNG_PIDS",
                stornierung::STORNIERUNG_PIDS,
            ),
            ("messwerte::MSCONS_PIDS", messwerte::MSCONS_PIDS),
            ("partin::PARTIN_STROM_PIDS", partin::PARTIN_STROM_PIDS),
            ("utilts::UTILTS_PIDS", utilts::UTILTS_PIDS),
            (
                "konfiguration_aenderung::ORDERS_ANFRAGE_PIDS",
                konfiguration_aenderung::ORDERS_ANFRAGE_PIDS,
            ),
            (
                "konfiguration_aenderung::ORDRSP_PIDS",
                konfiguration_aenderung::ORDRSP_PIDS,
            ),
            (
                "datenabruf::ORDERS_ANFRAGE_PIDS",
                datenabruf::ORDERS_ANFRAGE_PIDS,
            ),
            (
                "datenabruf::ORDRSP_ABLEHNUNG_PIDS",
                datenabruf::ORDRSP_ABLEHNUNG_PIDS,
            ),
            (
                "allokationsliste::ORDERS_ANFRAGE_PIDS",
                allokationsliste::ORDERS_ANFRAGE_PIDS,
            ),
            (
                "allokationsliste::ORDRSP_ABLEHNUNG_PIDS",
                allokationsliste::ORDRSP_ABLEHNUNG_PIDS,
            ),
            (
                "allokationsliste::MSCONS_RESPONSE_PIDS",
                allokationsliste::MSCONS_RESPONSE_PIDS,
            ),
            (
                "konfiguration_aenderung::IFTSTA_PIDS",
                konfiguration_aenderung::IFTSTA_PIDS,
            ),
        ];
        for (name, pids) in named {
            if pids.is_empty() {
                return Err(format!(
                    "gpke: PID slice '{name}' is empty — \
                     at least one PID must be registered for each workflow group",
                ));
            }
        }
        // ANFRAGE_PID is a scalar constant (55555); verify it's in the valid
        // Prüfidentifikator range as a sanity check.
        if anfrage_bestellung::ANFRAGE_PID < 10_000 || anfrage_bestellung::ANFRAGE_PID > 99_999 {
            return Err(format!(
                "gpke: anfrage_bestellung::ANFRAGE_PID {} is outside the valid \
                 Prüfidentifikator range 10000–99999",
                anfrage_bestellung::ANFRAGE_PID,
            ));
        }
        Ok(())
    }
}
