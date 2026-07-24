//! MaBiS ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery, extraction helpers, and the consent gate live in `super`.

use super::*;

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
