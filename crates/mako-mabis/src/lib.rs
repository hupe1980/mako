//! `mako-mabis` — MaBiS, the German electricity balance-group settlement
//! (BNetzA **BK6-24-174 Anlage 3**).
//!
//! Strom only: gas balances through GaBi Gas, on the Gastag and against a
//! Marktgebiet.
//!
//! # Three shapes MaBiS does not share with the other process families
//!
//! ## There is no Prüfmitteilung deadline
//!
//! A Summenzeitreihe arrives and a Prüfmitteilung goes back, so it is natural
//! to hang a response Frist off the arrival the way GPKE and WiM do. The
//! Festlegung says otherwise twice: Kap. 9.8.2 Nr. 1 leaves the Frist cell
//! **empty** and says the receiving party „kann" answer, and Kap. 13.8.2 — the
//! section a 1-Werktag deadline is usually attributed to — defines no answer at
//! all; its two rows are the **BIKO's own** dispatch dates.
//!
//! What bounds a Prüfmitteilung is the clearing window of Kap. 3.10 Tabelle 2,
//! a date range on the Bilanzierungsmonat rather than a countdown from an
//! arrival. See [`fristen`].
//!
//! ## A settlement is a sequence of versions
//!
//! Kap. 3.8.2: versions ascend „über die gesamte BKA". One MaBiS-Zählpunkt in
//! one Bilanzierungsmonat receives a stream of them, each checked, corrected
//! and superseded until the window closes. The version is the
//! **Erstellungszeitpunkt** — 17 characters in IFTSTA `SG4 RFF+AUU` and MSCONS
//! `SG6 DTM+293` — and it is the key both ends match on.
//!
//! ## 55062 / 55063 / 55064 are generic codes
//!
//! Eleven Summenzeitreihen share them and 55064 answers all of them out of
//! twelve different Entscheidungsbäume; six owe an answer and five do not. The
//! discriminator is `SG10 CCI+++ZB4` / `CAV` DE 7111 plus `SG10 CCI+6`
//! ([`zeitreihen::zeitreihe_aus_cav`], [`zp_lifecycle::ZpSerie::from_wire`]).
//!
//! # Workflows
//!
//! | Workflow | PIDs |
//! |---|---|
//! | `mabis-billing` | MSCONS 13003 · 13020 · 13023; IFTSTA 21000–21005 |
//! | `mabis-profile` | MSCONS 13010–13012; ORDERS 17211 |
//! | `mabis-clearingliste` | UTILMD 55067 · 55069 · 55070 · 55073 |
//! | `mabis-listenabgleich` | UTILMD 55065/55066 · 55195/55196 · 55201/55202 · 55223/55224 |
//! | `mabis-zp-lifecycle` | UTILMD 55062–55064 · 55071/55072 · 55197–55214 |
//! | `mabis-anforderung` | ORDERS 17201–17208 · 17210; ORDRSP 19204 |
//!
//! # The Kapitel-17 series expire on 30.09.2026
//!
//! BK6-23-241 Tenorziffer 5 repeals MaBiS Anlage 1 Kapitel 17 with the end of
//! **30.09.2026**. Kap. 17.1 and 17.3 continue as the „Anlage zur BilAReM";
//! Kap. **17.2** (Bilanzkreismonitoring, tägliche AAÜZ — PIDs 55197/55198) and
//! Kap. **17.3.2.1** do not. [`zeitreihen::Familie::endet_am`] and
//! [`zp_lifecycle::ZpSerie::endet_am`] carry the date.
//!
//! # Architecture
//!
//! Each BDEW process variant is a separate [`mako_engine::workflow::Workflow`].
//! This crate contains **only pure domain logic** — no I/O, no EDIFACT parsing,
//! no clock.
//!
//! # Example
//!
//! ```sh
//! cargo run --example mabis_bilanzkreisabrechnung -p mako-mabis
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

pub mod anforderung;
pub mod bilanzkreisabrechnung;
pub mod clearingliste;
pub mod fristen;
pub mod ids;
pub mod listenabgleich;
pub mod profile;
pub mod summenzeitreihe;
pub mod zeitreihen;
pub mod zp_lifecycle;

