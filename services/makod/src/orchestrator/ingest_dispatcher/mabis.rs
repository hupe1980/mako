//! MaBiS ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;
use mako_mabis::{MabisAnforderungWorkflow, MabisListenabgleichWorkflow, MabisZpLifecycleWorkflow};

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the MaBiS workflow family:
    /// `mabis-clearingliste`
    /// `mabis-billing`
    pub(super) async fn dispatch_mabis(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;

        match workflow_name {
            // ── MaBiS record-only Clearinglisten — 55067/55069/55070/55073 ────
            //
            // 55065 is **not** here: it owes a 55066 Korrekturliste and is
            // handled by `mabis-listenabgleich`.
            "mabis-clearingliste" => match pid {
                p if mako_mabis::clearingliste::CLEARINGLISTE_PIDS.contains(&p) => {
                    let cmd = adapters::mabis_clearingliste_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // No APERAK Frist for pure MABIS data messages.
                    self.spawn_or_resume::<MabisClearinglisteWorkflow>(
                        malo_id.as_str(),
                        "mabis-clearingliste",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                }
                _ => Ok(IngestOutcome::Skipped {
                    workflow_name: "mabis-clearingliste",
                    reason: "pid_not_in_dispatch_table",
                }),
            },
            // ── MaBiS-ZP lifecycle — Aktivierung/Deaktivierung ────────────────
            //
            // Both directions arrive. This participant activates its own
            // MaBiS-Zählpunkte at the BIKO and answers the Anfragen of the
            // benachbarter NB on the Netzzeitreihe axis, so an Anfrage spawns
            // and an Antwort resumes.
            //
            // The order of the three questions is load-bearing: for the eleven
            // generic Summenzeitreihen Prozessschritt 4 re-uses the *request*
            // code, so 55062/55063 are both an Anfrage PID and a Weiterleitung
            // PID and must be asked about as Anfragen first.
            //
            // 55062/55063 are shared by eleven Summenzeitreihen, so membership
            // is checked against the PID set rather than resolved to a family —
            // which series a message belongs to only the adapter can say, from
            // the MaBiS-Zählpunkt it names.
            "mabis-zp-lifecycle" => {
                if !mako_mabis::serien_fuer_pid(pid).is_empty() {
                    let cmd = adapters::mabis_zp_lifecycle_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // No deadline is registered on the Anfrage this participant
                    // *receives*. The 1-Werktag window of SD Nr. 2 is the
                    // answering party's, and it is armed on the process the
                    // Anfrage this participant *sends* opens — see
                    // `mako_fristen::antwort::MABIS`.
                    self.spawn_or_resume::<MabisZpLifecycleWorkflow>(
                        malo_id.as_str(),
                        "mabis-zp-lifecycle",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else if mako_mabis::ist_antwort_pid(pid) {
                    // Prozessschritt 2 — „angenommen oder abgelehnt"
                    // (BK6-24-174 Anlage 3 Kap. 5.2.2 / 10.4.2 / 17.3.3.1.2
                    // Nr. 2). It answers an Anfrage this participant sent, so it
                    // resumes that process rather than spawning: an answer with
                    // no question is an orphan, not a new lifecycle.
                    let cmd = adapters::mabis_zp_lifecycle_antwort_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.resume_by_key::<MabisZpLifecycleWorkflow>(
                        malo_id.as_str(),
                        "mabis-zp-lifecycle",
                        cmd,
                    )
                    .await
                } else if mako_mabis::ist_weiterleitung_pid(pid) {
                    // Prozessschritt 4 is addressed to the **BKV**, not to this
                    // deployment: „Der BIKO leitet nur den nicht abgelehnten
                    // MaBiS-ZP an den BKV … weiter." A Netzbetreiber is neither
                    // the BIKO that forwards nor the BKV that receives, so there
                    // is no command to build. It stays a coverage-gap skip so it
                    // dead-letters — the router resolved the PID, which means
                    // the transport was told this message is handled, and an
                    // acknowledged inbound message that is not processed is a
                    // § 147 AO / GoBD record.
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-zp-lifecycle",
                        reason: "pid_not_in_zp_lifecycle_for_this_role",
                    })
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-zp-lifecycle",
                        reason: "pid_not_in_zp_lifecycle_anfragen",
                    })
                }
            }
            // ── MaBiS Listenabgleich — list + Korrekturliste ──────────────────
            // Only the list PIDs are handled. The reply codes are registered so
            // the router resolves them and the adapter covers the list leg only,
            // so an inbound reply has no command to apply and dead-letters.
            "mabis-listenabgleich" => {
                if mako_mabis::listenabgleich::familie_for(pid).is_some() {
                    let cmd = adapters::mabis_listenabgleich_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<MabisListenabgleichWorkflow>(
                        malo_id.as_str(),
                        "mabis-listenabgleich",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-listenabgleich",
                        reason: "pid_not_in_listenabgleich_listen",
                    })
                }
            }
            // ── MaBiS Anforderungen — ORDERS 17201–17208 ──────────────────────
            "mabis-anforderung" => {
                if pid == mako_mabis::ABLEHNUNG_PID {
                    // ORDRSP 19204 refuses an Ab-/Bestellung this side sent
                    // (17207 only). It resumes the request's process rather
                    // than spawning: answering an answer would open a stream
                    // with nothing to answer.
                    let cmd =
                        adapters::mabis_anforderung_ablehnung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<MabisAnforderungWorkflow>(
                        malo_id.as_str(),
                        "mabis-anforderung",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else if mako_mabis::ANFORDERUNG_PIDS.contains(&pid) {
                    let cmd = adapters::mabis_anforderung_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // No APERAK Frist: BK6-24-174 defines no response window for
                    // the Anforderungen; the requested list is its own process.
                    self.spawn_or_resume::<MabisAnforderungWorkflow>(
                        malo_id.as_str(),
                        "mabis-anforderung",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-anforderung",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── MaBiS Bilanzkreisabrechnung — inbound IFTSTA ───────────────────
            //
            // Only 21002 (Abweisung), 21003 and 21004 (Datenstatus /
            // Weiterleitung Prüfmitteilung) arrive. 21000, 21001 and 21005 are
            // this participant's own outbound Prüfmitteilungen; the adapter
            // refuses them so one is never recorded as a check nobody made.
            //
            // **No deadline is registered.** BK6-24-174 Anlage 3 gives the
            // Prüfmitteilung an empty Frist cell (Kap. 9.8.2 Nr. 1: the
            // receiving party „kann" answer), and Kap. 13.8.2 — which the old
            // 1-Werktag deadline cited — defines no answer at all. What bounds
            // a Prüfmitteilung is the clearing window of Tabelle 2, which is a
            // date range on the Bilanzierungsmonat rather than a countdown from
            // this arrival. See `mako_mabis::fristen`.
            "mabis-billing" => {
                if mako_mabis::ist_zeitreihen_pid(pid) {
                    // MSCONS 13003 / 13020 / 13023 — a version of a
                    // Summenzeitreihe. The Erstaufschlag/Clearing phase it
                    // arrives in decides the Datenstatus the BIKO will assign
                    // (Kap. 3.8.3), so it is derived from the calendar here
                    // rather than taken from the message.
                    let cmd = adapters::mabis_summenzeitreihe_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    return self
                        .spawn_or_resume::<MabisBillingWorkflow>(
                            malo_id.as_str(),
                            "mabis-billing",
                            cmd,
                            &fv,
                            &[],
                        )
                        .await;
                }
                let inbound = mako_mabis::IFTSTA_DATENSTATUS_PIDS.contains(&pid)
                    || pid == mako_mabis::IFTSTA_ABWEISUNG_PID;
                if inbound {
                    let cmd = adapters::mabis_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<MabisBillingWorkflow>(
                        malo_id.as_str(),
                        "mabis-billing",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else if mako_mabis::IFTSTA_PRUEFMITTEILUNG_PIDS.contains(&pid) {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-billing",
                        reason: "outbound_pruefmitteilung_pid",
                    })
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-billing",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            // ── MaBiS normierte Profile — MSCONS 13010–13012, ORDERS 17211 ────
            "mabis-profile" => {
                if mako_mabis::profil_pids().contains(&pid) {
                    let cmd = adapters::mabis_profil_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    self.spawn_or_resume::<mako_mabis::MabisProfilWorkflow>(
                        malo_id.as_str(),
                        "mabis-profile",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-profile",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
