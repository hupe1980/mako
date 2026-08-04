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
            // ── MABIS Clearingliste — PIDs 55065/55069/55070 ──────────────────
            // PIDs 55065/55069/55070: MABIS IFTSTA Clearingliste (BKV ↔ ÜNB) — spawn.
            "mabis-clearingliste" => match pid {
                55065 | 55069 | 55070 => {
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
            // Only the Anfrage PIDs spawn. The Antwort and Weiterleitung codes
            // are registered so the router resolves them, but they arrive on a
            // process this side started, so they resume rather than spawn — and
            // spawning on one would answer an answer.
            "mabis-zp-lifecycle" => {
                if mako_mabis::familie_for(pid).is_some() {
                    let cmd = adapters::mabis_zp_lifecycle_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // No APERAK Frist: BK6-24-174 defines no response window for
                    // the lifecycle Anfragen themselves.
                    self.spawn_or_resume::<MabisZpLifecycleWorkflow>(
                        malo_id.as_str(),
                        "mabis-zp-lifecycle",
                        cmd,
                        &fv,
                        &[],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-zp-lifecycle",
                        reason: "answer_pid_resumes_only",
                    })
                }
            }
            // ── MaBiS Listenabgleich — list + Korrekturliste ──────────────────
            // Only the list PIDs spawn; the reply codes are registered so the
            // router resolves them, but they arrive on a process this side
            // started and resume it instead.
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
                        reason: "reply_pid_resumes_only",
                    })
                }
            }
            // ── MaBiS Anforderungen — ORDERS 17201–17208 ──────────────────────
            "mabis-anforderung" => {
                if mako_mabis::ANFORDERUNG_PIDS.contains(&pid) {
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
            // ── MABIS Bilanzkreisabrechnung IFTSTA (PIDs 21000–21005) ──────────
            "mabis-billing" => {
                if mako_mabis::bilanzkreisabrechnung::IFTSTA_PIDS.contains(&pid) {
                    let cmd = adapters::mabis_registry().dispatch(raw, &fv)?;
                    let malo_id = extract_malo_from_msg(msg);
                    // Prüfmitteilung deadline: 1 Werktag (BK6-24-174 §13.8).
                    let due_at = fristen::deadline_at_werktage(
                        OffsetDateTime::now_utc(),
                        1,
                        HolidayCalendar::BdewMaKo,
                    );
                    self.spawn_or_resume::<MabisBillingWorkflow>(
                        malo_id.as_str(),
                        "mabis-billing",
                        cmd,
                        &fv,
                        &[(mako_mabis::PRUEFMITTEILUNG_DEADLINE_LABEL, due_at)],
                    )
                    .await
                } else {
                    Ok(IngestOutcome::Skipped {
                        workflow_name: "mabis-billing",
                        reason: "pid_not_in_dispatch_table",
                    })
                }
            }
            wf_name => unknown_workflow_skip(wf_name, pid),
        }
    }
}
