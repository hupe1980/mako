//! Regulatory deadline calculation helpers.
//!
//! Two fundamentally different deadline semantics apply in BNetzA MaKo
//! processes, and they **must not be mixed up**:
//!
//! | Process family | Deadline unit | Reason |
//! |---|---|---|
//! | **GPKE Lieferantenwechsel** (BK6-22-024) | 24 wall-clock hours | BNetzA decision; no Werktag exemption |
//! | **WiM / GeLi Gas / MABIS** | Werktage (working days) | BDEW AHB Fristenregeln |
//!
//! ## GPKE 24h Lieferantenwechsel
//!
//! After receiving a UTILMD Lieferbeginn request, the network operator **must**
//! dispatch the APERAK acknowledgement within **24 consecutive wall-clock
//! hours** (BNetzA decision BK6-22-024). Weekends and public holidays do
//! **not** extend this window.
//!
//! ```rust
//! use mako_engine::fristen;
//! use time::OffsetDateTime;
//!
//! let received = OffsetDateTime::now_utc();
//! let due = fristen::add_hours(received, 24);
//! assert!(due > received);
//! ```
//!
//! ## WiM / GeLi Gas / MABIS Werktage
//!
//! ```rust
//! use mako_engine::fristen::{self, HolidayCalendar};
//! use time::{Date, Month};
//!
//! // 5 Werktage after Monday 2025-01-06 (federal only):
//! let start = Date::from_calendar_date(2025, Month::January, 6).unwrap();
//! let due   = fristen::add_werktage(start, 5, HolidayCalendar::BdewMaKo);
//! // Tue 07, Wed 08, Thu 09, Fri 10, Sat 11 → 2025-01-11
//! // (Saturday counts as Werktag in German energy regulation)
//! assert_eq!(due, Date::from_calendar_date(2025, Month::January, 11).unwrap());
//! ```
//!
//! ## Holiday calendar: BDEW-defined Germany-wide calendar
//!
//! [`HolidayCalendar::BdewMaKo`] is the single holiday calendar used in all
//! BNetzA MaKo processes. BDEW EDI@Energy specifies a conservative-inclusive
//! approach: every public holiday observed in *any* German state is treated as
//! a non-Werktag. This guarantees no APERAK Frist is ever shorter than the AHB
//! requires. Per-state calendars are **not** used in BDEW MaKo — there is one
//! Germany-wide calendar that all market participants use.
//!
//! ## CONTRL 6h Übertragungsquittung
//!
//! CONTRL AHB 1.0 §1.2 mandates that the recipient confirms syntactic validity
//! of a received EDIFACT interchange **within 6 wall-clock hours** of receipt.
//! This obligation applies at the transport layer (before any workflow
//! processing) and is independent of the process-level APERAK fristen.
//!
//! ```rust
//! use mako_engine::fristen;
//! use time::OffsetDateTime;
//!
//! let received = OffsetDateTime::now_utc();
//! let due = fristen::contrl_due_at(received);
//! assert_eq!(due - received, time::Duration::hours(6));
//! ```

use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime, Time, Weekday};
use time_tz::{OffsetDateTimeExt, OffsetResult, PrimitiveDateTimeExt, timezones};

// ── CONTRL Übertragungsquittung ───────────────────────────────────────────────

/// Maximum wall-clock hours within which a CONTRL must be sent after receiving
/// an EDIFACT interchange.
///
/// Per CONTRL AHB 1.0 §1.2: "Der Empfänger teilt dem Absender **unverzüglich,
/// jedoch spätestens 6 Stunden** nach Erhalt der Übertragungsdatei das
/// Ergebnis seiner syntaktischen Prüfung mittels CONTRL mit."
pub const CONTRL_FRIST_HOURS: i64 = 6;

/// Deadline label used in the `DeadlineStore` for CONTRL delivery obligations.
///
/// Register a `Deadline` with this label when enqueueing a CONTRL `PendingOutbox`
/// entry. The outbox worker clears the deadline after successful CONTRL delivery.
/// If the deadline fires before the CONTRL is delivered, the 6h Frist has been
/// violated (CONTRL AHB 1.0 §1.2).
pub const CONTRL_FRIST_LABEL: &str = "contrl-delivery";

