//! `mako-wim` — WiM (Wechselprozesse im Messwesen Strom) process engine for
//! German smart-meter market communication (BDEW MaKo).
//!
//! ## Process family
//!
//! WiM governs the switching processes for metering point operators in the
//! German electricity smart-meter rollout, regulated by the MsbG and BDEW
//! WiM process documentation:
//!
//! | Process | PIDs | Message | Module | EBD |
//! |---|---|---|---|---|
//! | Anmeldung MSB (MSBN → NB) | 55042 → 55043/55044 | UTILMD | `geraetewechsel` | `E_0201` |
//! | Kündigung MSB (MSBN → **MSBA**) | 55039 → 55040/55041 | UTILMD | `geraetewechsel` | `E_0200` |
//! | Ende MSB / Abmeldung (**MSBA → NB**) | 55051 → 55052/55053 | UTILMD | `geraetewechsel` | `E_0202` |
//! | Verpflichtungsanfrage (NB → **gMSB**) | 55168 → 55169/55170 | UTILMD | `geraetewechsel` | `E_0240` |
//! | Weiterverpflichtung (**NB → MSBA**) | 17002 → 19003/19004 | ORDERS/ORDRSP | `weiterverpflichtung` | `E_0203` |
//! | Ersteinbau iMS (**gMSB → wMSB**) | 21029 → 21030/21031 | IFTSTA | `ersteinbau` | `E_0233` |
//! | Geräteübernahme Bestellung (MSBN → MSBA) | 17001 → 19001/19002 | ORDERS/ORDRSP | `geraeteubernahme` | `E_0247` |
//! | Anzeige Gerätewechselabsicht (MSBN → MSBA) | 17009 → 19015/19016 | ORDERS/ORDRSP | `geraeteubernahme` | `E_0204` |
//! | Messlokationsänderung (NB/LF → MSB) | 17011/17118 → 19005/19006 | ORDERS/ORDRSP | `technik_aenderung` | `E_0249`/`E_0250` |
//! | Stammdaten Anfrage / Übermittlung | 17132 (req), 17102–17133 (resp) | ORDERS | `stammdaten` | — |
//! | Preisanfrage (REQOTE/QUOTES) | 35001/35002/35004/35005 → 15001/15002/15004/15005 | REQOTE, QUOTES | `preisanfrage` | — |
//! | Rechnungsabwicklung über den LF | 17005/17006 → 19009/19010 | ORDERS/ORDRSP | `rechnungsabwicklung` | `E_0206`/`E_0209` |
//! | Preisliste (PRICAT) | 27001–27003 | PRICAT | `preisliste` | — |
//! | ESA Wertebestellung | 35003, 15003, 17007/17008, 39002, 19011–19014 | REQOTE/QUOTES/ORDERS/ORDCHG/ORDRSP | `wertebestellung`, `esa_wertebestellung` | — |
//! | MSB-Rechnung (INVOIC) | 31009 → 33001/33003/33004, 29001 | INVOIC | `invoic` | — |
//! | INSRPT Störungsmeldung | 23001 → 23003/23004/23008/23011/23012 | INSRPT | `insrpt` | — |
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
//! ## Three clocks, three messages
//!
//! | Clock | Window | Message |
//! |---|---|---|
//! | **APERAK** — processability | 45 min for Strom UTILMD/ORDERS (APERAK AHB 1.0 §2.4.1) | APERAK BGM+312/313 |
//! | **Antwortfrist** — the business decision | 3 / 5 / 7 / 1 Werktage per process, from `antwort_frist_werktage(pid)` | the Antwort-PID's UTILMD or ORDRSP |
//! | **Vorlauffrist** — was the requested date admissible? | anchored on the date the message carries, `mako_fristen::vorlauf` | the inbound message itself |
//!
//! Only the second discharges the Antwortfrist; the APERAK decides nothing.
//! Where GPKE states its answer windows as clock times on the first Werktag
//! after the ÜT, WiM states Werktage — see
//! [`mako_fristen::antwort::GPKE_IS_NOT_TWENTY_FOUR_HOURS`].
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
//!                         .and_then(|t| t.marktlokation()).unwrap_or_default(),
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

