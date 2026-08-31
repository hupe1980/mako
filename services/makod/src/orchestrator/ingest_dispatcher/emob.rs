//! NZR-EMob / Modell 2 ingest dispatch arms.
//!
//! Split out of the flat `ingest_dispatcher` module. The spawn/resume
//! machinery and the extraction helpers live in `super`.
//!
//! # Only the request PID spawns
//!
//! Each leg registers both its PIDs to one workflow so the router resolves the
//! answer too — but an answer arrives on a process **this side started** and
//! must resume it. Spawning on a 55239 would open a process with nothing to
//! answer and leave the real one's Frist to expire as a false timeout.
//!
//! # The answer window is registered at spawn
//!
//! A `Deadline` whose label no `on_deadline` arm matches fires into `None` and
//! is lost, so the labels come from `mako_emob::modellwechsel` and the windows
//! from `mako_fristen::antwort` — the same table `processd` and `obsd` read.
//! Only the **inbound** leg gets one: it is our own answer that is owed. When
//! we sent the request, the counterparty owes the answer and `obsd` watches it.

use super::*;

use mako_emob::modellwechsel::{ABMELDUNG, ANMELDUNG, LegWire, ZUORDNUNGSENDE};
use mako_emob::{EmobAbmeldungWorkflow, EmobAnmeldungWorkflow, EmobZuordnungsendeWorkflow};

/// The instant our answer to `pid` is due, from the one Antwortfristen table.
fn antwortfrist(
    pid: u32,
    received: time::OffsetDateTime,
) -> Option<(&'static str, time::OffsetDateTime)> {
    let leg = match pid {
        p if p == ANMELDUNG.anfrage_pid => ANMELDUNG,
        p if p == ZUORDNUNGSENDE.anfrage_pid => ZUORDNUNGSENDE,
        p if p == ABMELDUNG.anfrage_pid => ABMELDUNG,
        _ => return None,
    };
    let obligation = mako_fristen::antwort::antwort_obligation(pid)?;
    Some((
        leg.window_label,
        obligation
            .frist
            .due_at(received, mako_fristen::HolidayCalendar::BdewMaKo),
    ))
}

impl EdifactIngestDispatcher {
    /// Phase-2 dispatch arms for the Modell-2 workflow family:
    /// `emob-anmeldung`, `emob-zuordnungsende`, `emob-abmeldung`.
    pub(super) async fn dispatch_emob(
        &self,
        msg: &AnyMessage,
        workflow_name: &str,
        pid: u32,
    ) -> Result<IngestOutcome, EngineError> {
        let fv = detect_format_version(msg);
        let raw: &dyn Any = msg;
        // Every leg keys on the **Marktlokation** — the one object all six
        // messages name in `SG5 LOC+Z16`. The three legs are separate workflow
        // names, so they can share it without colliding.
        let malo = extract_malo_from_msg(msg);
        let now = mako_fristen::berlin_now();

        macro_rules! leg {
            ($wf:ty, $wire:expr, $anfrage:ident, $antwort:ident) => {{
                let leg: LegWire = $wire;
                if pid == leg.anfrage_pid {
                    let cmd = adapters::$anfrage().dispatch(raw, &fv)?;
                    let deadlines: Vec<_> = antwortfrist(pid, now).into_iter().collect();
                    self.spawn_or_resume_guarded::<$wf>(
                        malo.as_str(),
                        <$wf>::WORKFLOW_NAME,
                        cmd,
                        &fv,
                        &deadlines,
                        |state: &<$wf as Workflow>::State| !state.ist_terminal(),
                    )
                    .await
                } else {
                    let cmd = adapters::$antwort().dispatch(raw, &fv)?;
                    self.resume_by_key::<$wf>(malo.as_str(), <$wf>::WORKFLOW_NAME, cmd)
                        .await
                }
            }};
        }

        match workflow_name {
            n if n == EmobAnmeldungWorkflow::WORKFLOW_NAME => leg!(
                EmobAnmeldungWorkflow,
                ANMELDUNG,
                emob_anmeldung_registry,
                emob_anmeldung_antwort_registry
            ),
            n if n == EmobZuordnungsendeWorkflow::WORKFLOW_NAME => leg!(
                EmobZuordnungsendeWorkflow,
                ZUORDNUNGSENDE,
                emob_zuordnungsende_registry,
                emob_zuordnungsende_antwort_registry
            ),
            n if n == EmobAbmeldungWorkflow::WORKFLOW_NAME => leg!(
                EmobAbmeldungWorkflow,
                ABMELDUNG,
                emob_abmeldung_registry,
                emob_abmeldung_antwort_registry
            ),
            other => Ok(IngestOutcome::Skipped {
                workflow_name: "emob",
                reason: {
                    tracing::warn!(workflow_name = %other, pid, "emob: unknown workflow name");
                    "pid_not_in_dispatch_table"
                },
            }),
        }
    }
}