pub use anforderung::{
    ANFORDERUNG_PIDS, AbonnementVorgang, AnforderungCommand, AnforderungData, AnforderungEvent,
    AnforderungKind, AnforderungState, MabisAnforderungWorkflow,
    WORKFLOW_NAME as ANFORDERUNG_WORKFLOW_NAME,
};
pub use bilanzkreisabrechnung::{
    AUSFALLARBEIT_PIDS, BillingCommand, BillingData, BillingEvent, BillingProjection,
    BillingRecord, BillingState, Datenstatus, IFTSTA_ABWEISUNG_PID, IFTSTA_DATENSTATUS_PIDS,
    IFTSTA_PIDS, IFTSTA_PRUEFMITTEILUNG_PIDS, InvalidSzrVersion, MabisBillingWorkflow,
    Pruefergebnis, RFF_QUALIFIER_VERSION, STS_KATEGORIE_DATENSTATUS, SUMMENZEITREIHE_PID,
    SzrVersion, VersionRecord, WORKFLOW_NAME as BILLING_WORKFLOW_NAME, ist_zeitreihen_pid,
};
pub use clearingliste::{
    CLEARINGLISTE_PIDS, ClearinglisteCommand, ClearinglisteData, ClearinglisteEvent,
    ClearinglisteKind, ClearinglisteState, MabisClearinglisteWorkflow,
    WORKFLOW_NAME as CLEARINGLISTE_WORKFLOW_NAME,
};
pub use fristen::{
    Abrechnungslauf, BIKO_DATENSTATUS_WERKTAGE, BIKO_WEITERLEITUNG_WERKTAGE, Bilanzierungsmonat,
    CLEARING_ENDE_LABEL, Fenster, Phase, Stichtag,
};
pub use listenabgleich::{
    LISTEN_FAMILIEN, ListenFamilie, ListenTyp, ListenabgleichCommand, ListenabgleichData,
    ListenabgleichEvent, ListenabgleichState, MabisListenabgleichWorkflow,
    WORKFLOW_NAME as LISTENABGLEICH_WORKFLOW_NAME, all_pids as listenabgleich_pids,
};
pub use zeitreihen::{
    Aggregationsebene, Bezugszeitraum, CCI_BEZEICHNUNG_SUMMENZEITREIHE,
    CCI_KLASSENTYP_VERANTWORTLICHER, Familie, KAPITEL_17_2_ENDE, Kategorie, Messtechnik, Rolle,
    UnbekannteKategorie, Zeitreihe, aggregationsverantwortung, alle as alle_zeitreihen,
    cav_aus_zeitreihe, cci_aus_rolle, rolle_aus_cci, zeitreihe_aus_cav,
};
// Canonical balance-group topology IDs (defined in `ids`).
pub use anforderung::{ABLEHNUNG_PID, all_pids as anforderung_pids};
pub use ids::{BilanzierungsgebietId, BilanzkreisId, InvalidMabisZaehlpunkt, MabisZaehlpunktId};
pub use profile::{
    Bilanzierungsverfahren, ERSTLIEFERUNG_WERKTAGE, MabisProfilWorkflow, PROFIL_PIDS,
    ProfilCommand, ProfilData, ProfilEvent, ProfilState, Profilart, REKLAMATION_EBD,
    REKLAMATION_PID, WORKFLOW_NAME as PROFIL_WORKFLOW_NAME, all_pids as profil_pids,
};
pub use summenzeitreihe::{
    MABIS_SLOT, SlotResolutionError, SumInterval, Summenzeitreihe, SummenzeitreiheBuilder,
};
pub use zp_lifecycle::{
    MabisZpLifecycleWorkflow, TAEGLICHE_AAUEZ_ENDE, WORKFLOW_NAME as ZP_LIFECYCLE_WORKFLOW_NAME,
    ZP_FAMILIEN, ZpFamilie, ZpLifecycleCommand, ZpLifecycleData, ZpLifecycleEvent,
    ZpLifecycleState, ZpSerie, ZpVorgang, all_pids as zp_lifecycle_pids, familie_for,
    serien_fuer_pid,
};

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for the MaBiS process family.
///
/// # PID ownership
///
/// | Workflow | PIDs |
/// |---|---|
/// | `mabis-billing` | MSCONS 13003 · 13020 · 13023; IFTSTA 21000–21005 |
/// | `mabis-profile` | MSCONS 13010–13012; ORDERS 17211 |
/// | `mabis-clearingliste` | UTILMD 55067 · 55069 · 55070 · 55073 |
/// | `mabis-listenabgleich` | UTILMD 55065/55066 · 55195/55196 · 55201/55202 · 55223/55224 |
/// | `mabis-zp-lifecycle` | UTILMD 55062–55064 · 55071/55072 · 55197–55214 |
/// | `mabis-anforderung` | ORDERS 17201–17208 · 17210; ORDRSP 19204 |
/// | `mabis-ausgleichsenergiepreis` | PRICAT 27001 |
///
/// Each workflow's module docs carry the use case it answers and the Fristen
/// the Festlegung attaches to it.
pub struct MabisModule;

