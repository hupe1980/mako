//! GPKE Strom **business answer Fristen**, per inbound Prüfidentifikator.
//!
//! BK6-24-174 GPKE Teil 2 states every window as a wall-clock instant in German
//! local time on the first Werktag *after* the Übertragungstag: 11:00 Uhr for a
//! Lieferbeginn, 06:00 for an Abmeldung, 05:00 for the NB-seitiges Lieferende,
//! 09:00 for the Anfrage zur Beendigung der Zuordnung.
//!
//! It is not a duration. A message arriving Friday afternoon is answerable until
//! Monday; one arriving Tuesday evening has under sixteen hours. Any flat-window
//! approximation is therefore both too tight and too loose, and the loose
//! direction is silent — it reports a lapsed Frist as still running.
//!
//! A **separate** 45-minute clock runs on the same message for the technical
//! acknowledgement ([`mako_engine::fristen::aperak_strom_due_at`]).
//!
//! A PID absent from [`ANTWORT_OBLIGATIONS`] is one whose Frist this codebase
//! has not read out of the Festlegung. Treat [`antwort_deadline`] returning
//! `None` as *unknown*, never as *no deadline*.
//!
//! # Sources
//!
//! - BK6-24-174 GPKE Teil 2 (Lesefassung) — the SD Fristen per Prozessschritt
//! - EDI@Energy Anwendungsübersicht der Prüfidentifikatoren 4.0 — roles, EBDs
//! - Entscheidungsbaum-Diagramme und Codelisten 4.3

use mako_engine::fristen::{self, HolidayCalendar};
use time::{OffsetDateTime, Time};

/// The shape of a GPKE answer Frist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FristShape {
    /// „Unverzüglich, jedoch spätester ÜZ ist `HH:MM` Uhr des 1. WT nach dem ÜT."
    ///
    /// A wall-clock instant in German local time on the first Werktag strictly
    /// after the arrival day.
    NextWerktagAt(Time),
    /// „Unverzüglich, jedoch spätester ÜT ist der `n`. WT nach dem ÜT."
    ///
    /// Day-granular: the Frist runs to the end of that Werktag.
    EndOfWerktag(u32),
}

impl FristShape {
    /// Resolve this Frist against the instant the message arrived.
    #[must_use]
    pub fn due_at(self, received: OffsetDateTime, cal: HolidayCalendar) -> OffsetDateTime {
        match self {
            Self::NextWerktagAt(at) => fristen::next_werktag_at(received, at, cal),
            Self::EndOfWerktag(n) => fristen::end_of_werktag_after(received, n, cal),
        }
    }
}

/// One inbound PID's answer obligation.
#[derive(Debug, Clone, Copy)]
pub struct AntwortObligation {
    /// The **inbound** Prüfidentifikator that starts the clock. Never an answer
    /// PID — `makod` only spawns a process from an inbound message.
    pub trigger_pid: u32,
    /// Human-readable process name, for operator-facing queue reasons and logs.
    pub name: &'static str,
    /// The Marktrolle that owes the answer, as the Anwendungsübersicht names it
    /// (`"NB"`, `"LF"`, `"LFA"`).
    pub answered_by: &'static str,
    /// Outbound PIDs carrying the positive and negative answer.
    pub antwort_pids: (u32, u32),
    /// The Entscheidungsbaum that decides the answer, where one is published.
    pub ebd: Option<&'static str>,
    /// The window shape.
    pub frist: FristShape,
    /// Citation for the Frist, for the audit trail.
    pub source: &'static str,
}

const fn at(hour: u8) -> Time {
    match Time::from_hms(hour, 0, 0) {
        Ok(t) => t,
        Err(_) => panic!("whole hour is a valid Time"),
    }
}

