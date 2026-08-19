//! GeLi Gas **business answer Fristen**, per inbound Prüfidentifikator.
//!
//! Every window is „bis zum **Ablauf** des *n*. Werktags nach Eingang": the
//! arrival day does not count (GeLi Gas 3.0 Kap. 2.6, § 187 Abs. 1 BGB) and the
//! Frist closes at the end of that Werktag, not at an end-of-business hour.
//!
//! ⚠️ The **10 Werktage** quoted for a Lieferantenwechsel is the *supplier's*
//! minimum lead time („mindestens 10 Werktage vor Aufnahme der Belieferung",
//! Kap. 3.2.3) — how far ahead the LF must send. The Netzbetreiber's answer
//! window for the same message is **4 Werktage**. The Abmeldung pairs a
//! 7-Werktage lead time with a 3-Werktage answer window.
//!
//! Only PIDs the Festlegung quantifies are listed; the Abmeldungsanfrage
//! (44010), the GNB-initiated Abmeldung NN (44007) and the Änderungsmeldung
//! (44020) have Fristen set per Netzbetreiber under Kap. 2.6, so
//! [`antwort_deadline`] returns `None` — *unknown*, never *unbounded*.
//!
//! Source: BK7-24-01-009 GeLi Gas 3.0, Anlage Prozessbeschreibung, Kap. 2.6 /
//! 3.1 / 3.2.2 / 3.2.3 / 3.3.2.

use mako_engine::fristen::{self, HolidayCalendar};
use time::OffsetDateTime;

/// One inbound Gas PID's answer obligation.
#[derive(Debug, Clone, Copy)]
pub struct AntwortObligation {
    /// The **inbound** Prüfidentifikator that starts the clock.
    pub trigger_pid: u32,
    /// Human-readable process name, for operator-facing queue reasons and logs.
    pub name: &'static str,
    /// The Marktrolle that owes the answer (`"NB"`, `"LFA"`, `"E/G"`).
    pub answered_by: &'static str,
    /// Outbound PIDs carrying the positive and negative answer.
    pub antwort_pids: (u32, u32),
    /// „bis zum Ablauf des *n*. Werktags nach Eingang".
    pub werktage: u32,
    /// Citation for the Frist, for the audit trail.
    pub source: &'static str,
}

/// Every GeLi Gas inbound PID whose answer Frist the Festlegung quantifies.
///
/// | PID | Process | Answerer | Frist |
/// |---|---|---|---|
/// | 44001 | Anmeldung NN (Lieferbeginn) | NB | Ablauf des 4. WT |
/// | 44004 | Abmeldung NN (Lieferende) | NB | Ablauf des 3. WT |
/// | 44013 | Zuordnung Ersatz-/Grundversorgung | E/G | Ablauf des 2. WT |
/// | 44016 | Kündigung beim Altlieferanten | LFA | Ablauf des 3. WT |
pub const ANTWORT_OBLIGATIONS: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 44_001,
        name: "Anmeldung NN (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (44_002, 44_003),
        werktage: 4,
        source: "GeLi Gas 3.0 Kap. 3.2.3 — „spätestens bis zum Ablauf des 4. Werktages nach \
                 Eingang der Anmeldung\"",
    },
    AntwortObligation {
        trigger_pid: 44_004,
        name: "Abmeldung NN (Lieferende)",
        answered_by: "NB",
        antwort_pids: (44_005, 44_006),
        werktage: 3,
        source: "GeLi Gas 3.0 Kap. 3.2.2 — „spätestens jedoch bis zum Ablauf des 3. Werktags \
                 nach Eingang der Abmeldung\"",
    },
    AntwortObligation {
        trigger_pid: 44_013,
        name: "Zuordnung Ersatz-/Grundversorgung",
        answered_by: "E/G",
        antwort_pids: (44_014, 44_015),
        werktage: 2,
        source: "GeLi Gas 3.0 Kap. 3.3.2 — „spätestens bis zum Ablauf des 2. Werktages\"",
    },
    AntwortObligation {
        trigger_pid: 44_016,
        name: "Kündigung beim Altlieferanten",
        answered_by: "LFA",
        antwort_pids: (44_017, 44_018),
        werktage: 3,
        source: "GeLi Gas 3.0 Kap. 3.1 — „spätestens jedoch bis zum Ablauf des 3. Werktages \
                 nach Eingang der Kündigung\"",
    },
];

