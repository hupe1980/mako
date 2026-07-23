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
//! | Anmeldung MSB (MSBN → NB) | 55042 → 55043/55044 | UTILMD | `geraetewechsel` | ✅ Implemented |
//! | Kündigung MSB (MSBN → **MSBA**) | 55039 → 55040/55041 | UTILMD | `geraetewechsel` | ✅ Implemented |
//! | Ende MSB / Abmeldung (**MSBA → NB**) | 55051 → 55052/55053 | UTILMD | `geraetewechsel` | ✅ Implemented |
//! | Verpflichtungsanfrage (NB → **gMSB**) | 55168 → 55169/55170 | UTILMD | `geraetewechsel` | ✅ Implemented |
//! | Bestellung Geräteübernahmeangebot | 17001–17011 | ORDERS | `geraeteubernahme` | ✅ Implemented |
//! | Stammdaten Anfrage / Übermittlung | 17132 (req), 17102–17133 (resp) | ORDERS | `stammdaten` | ✅ Implemented |
//! | Preisanfrage (REQOTE/QUOTES) | 35001–35005 (REQOTE in), 15001–15005 (QUOTES in) | REQOTE, QUOTES | `preisanfrage` | ✅ Implemented |
//! | Preisliste (PRICAT) | 27001–27003 | PRICAT | `preisliste` | ✅ Implemented |
//! | ESA Wertebestellung (Anfrage/Angebot/Bestellung/Storno) | 35002, 15003, 17007/17008, 39002, 19011–19014 | REQOTE/QUOTES/ORDERS/ORDCHG/ORDRSP | `wertebestellung`, `esa_wertebestellung` | ✅ Implemented |
//! | WiM-Rechnung / MSB-Rechnung (INVOIC) | 31003, 31009 | INVOIC | `rechnung` | ✅ Implemented (auto-REMADV pending in deadline_dispatch) |
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
//! | APERAK Frist (Strom UTILMD) | **45 min** | **45 min** |
//! | Business Antwortfrist | 24 h wall-clock | **1 / 3 / 5 / 7 Werktage, per process** |
//! | Frist helper | `fristen::add_hours(24)` | `geraetewechsel::antwort_frist_werktage(pid)` |
//! | Governing rule | BK6-22-024 | BK6-24-174 |
//!
//! Two distinct clocks, easily conflated: the **APERAK** window is the
//! processability acknowledgement (45 min for UTILMD/ORDERS in Strom, APERAK AHB
//! §2.4.1); the **Antwortfrist** is the counterparty's business answer and varies
//! per process — 3 WT Kündigung, 5 WT Anmeldung, 7 WT Abmeldung, 1 WT
//! Verpflichtungsanfrage.
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

pub mod esa_wertebestellung;
pub mod geraeteubernahme;
pub mod geraetewechsel;
pub mod insrpt;
pub mod preisanfrage;
pub mod preisliste;
pub mod rechnung;
pub mod stammdaten;
pub mod steuerungsauftrag;
pub mod technik_aenderung;
pub mod wertebestellung;

