//! Exact-value deadline UTC assertions for mako-engine `fristen`.
//!
//! These tests assert the **exact UTC output** of the deadline helpers at
//! specific Berlin local times — including around DST transitions.  They are
//! the primary guard against off-by-one-hour errors at the CET/CEST boundary
//! (UTC+1 ↔ UTC+2), which is a regulatory deadline violation (BNetzA §5).
//!
//! All expected values are pre-computed from the BDEW Allgemeine Festlegungen
//! and verified against the German DST schedule:
//!
//! - Last Sunday of March:  clocks spring forward at 02:00 CET → 03:00 CEST
//! - Last Sunday of October: clocks fall back at 03:00 CEST → 02:00 CET
//!
//! In 2025 and 2026 the transitions are:
//! - 2025-03-30 02:00 CET → 03:00 CEST (spring forward)
//! - 2025-10-26 03:00 CEST → 02:00 CET (fall back)
//! - 2026-03-29 02:00 CET → 03:00 CEST (spring forward)
//! - 2026-10-25 03:00 CEST → 02:00 CET (fall back)

use mako_fristen::{
    HolidayCalendar, add_hours, add_werktage, aperak_strom_due_at, deadline_at_werktage,
};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};
use time_tz::{OffsetDateTimeExt, timezones};

// ─────────────────────────────────────────────────────────────────────────────
// Helper: construct a UTC OffsetDateTime from date+time components
// ─────────────────────────────────────────────────────────────────────────────

fn utc(year: i32, month: Month, day: u8, hour: u8, minute: u8) -> OffsetDateTime {
    PrimitiveDateTime::new(
        Date::from_calendar_date(year, month, day).unwrap(),
        Time::from_hms(hour, minute, 0).unwrap(),
    )
    .assume_offset(UtcOffset::UTC)
}

// ─────────────────────────────────────────────────────────────────────────────
// add_hours: GPKE 24-hour wall-clock deadline
// ─────────────────────────────────────────────────────────────────────────────

/// `add_hours` is pure wall-clock arithmetic.
///
/// An instant 24 hours after 10:00 UTC on a Monday is 10:00 UTC on Tuesday, with
/// no timezone conversion involved.
///
/// **This is not a GPKE window.** No GPKE Festlegung publishes a 24-hour answer
/// Frist — Teil 2 states every one as a wall-clock instant on the first Werktag
/// after the ÜT, and Teil 1 Kap. 7 defines no duration at all. `add_hours` is a
/// generic helper; the AWH GeLi Gas Prozessschritt-5 sub-window is the one place
/// in these families where 24 hours is a real number, and it lives in
/// `mako_fristen::antwort::gas_lieferbeginn_antwort_nach_abmeldeanfrage`.
#[test]
fn add_hours_is_exact_across_a_normal_day() {
    let received = utc(2025, Month::October, 6, 10, 0);
    let due = add_hours(received, 24);
    let expected = utc(2025, Month::October, 7, 10, 0);
    assert_eq!(
        due, expected,
        "add_hours must be exactly 24 wall-clock hours after receipt"
    );
}

/// `add_hours` does NOT shift at DST boundaries. An instant 24 hours after
/// 10:00 UTC the night before spring-forward is 24 UTC hours later, not 23 or 25.
#[test]
fn add_hours_is_exact_across_spring_forward() {
    // 2025-03-29 10:00 UTC = 11:00 CET, night before spring-forward
    let received = utc(2025, Month::March, 29, 10, 0);
    let due = add_hours(received, 24);
    let expected = utc(2025, Month::March, 30, 10, 0);
    assert_eq!(
        due, expected,
        "add_hours must be exactly 24 UTC hours across spring-forward"
    );
}

