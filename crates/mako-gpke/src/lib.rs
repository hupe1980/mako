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
//! | 31009 | MSB-Rechnung (GPKE Teil 3 — NB/MSB)          | ✅ Implemented |
//!
//! All 7 PIDs use [`GpkeAbrechnungWorkflow`] (workflow name:
//! `"gpke-abrechnung"`). The `pruefidentifikator` stored in
//! [`abrechnung::AbrechnungData`] lets read-models distinguish variants.
//! PID 31003 (WiM-Rechnung) belongs to `mako-wim`. PID 31004 (Stornorechnung
//! WiM Gas) belongs to `mako-wim-gas` (BK7-24-01-009) — not registered here.
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

#![deny(missing_docs)]

pub mod abrechnung;
pub mod anfrage_bestellung;
pub mod konfiguration;
pub mod lf_abmeldung;
pub mod lf_anmeldung;
pub mod neuanlage;
pub mod post_acceptance;
pub mod sperrung;
pub mod stornierung;
pub mod wechselprozesse;

pub use abrechnung::{
    ABRECHNUNG_WINDOW_LABEL, AbrechnungCommand, AbrechnungData, AbrechnungEvent,
    AbrechnungProjection, AbrechnungRecord, AbrechnungState, GpkeAbrechnungWorkflow, INVOIC_PIDS,
};
pub use anfrage_bestellung::{
    ANFRAGE_PID as ANFRAGE_BESTELLUNG_PID, ANFRAGE_WINDOW_LABEL, AnfrageBestellungCommand,
    AnfrageBestellungEvent, AnfrageBestellungState, AnfrageData, GpkeAnfrageBestellungWorkflow,
    WORKFLOW_NAME as ANFRAGE_BESTELLUNG_WORKFLOW_NAME,
};
pub use konfiguration::{
    BeauftragungData, GpkeKonfigurationWorkflow, KONFIGURATION_WINDOW_LABEL, KonfigurationCommand,
    KonfigurationEvent, KonfigurationProjection, KonfigurationRecord, KonfigurationState,
    ORDERS_PIDS, ORDRSP_PIDS,
};
pub use lf_abmeldung::{
    GpkeLfAbmeldungWorkflow, LF_ABMELDUNG_APERAK_WINDOW_LABEL, LF_ABMELDUNG_PIDS,
    LfAbmeldungCommand, LfAbmeldungData, LfAbmeldungEvent, LfAbmeldungState,
};
pub use lf_anmeldung::{
    ANFRAGE_PIDS_LF, ANTWORT_PIDS_LF, GpkeLfAnmeldungWorkflow, LfAnmeldungCommand, LfAnmeldungData,
    LfAnmeldungEvent, LfAnmeldungState, NB_RESPONSE_WINDOW_LABEL,
};
pub use neuanlage::{
    GpkeNeuanlageWorkflow, NEUANLAGE_APERAK_WINDOW_LABEL, NEUANLAGE_PIDS, NeuanlageCommand,
    NeuanlageData, NeuanlageEvent, NeuanlageState,
};
pub use sperrung::{
    GpkeSperrungWorkflow, SPERRUNG_PIDS, SPERRUNG_WINDOW_LABEL, SperrungCommand, SperrungData,
    SperrungEvent, SperrungState,
};
pub use stornierung::{
    GpkeStornierungCommand, GpkeStornierungData, GpkeStornierungEvent, GpkeStornierungState,
    GpkeStornierungWorkflow,
    STORNIERUNG_APERAK_WINDOW_LABEL as STORNIERUNG_GPKE_APERAK_WINDOW_LABEL,
    STORNIERUNG_PIDS as STORNIERUNG_GPKE_PIDS,
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
            "gpke-sperrung",
            anfrage_bestellung::WORKFLOW_NAME,
            "gpke-abrechnung",
            "gpke-konfiguration",
            "gpke-neuanlage",
            "gpke-lf-abmeldung",
            stornierung::WORKFLOW_NAME,
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

        // ORDERS PIDs 17115/17116/17117 (Sperrung/Entsperrung) — separate workflow.
        // Per BDEW PID overview: "AWH Sperrprozesse" applies to both Strom and Gas.
        for &pid in SPERRUNG_PIDS {
            router.register(pid, "gpke-sperrung");
        }

        // INVOIC-based: all 6 billing PIDs use `GpkeAbrechnungWorkflow`.
        for &pid in INVOIC_PIDS {
            router.register(pid, "gpke-abrechnung");
        }

        // ORDRSP inbound PIDs for Konfigurationseinrichtung (19001/19002).
        //
        // NB role only: the NB sends ORDERS 17134/17135 outbound (via outbox)
        // to the designated MSB and receives ORDRSP 19001/19002 back.
        // On nMSB instances these same PIDs are WiM Geräteübernahme responses
        // and route to `wim-geraeteubernahme` — controlled via DeploymentRoles.
        if roles.contains(mako_engine::marktrolle::Marktrolle::Nb) {
            for &pid in ORDRSP_PIDS {
                router.register(pid, "gpke-konfiguration");
            }
        }

        // LF-side Anmeldung: inbound NB/LFA response PIDs (55003–55006, 55017, 55018).
        // Registered so the AS4 inbound layer can route them by conversation ID
        // to the correct GpkeLfAnmeldungWorkflow instance (makod acting as LF).
        for &pid in ANTWORT_PIDS_LF {
            router.register(pid, lf_anmeldung::WORKFLOW_NAME);
        }

        // IFTSTA GPKE Vollzugsmeldung PID 21033 only.
        //
        // PID 21033 is the single GPKE-owned IFTSTA PID (Ablehnung GPKE Teil 3).
        // PIDs 21024–21032 belong to WiM Strom / WiM Gas / GeLi Gas and are
        // not registered here. Routed to `gpke-supplier-change` via conversation ID.
        for &pid in wechselprozesse::IFTSTA_PIDS {
            router.register(pid, "gpke-supplier-change");
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
                message_type: "IFTSTA",
                label: "IFTSTA Vollzugsmeldung (GPKE 21033)",
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
            ("INVOIC_PIDS", INVOIC_PIDS),
            ("ORDRSP_PIDS", ORDRSP_PIDS),
            ("ANTWORT_PIDS_LF", ANTWORT_PIDS_LF),
            ("wechselprozesse::IFTSTA_PIDS", wechselprozesse::IFTSTA_PIDS),
            ("NEUANLAGE_PIDS", NEUANLAGE_PIDS),
            ("LF_ABMELDUNG_PIDS", LF_ABMELDUNG_PIDS),
            (
                "stornierung::STORNIERUNG_PIDS",
                stornierung::STORNIERUNG_PIDS,
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