pub use geraeteubernahme::{
    ANFRAGE_PIDS, BESTELLUNG_PIDS, GeraeteubernahmeCommand, GeraeteubernahmeData,
    GeraeteubernahmeEvent, GeraeteubernahmeProjection, GeraeteubernahmeRecord,
    GeraeteubernahmeRecordData, GeraeteubernahmeState,
    ORDRSP_DEADLINE_LABEL as GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL,
    STORNIERUNG_PIDS as GERAETEUBERNAHME_STORNIERUNG_PIDS,
    WORKFLOW_NAME as GERAETEUBERNAHME_WORKFLOW_NAME, WimGeraeteubernahmeWorkflow,
};
pub use geraetewechsel::{
    APERAK_WINDOW_LABEL as GERAETEWECHSEL_APERAK_WINDOW_LABEL, AUFTRAG_ANTWORT_WINDOW_LABEL,
    DEVICE_CHANGE_ANTWORT_PIDS, DEVICE_CHANGE_PIDS, DeviceChangeCommand, DeviceChangeData,
    DeviceChangeEvent, DeviceChangeProjection, DeviceChangeRecord, DeviceChangeState,
    WORKFLOW_NAME, WimDeviceChangeWorkflow, antwort_frist_werktage, antwort_pid_meaning,
};
pub use insrpt::{
    ANTWORT_WINDOW_LABEL as INSRPT_ANTWORT_WINDOW_LABEL, INSRPT_ANFRAGE_PIDS, INSRPT_ANTWORT_PIDS,
    StorungsmeldungCommand, StorungsmeldungData, StorungsmeldungEvent, StorungsmeldungState,
    WORKFLOW_NAME as INSRPT_WORKFLOW_NAME, WimInsrptWorkflow,
};
pub use preisanfrage::{
    PREISANFRAGE_DEADLINE_LABEL, PreisanfrageCommand, PreisanfrageData, PreisanfrageEvent,
    PreisanfrageState, QUOTES_PIDS, REQOTE_PIDS, WORKFLOW_NAME as PREISANFRAGE_WORKFLOW_NAME,
    WimPreisanfrageWorkflow,
};
pub use preisliste::{
    PRICAT_PIDS, PreislisteCommand, PreislisteData, PreislisteEvent, PreislisteState,
    WORKFLOW_NAME as PREISLISTE_WORKFLOW_NAME, WimPreislisteWorkflow,
};
pub use rechnung::{
    WIM_COMDIS_ABLEHNUNG_PID, WIM_INVOIC_PIDS, WIM_RECHNUNG_WINDOW_LABEL, WIM_REMADV_PIDS,
    WORKFLOW_NAME as RECHNUNG_WORKFLOW_NAME, WimRechnungCommand, WimRechnungEvent,
    WimRechnungState, WimRechnungWorkflow,
};
pub use stammdaten::{
    ANFORDERUNG_PID, STAMMDATEN_DEADLINE_LABEL, StammdatenCommand, StammdatenData, StammdatenEvent,
    StammdatenProjection, StammdatenRecord, StammdatenRecordData, StammdatenState,
    UEBERMITTLUNG_PIDS, WORKFLOW_NAME as STAMMDATEN_WORKFLOW_NAME, WimStammdatenWorkflow,
};
pub use steuerungsauftrag::{
    STEUERUNGSAUFTRAG_DEADLINE_LABEL, SteuerungsCommandType, SteuerungsauftragCommand,
    SteuerungsauftragData, SteuerungsauftragEvent, SteuerungsauftragState,
    WORKFLOW_NAME as STEUERUNGSAUFTRAG_WORKFLOW_NAME, WimSteuerungsauftragWorkflow,
};
pub use technik_aenderung::{
    AuftragData as TechnikAenderungAuftragData, ORDERS_PIDS as TECHNIK_AENDERUNG_ORDERS_PIDS,
    ORDRSP_PIDS as TECHNIK_AENDERUNG_ORDRSP_PIDS, TechnikAenderungCommand, TechnikAenderungEvent,
    TechnikAenderungState, WORKFLOW_NAME as TECHNIK_AENDERUNG_WORKFLOW_NAME,
    WimTechnikAenderungWorkflow,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the WiM process family.
///
/// Registers all WiM `Prüfidentifikator` values into the
/// [`mako_engine::pid_router::PidRouter`] at engine startup:
///
/// | PID(s) | Workflow key | Module | Role |
/// |---|---|---|---|
/// | 55039 | `wim-device-change` | Kündigung MSB (MSBN → MSBA) | any |
/// | 55042 | `wim-device-change` | Anmeldung MSB (MSBN → NB) | any |
/// | 55051 | `wim-device-change` | Ende MSB / Abmeldung (MSBA → NB) | any |
/// | 55168 | `wim-device-change` | Verpflichtungsanfrage / Aufforderung (NB → gMSB) | any |
/// | 17001–17011 | `wim-geraeteubernahme` | Geräteübernahme ORDERS (nMSB → NB) | any |
/// | 17132 | `wim-stammdaten` | Stammdaten Anforderung Strom (NB → MSB), MSB role | any |
/// | 17102–17133 | `wim-stammdaten` | Stammdatenübermittlung responses (MSB → NB), NB role | **Nb only** |
/// | 39002 | `wim-wertebestellung` | ESA Stornierung der Bestellung (ORDCHG) | **Msb only** |
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
            wertebestellung::WORKFLOW_NAME,
            esa_wertebestellung::WORKFLOW_NAME,
            "wim-steuerungsauftrag",
            "wim-preisanfrage",
            "wim-preisliste",
            "wim-rechnung",
            insrpt::WORKFLOW_NAME,
            technik_aenderung::WORKFLOW_NAME,
        ]
    }

    fn register_pids_with_roles(
        &self,
        router: &mut mako_engine::pid_router::PidRouter,
        roles: &mako_engine::marktrolle::DeploymentRoles,
    ) {
        // UTILMD WiM MSB-Wechsel family (PIDs 55039, 55042, 55051, 55168).
        //
        // 55039 — Kündigung MSB (MSBN → MSBA): contract layer between the two MSB;
        //         non-constitutive per BK6-24-174 Kap. 2.1.3 — the NB is not a party.
        // 55042 — Anmeldung MSB (MSBN → NB): new MSB initiates change.
        // 55051 — Ende MSB / Abmeldung (MSBA → NB): NB terminates MSB relationship.
        // 55168 — Verpflichtungsanfrage / Aufforderung (NB → gMSB).
        //
        // All four share WimDeviceChangeWorkflow; the PID is carried in the
        // DeviceChangeData and available for business-logic branching.
        for pid in [55_039_u32, 55_042, 55_051, 55_168] {
            router.register(pid, "wim-device-change");
        }

        // Antwort PIDs (Bestätigung / Ablehnung) for an order **we** sent.
        // 55040/55041 ← 55039 · 55043/55044 ← 55042
        // 55052/55053 ← 55051 · 55169/55170 ← 55168
        //
        // These resume the existing process by MeLo rather than spawning: the
        // ingest dispatcher uses `resume_by_malo`, so an answer with no open
        // order is skipped rather than creating an orphan stream.
        for &(antwort_pid, _, _) in geraetewechsel::DEVICE_CHANGE_ANTWORT_PIDS {
            router.register(antwort_pid, "wim-device-change");
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
                // register_with_module enforces the documented guarantee: if both NB
                // (GPKE Konfiguration) and nMSB (WiM Geräteübernahme) roles are active
                // simultaneously, build() panics instead of silently overwriting the
                // conflicting registration.
                router.register_with_module(pid, "wim-geraeteubernahme", "wim");
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
        //
        // PIDs 17115–17117 are excluded: GPKE/AWH Sperrprozesse ORDERS PIDs
        // (Sperrauftrag / Aufhebung Sperrauftrag / Sperrung nicht möglich) owned by
        // mako-gpke as "gpke-sperrung".
        //
        // The following GPKE-owned PIDs fall inside the 17102–17133 range and must
        // not be claimed by wim-stammdaten to avoid ownership conflicts on combined NB
        // deployments (both GpkeModule and WimModule active):
        //
        //   17102 (gpke-datenabruf, Datenabruf Anfrage LF→NB)
        //   17110 (gpke-allokationsliste, Anforderung Allokationsliste)
        //   17113 (gpke-datenabruf, Weitere Datenabruf Anfrage)
        //   17114 (gpke-allokationsliste, Abmeldung Allokationsliste)
        //   17120 (gpke-konfiguration-aenderung, Bestellung Konfiguration LF→NB)
        //   17121 (gpke-konfiguration-aenderung, Bestellung Konfiguration LF→NB)
        //   17122 (gpke-konfiguration-aenderung, Bestellung Konfigurationsänderung)
        //   17123 (gpke-konfiguration-aenderung, Stornierung Konfigurationsbestellung)
        //   17128 (gpke-konfiguration-aenderung, Bestellung Konfiguration LF→MSB)
        //   17129 (gpke-konfiguration-aenderung, Bestellung Konfiguration LF→MSB)
        //   17130 (gpke-konfiguration-aenderung, Bestellung Konfigurationsänderung LF→MSB)
        //   17131 (gpke-konfiguration-aenderung, Stornierung Konfigurationsbestellung LF→MSB)
        //   17133 (gpke-konfiguration-aenderung, Bestellung Konfiguration Reklamation)
        //
        // Source: docs/pid-reference.md (generated from BDEW xlsx PID 3.3 + PID 4.0).
        #[rustfmt::skip]
        const GPKE_OWNED_IN_RANGE: &[u32] = &[
            17102, 17113,                        // gpke-datenabruf
            17110, 17114,                        // gpke-allokationsliste
            17120, 17121, 17122, 17123,          // gpke-konfiguration-aenderung (LF→NB)
            17128, 17129, 17130, 17131, 17133,   // gpke-konfiguration-aenderung (LF→MSB)
            // 17115, 17116, 17117 already excluded by the matches!() guard below
        ];
        if !roles.is_all() && roles.contains(mako_engine::marktrolle::Marktrolle::Nb) {
            for pid in stammdaten::UEBERMITTLUNG_PIDS {
                if matches!(pid, 17115..=17117) {
                    // Sperrung PIDs — owned by mako-gpke (gpke-sperrung).
                    continue;
                }
                if GPKE_OWNED_IN_RANGE.contains(&pid) {
                    // GPKE-owned PIDs — must not be claimed by wim-stammdaten.
                    continue;
                }
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

        // ── ESA Wertebestellung (WiM Teil 2 Kap. 4) ───────────────────────
        //
        // The two sides register disjoint PIDs, so an integrated deployment can
        // hold both roles without a routing conflict.
        //
        // MSB side: inbound ORDERS 17007 Bestellung (UC 4.1 Nr. 3), 17008
        // Abbestellung (UC 4.3 Nr. 1) and ORDCHG 39002 Stornierung (UC 4.1 Nr. 5)
        // — all resume the *same* subscription process. §34 Abs. 2 S. 2 Nr. 10
        // MsbG makes serving an ESA a mandatory Zusatzleistung, so an MSB must be
        // able to process the order that authorises delivery, the one that stops
        // it, and the cancellation of a not-yet-delivered Bestellung. The answers
        // (ORDRSP 19011/19012/19013/19014) are outbox entries. The Stornierung
        // carries no LOC — it is correlated by the Bestellung's Belegnummer
        // echoed in RFF+ON (see the makod ingest dispatcher).
        if roles.contains(mako_engine::marktrolle::Marktrolle::Msb) {
            router.register(
                wertebestellung::BESTELLUNG_PID,
                wertebestellung::WORKFLOW_NAME,
            );
            router.register(
                wertebestellung::ABBESTELLUNG_PID,
                wertebestellung::WORKFLOW_NAME,
            );
            router.register(
                wertebestellung::STORNIERUNG_PID,
                wertebestellung::WORKFLOW_NAME,
            );
        }

        // ESA side: this deployment *is* the ESA and originates the order
        // handshake (REQOTE 35002 / ORDERS 17007 / ORDCHG 39002 / ORDERS 17008).
        // The MSB's answers (QUOTES 15003, ORDRSP 19011-19014) are inbound here
        // and resume the esa-wertebestellung process. Registered only for a
        // deployment that *is* an ESA — an ESA has no Zuordnung to a
        // Marktlokation, so nothing else may claim these. The set is disjoint
        // from the MSB inbound PIDs, so an integrated deployment holds both.
        if roles.contains(mako_engine::marktrolle::Marktrolle::Esa) {
            for &pid in esa_wertebestellung::ESA_INBOUND_PIDS {
                router.register(pid, esa_wertebestellung::WORKFLOW_NAME);
            }
        }

        // INVOIC 31003 (WiM-Rechnung) and 31009 (MSB-Rechnung).
        //
        // These PIDs are explicitly excluded from mako-gpke's INVOIC_PIDS array.
        // Without registration here, all inbound WiM-domain INVOIC messages would
        // be silently dead-lettered and no CONTRL acknowledgement would be sent,
        // violating the AS4 acknowledgement obligation (BDEW AS4-Profile §5).
        //
        // The WimRechnungWorkflow provides a complete state machine with Settle/Dispute
        // commands. Automatic outbound REMADV generation (auto-settlement deadline) is
        // tracked in TODO.md §WiM-Rechnung.
        for &pid in rechnung::WIM_INVOIC_PIDS {
            router.register(pid, "wim-rechnung");
        }

        // REMADV 33001–33002 — inbound payment advice for WiM billing (invoicer role).
        //
        // After the NB sends INVOIC 31009 (MSB-Rechnung), the payer (MSB) sends
        // back a REMADV (33001 = Bestätigung, 33002 = Ablehnung). Without this
        // registration, all REMADV messages for WiM billing are silently dropped.
        //
        // GPKE billing also registers 33003/33004 (Mehr-/Mindermenge REMADV);
        // WiM Strom only needs 33001/33002 — the others belong to GPKE Teil 2/3.
        // Both registrations coexist: the makod router checks the workflow context
        // (conversation ID) when routing to the correct process stream instance.
        //
        // Source: REMADV AHB 1.0, WiM Strom Teil 1, BK6-24-174.
        for &pid in rechnung::WIM_REMADV_PIDS {
            router.register(pid, "wim-rechnung");
        }

        // COMDIS 29001 — inbound Ablehnung REMADV (invoicer rejects payer's REMADV).
        //
        // Shared PID with GPKE billing. The router dispatches to the correct
        // workflow instance via conversation ID correlation.
        //
        // Source: COMDIS AHB 1.0, WiM Strom Teil 1, BK6-24-174.
        router.register(rechnung::WIM_COMDIS_ABLEHNUNG_PID, "wim-rechnung");

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

        // INSRPT Störungsmeldungen (WiM Strom Teil 2).
        //
        // 23001: Störungsmeldung (LF → MSB) — APERAK Frist 5 Werktage (BK6-24-174).
        // 23003–23012: Antwort/Ergebnisbericht/Informationsmeldung (MSB → LF).
        //
        // PIDs 23001/23003/23004/23008 are shared with WiM Gas (10 WT).  In a combined
        // Strom+Gas deployment both WimModule and WimGasModule register these PIDs — each
        // with their respective Sparte so that `route_with_sparte` can select the correct
        // workflow at ingest time:
        //
        //   route_with_sparte(23001, Sparte::Strom) → "wim-insrpt"        (5 WT)
        //   route_with_sparte(23001, Sparte::Gas)   → "wim-gas-insrpt"    (10 WT)
        //
        // The unambiguous `register` entry (Strom default) is the fallback for callers
        // that do not supply a Sparte (e.g. logging in the REST ingest endpoint).
        for &pid in insrpt::INSRPT_ANFRAGE_PIDS {
            router.register(pid, insrpt::WORKFLOW_NAME);
            router.register_with_sparte(
                pid,
                mako_engine::types::Sparte::Strom,
                insrpt::WORKFLOW_NAME,
            );
        }
        for &pid in insrpt::INSRPT_ANTWORT_PIDS {
            router.register(pid, insrpt::WORKFLOW_NAME);
            router.register_with_sparte(
                pid,
                mako_engine::types::Sparte::Strom,
                insrpt::WORKFLOW_NAME,
            );
        }

        // WiM Technikänderung — device/config change requests (ORDERS/ORDRSP).
        //
        // Covers LF→MSB (17003), ESA orders (17007/17008), MSB→MSB (17118).
        // ORDRSP: Bestätigung (19003/19005/19011) and Ablehnung (19004/19006/19007/19012).
        for &pid in technik_aenderung::ORDERS_PIDS {
            router.register(pid, technik_aenderung::WORKFLOW_NAME);
        }
        for &pid in technik_aenderung::ORDRSP_PIDS {
            router.register(pid, technik_aenderung::WORKFLOW_NAME);
        }
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
                label: "IFTSTA Statusmeldung (WiM 21007, 21009–21015, 21018, 21029–21032)",
            },
            ProfileRequirement {
                message_type: "INVOIC",
                label: "INVOIC WiM-Rechnung/MSB-Rechnung (31003, 31009)",
            },
            ProfileRequirement {
                message_type: "REMADV",
                label: "REMADV Zahlungsavis (WiM 33001/33002)",
            },
            ProfileRequirement {
                message_type: "COMDIS",
                label: "COMDIS Ablehnung REMADV (WiM 29001)",
            },
            ProfileRequirement {
                message_type: "INSRPT",
                label: "INSRPT Störungsmeldung (WiM Strom/Gas, 23001–23012)",
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
                "geraeteubernahme::BESTELLUNG_PIDS",
                geraeteubernahme::BESTELLUNG_PIDS,
            ),
            (
                "geraeteubernahme::STORNIERUNG_PIDS",
                geraeteubernahme::STORNIERUNG_PIDS,
            ),
            ("geraetewechsel::IFTSTA_PIDS", geraetewechsel::IFTSTA_PIDS),
            ("rechnung::WIM_INVOIC_PIDS", rechnung::WIM_INVOIC_PIDS),
            ("rechnung::WIM_REMADV_PIDS", rechnung::WIM_REMADV_PIDS),
            ("insrpt::INSRPT_ANFRAGE_PIDS", insrpt::INSRPT_ANFRAGE_PIDS),
            ("insrpt::INSRPT_ANTWORT_PIDS", insrpt::INSRPT_ANTWORT_PIDS),
            (
                "technik_aenderung::ORDERS_PIDS",
                technik_aenderung::ORDERS_PIDS,
            ),
            (
                "technik_aenderung::ORDRSP_PIDS",
                technik_aenderung::ORDRSP_PIDS,
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::{
        builder::EngineModule,
        marktrolle::{DeploymentRoles, Marktrolle},
        pid_router::PidRouter,
    };

    /// Regression test for the NB-role PID conflict between WiM Stammdaten
    /// UEBERMITTLUNG_PIDS (17102..=17133) and GPKE-owned PIDs in that range.
    ///
    /// Before the fix, `!roles.is_all() && roles.contains(Nb)` caused WiM to
    /// register GPKE PIDs → "wim-stammdaten", overwriting GPKE's entries and
    /// silently misrouting messages.
    #[test]
    fn nb_role_sperrung_not_overwritten_by_stammdaten_range() {
        let nb = DeploymentRoles::from_roles([Marktrolle::Nb]);
        let mut router = PidRouter::new();
        // Simulate GPKE registration first (as it happens in makod startup order).
        router.register(17115, "gpke-sperrung");
        router.register(17116, "gpke-sperrung");
        router.register(17117, "gpke-sperrung");
        // GPKE-owned PIDs in the 17102..=17133 range
        router.register(17102, "gpke-datenabruf");
        router.register(17113, "gpke-datenabruf");
        router.register(17110, "gpke-allokationsliste");
        router.register(17114, "gpke-allokationsliste");
        router.register(17120, "gpke-konfiguration-aenderung");
        router.register(17121, "gpke-konfiguration-aenderung");
        router.register(17122, "gpke-konfiguration-aenderung");
        router.register(17123, "gpke-konfiguration-aenderung");
        router.register(17128, "gpke-konfiguration-aenderung");
        router.register(17129, "gpke-konfiguration-aenderung");
        router.register(17130, "gpke-konfiguration-aenderung");
        router.register(17131, "gpke-konfiguration-aenderung");
        router.register(17133, "gpke-konfiguration-aenderung");

        // WiM registration must NOT overwrite GPKE entries.
        WimModule.register_pids_with_roles(&mut router, &nb);

        // Sperrung PIDs must still route to gpke-sperrung, not wim-stammdaten.
        assert_eq!(
            router.route(17115),
            Some("gpke-sperrung"),
            "17115 must route to gpke-sperrung"
        );
        assert_eq!(
            router.route(17116),
            Some("gpke-sperrung"),
            "17116 must route to gpke-sperrung"
        );
        assert_eq!(
            router.route(17117),
            Some("gpke-sperrung"),
            "17117 must route to gpke-sperrung"
        );

        // GPKE-owned PIDs in range must not be overwritten by wim-stammdaten.
        assert_eq!(
            router.route(17102),
            Some("gpke-datenabruf"),
            "17102 must route to gpke-datenabruf"
        );
        assert_eq!(
            router.route(17113),
            Some("gpke-datenabruf"),
            "17113 must route to gpke-datenabruf"
        );
        assert_eq!(
            router.route(17110),
            Some("gpke-allokationsliste"),
            "17110 must route to gpke-allokationsliste"
        );
        assert_eq!(
            router.route(17114),
            Some("gpke-allokationsliste"),
            "17114 must route to gpke-allokationsliste"
        );
        assert_eq!(
            router.route(17120),
            Some("gpke-konfiguration-aenderung"),
            "17120 must route to gpke-konfiguration-aenderung"
        );
        assert_eq!(
            router.route(17122),
            Some("gpke-konfiguration-aenderung"),
            "17122 must route to gpke-konfiguration-aenderung"
        );
        assert_eq!(
            router.route(17128),
            Some("gpke-konfiguration-aenderung"),
            "17128 must route to gpke-konfiguration-aenderung"
        );
        assert_eq!(
            router.route(17133),
            Some("gpke-konfiguration-aenderung"),
            "17133 must route to gpke-konfiguration-aenderung"
        );

        // True WiM Stammdaten PIDs in the range must still resolve to wim-stammdaten.
        assert_eq!(
            router.route(17132),
            Some("wim-stammdaten"),
            "17132 (ANFORDERUNG_PID) must route to wim-stammdaten"
        );
        // 17103 is a genuine wim-stammdaten PID (not GPKE-owned).
        assert_eq!(
            router.route(17103),
            Some("wim-stammdaten"),
            "17103 must route to wim-stammdaten"
        );
    }

    /// Sanity: with DeploymentRoles::all() (default/dev), the NB gate does not
    /// fire at all, so the UEBERMITTLUNG range is not registered and any prior
    /// sperrung registration is undisturbed.
    #[test]
    fn all_roles_uebermittlung_gate_does_not_fire() {
        let all = DeploymentRoles::all();
        let mut router = PidRouter::new();
        router.register(17115, "gpke-sperrung");
        router.register(17116, "gpke-sperrung");
        router.register(17117, "gpke-sperrung");
        WimModule.register_pids_with_roles(&mut router, &all);

        assert_eq!(router.route(17115), Some("gpke-sperrung"));
        assert_eq!(router.route(17116), Some("gpke-sperrung"));
        assert_eq!(router.route(17117), Some("gpke-sperrung"));
        // 17132 ANFORDERUNG_PID should also be registered by the non-role-gated path.
        assert_eq!(router.route(17132), Some("wim-stammdaten"));
    }
}