/// Same across fall-back (2025-10-26).
#[test]
fn add_hours_is_exact_across_fall_back() {
    // 2025-10-25 10:00 UTC = 12:00 CEST, night before fall-back
    let received = utc(2025, Month::October, 25, 10, 0);
    let due = add_hours(received, 24);
    let expected = utc(2025, Month::October, 26, 10, 0);
    assert_eq!(
        due, expected,
        "add_hours must be exactly 24 UTC hours across fall-back"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// add_werktage: calendar date arithmetic
// ─────────────────────────────────────────────────────────────────────────────

/// WiM 5-Werktage process Frist: Monday Jan 5 (2026) + 5 WT.
/// 2026-01-06 (Heilige Drei Könige) is a BDEW MaKo holiday and Sat/Sun are not
/// Werktage, so the five are Wed 7, Thu 8, Fri 9, Mon 12, Tue 13.
#[test]
fn add_werktage_5wt_skips_holiday_and_weekend() {
    let start = Date::from_calendar_date(2026, Month::January, 5).unwrap();
    let due = add_werktage(start, 5, HolidayCalendar::BdewMaKo);
    let expected = Date::from_calendar_date(2026, Month::January, 13).unwrap();
    assert_eq!(
        due, expected,
        "Monday + 5 WT (Heilige Drei Könige skipped, Sat/Sun not Werktage) = Tue Jan 13"
    );
}

/// GeLi Gas 10-Werktage Frist: Monday Jan 5 + 10 WT.
/// Jan 6 = Heilige Drei Könige (skipped); weekends are not Werktage.
/// Werktage: Wed 7, Thu 8, Fri 9, Mon 12, Tue 13, Wed 14, Thu 15, Fri 16, Mon 19, Tue 20.
#[test]
fn add_werktage_10wt_monday_two_weeks() {
    let start = Date::from_calendar_date(2026, Month::January, 5).unwrap();
    let due = add_werktage(start, 10, HolidayCalendar::BdewMaKo);
    let expected = Date::from_calendar_date(2026, Month::January, 20).unwrap();
    assert_eq!(
        due, expected,
        "Monday + 10 WT (Heilige Drei Könige skipped) = Tue Jan 20"
    );
}

/// Weekends and holidays are skipped — 5 WT starting on Saturday Jan 3 (2026).
/// Jan 4 (Sun, skip), Jan 5 (Mon=1), Jan 6 (holiday, skip), Jan 7 (Wed=2),
/// Jan 8 (Thu=3), Jan 9 (Fri=4), Jan 10/11 (weekend, skip), Jan 12 (Mon=5).
#[test]
fn add_werktage_skips_weekends_and_holidays() {
    let start = Date::from_calendar_date(2026, Month::January, 3).unwrap();
    let due = add_werktage(start, 5, HolidayCalendar::BdewMaKo);
    let expected = Date::from_calendar_date(2026, Month::January, 12).unwrap();
    assert_eq!(
        due, expected,
        "add_werktage must skip Saturdays, Sundays and holidays"
    );
}

/// Neujahr (2026-01-01) is a holiday — starting on New Year's Eve (Wed) + 1 WT
/// must skip to 2026-01-02 (Fri), not to 2026-01-01.
#[test]
fn add_werktage_skips_neujahr() {
    // 2025-12-31 (Wednesday) + 1 WT → must skip 2026-01-01 (holiday) → 2026-01-02 (Thu)
    let start = Date::from_calendar_date(2025, Month::December, 31).unwrap();
    let due = add_werktage(start, 1, HolidayCalendar::BdewMaKo);
    let expected = Date::from_calendar_date(2026, Month::January, 2).unwrap();
    assert_eq!(due, expected, "add_werktage must skip Neujahr (2026-01-01)");
}

// ─────────────────────────────────────────────────────────────────────────────
// deadline_at_werktage: exact UTC output including 17:00 Berlin local time
// ─────────────────────────────────────────────────────────────────────────────

/// A Werktage Frist runs to the **end** of the due Werktag, and the offset it
/// carries is the one in force in Berlin **on that date** — not on the day the
/// message arrived.
///
/// Both halves matter. The rulebook states this Frist shape as a day („Ablauf
/// des n. WT", „spätester ÜT ist der n. WT"), so cutting it at an
/// end-of-business hour expires it early and escalates a counterparty still
/// inside its window; and resolving the offset against the arrival date instead
/// of the due date is the off-by-one-hour error these tests exist for.
///
/// Winter: Mon 2026-01-05 + 5 WT, skipping Heilige Drei Könige (Jan 6) → due
/// Tue 2026-01-13, CET.
#[test]
fn deadline_at_werktage_5wt_winter_cet() {
    let received = utc(2026, Month::January, 5, 9, 0);
    let due = deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo);

    assert_eq!(
        mako_fristen::berlin_date(due),
        Date::from_calendar_date(2026, Month::January, 13).unwrap(),
        "5 WT with Heilige Drei Könige skipped is Tue 2026-01-13"
    );
    assert_eq!(
        due.to_timezone(timezones::db::europe::BERLIN).offset(),
        UtcOffset::from_hms(1, 0, 0).unwrap(),
        "January is CET (UTC+1)"
    );
    assert_eq!(
        due,
        mako_fristen::end_of_werktag_after(received, 5, HolidayCalendar::BdewMaKo),
        "the window runs to the end of the Werktag, not to a cut-off hour"
    );
}

/// Summer: Wed 2025-06-04 + 10 WT, skipping Pfingstmontag (Jun 9) → due
/// Fri 2025-06-20, CEST.
#[test]
fn deadline_at_werktage_10wt_summer_cest() {
    let received = utc(2025, Month::June, 4, 9, 0);
    let due = deadline_at_werktage(received, 10, HolidayCalendar::BdewMaKo);

    assert_eq!(
        mako_fristen::berlin_date(due),
        Date::from_calendar_date(2025, Month::June, 20).unwrap(),
        "10 WT with Pfingstmontag skipped is Fri 2025-06-20"
    );
    assert_eq!(
        due.to_timezone(timezones::db::europe::BERLIN).offset(),
        UtcOffset::from_hms(2, 0, 0).unwrap(),
        "June is CEST (UTC+2)"
    );
    assert_eq!(
        due,
        mako_fristen::end_of_werktag_after(received, 10, HolidayCalendar::BdewMaKo)
    );
}

/// The **due** date decides the offset, not the received date.
///
/// Received Mon 2025-03-24, still CET; +10 Werktage lands on Mon 2025-04-07,
/// after the 2025-03-30 spring-forward, so the deadline is resolved against
/// CEST. Resolving it against the arrival date's CET would move the instant by
/// an hour.
#[test]
fn deadline_at_werktage_due_date_drives_offset() {
    let received = utc(2025, Month::March, 24, 9, 0);
    let due = deadline_at_werktage(received, 10, HolidayCalendar::BdewMaKo);

    assert_eq!(
        mako_fristen::berlin_date(due),
        Date::from_calendar_date(2025, Month::April, 7).unwrap()
    );
    assert_eq!(
        due.to_timezone(timezones::db::europe::BERLIN).offset(),
        UtcOffset::from_hms(2, 0, 0).unwrap(),
        "the due date is past spring-forward, so CEST — even though arrival was CET"
    );
}

/// A window spanning the spring-forward boundary (2025-03-30) resolves on the
/// due date: received Thu 2025-03-27 (CET), +5 WT → Thu 2025-04-03 (CEST).
#[test]
fn deadline_at_werktage_crosses_spring_forward_2025() {
    let received = utc(2025, Month::March, 27, 9, 0);
    let due = deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo);

    assert_eq!(
        mako_fristen::berlin_date(due),
        Date::from_calendar_date(2025, Month::April, 3).unwrap()
    );
    assert_eq!(
        due.to_timezone(timezones::db::europe::BERLIN).offset(),
        UtcOffset::from_hms(2, 0, 0).unwrap(),
        "CEST on the due date"
    );
}