impl mako_engine::builder::EngineModule for MabisModule {
    fn name(&self) -> &'static str {
        "mabis"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        // Every entry is the owning module's own constant. A literal here can
        // disagree with the name `register_pids` routes to, and the two are
        // checked against each other only at `EngineBuilder::build`.
        &[
            bilanzkreisabrechnung::WORKFLOW_NAME,
            profile::WORKFLOW_NAME,
            clearingliste::WORKFLOW_NAME,
            zp_lifecycle::WORKFLOW_NAME,
            anforderung::WORKFLOW_NAME,
            listenabgleich::WORKFLOW_NAME,
        ]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // ── MSCONS Summenzeitreihen ─────────────────────────────────────────
        //
        // 13003 „Summenzeitreihen und Ausfallarbeitssummen" (MSCONS AHB 3.1g §5)
        // carries every BG-/BK-/LF-SZR, the DZÜ, the NZR and the
        // Abrechnungssummenzeitreihe.
        //
        // 13020 Ausfallarbeitsüberführungszeitreihe and 13023
        // Lieferantenausfallarbeitssummenzeitreihe are **MaBiS**, not
        // Redispatch: the PID overview files both under the MaBiS
        // Prozessbeschreibung and both carry the full Prüfmitteilung/
        // Datenstatus cycle (IFTSTA 21000/21002–21005). They were routed to a
        // Redispatch workflow, which had no settlement stream to put them in.
        //
        // 13022 stays with `mako-redispatch`: it is the TR-scharfe Einzel-
        // zeitreihe the BTR and the NB reconcile, not a Summenzeitreihe.
        // 13021 (meteorologische Ex-post-Daten) and 13026 (EEG-Überführungs-
        // zeitreihe) are likewise not MaBiS.
        //
        // Confirmed absent: PID 13001 does not exist in any MSCONS AHB version.
        router.register(bilanzkreisabrechnung::SUMMENZEITREIHE_PID, "mabis-billing");
        for &pid in bilanzkreisabrechnung::AUSFALLARBEIT_PIDS {
            router.register(pid, "mabis-billing");
        }

        // ── IFTSTA MaBiS Statusmeldungen 21000–21005 ────────────────────────
        //
        // All six route to `mabis-billing` so they correlate with their
        // settlement stream by conversation ID. Their *direction* is not
        // uniform: 21000/21001/21005 are this participant's own outbound
        // Prüfmitteilungen, 21002 is the BIKO's Abweisung, and **both** 21003
        // and 21004 carry a Datenstatus — 21003 to the NB/ÜNB, 21004 to the
        // BKV. See `bilanzkreisabrechnung` for the table.
        //
        // PID 21006 does not exist. PID 21007 is WiM Strom Teil 1 / WiM Gas and
        // is registered in `mako-wim` (`wim-device-change`).
        for &pid in bilanzkreisabrechnung::IFTSTA_PIDS {
            router.register(pid, "mabis-billing");
        }

        // ── Normierte Profile (Kap. 6.5 / 6.7) ──────────────────────────────
        //
        // MSCONS 13010/13011/13012 deliver the values; ORDERS 17211 is the LF's
        // Reklamation (EBD E_0100). 17211 was filed with the Redispatch ORDERS
        // codes, which left the delivery with no correction leg at all.
        for pid in profile::all_pids() {
            router.register(pid, profile::WORKFLOW_NAME);
        }

        // ── Record-only UTILMD lists ────────────────────────────────────────
        //
        // 55067 Bilanzkreiszuordnungsliste, 55069 Clearingliste DZR,
        // 55070 Clearingliste BAS, 55073 Liste der Profildefinitionen.
        //
        // 55065 is deliberately **not** here: it owes a 55066 Korrekturliste and
        // belongs to `mabis-listenabgleich`.
        for &pid in clearingliste::CLEARINGLISTE_PIDS {
            router.register(pid, clearingliste::WORKFLOW_NAME);
        }

        // ── UTILMD lists with a correction leg ──────────────────────────────
        //
        // 55065/55066, 55195/55196, 55201/55202, 55223/55224.
        for pid in listenabgleich::all_pids() {
            router.register(pid, listenabgleich::WORKFLOW_NAME);
        }

        // ── MaBiS-Zählpunkt lifecycle ───────────────────────────────────────
        //
        // The PID set comes from `zp_lifecycle::ZP_FAMILIEN`, so the router and
        // the state machine cannot disagree about which codes exist. 55062,
        // 55063 and 55064 are **generic**: eleven series share them and 55064 is
        // answered out of twelve different EBDs, so the workflow is keyed on the
        // series and not on the PID.
        for pid in zp_lifecycle::all_pids() {
            router.register(pid, zp_lifecycle::WORKFLOW_NAME);
        }