/// Compute the CONTRL delivery deadline as 6 wall-clock hours after `received`.
///
/// # Example
///
/// ```rust
/// use mako_engine::fristen;
/// use time::OffsetDateTime;
///
/// let received = OffsetDateTime::now_utc();
/// let due = fristen::contrl_due_at(received);
/// assert_eq!(due - received, time::Duration::hours(6));
/// ```
#[must_use]
pub fn contrl_due_at(received: OffsetDateTime) -> OffsetDateTime {
    received + Duration::hours(CONTRL_FRIST_HOURS)
}

/// Selects which set of public holidays to observe when counting Werktage.
///
/// BDEW MaKo processes use a single Germany-wide holiday calendar defined by
/// BDEW EDI@Energy. This calendar is conservative-inclusive: it treats every
/// public holiday observed in *any* German state as a non-Werktag, ensuring
/// no deadline is ever shorter than the AHB requires for any counterparty.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolidayCalendar {
    /// BDEW-defined Germany-wide holiday calendar for MaKo Werktag calculations.
    ///
    /// This is the single calendar used by all BNetzA MaKo processes (GPKE,
    /// WiM, GeLi Gas, MABIS). BDEW EDI@Energy specifies a conservative-inclusive
    /// approach: every holiday observed in *any* German state is treated as a
    /// non-Werktag. This guarantees that no APERAK Frist is ever computed shorter
    /// than the AHB requires for any market participant in Germany.
    ///
    /// Includes the 9 nationwide (*bundesweite*) public holidays **plus** all
    /// *Landesfeiertage* that are observed in at least one German state:
    ///
    /// | Date | Holiday | States |
    /// |------|---------|--------|
    /// | 1 Jan | Neujahr | all |
    /// | 6 Jan | Heilige Drei Könige | BY, BW, ST |
    /// | 1 May | Tag der Arbeit | all |
    /// | 3 Oct | Tag der Deutschen Einheit | all |
    /// | 31 Oct | Reformationstag | BB, HB, HH, MV, NI, SN, ST, SH, TH |
    /// | 1 Nov | Allerheiligen | BW, BY, NW, RP, SL |
    /// | 25 Dec | 1. Weihnachtstag | all |
    /// | 26 Dec | 2. Weihnachtstag | all |
    /// | Easter−2 | Karfreitag | all |
    /// | Easter+1 | Ostermontag | all |
    /// | Easter+39 | Christi Himmelfahrt | all |
    /// | Easter+49 | Pfingstsonntag | all |
    /// | Easter+50 | Pfingstmontag | all |
    /// | Easter+60 | Fronleichnam | BW, BY, HE, NW, RP, SL, SN (parts), TH (parts) |
    /// | 15 Aug | Mariä Himmelfahrt | BY, SL |
    ///
    /// **Rationale**: A counterparty in any of these states is legally entitled
    /// not to process messages on their regional holiday. Using a maximally
    /// inclusive calendar ensures no deadline is shorter than the AHB requires
    /// for any market participant in Germany, at the cost of occasionally
    /// granting one extra day to counterparties in states where that day is a
    /// regular Werktag.
    BdewMaKo,
}

// ── Wall-clock helpers ────────────────────────────────────────────────────────

/// Add `hours` wall-clock hours to `from`.
///
/// Use this for the **GPKE 24h Lieferantenwechsel** window (BK6-22-024).
/// Weekends and public holidays do **not** extend the window.
///
/// # Example
///
/// ```rust
/// use mako_engine::fristen;
/// use time::OffsetDateTime;
///
/// let received = OffsetDateTime::now_utc();
/// let due = fristen::add_hours(received, 24);
/// assert_eq!(due - received, time::Duration::hours(24));
/// ```
#[must_use]
pub fn add_hours(from: OffsetDateTime, hours: u32) -> OffsetDateTime {
    from + Duration::hours(i64::from(hours))
}

// ── Werktage helpers ──────────────────────────────────────────────────────────

