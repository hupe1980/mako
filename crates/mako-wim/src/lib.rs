//! `mako-wim` — WiM (Wechselprozesse im Messwesen Strom) process engine for
//! German smart-meter market communication (BDEW MaKo).
//!
//! ## Process family
//!
//! WiM governs the switching processes for metering point operators in the
//! German electricity smart-meter rollout, regulated by the MsbG and BDEW
//! WiM process documentation:
//!
//! | Process | PIDs | Message | Module | Status |
//! |---|---|---|---|---|
//! | Gerätewechsel Anmeldung (neuer MSB) | 11001 | UTILMD | `geraetewechsel` | ✅ Implemented |
//! | Gerätewechsel Abmeldung (alter MSB) | 11002 | UTILMD | `geraetewechsel` | ✅ Registered (shared workflow) |
//! | Stammdatenänderung | 11003 | UTILMD | `geraetewechsel` | ✅ Registered (shared workflow) |
//! | Bestellung Geräteübernahmeangebot | 17001–17011 | ORDERS | `geraeteubernahme` | ✅ Implemented |
//! | Stammdaten Anfrage / Übermittlung | 17132 (req), 17102–17133 (resp) | ORDERS | `stammdaten` | ✅ Implemented |
//! | Preisanfrage (REQOTE/QUOTES) | 35001–35005 (REQOTE in), 15001–15005 (QUOTES in) | REQOTE, QUOTES | `preisanfrage` | ✅ Implemented |
//! | Preisliste (PRICAT) | 27001–27003 | PRICAT | `preisliste` | ✅ Implemented |
//! | Stornierung Sperr-/Entsperrauftrag | 39000 | ORDCHG | `stornierung` | ✅ Implemented |
//! | WiM-Rechnung / MSB-Rechnung (INVOIC) | 31003, 31009 | INVOIC | `rechnung` | ✅ Implemented (stub, full settlement pending) |
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
//!               └── DeviceChangeCommand { pid, melo_id, device_id, … }
//!                   └── Process::execute(cmd)  ← pure domain logic here
//! ```
//!
//! ## Key regulatory distinction from GPKE
//!
//! | Aspect | GPKE | WiM |
//! |---|---|---|
//! | APERAK Frist | **24 h** wall-clock | **5 Werktage** |
//! | Frist helper | `fristen::add_hours(24)` | `fristen::add_werktage(5, BdewMaKo)` |
//! | Governing rule | BK6-22-024 | BK6-24-174 |
//!
//! ## Command construction example
//!
//! ```rust,ignore
//! use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
//! use mako_wim::geraetewechsel::{WimDeviceChangeWorkflow, DeviceChangeCommand};
//!
//! let msg    = Platform::with_all_profiles().parse(&raw_bytes)?;
//! let report = msg.validate()?;
//! let AnyMessage::Utilmd(u) = &msg else { anyhow::bail!("not UTILMD") };
//!
//! let cmd = DeviceChangeCommand::ReceiveUtilmd {
//!     pid:               msg.detect_pruefidentifikator()?,
//!     sender:            u.sender().and_then(|n| n.party_id.clone()).unwrap_or_default(),
//!     receiver:          u.receiver().and_then(|n| n.party_id.clone()).unwrap_or_default(),
//!     melo_id:           u.transactions().first()
//!                         .and_then(|t| t.ide.object_id.clone()).unwrap_or_default(),
//!     device_id:         u.transactions().first()
//!                         .and_then(|t| t.device_id().cloned()).unwrap_or_default(),
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

pub mod geraeteubernahme;
pub mod geraetewechsel;
pub mod preisanfrage;
pub mod preisliste;
pub mod rechnung;
pub mod stammdaten;
pub mod steuerungsauftrag;
pub mod stornierung;