        // ── MaBiS Anforderungen ─────────────────────────────────────────────
        //
        // ORDERS 17201–17208 and 17210, plus the one Ablehnung the family has,
        // ORDRSP 19204 (only 17207 can be refused). 17210 was filed with the
        // Redispatch codes; it asks the ANB for the
        // Lieferantenausfallarbeitsclearingliste, which is a MaBiS list.
        for pid in anforderung::all_pids() {
            router.register(pid, anforderung::WORKFLOW_NAME);
        }
    }

    fn profile_requirements(&self) -> &'static [mako_engine::profile::ProfileRequirement] {
        use mako_engine::profile::ProfileRequirement;
        &[
            ProfileRequirement {
                message_type: "MSCONS",
                label: "MSCONS Summenzeitreihen und Profile (MaBiS 13003, 13010–13012, 13020, 13023)",
            },
            ProfileRequirement {
                message_type: "IFTSTA",
                label: "IFTSTA Statusmeldung (MaBiS 21000–21005)",
            },
            ProfileRequirement {
                message_type: "UTILMD",
                label: "UTILMD MaBiS-Listen und ZP-Lifecycle (55062–55073, 55195–55224)",
            },
            ProfileRequirement {
                message_type: "ORDERS",
                label: "ORDERS MaBiS Anforderungen (17201–17208, 17210, 17211)",
            },
            ProfileRequirement {
                message_type: "ORDRSP",
                label: "ORDRSP Ablehnung Ab-/Bestellung der Aggregationsebene (19204)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        // No two workflows may claim the same PID: the router is last-write-wins,
        // so a collision would silently route a message to whichever module
        // registered second.
        let mut seen: Vec<(u32, &'static str)> = Vec::new();
        let mut push = |pids: Vec<u32>, wf: &'static str| -> Result<(), String> {
            for pid in pids {
                if let Some((_, other)) = seen.iter().find(|(p, _)| *p == pid) {
                    return Err(format!("PID {pid} claimed by both {other} and {wf}"));
                }
                seen.push((pid, wf));
            }
            Ok(())
        };
        let mut billing = vec![bilanzkreisabrechnung::SUMMENZEITREIHE_PID];
        billing.extend_from_slice(bilanzkreisabrechnung::AUSFALLARBEIT_PIDS);
        billing.extend_from_slice(bilanzkreisabrechnung::IFTSTA_PIDS);
        push(billing, "mabis-billing")?;
        push(profile::all_pids(), profile::WORKFLOW_NAME)?;
        push(
            clearingliste::CLEARINGLISTE_PIDS.to_vec(),
            clearingliste::WORKFLOW_NAME,
        )?;
        push(listenabgleich::all_pids(), listenabgleich::WORKFLOW_NAME)?;
        push(zp_lifecycle::all_pids(), zp_lifecycle::WORKFLOW_NAME)?;
        push(anforderung::all_pids(), anforderung::WORKFLOW_NAME)?;
        Ok(())
    }
}

#[cfg(test)]
mod module_tests {
    use super::*;
    use mako_engine::builder::EngineModule;

    #[test]
    fn no_two_workflows_claim_the_same_pid() {
        MabisModule.configure().expect("PID ownership is disjoint");
    }

    #[test]
    fn the_lieferantenclearingliste_is_not_record_only() {
        assert!(!clearingliste::CLEARINGLISTE_PIDS.contains(&55065));
        assert!(listenabgleich::all_pids().contains(&55065));
        assert!(listenabgleich::all_pids().contains(&55066));
    }

    #[test]
    fn the_mabis_ausfallarbeit_series_are_registered_here() {
        // 13020 (AAÜZ) and 13023 (LF-AASZR) are MaBiS Summenzeitreihen with a
        // full Prüfmitteilung/Datenstatus cycle, so they settle here; a
        // Redispatch workflow has no settlement stream for them.
        assert_eq!(bilanzkreisabrechnung::AUSFALLARBEIT_PIDS, &[13_020, 13_023]);
    }

    #[test]
    fn the_redispatch_only_mscons_pids_stay_out() {
        // 13021 meteorologische Daten, 13022 Einzelzeitreihe Ausfallarbeit,
        // 13026 EEG-Überführungszeitreihe are not MaBiS.
        let mut claimed = vec![bilanzkreisabrechnung::SUMMENZEITREIHE_PID];
        claimed.extend_from_slice(bilanzkreisabrechnung::AUSFALLARBEIT_PIDS);
        claimed.extend(profile::all_pids());
        for pid in [13_021, 13_022, 13_026] {
            assert!(!claimed.contains(&pid), "{pid} is not a MaBiS PID");
        }
    }

    #[test]
    fn the_hkn_register_ordrsp_codes_stay_out() {
        // 19301/19302 belong to the Herkunftsnachweisregister exchange, not to
        // MaBiS and not to Redispatch.
        for pid in [19_301_u32, 19_302] {
            assert!(!anforderung::all_pids().contains(&pid));
        }
    }

    #[test]
    fn every_workflow_name_is_prefixed() {
        for name in MabisModule.workflow_names() {
            assert!(name.starts_with("mabis-"), "{name}");
        }
    }
}
