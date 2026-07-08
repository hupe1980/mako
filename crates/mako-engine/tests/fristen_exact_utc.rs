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

use mako_engine::fristen::{
    HolidayCalendar, add_hours, add_werktage, aperak_strom_due_at, deadline_at_werktage,
};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

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

/// GPKE process Frist: 24 wall-clock hours.
/// A message arriving at 10:00 UTC on a Monday must be due at 10:00 UTC the
/// next day — pure wall-clock, no timezone conversion involved.
#[test]
fn add_hours_gpke_24h_normal_day() {
    let received = utc(2025, Month::October, 6, 10, 0);
    let due = add_hours(received, 24);
    let expected = utc(2025, Month::October, 7, 10, 0);
    assert_eq!(
        due, expected,
        "GPKE 24h deadline must be exactly 24 wall-clock hours after receipt"
    );
}

/// add_hours is a pure wall-clock addition — it does NOT shift at DST
/// boundaries.  A message arriving at 10:00 UTC the night before spring-forward
/// must still be due 24 UTC hours later (not 23 or 25).
#[test]
fn add_hours_gpke_24h_across_spring_forward() {
    // 2025-03-29 10:00 UTC = 11:00 CET, night before spring-forward
    let received = utc(2025, Month::March, 29, 10, 0);
    let due = add_hours(received, 24);
    let expected = utc(2025, Month::March, 30, 10, 0);
    assert_eq!(
        due, expected,
        "GPKE 24h deadline must be exactly 24 UTC hours across spring-forward"
    );
}