pub use geraeteubernahme::{
    ANFRAGE_PIDS, BESTELLUNG_PIDS, GeraeteubernahmeCommand, GeraeteubernahmeData,
    GeraeteubernahmeEvent, GeraeteubernahmeProjection, GeraeteubernahmeRecord,
    GeraeteubernahmeRecordData, GeraeteubernahmeState,
    ORDRSP_DEADLINE_LABEL as GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL,
    STORNIERUNG_PIDS as GERAETEUBERNAHME_STORNIERUNG_PIDS, WimGeraeteubernahmeWorkflow,
};
pub use geraetewechsel::{
    APERAK_WINDOW_LABEL as GERAETEWECHSEL_APERAK_WINDOW_LABEL, DeviceChangeCommand,
    DeviceChangeData, DeviceChangeEvent, DeviceChangeProjection, DeviceChangeRecord,
    DeviceChangeState, WORKFLOW_NAME, WimDeviceChangeWorkflow,
};
pub use preisanfrage::{
    PREISANFRAGE_DEADLINE_LABEL, PreisanfrageCommand, PreisanfrageData, PreisanfrageEvent,
    PreisanfrageState, QUOTES_PIDS, REQOTE_PIDS, WimPreisanfrageWorkflow,
};
pub use preisliste::{
    PRICAT_PIDS, PreislisteCommand, PreislisteData, PreislisteEvent, PreislisteState,
    WimPreislisteWorkflow,
};
pub use rechnung::{
    WIM_INVOIC_PIDS, WIM_RECHNUNG_WINDOW_LABEL, WORKFLOW_NAME as RECHNUNG_WORKFLOW_NAME,
    WimRechnungCommand, WimRechnungEvent, WimRechnungState, WimRechnungWorkflow,
};
pub use stammdaten::{
    ANFORDERUNG_PID, STAMMDATEN_DEADLINE_LABEL, StammdatenCommand, StammdatenData, StammdatenEvent,
    StammdatenProjection, StammdatenRecord, StammdatenRecordData, StammdatenState,
    UEBERMITTLUNG_PIDS, WimStammdatenWorkflow,
};
pub use steuerungsauftrag::{
    STEUERUNGSAUFTRAG_DEADLINE_LABEL, SteuerungsCommandType, SteuerungsauftragCommand,
    SteuerungsauftragData, SteuerungsauftragEvent, SteuerungsauftragState,
    WORKFLOW_NAME as STEUERUNGSAUFTRAG_WORKFLOW_NAME, WimSteuerungsauftragWorkflow,
};
pub use stornierung::{
    ABLEHNUNG_PID, BESTAETIGUNG_PID, STORNIERUNG_DEADLINE_LABEL, STORNIERUNG_PID,
    StornierungCommand, StornierungData, StornierungEvent, StornierungProjection,
    StornierungRecord, StornierungRecordData, StornierungState, WimStornierungWorkflow,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the WiM process family.
///
/// Registers all WiM `Prüfidentifikator` values into the
/// [`mako_engine::pid_router::PidRouter`] at engine startup:
///
/// | PID(s) | Workflow key | Module | Role |
/// |---|---|---|---|
/// | 11001 | `wim-device-change` | Gerätewechsel Anmeldung (nMSB → NB) | any |
/// | 11002 | `wim-device-change` | Gerätewechsel Abmeldung / Kündigung (NB → aMSB) | any |
/// | 11003 | `wim-device-change` | Stammdatenänderung (NB ↔ MSB) | any |
/// | 17001–17011 | `wim-geraeteubernahme` | Geräteübernahme ORDERS (nMSB → NB) | any |
/// | 17132 | `wim-stammdaten` | Stammdaten Anforderung Strom (NB → MSB), MSB role | any |
/// | 17102–17133 | `wim-stammdaten` | Stammdatenübermittlung responses (MSB → NB), NB role | **Nb only** |
/// | 39000 | `wim-stornierung` | Stornierung (ORDCHG) | any |
/// | 19001, 19002 | `wim-geraeteubernahme` | ORDRSP Bestellbestätigung/Ablehnung from NB | **nMSB only** |
/// | 19015, 19016 | `wim-geraeteubernahme` | ORDRSP Gerätewechselabsicht Bestätigung/Ablehnung | **nMSB only** |
///
/// ## Role-conditional PIDs (ORDRSP 19001/19002/19015/19016)
///
/// When this `makod` instance serves the **nMSB** role it sends outbound ORDERS
/// to the NB and receives inbound ORDRSP responses. These PIDs are only registered
/// when [`DeploymentRoles`] contains [`Marktrolle::Nmsb`]:
///
/// | ORDRSP PID | AHB process name | Responds to ORDERS |
/// |---|---|---|
/// | 19001 | Bestellbestätigung | 17001 (Bestellung Geräteübernahmeangebot) |
/// | 19002 | Ablehnung der Bestellung | 17001 (Bestellung Geräteübernahmeangebot) |
/// | 19015 | Bestätigung Gerätewechselabsicht | 17009 (Ankündigung Gerätewechselabsicht) |
/// | 19016 | Ablehnung Gerätewechselabsicht | 17009 (Ankündigung Gerätewechselabsicht) |
///
/// **Conflict note:** PIDs 19001/19002 are also used by GPKE Konfiguration when
/// the instance is NB (receiving ORDRSP from the MSB after sending ORDERS 17134/17135).
/// Use [`DeploymentRoles::nmsb()`] for nMSB-only deployments and [`DeploymentRoles::nb()`]
/// for NB-only deployments to prevent both modules from registering these PIDs simultaneously.
///
/// [`DeploymentRoles`]: mako_engine::marktrolle::DeploymentRoles
/// [`Marktrolle::Nmsb`]: mako_engine::marktrolle::Marktrolle::Nmsb
/// [`DeploymentRoles::nmsb()`]: mako_engine::marktrolle::DeploymentRoles::nmsb
/// [`DeploymentRoles::nb()`]: mako_engine::marktrolle::DeploymentRoles::nb
pub struct WimModule;

impl mako_engine::builder::EngineModule for WimModule {
    fn name(&self) -> &'static str {
        "wim"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            "wim-device-change",
            "wim-geraeteubernahme",
            "wim-stammdaten",
            "wim-stornierung",
            "wim-steuerungsauftrag",
            "wim-preisanfrage",
            "wim-preisliste",
            "wim-rechnung",
        ]
    }

    fn register_pids_with_roles(
        &self,
        router: &mut mako_engine::pid_router::PidRouter,
        roles: &mako_engine::marktrolle::DeploymentRoles,
    ) {
        // UTILMD WiM device-change family (PIDs 11001–11003).
        //
        // 11001 — Anmeldung Messstellenbetrieb (nMSB → NB): new MSB initiates change.
        // 11002 — Abmeldung / Kündigung Messstellenbetrieb (NB → aMSB): NB terminates
        //         the old MSB relationship.  makod in MSB role receives this inbound.
        // 11003 — Stammdatenänderung (NB ↔ MSB): master-data update notification.
        //
        // All three share WimDeviceChangeWorkflow; the PID is carried in the
        // DeviceChangeData and available for business-logic branching.
        for pid in [11_001_u32, 11_002, 11_003] {
            router.register(pid, "wim-device-change");
        }

        // ORDERS 17001–17011 — Geräteübernahme (Anfrage, Bestellung, Stornierung).
        for &pid in geraeteubernahme::ANFRAGE_PIDS
            .iter()
            .chain(geraeteubernahme::BESTELLUNG_PIDS)
            .chain(geraeteubernahme::STORNIERUNG_PIDS)
        {
            router.register(pid, "wim-geraeteubernahme");
        }

        // nMSB role: inbound ORDRSP responses from NB to nMSB ORDERS.
        //
        // ONLY registered when Nmsb is explicitly declared in DeploymentRoles
        // (not triggered by DeploymentRoles::all(), which is the backward-compatible
        // default where GPKE owns 19001/19002 unchanged).
        //
        // When makod acts as nMSB it sends ORDERS 17001/17009 outbound (via outbox)
        // and receives inbound ORDRSP responses back from the NB. These PIDs are
        // only registered when the nMSB role is active, preventing routing conflicts
        // with GPKE Konfiguration on NB instances (which also uses 19001/19002).
        //
        // PID 19001/19002 (AHB fv20251001): ORDRSP Bestellbestätigung/Ablehnung
        //   → response to ORDERS 17001 (Bestellung Geräteübernahmeangebot)
        // PID 19015/19016 (AHB fv20251001): ORDRSP Gerätewechselabsicht Bestätigung/Ablehnung
        //   → response to ORDERS 17009 (Ankündigung Gerätewechselabsicht, §14a EnWG)
        if !roles.is_all() && roles.contains(mako_engine::marktrolle::Marktrolle::Nmsb) {
            for pid in [19_001_u32, 19_002, 19_015, 19_016] {
                router.register(pid, "wim-geraeteubernahme");
            }
        }

        // ORDERS 17132 — Stammdaten Anforderung Strom (NB → MSB).
        //
        // When makod acts as MSB it receives this inbound (NB sends the request).
        // When makod acts as NB it sends this outbound via the outbox; the MSB responds
        // with one of the UEBERMITTLUNG_PIDS (17102–17133) which the NB then receives
        // inbound — those are registered below under the Nb role guard.
        //
        // Note: 17101 (“Anfrage zur Übermittlung von Stammdaten Gas”) is the GAS counterpart
        // and belongs to mako-wim-gas, not here.
        router.register(stammdaten::ANFORDERUNG_PID, "wim-stammdaten");

        // Nb role: inbound Stammdatenübermittlung responses (MSB → NB).
        //
        // When makod acts as NB it sends ORDERS 17132 outbound and receives the MSB's
        // response (one of PIDs 17102–17133) inbound. These are registered only for
        // explicit Nb deployments to avoid routing conflicts on MSB-only instances.
        //
        // PIDs 17134/17135 are excluded: they are GPKE Konfiguration PIDs owned by
        // mako-gpke and must not be claimed by the WiM Stammdaten module.
        if !roles.is_all() && roles.contains(mako_engine::marktrolle::Marktrolle::Nb) {
            for pid in stammdaten::UEBERMITTLUNG_PIDS {
                router.register(pid, "wim-stammdaten");
            }
        }

        // REQOTE 35001–35005 (Preisanfrage) and QUOTES 15001–15005 (Angebot).
        for &pid in preisanfrage::REQOTE_PIDS
            .iter()
            .chain(preisanfrage::QUOTES_PIDS)
        {
            router.register(pid, "wim-preisanfrage");
        }

        // PRICAT 27001–27003 (Preisliste).
        for &pid in preisliste::PRICAT_PIDS {
            router.register(pid, "wim-preisliste");
        }

        // ORDCHG 39000 — Stornierung Sperr-/Entsperrauftrag.
        // Response PIDs (39001 Bestätigung, 39002 Ablehnung) are outbox entries.
        router.register(stornierung::STORNIERUNG_PID, "wim-stornierung");

        // INVOIC 31003 (WiM-Rechnung) and 31009 (MSB-Rechnung).
        //
        // These PIDs are explicitly excluded from mako-gpke's INVOIC_PIDS array.
        // Without registration here, all inbound WiM-domain INVOIC messages would
        // be silently dead-lettered and no CONTRL acknowledgement would be sent,
        // violating the AS4 acknowledgement obligation (BDEW AS4-Profile §5).
        //
        // The WimRechnungWorkflow provides a complete state machine;
        // full settlement/dispute business logic is marked for follow-up in TODO.md.
        for &pid in rechnung::WIM_INVOIC_PIDS {
            router.register(pid, "wim-rechnung");
        }

        // IFTSTA WiM PIDs 21009–21018 (MSB-Wechsel status messages).
        //
        // These are Vollzugsmeldungen and process-status notifications that
        // accompany the WiM UTILMD device-change process. All are routed to
        // `wim-device-change` for correlation via conversation ID (CI tag).
        for &pid in geraetewechsel::IFTSTA_PIDS {
            router.register(pid, "wim-device-change");
        }

        // `wim-steuerungsauftrag` is intentionally NOT registered here.
        //
        // The Steuerungsauftrag workflow is driven exclusively by the BDEW
        // API-Webdienste Strom `controlMeasuresV1` REST channel (BDEW
        // API-Guideline 1.0a). There is no EDIFACT message type for this
        // workflow; it receives no inbound PID dispatch from the `PidRouter`.
        // The REST adapter (`energy-api`) creates process commands directly.
        // Do not add EDIFACT PID registrations for this workflow.
    }

    fn profile_requirements(&self) -> &'static [mako_engine::profile::ProfileRequirement] {
        use mako_engine::profile::ProfileRequirement;
        &[
            ProfileRequirement {
                message_type: "UTILMD",
                label: "UTILMD Strom (WiM Gerätewechsel)",
            },
            ProfileRequirement {
                message_type: "APERAK",
                label: "APERAK (WiM)",
            },
            ProfileRequirement {
                message_type: "ORDERS",
                label: "ORDERS (WiM Geräteübernahme/Stammdaten)",
            },
            ProfileRequirement {
                message_type: "ORDRSP",
                label: "ORDRSP (WiM Geräteübernahme Bestätigung 19001/19002/19015/19016)",
            },
            ProfileRequirement {
                message_type: "ORDCHG",
                label: "ORDCHG (WiM Stornierung)",
            },
            ProfileRequirement {
                message_type: "IFTSTA",
                label: "IFTSTA Statusmeldung (WiM 21009–21018)",
            },
            ProfileRequirement {
                message_type: "INVOIC",
                label: "INVOIC WiM-Rechnung/MSB-Rechnung (31003, 31009)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        // Verify that all static PID slices referenced by register_pids_with_roles()
        // are non-empty. An accidental empty const (e.g. from a codegen regression)
        // would silently mean the module registers no routes for an entire workflow
        // family, discoverable only on first inbound message.
        let named: &[(&str, &[u32])] = &[
            (
                "geraeteubernahme::ANFRAGE_PIDS",
                geraeteubernahme::ANFRAGE_PIDS,
            ),
            (
                "geraeteubernahme::BESTELLUNG_PIDS",
                geraeteubernahme::BESTELLUNG_PIDS,
            ),
            (
                "geraeteubernahme::STORNIERUNG_PIDS",
                geraeteubernahme::STORNIERUNG_PIDS,
            ),
            ("geraetewechsel::IFTSTA_PIDS", geraetewechsel::IFTSTA_PIDS),
        ];
        for (name, pids) in named {
            if pids.is_empty() {
                return Err(format!(
                    "wim: PID slice '{name}' is empty — \
                     at least one PID must be registered for each workflow group",
                ));
            }
        }
        // UEBERMITTLUNG_PIDS is a RangeInclusive<u32>, not a slice; verify it is non-empty.
        if stammdaten::UEBERMITTLUNG_PIDS.is_empty() {
            return Err("wim: stammdaten::UEBERMITTLUNG_PIDS is empty — \
                 at least one PID must be registered for the Stammdaten workflow"
                .to_owned());
        }
        Ok(())
    }
}