/// Add `n` Werktage (working days) to `from`.
///
/// A Werktag is any day that is neither a Sunday nor a public holiday in the
/// given `cal`. **Saturdays count as Werktage** in German energy regulation
/// (BDEW AHB).
///
/// Use this for **WiM / GeLi Gas / MABIS** deadlines.
///
/// # Semantics of `n = 0`
///
/// Returns `from` unchanged regardless of whether `from` is itself a Werktag.
/// To find the first Werktag on or after a given date, use
/// [`next_werktag`] instead.
///
/// # Example
///
/// ```rust
/// use mako_engine::fristen::{self, HolidayCalendar};
/// use time::{Date, Month};
///
/// // Monday + 5 Werktage (no holidays in this week):
/// let start = Date::from_calendar_date(2025, Month::January, 6).unwrap();
/// let due   = fristen::add_werktage(start, 5, HolidayCalendar::BdewMaKo);
/// // Tue 07, Wed 08, Thu 09, Fri 10, Sat 11 → 2025-01-11
/// assert_eq!(due, Date::from_calendar_date(2025, Month::January, 11).unwrap());
/// ```
///
/// # Panics
///
/// Panics if date arithmetic overflows the calendar (unreachable for any
/// realistic date within the Gregorian calendar range).
#[must_use]
pub fn add_werktage(from: Date, n: u32, cal: HolidayCalendar) -> Date {
    let mut current = from;
    let mut remaining = n;
    while remaining > 0 {
        current = current.next_day().expect("date overflow");
        if is_werktag(current, cal) {
            remaining -= 1;
        }
    }
    current
}

/// Return the first Werktag that is on or after `from`.
///
/// Unlike `add_werktage(from, 0, cal)` (which always returns `from`
/// unchanged), `next_werktag` advances past Sundays and public holidays.
///
/// # Example
///
/// ```rust
/// use mako_engine::fristen::{self, HolidayCalendar};
/// use time::{Date, Month};
///
/// // Sunday 2025-01-12 → next Werktag is Monday 2025-01-13 (no holiday).
/// // Note: 2025-01-06 (Heilige Drei Könige) is in the fristen federal
/// // calendar and must not be used as the expected "next Monday" here.
/// let sunday = Date::from_calendar_date(2025, Month::January, 12).unwrap();
/// assert_eq!(
///     fristen::next_werktag(sunday, HolidayCalendar::BdewMaKo),
///     Date::from_calendar_date(2025, Month::January, 13).unwrap(),
/// );
///
/// // Monday 2025-01-13 is already a Werktag → returned unchanged.
/// let monday = Date::from_calendar_date(2025, Month::January, 13).unwrap();
/// assert_eq!(fristen::next_werktag(monday, HolidayCalendar::BdewMaKo), monday);
/// ```
///
/// # Panics
///
/// Panics if date arithmetic overflows the calendar (unreachable for any
/// realistic date within the Gregorian calendar range).
#[must_use]
pub fn next_werktag(from: Date, cal: HolidayCalendar) -> Date {
    let mut current = from;
    while !is_werktag(current, cal) {
        current = current.next_day().expect("date overflow");
    }
    current
}