/// Same across fall-back (2025-10-26).
#[test]
fn add_hours_gpke_24h_across_fall_back() {
    // 2025-10-25 10:00 UTC = 12:00 CEST, night before fall-back
    let received = utc(2025, Month::October, 25, 10, 0);
    let due = add_hours(received, 24);
    let expected = utc(2025, Month::October, 26, 10, 0);
    assert_eq!(
        due, expected,
        "GPKE 24h deadline must be exactly 24 UTC hours across fall-back"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// add_werktage: calendar date arithmetic
// ─────────────────────────────────────────────────────────────────────────────

/// WiM 5-Werktage process Frist: Monday Jan 5 (2026) + 5 WT.
/// Note: 2026-01-06 (Heilige Drei Könige) is a BDEW MaKo holiday — it is
/// skipped. So the 5 Werktage are: Wed Jan 7, Thu Jan 8, Fri Jan 9, Sat Jan
/// 10, Mon Jan 12. Due date = 2026-01-12 (Monday).
#[test]
fn add_werktage_5wt_monday_to_saturday() {
    // 2026-01-05 (Monday). Jan 6 = Heilige Drei Könige (holiday).
    let start = Date::from_calendar_date(2026, Month::January, 5).unwrap();
    let due = add_werktage(start, 5, HolidayCalendar::BdewMaKo);
    // Skip Tue Jan 6 (holiday) → Wed 7, Thu 8, Fri 9, Sat 10, Mon 12
    let expected = Date::from_calendar_date(2026, Month::January, 12).unwrap();
    assert_eq!(
        due, expected,
        "Monday + 5 WT (with Heilige Drei Könige holiday) = Mon Jan 12"
    );
}

/// GeLi Gas 10-Werktage Frist: Monday Jan 5 + 10 WT.
/// Jan 6 = Heilige Drei Könige (holiday, skipped).
/// Werktage: Wed 7, Thu 8, Fri 9, Sat 10, Mon 12, Tue 13, Wed 14, Thu 15, Fri 16, Sat 17.
#[test]
fn add_werktage_10wt_monday_two_weeks() {
    // 2026-01-05 (Monday)
    let start = Date::from_calendar_date(2026, Month::January, 5).unwrap();
    let due = add_werktage(start, 10, HolidayCalendar::BdewMaKo);
    // Skip Jan 6 holiday → 5 WT get us to Mon Jan 12 (see 5wt test),
    // then 5 more: Tue 13, Wed 14, Thu 15, Fri 16, Sat 17 → Jan 17
    let expected = Date::from_calendar_date(2026, Month::January, 17).unwrap();
    assert_eq!(
        due, expected,
        "Monday + 10 WT (Heilige Drei Könige holiday) = Sat Jan 17"
    );
}

/// Sundays are skipped — 5 WT starting on Saturday Jan 3 (2026).
/// Sat Jan 3 → next Werktage (starting from next day):
/// Mon Jan 5, Tue Jan 5… wait: Jan 4 (Sun, skip), Jan 5 (Mon), Jan 6 (holiday, skip),
/// Jan 7 (Wed), Jan 8 (Thu), Jan 9 (Fri) = 4 WT. Jan 10 (Sat) = 5th WT → Jan 10.
#[test]
fn add_werktage_skips_sundays() {
    // 2026-01-03 (Saturday)
    let start = Date::from_calendar_date(2026, Month::January, 3).unwrap();
    let due = add_werktage(start, 5, HolidayCalendar::BdewMaKo);
    // Jan 4 (Sun, skip), Jan 5 (Mon=1), Jan 6 (holiday, skip), Jan 7 (Wed=2),
    // Jan 8 (Thu=3), Jan 9 (Fri=4), Jan 10 (Sat=5)
    let expected = Date::from_calendar_date(2026, Month::January, 10).unwrap();
    assert_eq!(due, expected, "add_werktage must skip Sundays and holidays");
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

/// deadline_at_werktage with 5 WT starting on a winter Monday:
/// Mon Jan 5, 2026 + 5 WT (Jan 6 is holiday) → Mon Jan 12.
/// 17:00 CET = 16:00 UTC.
///
/// Note: deadline_at_werktage returns OffsetDateTime with local offset;
/// compare as UTC instants via to_offset(UTC).
#[test]
fn deadline_at_werktage_5wt_winter_cet() {
    // 2026-01-05 09:00 UTC = 10:00 CET (Monday, winter)
    let received = utc(2026, Month::January, 5, 9, 0);
    let due =
        deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo).to_offset(UtcOffset::UTC);
    // Due date: 2026-01-12 (Monday after holiday), 17:00 CET = 16:00 UTC
    let expected = utc(2026, Month::January, 12, 16, 0);
    assert_eq!(
        due, expected,
        "5 WT winter deadline (with Heilige Drei Könige) must be Mon Jan 12 at 17:00 CET = 16:00 UTC"
    );
}

/// deadline_at_werktage with 10 WT starting on a summer Wednesday:
/// Wed Jun 4, 2025 + 10 WT, accounting for Pfingstmontag (Jun 9 = holiday).
/// Thu 5(1), Fri 6(2), Sat 7(3), [Mon 9 = Pfingstmontag, skip],
/// Tue 10(4), Wed 11(5), Thu 12(6), Fri 13(7), Sat 14(8), Mon 16(9), Tue 17(10).
/// Due: Tue Jun 17. 17:00 CEST = 15:00 UTC.
#[test]
fn deadline_at_werktage_10wt_summer_cest() {
    // 2025-06-04 09:00 UTC = 11:00 CEST (Wednesday, summer)
    let received = utc(2025, Month::June, 4, 9, 0);
    let due =
        deadline_at_werktage(received, 10, HolidayCalendar::BdewMaKo).to_offset(UtcOffset::UTC);
    // Thu 5(1), Fri 6(2), Sat 7(3), [Mon 9=Pfingstmontag, skip],
    // Tue 10(4), Wed 11(5), Thu 12(6), Fri 13(7), Sat 14(8), Mon 16(9), Tue 17(10)
    // 17:00 CEST (UTC+2) = 15:00 UTC
    let expected = utc(2025, Month::June, 17, 15, 0);
    assert_eq!(
        due, expected,
        "10 WT summer deadline (Pfingstmontag skip) must be due Tue Jun 17 at 17:00 CEST = 15:00 UTC"
    );
}

/// deadline_at_werktage must produce 15:00 UTC (17:00 CEST) when the due date
/// falls in summer time, even if the received timestamp is in winter time.
/// Starting Mon Mar 24 (CET), +10 WT:
/// Tue 25(1), Wed 26(2), Thu 27(3), Fri 28(4), Sat 29(5),
/// Mon 31(6), Tue Apr 1(7), Wed 2(8), Thu 3(9), Fri 4(10) → due Fri Apr 4.
/// Spring forward was Mar 30, so Apr 4 is CEST. 17:00 CEST = 15:00 UTC.
#[test]
fn deadline_at_werktage_due_date_drives_offset() {
    // 2025-03-24 09:00 UTC = 10:00 CET (Monday, still winter)
    let received = utc(2025, Month::March, 24, 9, 0);
    let due =
        deadline_at_werktage(received, 10, HolidayCalendar::BdewMaKo).to_offset(UtcOffset::UTC);
    // Tue 25(1), Wed 26(2), Thu 27(3), Fri 28(4), Sat 29(5),
    // Mon 31(6), Tue Apr 1(7), Wed 2(8), Thu 3(9), Fri 4(10)
    // Due: 2025-04-04 (Friday, CEST). 17:00 CEST = 15:00 UTC.
    let expected = utc(2025, Month::April, 4, 15, 0);
    assert_eq!(
        due, expected,
        "Due date in CEST must produce 15:00 UTC (17:00 CEST), \
         even when received date was in CET"
    );
}

/// deadline_at_werktage crossing the spring-forward boundary (2025-03-30):
/// 5 WT starting Thu Mar 27: Fri 28(1), Sat 29(2), Mon 31(3), Tue Apr 1(4), Wed Apr 2(5).
/// Due: Wed Apr 2, in CEST. 17:00 CEST = 15:00 UTC.
#[test]
fn deadline_at_werktage_crosses_spring_forward_2025() {
    // 2025-03-27 09:00 UTC = 10:00 CET (Thursday)
    let received = utc(2025, Month::March, 27, 9, 0);
    let due =
        deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo).to_offset(UtcOffset::UTC);
    // Fri 28(1), Sat 29(2), Mon 31(3), Tue Apr 1(4), Wed Apr 2(5)
    // Due: 2025-04-02 (Wednesday, CEST). 17:00 CEST = 15:00 UTC.
    let expected = utc(2025, Month::April, 2, 15, 0);
    assert_eq!(
        due, expected,
        "Deadline spanning spring-forward must use CEST offset on due date (15:00 UTC)"
    );
}

/// deadline_at_werktage crossing the fall-back boundary (2025-10-26):
/// 5 WT starting Fri Oct 24: Sat 25(1), Mon 27(2), Tue 28(3), Wed 29(4), Thu 30(5).
/// Due: Thu Oct 30, after fall-back (Oct 26), so CET. 17:00 CET = 16:00 UTC.
#[test]
fn deadline_at_werktage_crosses_fall_back_2025() {
    // 2025-10-24 09:00 UTC = 11:00 CEST (Friday — still summer before fall-back Sun)
    let received = utc(2025, Month::October, 24, 9, 0);
    let due =
        deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo).to_offset(UtcOffset::UTC);
    // Sat 25(1), Mon 27(2), Tue 28(3), Wed 29(4), Thu 30(5)
    // Due: 2025-10-30 (Thursday) — after fall-back (Oct 26), so CET
    // 17:00 CET (UTC+1) = 16:00 UTC
    let expected = utc(2025, Month::October, 30, 16, 0);
    assert_eq!(
        due, expected,
        "Deadline spanning fall-back must use CET offset on due date (16:00 UTC)"
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