/// The answer obligation for an inbound GeLi Gas PID, where the Festlegung
/// quantifies one.
#[must_use]
pub fn antwort_obligation(trigger_pid: u32) -> Option<&'static AntwortObligation> {
    ANTWORT_OBLIGATIONS
        .iter()
        .find(|o| o.trigger_pid == trigger_pid)
}

/// The instant by which the answer to `trigger_pid` must have been sent.
///
/// `received` is the arrival instant of the inbound message. Returns `None` for
/// a PID not in [`ANTWORT_OBLIGATIONS`].
#[must_use]
pub fn antwort_deadline(trigger_pid: u32, received: OffsetDateTime) -> Option<OffsetDateTime> {
    antwort_obligation(trigger_pid)
        .map(|o| fristen::end_of_werktag_after(received, o.werktage, HolidayCalendar::BdewMaKo))
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

    /// Every listed trigger must be one the workflow actually spawns from.
    #[test]
    fn every_trigger_is_an_anfrage_pid() {
        for o in ANTWORT_OBLIGATIONS {
            assert!(
                crate::lieferbeginn::ANFRAGE_PIDS.contains(&o.trigger_pid),
                "{} ({}) is not a GeLi Gas Anfrage PID",
                o.trigger_pid,
                o.name
            );
        }
    }

    /// The answer PIDs must agree with the workflow's own derivation.
    #[test]
    fn answer_pids_match_the_workflow_derivation() {
        for o in ANTWORT_OBLIGATIONS {
            let accept = crate::lieferbeginn::response_pid_for(o.trigger_pid, true)
                .expect("an answerable PID has a positive answer");
            let reject = crate::lieferbeginn::response_pid_for(o.trigger_pid, false)
                .expect("an answerable PID has a negative answer");
            assert_eq!(
                (accept.as_u32(), reject.as_u32()),
                o.antwort_pids,
                "answer PIDs for {} disagree with response_pid_for",
                o.trigger_pid
            );
        }
    }

    /// The NB's answer window is four Werktage, not the ten-Werktag lead time
    /// the supplier has to observe when sending.
    #[test]
    fn the_anmeldung_window_is_four_werktage_not_ten() {
        // Monday 2025-01-13 → Tue 14, Wed 15, Thu 16, Fri 17.
        let due = antwort_deadline(44_001, utc(2025, Month::January, 13, 9)).expect("known PID");
        assert_eq!(
            due.date(),
            Date::from_calendar_date(2025, Month::January, 17).expect("valid date")
        );
    }

    /// The Frist runs to the end of the Werktag, not to an end-of-business
    /// hour: an answer sent at 18:00 on the last Werktag is still in time.
    #[test]
    fn the_window_closes_at_the_end_of_the_werktag() {
        let due = antwort_deadline(44_004, utc(2025, Month::January, 13, 9)).expect("known PID");
        assert_eq!(due.hour(), 23);
    }

    /// The Abmeldung window is tighter than the Anmeldung one — a single shared
    /// value for "Gas" loses a Werktag on it.
    #[test]
    fn the_abmeldung_window_is_tighter_than_the_anmeldung_window() {
        let received = utc(2025, Month::January, 13, 9);
        assert!(antwort_deadline(44_004, received) < antwort_deadline(44_001, received));
    }

    #[test]
    fn an_unquantified_pid_has_no_deadline() {
        // 44010 Abmeldungsanfrage — Frist set per Netzbetreiber under Kap. 2.6.
        assert!(antwort_deadline(44_010, OffsetDateTime::now_utc()).is_none());
    }
}