/// And one spanning the fall-back boundary (2025-10-26): received Fri
/// 2025-10-24 (CEST), +5 WT → Mon 2025-11-03 (CET).
#[test]
fn deadline_at_werktage_crosses_fall_back_2025() {
    let received = utc(2025, Month::October, 24, 9, 0);
    let due = deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo);

    assert_eq!(
        mako_fristen::berlin_date(due),
        Date::from_calendar_date(2025, Month::November, 3).unwrap()
    );
    assert_eq!(
        due.to_timezone(timezones::db::europe::BERLIN).offset(),
        UtcOffset::from_hms(1, 0, 0).unwrap(),
        "CET on the due date"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// aperak_strom_due_at: exact UTC assertions
// ─────────────────────────────────────────────────────────────────────────────

/// APERAK 45-minute window on a normal weekday: exact UTC output.
#[test]
fn aperak_strom_due_at_weekday_exact_utc() {
    // Monday 2025-10-06 09:00 UTC: due 09:45 UTC
    let received = utc(2025, Month::October, 6, 9, 0);
    let due = aperak_strom_due_at(received);
    let expected = utc(2025, Month::October, 6, 9, 45);
    assert_eq!(
        due, expected,
        "APERAK weekday due_at must be exactly received + 45 minutes"
    );
}

/// APERAK 45-minute window on Saturday must be Sunday noon Berlin time.
/// Saturday 2025-01-04 (CET, UTC+1): Sunday noon CET = 11:00 UTC.
#[test]
fn aperak_strom_due_at_saturday_winter_is_sunday_noon_utc() {
    // Saturday 2025-01-04 20:00 UTC = 21:00 CET
    let received = utc(2025, Month::January, 4, 20, 0);
    let due = aperak_strom_due_at(received);
    // Sunday 2025-01-05 12:00 CET = 11:00 UTC
    let expected = utc(2025, Month::January, 5, 11, 0);
    assert_eq!(
        due, expected,
        "APERAK Saturday CET: due must be Sunday 12:00 CET = 11:00 UTC"
    );
}

/// APERAK 45-minute window on Saturday in summer (CEST, UTC+2):
/// Sunday noon CEST = 10:00 UTC.
#[test]
fn aperak_strom_due_at_saturday_summer_is_sunday_noon_cest_utc() {
    // Saturday 2025-07-05 20:00 UTC = 22:00 CEST
    let received = utc(2025, Month::July, 5, 20, 0);
    let due = aperak_strom_due_at(received);
    // Sunday 2025-07-06 12:00 CEST = 10:00 UTC
    let expected = utc(2025, Month::July, 6, 10, 0);
    assert_eq!(
        due, expected,
        "APERAK Saturday CEST: due must be Sunday 12:00 CEST = 10:00 UTC"
    );
}