pub mod consent;
pub mod ersteinbau;
pub mod esa;
pub mod esa_wertebestellung;
pub mod geraeteubernahme;
pub mod geraetewechsel;
pub mod insrpt;
pub mod invoic;
pub mod preisanfrage;
pub mod preisliste;
pub mod rechnungsabwicklung;
pub mod stammdaten;
pub mod steuerungsauftrag;
pub mod technik_aenderung;
pub mod weiterverpflichtung;
pub mod wertebestellung;

pub use geraeteubernahme::{
    ANKUENDIGUNG_PIDS as GERAETEUBERNAHME_ANKUENDIGUNG_PIDS, BESTELLUNG_PIDS,
    GERAETEUBERNAHME_PIDS, GeraeteubernahmeCommand, GeraeteubernahmeData, GeraeteubernahmeEvent,
    GeraeteubernahmeProjection, GeraeteubernahmeRecord, GeraeteubernahmeRecordData,
    GeraeteubernahmeState, ORDRSP_DEADLINE_LABEL as GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL,
    WORKFLOW_NAME as GERAETEUBERNAHME_WORKFLOW_NAME, WimGeraeteubernahmeWorkflow,
};
pub use geraetewechsel::{
    ANTWORT_FRIST_WINDOW_LABEL as GERAETEWECHSEL_ANTWORT_FRIST_WINDOW_LABEL,
    AUFTRAG_ANTWORT_WINDOW_LABEL, DEVICE_CHANGE_ANTWORT_PIDS, DEVICE_CHANGE_PIDS,
    DeviceChangeCommand, DeviceChangeData, DeviceChangeEvent, DeviceChangeProjection,
    DeviceChangeRecord, DeviceChangeState, WORKFLOW_NAME, WimDeviceChangeWorkflow,
    antwort_frist_werktage, antwort_pid_meaning,
};
pub use insrpt::{
    ANTWORT_WINDOW_LABEL as INSRPT_ANTWORT_WINDOW_LABEL,
    ERGEBNIS_WINDOW_LABEL as INSRPT_ERGEBNIS_WINDOW_LABEL, INSRPT_ANFRAGE_PIDS,
    INSRPT_ANTWORT_PIDS, INSRPT_ERGEBNIS_PID, INSRPT_INFORMATIONS_PIDS, Seite as InsrptSeite,
    StoerungsmeldungCommand, StoerungsmeldungData, StoerungsmeldungEvent, StoerungsmeldungState,
    WEITERLEITUNG_WINDOW_LABEL as INSRPT_WEITERLEITUNG_WINDOW_LABEL,
    WORKFLOW_NAME as INSRPT_WORKFLOW_NAME, WimInsrptWorkflow,
};
pub use invoic::{
    GasAblehnung, SETTLEMENT_WINDOW_LABEL as INVOIC_SETTLEMENT_WINDOW_LABEL,
    WIM_COMDIS_ABLEHNUNG_PID, WIM_INVOIC_PIDS, WIM_REMADV_PIDS,
    WORKFLOW_NAME as INVOIC_WORKFLOW_NAME, WimInvoic, WimInvoicWorkflow, gas_ablehnungs_ebd,
};
pub use preisanfrage::{
    PREISANFRAGE_DEADLINE_LABEL, PreisanfrageCommand, PreisanfrageData, PreisanfrageEvent,
    PreisanfrageState, QUOTES_PIDS, REQOTE_PIDS, WORKFLOW_NAME as PREISANFRAGE_WORKFLOW_NAME,
    WimPreisanfrageWorkflow, antwort_frist_werktage as preisanfrage_antwort_frist_werktage,
};
pub use preisliste::{
    PRICAT_PIDS, PreislisteCommand, PreislisteData, PreislisteEvent, PreislisteState,
    WORKFLOW_NAME as PREISLISTE_WORKFLOW_NAME, WimPreislisteWorkflow,
};
pub use rechnungsabwicklung::{
    RECHNUNGSABWICKLUNG_DEADLINE_LABEL, RECHNUNGSABWICKLUNG_ORDERS_PIDS,
    RECHNUNGSABWICKLUNG_ORDRSP_PIDS, RechnungsabwicklungCommand, RechnungsabwicklungData,
    RechnungsabwicklungEvent, RechnungsabwicklungState,
    WORKFLOW_NAME as RECHNUNGSABWICKLUNG_WORKFLOW_NAME, WimRechnungsabwicklungWorkflow,
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
pub use weiterverpflichtung::{
    ANTWORT_WINDOW_LABEL as WEITERVERPFLICHTUNG_ANTWORT_WINDOW_LABEL,
    AUFTRAG_PID as WEITERVERPFLICHTUNG_AUFTRAG_PID, WEITERVERPFLICHTUNG_PIDS,
    WORKFLOW_NAME as WEITERVERPFLICHTUNG_WORKFLOW_NAME, WeiterverpflichtungCommand,
    WeiterverpflichtungData, WeiterverpflichtungEvent, WeiterverpflichtungProjection,
    WeiterverpflichtungState, WimWeiterverpflichtungWorkflow,
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
/// | 17001, 17002, 17009 | `wim-geraeteubernahme` | Geräteübernahme ORDERS | any |
/// | 17132 | `wim-stammdaten` | Stammdaten Anforderung Strom (NB → MSB), MSB role | any |
/// | 17102–17133 | `wim-stammdaten` | Stammdatenübermittlung responses (MSB → NB), NB role | **Nb only** |
/// | 39002 | `wim-wertebestellung` | ESA Stornierung der Bestellung (ORDCHG) | **Msb only** |
/// | 19001, 19002 | `wim-geraeteubernahme` | ORDRSP Bestellbestätigung/Ablehnung from NB | **nMSB only** |
/// | 19015, 19016 | `wim-geraeteubernahme` | ORDRSP Gerätewechselabsicht Bestätigung/Ablehnung | any |
///
/// ## Role-conditional PIDs (ORDRSP 19001/19002)
///
/// GPKE Konfiguration claims 19001/19002 on an NB instance — it receives them
/// after sending ORDERS 17134/17135 — and WiM Geräteübernahme claims them on an
/// nMSB instance, answering its own ORDERS 17001. Only one reading can win, so
/// the WiM one is registered when [`DeploymentRoles`] contains
/// [`Marktrolle::Nmsb`]. Use [`DeploymentRoles::nmsb()`] and
/// [`DeploymentRoles::nb()`] rather than a catch-all role set on a deployment
/// that is both.
///
/// 19015/19016 are **not** gated: nothing else claims them, and the deployment
/// that receives them is the one that sent the 17009 they answer.
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
            rechnungsabwicklung::WORKFLOW_NAME,
            weiterverpflichtung::WORKFLOW_NAME,
            "wim-invoic",
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
        //         non-constitutive per BK6-22-024 WiM Teil 1 Kap. 2.1.3 — the NB is not a party.
        // 55042 — Anmeldung MSB (MSBN → NB): new MSB initiates change.
        // 55051 — Ende MSB / Abmeldung (MSBA → NB): NB terminates MSB relationship.
        // 55168 — Verpflichtungsanfrage / Aufforderung (NB → gMSB).
        //
        // The Gas twins 44039 / 44042 / 44051 / 44168 (AWH WiM Gas 2.0) run the
        // **same** Use-Cases with the same Fristen and are handled by the same
        // workflow; `wim_sparte` reads the Sparte off the PID and it decides the
        // Entscheidungsbaum, the Codeliste, the APERAK regime and the
        // Zuordnungszeitpunkt (06:00 Uhr Gastag against 00:00).
        //
        // All eight share WimDeviceChangeWorkflow; the PID is carried in the
        // DeviceChangeData and available for business-logic branching.
        for &pid in geraetewechsel::DEVICE_CHANGE_PIDS {
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

        // ORDERS 17002 → ORDRSP 19003/19004 — Weiterverpflichtung des MSB
        // (WiM Teil 1 Kap. 2.4.2 Nr. 5/6, `E_0203`).
        //
        // Only the inbound leg is registered. 19003/19004 are *our* answer,
        // rendered from the outbox — the NB-side receiver (mako sending 17002
        // and awaiting the MSBA's ORDRSP) is not implemented, and registering
        // an outbound-only PID would claim a dispatch arm that can never fire.
        router.register(
            weiterverpflichtung::AUFTRAG_PID,
            weiterverpflichtung::WORKFLOW_NAME,
        );

        // ORDERS 17001/17009 — Geräteübernahme Bestellung and Anzeige
        // Gerätewechselabsicht.
        //
        // **One workflow for both Sparten.** ORDERS and ORDRSP are Sparte-neutral
        // AHBs, so these PIDs carry the Strom *and* the Gas Use-Case; the Sparte
        // is the recipient MP-ID's and travels in the command, where it picks
        // the Entscheidungsbaum and the Codeliste.
        for &pid in geraeteubernahme::GERAETEUBERNAHME_PIDS {
            router.register(pid, geraeteubernahme::WORKFLOW_NAME);
        }

        // ORDRSP 19015/19016 — Bestätigung/Ablehnung der Gerätewechselabsicht,
        // the answer to the ORDERS 17009 the MSBN sends. No other module claims
        // them, so they are registered unconditionally: gating them would
        // dead-letter the answer to a message this deployment itself sent.
        for pid in [19_015_u32, 19_016] {
            router.register(pid, geraeteubernahme::WORKFLOW_NAME);
        }

        // ORDRSP 19001/19002 — Bestellbestätigung/Ablehnung, the answer to
        // ORDERS 17001. GPKE Konfiguration claims the same two PIDs on an NB
        // instance, so the MSBN reading is registered only when the Nmsb role is
        // declared; `register_with_module` then panics at build() rather than
        // silently letting one module overwrite the other.
        if !roles.is_all() && roles.contains(mako_engine::marktrolle::Marktrolle::Nmsb) {
            for pid in [19_001_u32, 19_002] {
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
        // Note: 17101 („Anfrage zur Übermittlung von Stammdaten Gas") is the Gas
        // counterpart. It is a GeLi Gas Geschäftsdatenanfrage, not a WiM
        // Stammdatenanforderung, and is not routed here.
        router.register(stammdaten::ANFORDERUNG_PID.as_u32(), "wim-stammdaten");

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
        // Source: site/content/docs/regulatory/pid-reference.md (generated from BDEW xlsx PID 3.3 + PID 4.0).
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

        // REQOTE 35001/35002/35004/35005 (Preisanfrage) and QUOTES 15001/15002/15004/15005 (Angebot).
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

        // Rechnungsabwicklung MSB über LF (WiM Strom Teil 1): ORDERS 17005
        // (Bestellung — the LF accepting the quote; nothing answers it) and
        // 17006 (Beendigung, either direction), plus ORDRSP 19009/19010
        // (Bestätigung/Ablehnung der Beendigung) resuming a Beendigung mako
        // sent. Directions per BDEW PID overview 4.0 / AWH Aktivitätsdiagramme
        // WiM V1.3 §§2.8–2.11 (EBDs E_0206/E_0209).
        for &pid in rechnungsabwicklung::RECHNUNGSABWICKLUNG_ORDERS_PIDS
            .iter()
            .chain(rechnungsabwicklung::RECHNUNGSABWICKLUNG_ORDRSP_PIDS)
        {
            router.register(pid, rechnungsabwicklung::WORKFLOW_NAME);
        }

        // IFTSTA 21032 „Antwort auf das Angebot" — the *other* half of the
        // Prozessschritt ORDERS 17005 answers. 17005 is the LF's acceptance
        // and carries no code; 21032 is its refusal and carries `E_0205` resp.
        // `E_0208` (PID-Übersicht 4.0 lfd. Nr. 30930/31020). Registering only
        // 17005 records every yes and dead-letters every no.
        router.register(
            rechnungsabwicklung::RECHNUNGSABWICKLUNG_ABLEHNUNG_PID,
            rechnungsabwicklung::WORKFLOW_NAME,
        );

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
            // REQOTE 35003 opens the handshake. ESA-specific (REQOTE AHB 1.1
            // §4.3), so it routes straight here — it is not part of the
            // Preisanfrage REQOTE set.
            router.register(
                wertebestellung::ANFRAGE_PID.as_u32(),
                wertebestellung::WORKFLOW_NAME,
            );
            router.register(
                wertebestellung::BESTELLUNG_PID.as_u32(),
                wertebestellung::WORKFLOW_NAME,
            );
            router.register(
                wertebestellung::ABBESTELLUNG_PID.as_u32(),
                wertebestellung::WORKFLOW_NAME,
            );
            router.register(
                wertebestellung::STORNIERUNG_PID.as_u32(),
                wertebestellung::WORKFLOW_NAME,
            );
        }

        // ESA side: this deployment *is* the ESA and originates the order
        // handshake (REQOTE 35003 / ORDERS 17007 / ORDCHG 39002 / ORDERS 17008).
        // The MSB's answers (QUOTES 15003, ORDRSP 19011-19014) are inbound here
        // and resume the esa-wertebestellung process. Registered only for a
        // deployment that *is* an ESA — an ESA has no Zuordnung to a
        // Marktlokation, so nothing else may claim these. The set is disjoint
        // from the MSB inbound PIDs, so an integrated deployment holds both.
        if roles.contains(mako_engine::marktrolle::Marktrolle::Esa) {
            for &pid in esa_wertebestellung::ESA_INBOUND_PIDS {
                router.register(pid.as_u32(), esa_wertebestellung::WORKFLOW_NAME);
            }
        }

        // INVOIC — the WiM-Rechnung in both Sparten: 31009 (MSB → NB/LF/ESA,
        // WiM Strom Teil 1 Kap. 3.6/4), 31003 (MSBA → NB und MSBA → MSBN, AWH
        // WiM Gas 2.0 Kap. 4.7) and the Sparte-neutral Stornorechnung 31004.
        //
        // These PIDs are explicitly excluded from mako-gpke's GPKE_INVOIC_PIDS array.
        // Without registration here, all inbound WiM-domain INVOIC messages would
        // be silently dead-lettered and no CONTRL acknowledgement would be sent,
        // violating the AS4 acknowledgement obligation (BDEW AS4-Profile §5).
        //
        // The WimInvoicWorkflow provides a complete state machine with Settle/Dispute
        // commands. Automatic outbound REMADV generation on the auto-settlement
        // deadline is not implemented; settlement is driven by an explicit command.
        for &pid in invoic::WIM_INVOIC_PIDS {
            router.register(pid, "wim-invoic");
        }

        // REMADV 33001–33002 — inbound payment advice for WiM billing (invoicer role).
        //
        // After the NB sends INVOIC 31009 (MSB-Rechnung), the payer (MSB) sends
        // back a REMADV (33001 = Bestätigung, 33002 = Ablehnung). Without this
        // registration, all REMADV messages for WiM billing are silently dropped.
        //
        // GPKE billing registers 33003/33004 (Strom Abweisung Kopf und Summe /
        // Position — itemized rejections). Per REMADV AHB 1.0a, WiM Strom billing
        // (incl. ESA→MSB) ALSO rejects with the itemized 33003/33004; today mako-wim
        // registers only 33001/33002 and leans on GPKE's 33003/34 registration, so a
        // WiM itemized rejection is not yet routed to `wim-invoic` — see ROADMAP
        // "REMADV itemized rejections in WiM scope". The registrations coexist because
        // the makod router disambiguates shared REMADV PIDs by conversation ID
        // (invoice correlation), not by PID alone.
        //
        // Source: REMADV AHB 1.0a §3, WiM Strom Teil 1 (BK6-22-024).
        for &pid in invoic::WIM_REMADV_PIDS {
            router.register(pid, "wim-invoic");
        }

        // COMDIS 29001 — inbound Ablehnung REMADV (invoicer rejects payer's REMADV).
        //
        // Shared PID with GPKE billing. The router dispatches to the correct
        // workflow instance via conversation ID correlation.
        //
        // Source: COMDIS AHB 1.0, WiM Strom Teil 1 (BK6-22-024).
        router.register(invoic::WIM_COMDIS_ABLEHNUNG_PID.as_u32(), "wim-invoic");

        // UTILMD 44183 „Ende MSB von NB" — the Gas NB informing the MSB of a
        // Stilllegung (AWH WiM Gas 2.0 Kap. 3.7). Informational: it carries no
        // Status der Antwort and has no answer Prüfidentifikator, so it lands
        // on the same `ReceiveInformation` path as the IFTSTA Statusmeldungen.
        router.register(geraetewechsel::ENDE_MSB_VOM_NB_PID, "wim-device-change");

        // IFTSTA WiM PIDs 21009–21018 (MSB-Wechsel status messages).
        //
        // These are Vollzugsmeldungen and process-status notifications that
        // accompany the WiM UTILMD device-change process. All are routed to
        // `wim-device-change` for correlation via conversation ID (CI tag).
        for &pid in geraetewechsel::IFTSTA_PIDS {
            router.register(pid, "wim-device-change");
        }

        // Ersteinbau eines iMS in eine bestehende Messlokation — WiM Strom
        // Teil 1 Kap. 3.5 (IFTSTA 21029 → 21030/21031, `E_0233`).
        //
        // Registered unconditionally because both sides of it are MSB work and
        // a deployment can hold either: the grundzuständiger MSB sends the
        // Vorabinformation and receives the answer, the wettbewerblicher MSB
        // receives it and owes one in three Werktagen. Nothing else claims
        // 21029–21031 — the Anwendungsübersicht 4.0 publishes them under
        // „WiM Strom Teil 1 / Ersteinbau" alone.
        //
        // Strom only: there is no iMS rollout obligation in Gas, so AWH WiM Gas
        // 2.0 has no Kap. 3.5 equivalent.
        for &pid in ersteinbau::ERSTEINBAU_PIDS {
            router.register(pid, ersteinbau::WORKFLOW_NAME);
        }

        // `wim-steuerungsauftrag` is intentionally NOT registered here.
        //
        // The Steuerungsauftrag workflow is driven exclusively by the BDEW
        // API-Webdienste Strom `controlMeasuresV1` REST channel (BDEW
        // API-Guideline 1.0a). There is no EDIFACT message type for this
        // workflow; it receives no inbound PID dispatch from the `PidRouter`.
        // The REST adapter (`energy-api`) creates process commands directly.
        // Do not add EDIFACT PID registrations for this workflow.

        // INSRPT Störungsbehebung in der Messlokation — WiM Strom Teil 2 Kap. 1
        // and AWH WiM Gas 2.0 Kap. 4.3.
        //
        // 23001 Störungsmeldung (LF/NB → MSB), 23003/23004 Antwort, 23008
        // Ergebnisbericht, 23005/23009 the Gas Informationsmeldungen an den NB,
        // 23011/23012 the Strom Weiterleitung an betroffene Marktlokationen.
        //
        // **One workflow for both Sparten.** The INSRPT AHB is Sparte-neutral;
        // what differs is the Frist, and the Frist is not a function of the PID
        // in either Sparte — Strom branches on the Messtechnik and the
        // Spannungsebene, Gas states one flat number. Both live in
        // `insrpt::antwort_werktage` / `insrpt::ergebnis_werktage`, which take
        // the Sparte as an argument.
        for &pid in insrpt::INSRPT_ANFRAGE_PIDS
            .iter()
            .chain(insrpt::INSRPT_ANTWORT_PIDS)
        {
            router.register(pid, insrpt::WORKFLOW_NAME);
        }

        // WiM Technikänderung — device/config change requests (ORDERS/ORDRSP).
        //
        // Covers LF→MSB Änderung der Technik (17011) and MSB→MSB Bestellung
        // Konfigurationsänderung (17118). The ESA order PIDs (17007/17008,
        // ORDRSP 19011–19014) belong to `wertebestellung`.
        // ORDRSP: Bestätigung (19003/19005) and Ablehnung (19004/19006/19007).
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
                label: "IFTSTA (WiM MSB-Wechsel 21007/21009–21013/21018/21036, \
                        Ersteinbau iMS 21029–21031, Durchführungsmeldung 21025/21027)",
            },
            ProfileRequirement {
                message_type: "INVOIC",
                label: "INVOIC MSB-Rechnung (31009)",
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
                "geraeteubernahme::ANKUENDIGUNG_PIDS",
                geraeteubernahme::ANKUENDIGUNG_PIDS,
            ),
            ("geraetewechsel::IFTSTA_PIDS", geraetewechsel::IFTSTA_PIDS),
            ("invoic::WIM_INVOIC_PIDS", invoic::WIM_INVOIC_PIDS),
            ("invoic::WIM_REMADV_PIDS", invoic::WIM_REMADV_PIDS),
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
    /// A bare `!roles.is_all() && roles.contains(Nb)` has WiM register the
    /// GPKE-owned PIDs in that range to "wim-stammdaten", overwriting GPKE's
    /// entries and silently misrouting the messages.
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
