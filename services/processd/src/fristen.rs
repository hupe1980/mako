//! The business answer deadline for every process this deployment can be asked
//! to answer, and the operator window derived from it.
//!
//! The instant a queued decision expires at must be the one `makod` registered
//! on the process, so both read the same per-family tables rather than restating
//! them:
//!
//! | Family | Table | Shape |
//! |---|---|---|
//! | GPKE Strom | [`mako_gpke::antwort_deadline`] | „HH:00 Uhr des 1. WT nach dem ÜT" |
//! | GeLi Gas | [`mako_geli_gas::antwortfrist::antwort_deadline`] | „Ablauf des n. Werktags nach Eingang" |
//! | WiM MSB-Wechsel | [`mako_wim::antwort_frist_werktage`] | n Werktage, 17:00 Berlin |
//! | WiM Preisanfrage | [`mako_wim::preisanfrage_antwort_frist_werktage`] | 5 Werktage, 17:00 Berlin |
//!
//! None of them is a flat duration, and a flat approximation fails silently in
//! the loose direction — it reports a lapsed Frist as still running.
//!
//! A PID no table quantifies yields [`OperatorWindow::unknown`]: the entry still
//! expires, on a conservative fallback, and `is_regulatory` tells the caller the
//! instant is an operating convention rather than a Festlegung citation.

use time::{Duration, OffsetDateTime};

/// Headroom subtracted from the regulatory Frist to give the answer time to
/// reach the counterparty after an operator acts.
///
/// An operator approving at the deadline itself produces a market message that
/// arrives late; expiring the entry an hour early is the difference between a
/// tight decision and a missed obligation.
pub const OPERATOR_HEADROOM: Duration = Duration::hours(1);

/// Fallback window for a process whose Frist is not in any table.
///
/// Deliberately short. An entry that never expires is invisible in
/// `processd_approval_queue_overdue`, which is the one signal an operator has
/// that a market message is going unanswered.
const UNKNOWN_FRIST_FALLBACK: Duration = Duration::hours(24);

/// When an operator must have decided a queued process, and where that instant
/// comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorWindow {
    /// The regulatory answer deadline itself.
    pub deadline: OffsetDateTime,
    /// When the queue entry expires — `deadline` less [`OPERATOR_HEADROOM`].
    pub expires_at: OffsetDateTime,
    /// `true` when `deadline` came from a Festlegung table, `false` when it is
    /// the 24-hour operating convention [`OperatorWindow::unknown`] applies.
    pub is_regulatory: bool,
    /// Citation for `deadline`, for the operator-facing queue reason and the
    /// audit log.
    pub source: &'static str,
}

impl OperatorWindow {
    fn regulatory(deadline: OffsetDateTime, source: &'static str) -> Self {
        Self {
            deadline,
            expires_at: deadline - OPERATOR_HEADROOM,
            is_regulatory: true,
            source,
        }
    }

    /// The fallback for a PID no table quantifies.
    #[must_use]
    pub fn unknown(received: OffsetDateTime) -> Self {
        let deadline = received + UNKNOWN_FRIST_FALLBACK;
        Self {
            deadline,
            expires_at: deadline - OPERATOR_HEADROOM,
            is_regulatory: false,
            source: "no Frist published for this Prüfidentifikator — 24 h operating \
                     convention, not a regulatory deadline",
        }
    }
}

/// The operator window for an inbound PID received at `received`.
///
/// Consults the GPKE Strom, GeLi Gas and WiM Strom tables in that order and
/// falls back to [`OperatorWindow::unknown`].
#[must_use]
pub fn operator_window(trigger_pid: u32, received: OffsetDateTime) -> OperatorWindow {
    if let Some(o) = mako_gpke::antwort_obligation(trigger_pid) {
        return OperatorWindow::regulatory(
            o.frist
                .due_at(received, mako_engine::fristen::HolidayCalendar::BdewMaKo),
            o.source,
        );
    }
    if let Some(o) = mako_geli_gas::antwortfrist::antwort_obligation(trigger_pid) {
        return OperatorWindow::regulatory(
            mako_engine::fristen::end_of_werktag_after(
                received,
                o.werktage,
                mako_engine::fristen::HolidayCalendar::BdewMaKo,
            ),
            o.source,
        );
    }
    if let Some(werktage) = mako_wim::antwort_frist_werktage(trigger_pid) {
        return OperatorWindow::regulatory(
            mako_engine::fristen::deadline_at_werktage(
                received,
                werktage,
                mako_engine::fristen::HolidayCalendar::BdewMaKo,
            ),
            "WiM Strom Teil 1 — per-PID Antwortfrist (3 / 5 / 7 / 1 Werktage)",
        );
    }
    if let Some(werktage) = mako_wim::preisanfrage_antwort_frist_werktage(trigger_pid) {
        return OperatorWindow::regulatory(
            mako_engine::fristen::deadline_at_werktage(
                received,
                werktage,
                mako_engine::fristen::HolidayCalendar::BdewMaKo,
            ),
            "BK6-24-174 (WiM Strom) — REQOTE Preisanfrage, 5 Werktage",
        );
    }
    OperatorWindow::unknown(received)
}

/// The regulatory answer deadline alone, where one is published.
#[must_use]
pub fn antwort_deadline(trigger_pid: u32, received: OffsetDateTime) -> Option<OffsetDateTime> {
    let w = operator_window(trigger_pid, received);
    w.is_regulatory.then_some(w.deadline)
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

    /// Every PID a compiled role can answer must resolve to a *regulatory*
    /// window — an operating-convention fallback on a process this deployment
    /// actually runs means an unread Festlegung, not an acceptable default.
    #[test]
    fn every_answerable_pid_has_a_published_frist() {
        let received = utc(2026, Month::March, 2, 9);
        let unknown: Vec<u32> = crate::handler::answerable_pids()
            .into_iter()
            .filter(|p| !operator_window(*p, received).is_regulatory)
            .collect();
        assert!(
            unknown.is_empty(),
            "these PIDs are answered but have no published Antwortfrist: {unknown:?}"
        );
    }

    /// The headroom must never invert the window.
    #[test]
    fn the_queue_expires_before_the_deadline() {
        let received = utc(2026, Month::March, 2, 9);
        for pid in crate::handler::answerable_pids() {
            let w = operator_window(pid, received);
            assert!(w.expires_at < w.deadline, "PID {pid}");
            assert!(
                w.expires_at > received,
                "PID {pid} expires before it arrives"
            );
        }
    }

    /// The GPKE table wins over the WiM one for a PID in both — it never
    /// happens today, and this pins that it stays that way.
    #[test]
    fn gpke_and_wim_pid_spaces_do_not_overlap() {
        for o in mako_gpke::ANTWORT_OBLIGATIONS {
            assert!(
                mako_wim::antwort_frist_werktage(o.trigger_pid).is_none(),
                "PID {} is claimed by both the GPKE and the WiM table",
                o.trigger_pid
            );
        }
    }

    /// A Friday Gas Anmeldung: four Werktage, not ten, and not 24 hours.
    #[test]
    fn a_gas_anmeldung_is_four_werktage() {
        let w = operator_window(44_001, utc(2026, Month::March, 2, 9));
        assert!(w.is_regulatory);
        assert_eq!(
            w.deadline.date(),
            Date::from_calendar_date(2026, Month::March, 6).expect("valid date")
        );
    }

    #[test]
    fn an_unknown_pid_still_expires() {
        let received = utc(2026, Month::March, 2, 9);
        let w = operator_window(99_999, received);
        assert!(!w.is_regulatory);
        assert!(w.expires_at > received);
    }
}
