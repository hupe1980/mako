//! mako's Werktage, as agentplane's calendar.
//!
//! agentplane resolves every obligation through a [`Calendar`] and journals the
//! instant it produced together with the calendar's digest. The built-in
//! `WallClock` understands `minutes`, `hours` and `days` and deliberately
//! refuses to guess at working days — which is the honest position for a
//! domain-agnostic crate and useless for ours, where almost every deadline is
//! stated in *Werktage*.
//!
//! This adapter supplies the missing kind. `working-days` resolves through
//! [`mako_fristen`] — the same BDEW MaKo holiday table that computes an
//! APERAK Frist, so an agent's approval window and the regulatory window it
//! guards cannot disagree about when Karfreitag is.
//!
//! ## What the digest covers
//!
//! [`Calendar::digest`] identifies the *ruleset*, and agentplane records it on
//! every deadline it resolves. Ours names the holiday table and a version. Two
//! consequences worth stating, because both are the point:
//!
//! * A resolved deadline is read back from the journal on replay, never
//!   recomputed — so correcting the holiday table cannot retroactively move a
//!   window somebody already relied on.
//! * Changing the rules means changing the ruleset string this module hashes,
//!   which makes the shift visible in the journal as a different digest rather
//!   than as a silent re-interpretation.

use agentplane::core::{Calendar, CalendarError, DeadlineSpec, Digest, Timestamp, WallClock};
use mako_fristen::{self as fristen, HolidayCalendar};

/// Names the rules this calendar applies. Bump the version on any change to
/// the holiday table, the cut-off hour or the timezone.
const RULESET: &[u8] = b"mako/werktage/bdew-mako/17:00 Europe-Berlin/v1";

/// The deadline kind this adds to the built-in set.
const WORKING_DAYS: &str = "working-days";

/// BDEW MaKo Werktage for agentplane's obligations.
///
/// Unknown kinds fall through to [`WallClock`], so `hours` and `minutes` keep
/// working — a 45-minute APERAK window is a wall-clock window, and forcing it
/// through a working-day rule would be wrong in the other direction.
#[derive(Debug, Clone, Copy, Default)]
pub struct MakoCalendar;

impl MakoCalendar {
    /// `n`, the count every kind here takes.
    ///
    /// Refused rather than defaulted: a `working-days` spec with no `n` is a
    /// manifest that does not say how long the window is, and picking a number
    /// for it would be inventing a regulatory deadline.
    fn count(spec: &DeadlineSpec) -> Result<u32, CalendarError> {
        spec.params
            .get("n")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| CalendarError::BadParams {
                kind: spec.kind.clone(),
                detail: "expected a non-negative integer field `n`".into(),
            })
    }
}

impl Calendar for MakoCalendar {
    fn resolve(&self, from: Timestamp, spec: &DeadlineSpec) -> Result<Timestamp, CalendarError> {
        if spec.kind == WORKING_DAYS {
            // 17:00 Europe/Berlin on the n-th Werktag, computed in local time —
            // the DST handling is `fristen`'s, so an agent deadline and an
            // APERAK deadline land on the same instant on 30 March.
            return Ok(fristen::deadline_at_werktage(
                from,
                Self::count(spec)?,
                HolidayCalendar::BdewMaKo,
            ));
        }
        WallClock.resolve(from, spec)
    }

    fn digest(&self) -> Digest {
        Digest::of(RULESET)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::{Date, Month, OffsetDateTime, Time};

    fn at(y: i32, m: Month, d: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(y, m, d).expect("date"),
            Time::MIDNIGHT,
        )
    }

    /// A working-day window skips the weekend, and lands at the German cut-off.
    ///
    /// The failure this prevents is the one that makes a domain calendar worth
    /// wiring at all: `WallClock` would answer "Monday + 5 days = Saturday",
    /// which is not a Frist any AHB states.
    #[test]
    fn five_working_days_from_a_monday_is_the_next_monday() {
        let spec = DeadlineSpec::new(WORKING_DAYS, json!({ "n": 5 }));
        let due = MakoCalendar
            .resolve(at(2025, Month::January, 6), &spec)
            .expect("resolves");

        assert_eq!(
            due.date(),
            Date::from_calendar_date(2025, Month::January, 13).expect("date"),
            "Tue–Fri then Monday: the weekend is not a Werktag"
        );
        assert_eq!(due.hour(), 17, "17:00 Europe/Berlin is the MaKo cut-off");
    }

    /// The wall-clock kinds still work — a 45-minute APERAK window is minutes.
    #[test]
    fn wall_clock_kinds_fall_through_unchanged() {
        let from = at(2026, Month::August, 9);
        let due = MakoCalendar
            .resolve(from, &DeadlineSpec::new("minutes", json!({ "n": 45 })))
            .expect("resolves");
        assert_eq!(due - from, time::Duration::minutes(45));
    }

    /// A kind nobody implements is refused rather than silently treated as now.
    #[test]
    fn an_unknown_kind_is_an_error() {
        let err = MakoCalendar
            .resolve(
                at(2026, Month::August, 9),
                &DeadlineSpec::new("fortnights", json!({ "n": 1 })),
            )
            .expect_err("unknown kind");
        assert!(matches!(err, CalendarError::UnknownKind(_)));
    }

    /// `working-days` without `n` is refused, not defaulted.
    #[test]
    fn a_working_day_spec_without_a_count_is_refused() {
        let err = MakoCalendar
            .resolve(
                at(2026, Month::August, 9),
                &DeadlineSpec::new(WORKING_DAYS, json!({})),
            )
            .expect_err("no count");
        assert!(matches!(err, CalendarError::BadParams { .. }));
    }

    /// The digest is stable, and it is not the built-in calendar's.
    ///
    /// Stability is what makes a journaled deadline auditable; distinctness is
    /// what stops a `WallClock`-resolved instant reading as if these rules
    /// produced it.
    #[test]
    fn the_ruleset_digest_is_stable_and_distinct() {
        assert_eq!(MakoCalendar.digest(), MakoCalendar.digest());
        assert_ne!(MakoCalendar.digest(), WallClock.digest());
    }
}
