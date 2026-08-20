//! The operator window `processd` sizes its approval queue by.
//!
//! The regulatory instant itself is **not** computed here: it comes from
//! [`mako_fristen::antwort`], the one table `makod` registers deadlines from and
//! `obsd` raises breach alerts against. Three services holding three copies of
//! the same Festlegung is how they came to disagree — `obsd` carried a flat
//! 24-hour GPKE window and a flat 10-Werktage Gas window while this file already
//! read the per-PID tables.
//!
//! What stays here is the one thing that is `processd`'s own: the hour of
//! headroom between when an operator must decide and when the answer is due.

pub use mako_fristen::antwort::{
    OPERATOR_HEADROOM, OperatorWindow, antwort_deadline, operator_window,
};

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month, OffsetDateTime, Time};

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