/// Compute a deadline `werktage` Werktage after `from`, expressed as an
/// [`OffsetDateTime`] at **17:00 Europe/Berlin** on the deadline date.
///
/// The deadline is computed in German local time (CET in winter, CEST in
/// summer). 17:00 CET = 16:00 UTC; 17:00 CEST = 15:00 UTC. Using UTC
/// directly would give a systematic 1–2 hour error on every regulatory
/// deadline.
///
/// 17:00 is never in a DST transition window for Europe/Berlin (transitions
/// happen at 02:00), so the conversion is unambiguous on all dates.
///
/// # Example
///
/// ```rust
/// use mako_engine::fristen::{self, HolidayCalendar};
/// use time::{Date, Month, OffsetDateTime, Time, UtcOffset};
///
/// let received = OffsetDateTime::new_utc(
///     Date::from_calendar_date(2025, Month::January, 6).unwrap(),
///     Time::MIDNIGHT,
/// );
/// let due = fristen::deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo);
/// assert_eq!(due.date(), Date::from_calendar_date(2025, Month::January, 11).unwrap());
/// // January is CET (UTC+1): the deadline is 17:00 local time.
/// // Local hour is 17; the UTC equivalent is 16:00.
/// assert_eq!(due.hour(), 17);  // local time (CET)
/// assert_eq!(due.to_offset(UtcOffset::UTC).hour(), 16); // UTC equivalent
/// ```
///
/// # Panics
///
/// Panics if date arithmetic overflows the calendar (unreachable for any
/// realistic date within the Gregorian calendar range).
#[must_use]
pub fn deadline_at_werktage(
    from: OffsetDateTime,
    werktage: u32,
    cal: HolidayCalendar,
) -> OffsetDateTime {
    let berlin = timezones::db::europe::BERLIN;
    // Convert to Berlin local time before extracting the calendar date.
    // `from.date()` returns the UTC date which is wrong for messages arriving
    // between 23:00–00:00 UTC (= 00:00–01:00 CET next day in winter, or
    // 00:00–02:00 CEST in summer).  Using the UTC date would count Werktage
    // starting from yesterday's calendar date, yielding a deadline that is one
    // calendar day — and potentially one Werktag — too early.
    let start_date = from.to_timezone(berlin).date();
    let due_date = add_werktage(start_date, werktage, cal);
    // Construct 17:00 as a PrimitiveDateTime in local (Europe/Berlin) time, then
    // obtain the correct UTC offset for that moment.  17:00 is never inside a
    // DST gap or fold for Europe/Berlin, so assume_timezone always returns Some.
    let local_17 = PrimitiveDateTime::new(
        due_date,
        Time::from_hms(17, 0, 0).expect("17:00:00 is valid"),
    );
    match local_17.assume_timezone(berlin) {
        OffsetResult::Some(dt) => dt,
        // 17:00 Europe/Berlin is never inside a DST gap or fold, so Ambiguous
        // and None are unreachable in practice. If the timezone database is
        // broken or absent, we must not silently compute a wrong deadline:
        // UTC+1 in a CEST month (UTC+2) produces a deadline 1 hour late, which
        // is a reportable BNetzA regulatory violation.
        OffsetResult::Ambiguous(earlier, _later) => earlier,
        OffsetResult::None => {
            // SAFETY: 17:00 is never inside a DST gap for Europe/Berlin. If we
            // land here the timezone database is corrupt or missing. Panic loudly
            // so the operator detects the failure before it silently produces
            // wrong APERAK deadlines. A wrong deadline is worse than a crash
            // because it is a regulatory violation without any visible signal.
            panic!(
                "CRITICAL: timezone database failure — could not resolve \
                 17:00 Europe/Berlin for date {due_date}. \
                 Cannot compute a correct APERAK deadline. \
                 A wrong fallback offset violates BNetzA deadline obligations. \
                 Ensure the system timezone database (tzdata) is installed and \
                 up to date. Aborting rather than silently producing an \
                 incorrect deadline."
            );
        }
    }
}

// ── Holiday tables ────────────────────────────────────────────────────────────

/// Return `true` when `date` is a non-Werktag public holiday under the
/// [`HolidayCalendar::BdewMaKo`] calendar.
///
/// Covers all 9 *bundesweite* public holidays plus the *Landesfeiertage*
/// observed in at least one German state. See [`HolidayCalendar::BdewMaKo`]
/// for the complete list and rationale.
///
/// Easter is computed algorithmically using the Anonymous Gregorian algorithm —
/// no pre-computed table, no year ceiling.
#[must_use]
fn is_bdew_mako_holiday(date: Date) -> bool {
    let (y, m, d) = (date.year(), date.month() as u8, date.day());

    // Fixed-date holidays (bundesweit + Landesfeiertage):
    if matches!(
        (m, d),
        (1 | 5 | 11, 1) | (1, 6) | (8, 15) | (10, 3 | 31) | (12, 25 | 26) // 2. Weihnachtstag
    ) {
        return true;
    }

    // Moveable Easter-based holidays — computed algorithmically.
    let e_date = easter_sunday(y);

    let offsets: &[i64] = &[
        -2, // Karfreitag
        1,  // Ostermontag
        39, // Christi Himmelfahrt
        49, // Pfingstsonntag
        50, // Pfingstmontag
        60, // Fronleichnam (BW, BY, HE, NW, RP, SL, SN/TH parts)
    ];

    for &offset in offsets {
        let holiday = e_date + Duration::days(offset);
        if holiday == date {
            return true;
        }
    }

    false
}