/// Every GPKE Strom inbound PID whose answer Frist is stated in Teil 2.
///
/// | PID | Process | Answerer | Frist |
/// |---|---|---|---|
/// | 55001 | Anmeldung verb. MaLo | NB | 11:00 des 1. WT nach dem ÜT |
/// | 55077 | Anmeldung erz. MaLo | NB | 11:00 des 1. WT nach dem ÜT |
/// | 55004 | Abmeldung (Lieferende von LF an NB) | NB | 06:00 des 1. WT nach dem ÜT |
/// | 55007 | Ankündigung der Beendigung der Zuordnung | LF | 05:00 des 1. WT nach dem ÜT |
/// | 55010 | Anfrage zur Beendigung der Zuordnung | LFA | 09:00 des 1. WT nach dem ÜT |
/// | 55016 | Kündigung | LFA | Ablauf des 1. WT nach dem ÜT |
pub const ANTWORT_OBLIGATIONS: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 55_001,
        name: "Anmeldung verb. Marktlokation (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (55_002, 55_003),
        ebd: Some("E_0622"),
        frist: FristShape::NextWerktagAt(at(11)),
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
    },
    AntwortObligation {
        trigger_pid: 55_077,
        name: "Anmeldung erz. Marktlokation (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (55_078, 55_080),
        ebd: Some("E_0622"),
        frist: FristShape::NextWerktagAt(at(11)),
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
    },
    AntwortObligation {
        trigger_pid: 55_004,
        name: "Abmeldung (Lieferende von LF an NB)",
        answered_by: "NB",
        antwort_pids: (55_005, 55_006),
        ebd: Some("E_0607"),
        frist: FristShape::NextWerktagAt(at(6)),
        source: "BK6-24-174 GPKE Teil 2, SD Lieferende von LF an NB Prozessschritte 2/3",
    },
    AntwortObligation {
        trigger_pid: 55_007,
        name: "Ankündigung der Beendigung der Zuordnung (Lieferende von NB an LF)",
        answered_by: "LF",
        antwort_pids: (55_008, 55_009),
        ebd: Some("E_0609"),
        frist: FristShape::NextWerktagAt(at(5)),
        source: "BK6-24-174 GPKE Teil 2, SD Lieferende von NB an LF Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_010,
        name: "Anfrage zur Beendigung der Zuordnung",
        answered_by: "LFA",
        antwort_pids: (55_011, 55_012),
        ebd: Some("E_0624"),
        frist: FristShape::NextWerktagAt(at(9)),
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritt 4",
    },
    AntwortObligation {
        trigger_pid: 55_016,
        name: "Kündigung",
        answered_by: "LFA",
        antwort_pids: (55_017, 55_018),
        ebd: Some("E_0614"),
        frist: FristShape::EndOfWerktag(1),
        source: "BK6-24-174 GPKE Teil 2, SD Kündigung Prozessschritt 2",
    },
];

/// The answer obligation for an inbound GPKE Strom PID, if this codebase has
/// read one out of the Festlegung.
#[must_use]
pub fn antwort_obligation(trigger_pid: u32) -> Option<&'static AntwortObligation> {
    ANTWORT_OBLIGATIONS
        .iter()
        .find(|o| o.trigger_pid == trigger_pid)
}

/// The instant by which the answer to `trigger_pid` must have been sent.
///
/// `received` is the arrival instant of the inbound message (the ÜT). Returns
/// `None` for a PID not in [`ANTWORT_OBLIGATIONS`] — treat that as *unknown*,
/// never as *unbounded*.
#[must_use]
pub fn antwort_deadline(trigger_pid: u32, received: OffsetDateTime) -> Option<OffsetDateTime> {
    antwort_obligation(trigger_pid).map(|o| o.frist.due_at(received, HolidayCalendar::BdewMaKo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};

    fn utc(y: i32, m: Month, d: u8, h: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(y, m, d).expect("valid date"),
            Time::from_hms(h, 0, 0).expect("valid time"),
        )
    }

    /// Every listed trigger must be an inbound PID of a registered workflow —
    /// `makod` only emits `process.initiated` for messages it spawns from, so a
    /// table entry keyed on an answer PID would describe a clock that never
    /// starts.
    #[test]
    fn every_trigger_is_an_inbound_pid() {
        let inbound: std::collections::BTreeSet<u32> = crate::UTILMD_ANFRAGE_PIDS
            .iter()
            .copied()
            .chain(crate::LF_ABMELDUNG_PIDS.iter().copied())
            .chain(crate::BEENDIGUNG_ZUORDNUNG_PIDS.iter().copied())
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

    /// No trigger may also appear as one of the answers — that inversion is the
    /// recurring failure mode these tables exist to prevent.
    #[test]
    fn no_trigger_is_also_an_answer() {
        let answers: std::collections::BTreeSet<u32> = ANTWORT_OBLIGATIONS
            .iter()
            .flat_map(|o| [o.antwort_pids.0, o.antwort_pids.1])
            .collect();
        for o in ANTWORT_OBLIGATIONS {
            assert!(
                !answers.contains(&o.trigger_pid),
                "{} is listed both as a trigger and as an answer",
                o.trigger_pid
            );
        }
    }

    #[test]
    fn triggers_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for o in ANTWORT_OBLIGATIONS {
            assert!(
                seen.insert(o.trigger_pid),
                "duplicate trigger {}",
                o.trigger_pid
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
        assert!(antwort_deadline(55_557, OffsetDateTime::now_utc()).is_none());
    }
}
