//! GPKE Strom answer Fristen — the table itself now lives in
//! [`mako_fristen::antwort`], and what stays here is the check that it
//! agrees with these workflows.
//!
//! The table is data: a PID, a window shape and a Fundstelle. Beside the
//! workflows it would sit above `mako-engine`, so no crate could hold all four
//! families and each service would aggregate them itself.
//!
//! What belongs beside a workflow is the cross-check: every trigger is a PID
//! this crate spawns from, and the answer PIDs match `response_pid_for`'s own
//! derivation. Those tests are below.

pub use mako_fristen::antwort::{AntwortObligation, FristShape, GPKE as ANTWORT_OBLIGATIONS};

use mako_fristen::antwort;
use time::OffsetDateTime;

/// The answer obligation for an inbound GPKE Strom PID, if the Festlegung
/// states one.
#[must_use]
pub fn antwort_obligation(trigger_pid: u32) -> Option<&'static AntwortObligation> {
    antwort::antwort_obligation(trigger_pid).filter(|o| o.family == antwort::Family::Gpke)
}

/// The instant by which the answer to `trigger_pid` must have been sent.
///
/// `received` is the arrival instant of the inbound message (the ÜT). Returns
/// `None` for a PID not in [`ANTWORT_OBLIGATIONS`] — treat that as *unknown*,
/// never as *unbounded*.
#[must_use]
pub fn antwort_deadline(trigger_pid: u32, received: OffsetDateTime) -> Option<OffsetDateTime> {
    antwort_obligation(trigger_pid).map(|o| {
        o.frist
            .due_at(received, mako_fristen::HolidayCalendar::BdewMaKo)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month, Time};

    fn utc(y: i32, m: Month, d: u8, h: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(y, m, d).expect("valid date"),
            Time::from_hms(h, 0, 0).expect("valid time"),
        )
    }

    /// Trigger PIDs whose answer Frist is published but which **no workflow in
    /// this crate spawns from yet**.
    ///
    /// The Frist is real — `obsd` alerts on it and the operator queue sizes by
    /// it — but no `process.initiated` fires, so nothing starts the clock.
    /// Enumerating them keeps the gap visible; the list may only shrink.
    /// `ROADMAP.md` carries the routing work.
    const UNROUTED_TRIGGERS: &[u32] = &[
        // GPKE Teil 2 § 3.1 — Rückmeldung / Bestellung Abrechnungsdaten.
        55_156, 55_220, 55_673,
        // GPKE Teil 4 — Stammdatenänderungen missing from `STAMMDATEN_PAIRS`.
        55_230, 55_557,
    ];

    /// Every listed trigger must be an inbound PID of a registered workflow —
    /// `makod` only emits `process.initiated` for messages it spawns from, so a
    /// table entry keyed on an answer PID would describe a clock that never
    /// starts.
    ///
    /// This is the check the table cannot make about itself, which is why it
    /// stayed here when the data moved to `mako-fristen`.
    #[test]
    fn every_trigger_is_an_inbound_pid() {
        let inbound: std::collections::BTreeSet<u32> = crate::UTILMD_ANFRAGE_PIDS
            .iter()
            .copied()
            .chain(crate::LF_ABMELDUNG_PIDS.iter().copied())
            .chain(crate::BEENDIGUNG_ZUORDNUNG_PIDS.iter().copied())
            .chain(crate::kuendigung::KUENDIGUNG_PIDS.iter().copied())
            .chain(crate::neuanlage::NEUANLAGE_PIDS.iter().copied())
            .chain(crate::sperrung::SPERRUNG_PIDS.iter().copied())
            .chain(crate::sperrung::ORDCHG_STORNIERUNG_PIDS.iter().copied())
            .chain(
                crate::stammdatenaenderung::STAMMDATEN_PAIRS
                    .iter()
                    .map(|(aenderung, _, _)| *aenderung),
            )
            .chain(UNROUTED_TRIGGERS.iter().copied())
            .collect();
        for o in ANTWORT_OBLIGATIONS {
            assert!(
                inbound.contains(&o.trigger_pid),
                "{} ({}) is not an inbound PID of any registered workflow",
                o.trigger_pid,
                o.name
            );
        }
    }

    /// The unrouted list may only shrink: once a workflow spawns from one of
    /// these, its entry here is stale and must go.
    #[test]
    fn the_unrouted_list_only_shrinks() {
        let routed: std::collections::BTreeSet<u32> = crate::UTILMD_ANFRAGE_PIDS
            .iter()
            .copied()
            .chain(crate::LF_ABMELDUNG_PIDS.iter().copied())
            .chain(crate::BEENDIGUNG_ZUORDNUNG_PIDS.iter().copied())
            .chain(crate::kuendigung::KUENDIGUNG_PIDS.iter().copied())
            .chain(crate::neuanlage::NEUANLAGE_PIDS.iter().copied())
            .chain(crate::sperrung::SPERRUNG_PIDS.iter().copied())
            .chain(crate::sperrung::ORDCHG_STORNIERUNG_PIDS.iter().copied())
            .chain(
                crate::stammdatenaenderung::STAMMDATEN_PAIRS
                    .iter()
                    .map(|(aenderung, _, _)| *aenderung),
            )
            .collect();
        for pid in UNROUTED_TRIGGERS {
            assert!(
                !routed.contains(pid),
                "PID {pid} is routed now — drop it from UNROUTED_TRIGGERS"
            );
        }
    }

    /// A trigger must never be an **answer** PID: an answer discharges a Frist,
    /// it does not start one, and `makod` never spawns a process from it.
    #[test]
    fn no_trigger_is_an_answer_pid() {
        let answers: std::collections::BTreeSet<u32> = ANTWORT_OBLIGATIONS
            .iter()
            .flat_map(|o| [o.antwort_pids.0, o.antwort_pids.1])
            .collect();
        for o in ANTWORT_OBLIGATIONS {
            assert!(
                !answers.contains(&o.trigger_pid),
                "{} ({}) is an answer PID of another obligation",
                o.trigger_pid,
                o.name
            );
        }
    }

    /// The answer PIDs must agree with the workflow's own derivation, which is
    /// what actually goes on the wire.
    #[test]
    fn answer_pids_match_the_workflow_derivation() {
        for o in ANTWORT_OBLIGATIONS {
            let Some(accept) = crate::wechselprozesse::response_pid_for(o.trigger_pid, true) else {
                continue; // not a supplier-change PID
            };
            let reject = crate::wechselprozesse::response_pid_for(o.trigger_pid, false)
                .expect("a PID with a positive answer has a negative one");
            assert_eq!(
                (accept.as_u32(), reject.as_u32()),
                o.antwort_pids,
                "answer PIDs for {} disagree with response_pid_for",
                o.trigger_pid
            );
        }
    }

    /// The GPKE view is the GPKE table and nothing else: a Gas or WiM PID must
    /// not resolve through this module.
    #[test]
    fn the_gpke_view_excludes_the_other_families() {
        for foreign in [44_001_u32, 55_039, 44_042, 35_001] {
            assert!(
                antwort_obligation(foreign).is_none(),
                "PID {foreign} is not GPKE"
            );
        }
    }

    /// A Friday Anmeldung is answerable on Monday morning, not on Saturday
    /// afternoon.
    #[test]
    fn a_friday_anmeldung_is_due_monday_1100_berlin() {
        let due = antwort_deadline(55_001, utc(2025, Month::January, 10, 13)).expect("known PID");
        assert_eq!(
            due.date(),
            Date::from_calendar_date(2025, Month::January, 13).expect("valid date")
        );
        assert_eq!(due.to_offset(time::UtcOffset::UTC).hour(), 10);
    }

    /// The Abmeldung window (06:00) is strictly tighter than the Anmeldung one
    /// (11:00) for the same arrival — sizing both alike loses five hours on the
    /// one that matters.
    #[test]
    fn the_abmeldung_window_is_tighter_than_the_anmeldung_window() {
        let received = utc(2025, Month::January, 13, 8);
        assert!(
            antwort_deadline(55_004, received) < antwort_deadline(55_001, received),
            "55004 is due 06:00, 55001 is due 11:00 of the same Werktag"
        );
    }

    /// An unlisted PID is unknown, not unbounded.
    #[test]
    fn an_unlisted_pid_has_no_deadline() {
        assert!(antwort_deadline(55_699, OffsetDateTime::now_utc()).is_none());
    }
}