/// Compute Easter Sunday for `year` using the Anonymous Gregorian algorithm.
///
/// Valid for all years in the proleptic Gregorian calendar. No table, no
/// year ceiling.
///
/// # Example
///
/// ```rust,ignore
/// // Easter 2025: 20 April
/// let e = easter_sunday(2025);
/// assert_eq!((e.year(), e.month() as u8, e.day()), (2025, 4, 20));
/// ```
#[allow(clippy::many_single_char_names)]
fn easter_sunday(year: i32) -> Date {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    // The algorithm guarantees month in 3..=4 and day in 1..=31; both casts are safe.
    let month_u8 = u8::try_from(month).expect("algorithm yields valid month index");
    let day_u8 = u8::try_from(day).expect("algorithm yields valid day");
    Date::from_calendar_date(
        year,
        time::Month::try_from(month_u8).expect("algorithm yields valid month"),
        day_u8,
    )
    .expect("algorithm yields valid date")
}

/// Return `true` when `date` is a Werktag under `cal`.
///
/// In German energy regulation (BDEW AHB), Sundays and public holidays are
/// **not** Werktage. Saturdays **are** Werktage.
fn is_werktag(date: Date, cal: HolidayCalendar) -> bool {
    if date.weekday() == Weekday::Sunday {
        return false;
    }
    match cal {
        HolidayCalendar::BdewMaKo => !is_bdew_mako_holiday(date),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month, OffsetDateTime, Time};

    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), d).unwrap()
    }

    // ── add_hours ─────────────────────────────────────────────────────────────

    #[test]
    fn add_hours_advances_exactly() {
        let t = OffsetDateTime::now_utc();
        assert_eq!(add_hours(t, 24) - t, Duration::hours(24));
    }

    #[test]
    fn add_hours_crosses_midnight() {
        let t = OffsetDateTime::now_utc();
        let due = add_hours(t, 24);
        // 24h later is exactly one day forward (ignoring leap-seconds):
        assert_eq!(due.date(), t.date() + Duration::days(1));
    }

    // ── contrl_due_at ─────────────────────────────────────────────────────────

    #[test]
    fn contrl_due_at_is_exactly_6h_after_received() {
        let received = OffsetDateTime::now_utc();
        let due = contrl_due_at(received);
        assert_eq!(
            due - received,
            Duration::hours(6),
            "CONTRL AHB 1.0 §1.2 requires exactly 6h frist"
        );
    }

    #[test]
    fn contrl_frist_label_is_stable() {
        // Changing this label would silently orphan all existing Deadline records.
        assert_eq!(CONTRL_FRIST_LABEL, "contrl-delivery");
    }

    #[test]
    fn contrl_frist_hours_matches_constant() {
        let received = OffsetDateTime::now_utc();
        assert_eq!(
            contrl_due_at(received) - received,
            Duration::hours(CONTRL_FRIST_HOURS)
        );
    }

    // ── is_bdew_mako_holiday ────────────────────────────────────────────────────

    #[test]
    fn fixed_holidays_are_detected() {
        assert!(is_bdew_mako_holiday(date(2025, 1, 1)), "Neujahr");
        assert!(
            is_bdew_mako_holiday(date(2025, 1, 6)),
            "Heilige Drei Könige"
        );
        assert!(is_bdew_mako_holiday(date(2025, 5, 1)), "Tag der Arbeit");
        assert!(is_bdew_mako_holiday(date(2025, 8, 15)), "Mariä Himmelfahrt");
        assert!(
            is_bdew_mako_holiday(date(2025, 10, 3)),
            "Tag der Deutschen Einheit"
        );
        assert!(is_bdew_mako_holiday(date(2025, 10, 31)), "Reformationstag");
        assert!(is_bdew_mako_holiday(date(2025, 11, 1)), "Allerheiligen");
        assert!(is_bdew_mako_holiday(date(2025, 12, 25)), "1. Weihnachtstag");
        assert!(is_bdew_mako_holiday(date(2025, 12, 26)), "2. Weihnachtstag");
    }

    #[test]
    fn easter_2025_moveable_holidays() {
        // Easter Sunday 2025-04-20
        assert!(is_bdew_mako_holiday(date(2025, 4, 18)), "Karfreitag");
        assert!(is_bdew_mako_holiday(date(2025, 4, 21)), "Ostermontag");
        assert!(
            is_bdew_mako_holiday(date(2025, 5, 29)),
            "Christi Himmelfahrt"
        );
        assert!(is_bdew_mako_holiday(date(2025, 6, 8)), "Pfingstsonntag");
        assert!(is_bdew_mako_holiday(date(2025, 6, 9)), "Pfingstmontag");
        assert!(is_bdew_mako_holiday(date(2025, 6, 19)), "Fronleichnam");
    }

    /// Verify the Anonymous Gregorian algorithm is correct beyond the old 2035
    /// table ceiling.
    #[test]
    fn easter_beyond_2035_table_ceiling() {
        // 2036: Easter Sunday = 13 April (verified against multiple Easter calculators)
        assert_eq!(easter_sunday(2036), date(2036, 4, 13));
        assert!(is_bdew_mako_holiday(date(2036, 4, 11)), "Karfreitag 2036"); // -2
        assert!(is_bdew_mako_holiday(date(2036, 4, 14)), "Ostermontag 2036"); // +1
        assert!(
            is_bdew_mako_holiday(date(2036, 5, 22)),
            "Christi Himmelfahrt 2036"
        ); // +39
        assert!(
            is_bdew_mako_holiday(date(2036, 6, 1)),
            "Pfingstsonntag 2036"
        ); // +49
        assert!(is_bdew_mako_holiday(date(2036, 6, 2)), "Pfingstmontag 2036"); // +50
        assert!(is_bdew_mako_holiday(date(2036, 6, 12)), "Fronleichnam 2036"); // +60

        // 2050: Easter Sunday = 10 April
        assert_eq!(easter_sunday(2050), date(2050, 4, 10));
    }

    #[test]
    fn saturday_is_not_a_holiday() {
        // 2025-01-04 is a Saturday — not a holiday
        assert!(!is_bdew_mako_holiday(date(2025, 1, 4)));
    }

    // ── is_werktag ────────────────────────────────────────────────────────────

    #[test]
    fn sunday_is_not_werktag() {
        assert!(!is_werktag(date(2025, 1, 5), HolidayCalendar::BdewMaKo));
    }

    #[test]
    fn saturday_is_werktag() {
        assert!(is_werktag(date(2025, 1, 4), HolidayCalendar::BdewMaKo));
    }

    #[test]
    fn holiday_is_not_werktag() {
        assert!(!is_werktag(date(2025, 1, 1), HolidayCalendar::BdewMaKo));
    }

    #[test]
    fn landesfeiertage_are_not_werktage() {
        // Heilige Drei Könige
        assert!(!is_werktag(date(2025, 1, 6), HolidayCalendar::BdewMaKo));
        // Mariä Himmelfahrt
        assert!(!is_werktag(date(2025, 8, 15), HolidayCalendar::BdewMaKo));
        // Reformationstag 2025 falls on a Friday
        assert!(!is_werktag(date(2025, 10, 31), HolidayCalendar::BdewMaKo));
        // Allerheiligen
        assert!(!is_werktag(date(2025, 11, 1), HolidayCalendar::BdewMaKo));
    }

    // ── add_werktage ──────────────────────────────────────────────────────────

    #[test]
    fn five_werktage_plain_week() {
        // Monday 2025-01-06, no holidays.
        // Tue 07, Wed 08, Thu 09, Fri 10, Sat 11 → 2025-01-11
        // (Saturday counts as Werktag in German energy regulation)
        let start = date(2025, 1, 6);
        let due = add_werktage(start, 5, HolidayCalendar::BdewMaKo);
        assert_eq!(due, date(2025, 1, 11));
    }

    #[test]
    fn skips_reformationstag_and_allerheiligen() {
        // 2025-10-29 is a Wednesday.
        // +5 Werktage:
        //   Thu 30 (+1), Fri 31 = Reformationstag (skip), Sat 01 Nov = Allerheiligen (skip),
        //   Sun 02 (skip), Mon 03 (+2), Tue 04 (+3), Wed 05 (+4), Thu 06 (+5) → 2025-11-06
        let start = date(2025, 10, 29);
        let due = add_werktage(start, 5, HolidayCalendar::BdewMaKo);
        assert_eq!(due, date(2025, 11, 6));
    }

    #[test]
    fn skips_heilige_drei_koenige() {
        // 2025-01-04 is a Saturday (Werktag).
        // +1 Werktag: Sun 05 (skip), Mon 06 = Heilige Drei Könige (skip),
        //              Tue 07 → 2025-01-07
        let start = date(2025, 1, 4);
        let due = add_werktage(start, 1, HolidayCalendar::BdewMaKo);
        assert_eq!(due, date(2025, 1, 7));
    }

    #[test]
    fn skips_sunday_correctly() {
        // Saturday 2025-01-11:  +1 Werktag → skip Sun 12 → Mon 13
        // (Using a date that avoids Heilige Drei Könige on 06-Jan)
        let start = date(2025, 1, 11);
        let due = add_werktage(start, 1, HolidayCalendar::BdewMaKo);
        assert_eq!(due, date(2025, 1, 13));
    }

    #[test]
    fn skips_holiday_and_sunday() {
        // 2025-04-17 is Thursday before Easter.
        // +1 Werktag: Fri 18 = Karfreitag (holiday → skip), Sat 19 is Werktag → 2025-04-19
        let start = date(2025, 4, 17);
        let due = add_werktage(start, 1, HolidayCalendar::BdewMaKo);
        assert_eq!(due, date(2025, 4, 19));
    }

    #[test]
    fn zero_werktage_returns_start() {
        let start = date(2025, 1, 6);
        assert_eq!(add_werktage(start, 0, HolidayCalendar::BdewMaKo), start);
    }

    // ── next_werktag ──────────────────────────────────────────────────────────

    #[test]
    fn next_werktag_from_sunday_advances_to_monday() {
        // Use Jan 12 (Sunday) → Jan 13 (Monday, no holiday).
        // Jan 6 is Heilige Drei Könige (included in the fristen federal calendar),
        // so that date cannot be used as the expected "next regular Monday".
        let sunday = date(2025, 1, 12);
        assert_eq!(
            next_werktag(sunday, HolidayCalendar::BdewMaKo),
            date(2025, 1, 13), // Monday
        );
    }

    #[test]
    fn next_werktag_from_werktag_returns_same() {
        let monday = date(2025, 1, 13);
        assert_eq!(next_werktag(monday, HolidayCalendar::BdewMaKo), monday);
    }

    #[test]
    fn next_werktag_from_holiday_advances_to_next_werktag() {
        // Neujahr 2025-01-01 is Wednesday; next Werktag is Thursday 2025-01-02.
        assert_eq!(
            next_werktag(date(2025, 1, 1), HolidayCalendar::BdewMaKo),
            date(2025, 1, 2),
        );
    }

    // ── deadline_at_werktage ──────────────────────────────────────────────────

    ///  deadline must be 17:00 CET (16:00 UTC) in winter, not 17:00 UTC.
    #[test]
    fn deadline_at_werktage_winter_cet() {
        // January is CET (UTC+1).  17:00 CET = 16:00 UTC.
        let received = OffsetDateTime::new_utc(date(2025, 1, 6), Time::MIDNIGHT);
        let due = deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo);
        assert_eq!(due.date(), date(2025, 1, 11));
        assert_eq!(
            due.to_offset(time::UtcOffset::UTC).hour(),
            16,
            "winter: 17:00 CET = 16:00 UTC"
        );
        assert_eq!(due.to_offset(time::UtcOffset::UTC).minute(), 0);
    }

    ///  deadline must be 17:00 CEST (15:00 UTC) in summer, not 17:00 UTC.
    #[test]
    fn deadline_at_werktage_summer_cest() {
        // July is CEST (UTC+2).  17:00 CEST = 15:00 UTC.
        let received = OffsetDateTime::new_utc(date(2025, 7, 1), Time::MIDNIGHT);
        let due = deadline_at_werktage(received, 1, HolidayCalendar::BdewMaKo);
        assert_eq!(
            due.to_offset(time::UtcOffset::UTC).hour(),
            15,
            "summer: 17:00 CEST = 15:00 UTC"
        );
        assert_eq!(due.to_offset(time::UtcOffset::UTC).minute(), 0);
    }

    /// Deadline that lands on the day *after* the spring-forward transition
    /// must use CEST (UTC+2), not CET (UTC+1).
    ///
    /// 2025-03-30 02:00 CET → 03:00 CEST (spring-forward).
    /// received = Wednesday 2025-03-26; +4 Werktage:
    ///   Thu 27 (+1), Fri 28 (+2), Sat 29 (+3), Sun 30 (skip), Mon 31 (+4).
    /// Deadline falls on Monday 2025-03-31 which is CEST: 17:00 CEST = 15:00 UTC.
    #[test]
    fn deadline_on_day_after_spring_forward_is_cest() {
        let received = OffsetDateTime::new_utc(date(2025, 3, 26), Time::MIDNIGHT);
        let due = deadline_at_werktage(received, 4, HolidayCalendar::BdewMaKo);
        assert_eq!(
            due.date(),
            date(2025, 3, 31),
            "should land on Monday 2025-03-31"
        );
        assert_eq!(
            due.to_offset(time::UtcOffset::UTC).hour(),
            15,
            "CEST: 17:00 local = 15:00 UTC (spring-forward already happened)"
        );
        assert_eq!(due.to_offset(time::UtcOffset::UTC).minute(), 0);
    }

    /// Deadline that lands on the day *after* the fall-back transition must
    /// use CET (UTC+1), not CEST (UTC+2).
    ///
    /// 2025-10-26 03:00 CEST → 02:00 CET (fall-back).
    /// received = Wednesday 2025-10-22; +4 Werktage:
    ///   Thu 23 (+1), Fri 24 (+2), Sat 25 (+3), Sun 26 (skip), Mon 27 (+4).
    /// Deadline falls on Monday 2025-10-27 which is CET: 17:00 CET = 16:00 UTC.
    #[test]
    fn deadline_on_day_after_fall_back_is_cet() {
        let received = OffsetDateTime::new_utc(date(2025, 10, 22), Time::MIDNIGHT);
        let due = deadline_at_werktage(received, 4, HolidayCalendar::BdewMaKo);
        assert_eq!(
            due.date(),
            date(2025, 10, 27),
            "should land on Monday 2025-10-27"
        );
        assert_eq!(
            due.to_offset(time::UtcOffset::UTC).hour(),
            16,
            "CET: 17:00 local = 16:00 UTC (fall-back already happened)"
        );
        assert_eq!(due.to_offset(time::UtcOffset::UTC).minute(), 0);
    }

    /// Regression test for the UTC-date edge case (F-005).
    ///
    /// A message arriving at 23:30 UTC on 2025-01-06 (Monday) is already
    /// 00:30 CET on 2025-01-07 (Tuesday) in Berlin local time.  The deadline
    /// must be counted from 2025-01-07 (Tuesday), not 2025-01-06 (Monday).
    ///
    /// Counting from Monday: Tue 07, Wed 08, Thu 09, Fri 10, Sat 11 → 2025-01-11
    /// Counting from Tuesday: Wed 08, Thu 09, Fri 10, Sat 11, Mon 13 → 2025-01-13
    ///   (2025-01-12 is Sunday; 2025-01-13 is Monday)
    #[test]
    fn deadline_at_werktage_uses_berlin_date_not_utc_date() {
        use time::Time;
        // 23:30 UTC on 2025-01-06 (Monday) = 00:30 CET on 2025-01-07 (Tuesday)
        let received =
            OffsetDateTime::new_utc(date(2025, 1, 6), Time::from_hms(23, 30, 0).unwrap());
        let due = deadline_at_werktage(received, 5, HolidayCalendar::BdewMaKo);
        // Should start from 2025-01-07 (Tuesday Berlin date), not 2025-01-06
        assert_eq!(
            due.date(),
            date(2025, 1, 13),
            "5 WT from Tuesday 2025-01-07: Wed 08 (+1), Thu 09 (+2), Fri 10 (+3), \
             Sat 11 (+4), Mon 13 (+5) — Sunday 12 skipped"
        );
    }

    /// Edge case: message at 23:59 UTC on 2025-01-10 (Friday) is already
    /// Saturday 00:59 CET in Berlin.  Saturday is a Werktag, so 1 WT after
    /// Saturday is Monday (Sunday skipped).
    #[test]
    fn deadline_at_werktage_friday_night_utc_is_saturday_berlin() {
        use time::Time;
        // 23:59 UTC on Friday 2025-01-10 = 00:59 CET on Saturday 2025-01-11
        let received =
            OffsetDateTime::new_utc(date(2025, 1, 10), Time::from_hms(23, 59, 0).unwrap());
        let due = deadline_at_werktage(received, 1, HolidayCalendar::BdewMaKo);
        // Starting from Saturday 2025-01-11: 1 WT = Monday 2025-01-13 (Sunday skipped)
        assert_eq!(
            due.date(),
            date(2025, 1, 13),
            "1 WT from Saturday 2025-01-11 is Monday 2025-01-13 (Sunday not a Werktag)"
        );
    }
}
