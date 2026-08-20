//! GeLi Gas answer Fristen — the table itself now lives in
//! [`mako_fristen::antwort`], and what stays here is the check that it
//! agrees with these workflows.
//!
//! ⚠️ The **10 Werktage** quoted for a Lieferantenwechsel is the *supplier's*
//! minimum lead time („mindestens 10 Werktage vor Aufnahme der Belieferung",
//! Kap. 3.2.3) — how far ahead the LF must send. The Netzbetreiber's answer
//! window for the same message is **4 Werktage**. `obsd` computed every Gas
//! breach alert from the lead time, six Werktage late.
//!
//! Only PIDs the Festlegung quantifies are listed; the Abmeldungsanfrage
//! (44010), the GNB-initiated Abmeldung NN (44007) and the Änderungsmeldung
//! (44020) have Fristen set per Netzbetreiber under Kap. 2.6, so
//! [`antwort_deadline`] returns `None` — *unknown*, never *unbounded*.

pub use mako_fristen::antwort::{AntwortObligation, GELI_GAS as ANTWORT_OBLIGATIONS};

use mako_fristen::antwort;
use time::OffsetDateTime;

/// The answer obligation for an inbound GeLi Gas PID, where the Festlegung
/// quantifies one.
#[must_use]
pub fn antwort_obligation(trigger_pid: u32) -> Option<&'static AntwortObligation> {
    antwort::antwort_obligation(trigger_pid).filter(|o| o.family == antwort::Family::GeliGas)
}

/// The instant by which the answer to `trigger_pid` must have been sent.
///
/// `received` is the arrival instant of the inbound message. Returns `None` for
/// a PID not in [`ANTWORT_OBLIGATIONS`].
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

    /// Every listed trigger must be one the workflow actually spawns from.
    ///
    /// This is the check the table cannot make about itself, which is why it
    /// stayed here when the data moved to `mako-fristen`.
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

    /// The GeLi Gas view is the GeLi Gas table and nothing else — in particular
    /// not the WiM Gas PIDs, which sit in the same 44xxx space at 10 Werktage.
    #[test]
    fn the_geli_gas_view_excludes_wim_gas() {
        for foreign in [44_039_u32, 44_042, 44_051, 44_168, 55_001] {
            assert!(
                antwort_obligation(foreign).is_none(),
                "PID {foreign} is not GeLi Gas"
            );
        }
    }

    /// A Monday Anmeldung is answerable by the 4th Werktag — not the 10th.
    #[test]
    fn the_anmeldung_is_four_werktage_not_the_suppliers_ten() {
        let received = utc(2026, Month::March, 2, 9); // Monday
        let due = antwort_deadline(44_001, received).expect("known PID");
        assert_eq!(
            due.date(),
            Date::from_calendar_date(2026, Month::March, 6).expect("valid date")
        );
    }

    /// The Abmeldung (3 WT) closes before the Anmeldung (4 WT).
    #[test]
    fn the_abmeldung_window_is_tighter_than_the_anmeldung_window() {
        let received = utc(2026, Month::March, 2, 9);
        assert!(antwort_deadline(44_004, received) < antwort_deadline(44_001, received));
    }

    /// A PID whose Frist is set per Netzbetreiber is unknown, not unbounded.
    #[test]
    fn a_per_netzbetreiber_frist_is_unknown() {
        let received = utc(2026, Month::March, 2, 9);
        for pid in [44_007_u32, 44_010, 44_019, 44_020] {
            assert!(antwort_deadline(pid, received).is_none(), "PID {pid}");
        }
    }
}
