//! Parsing the date shapes a `de.mako.process.initiated` payload can carry.
//!
//! | Shape | Where it comes from |
//! |---|---|
//! | `YYYYMMDD` | an ERP command through `makod`'s REST API, or an inbound date `makod` already read back |
//! | `YYYY-MM-DD` | a JSON payload written by hand or by a connector |
//! | `CCYYMMDDHHMM±ZZ` | **EDIFACT** — DE 2379 format code `303` |
//!
//! A `303` process date is not a calendar date with a time attached: it is the
//! UTC instant at which the German day begins — 00:00 Europe/Berlin in Strom,
//! the Gastag's 06:00 in Gas (Allgemeine Festlegungen V6.1d Kap. 3.1). So a
//! Strom „Beginn zum" 01.01.2027 is `202612312300+00`, whose first eight digits
//! are the day before. [`parse`](crate::wire_date::parse) reads it back through
//! [`mako_fristen::tag_des_beginns`] in the message's Sparte, and refuses an
//! instant that begins no German day there rather than rounding it to one.

use mako_fristen::Sparte;
use time::Date;

/// The `[year][month][day]` shape of `YYYYMMDD`.
const CCYYMMDD: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[year][month][day]");

/// Parse a process date from any shape a process payload carries, a `303`
/// stamp read as the German day it begins in `sparte`.
///
/// `None` for an absent or malformed value, and for a `303` instant that is not
/// the beginning of a German day in `sparte` — `202610010000+00` is 01:00 or
/// 02:00 Berlin and names no Strom day.
#[must_use]
pub fn parse(raw: &str, sparte: Sparte) -> Option<Date> {
    let raw = raw.trim();
    if let Some(instant) = instant_303(raw) {
        return mako_fristen::tag_des_beginns(instant, sparte).ok();
    }
    kalendertag(raw)
}

/// Parse a calendar date written as one — `YYYYMMDD` or `YYYY-MM-DD` — for a
/// value that is not a wire process date (a contract date from `vertragd`).
///
/// A `303` stamp is refused here: without its Sparte it names no German day.
#[must_use]
pub fn kalendertag(raw: &str) -> Option<Date> {
    let raw = raw.trim();
    if raw.len() == 8 {
        return Date::parse(raw, CCYYMMDD).ok();
    }
    if instant_303(raw).is_some() {
        return None;
    }
    // `YYYY-MM-DD`, optionally with a time suffix a JSON producer appended.
    //
    // `get` rather than a slice: byte 10 need not be a character boundary. The
    // value reaches this from a webhook body, so a multi-byte character
    // straddling the cut is inbound data, not a programming error — and a
    // slice there panics the handler. A cut that lands mid-character yields
    // the whole string, which then fails to parse, which is the answer.
    Date::parse(
        raw.get(..10).unwrap_or(raw),
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()
}

/// `CCYYMMDDHHMM±ZZ` — DE 2379 `303` — as the instant it states.
///
/// Recognised by its zone sign rather than by length alone, so a malformed
/// value falls through to a failure instead of being read as a date.
fn instant_303(raw: &str) -> Option<time::OffsetDateTime> {
    if raw.len() < 13
        || !raw.is_char_boundary(12)
        || !matches!(raw.as_bytes()[12], b'+' | b'-')
        || !raw[..12].bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let zone: i8 = raw[12..].parse().ok()?;
    let day = Date::parse(&raw[..8], CCYYMMDD).ok()?;
    let hour: u8 = raw[8..10].parse().ok()?;
    let minute: u8 = raw[10..12].parse().ok()?;
    let offset = time::UtcOffset::from_hms(zone, 0, 0).ok()?;
    Some(
        day.with_time(time::Time::from_hms(hour, minute, 0).ok()?)
            .assume_offset(offset),
    )
}

#[cfg(test)]
mod tests {
    use super::{kalendertag, parse};
    use mako_fristen::Sparte;
    use time::macros::date;

    /// A `303` process date is the instant the German day begins: in Strom
    /// the evening before in UTC, in Gas the Gastag's 06:00 — and it reads
    /// back as that German day, not as its first eight digits.
    #[test]
    fn a_303_process_date_reads_as_the_german_day_it_begins() {
        assert_eq!(
            parse("202612312300+00", Sparte::Strom),
            Some(date!(2027 - 01 - 01))
        );
        assert_eq!(
            parse("202706302200+00", Sparte::Strom),
            Some(date!(2027 - 07 - 01))
        );
        assert_eq!(
            parse("202710010400+00", Sparte::Gas),
            Some(date!(2027 - 10 - 01))
        );
        // The zone is honoured: 00:00 at UTC+01 is 23:00 UTC the day before.
        assert_eq!(
            parse("202701010000+01", Sparte::Strom),
            Some(date!(2027 - 01 - 01))
        );
    }

    /// An instant that begins no German day in the Sparte names no date —
    /// refused, never truncated or rounded.
    #[test]
    fn a_303_instant_that_begins_no_day_is_refused() {
        assert_eq!(parse("202610010000+00", Sparte::Strom), None);
        assert_eq!(parse("202612312300+00", Sparte::Gas), None);
        assert_eq!(parse("202710010400+00", Sparte::Strom), None);
    }

    #[test]
    fn the_rest_and_json_shapes_still_parse() {
        for sparte in [Sparte::Strom, Sparte::Gas] {
            assert_eq!(parse("20261001", sparte), Some(date!(2026 - 10 - 01)));
            assert_eq!(parse("2026-10-01", sparte), Some(date!(2026 - 10 - 01)));
            assert_eq!(
                parse("2026-10-01T00:00:00Z", sparte),
                Some(date!(2026 - 10 - 01))
            );
        }
        assert_eq!(kalendertag("20261001"), Some(date!(2026 - 10 - 01)));
        assert_eq!(kalendertag("2026-10-01"), Some(date!(2026 - 10 - 01)));
    }

    /// A calendar date reader cannot read a `303` stamp: without the Sparte it
    /// names no German day.
    #[test]
    fn a_calendar_date_is_not_a_303_stamp() {
        assert_eq!(kalendertag("202612312300+00"), None);
    }

    /// An absent date stays a failure — it is the signal that something
    /// upstream dropped it.
    #[test]
    fn nothing_is_not_a_date() {
        assert_eq!(parse("", Sparte::Strom), None);
        assert_eq!(parse("   ", Sparte::Strom), None);
        assert_eq!(parse("not-a-date", Sparte::Strom), None);
    }

    /// A 15-character value that is not a `303` stamp must not be truncated
    /// into a plausible-looking date.
    #[test]
    fn a_malformed_stamp_is_refused() {
        assert_eq!(parse("2026100100000000", Sparte::Strom), None);
        assert_eq!(parse("20261001abcd+00", Sparte::Strom), None);
    }

    /// A multi-byte character straddling the cut is refused, not a panic.
    ///
    /// `"2026-10-0ü"` is eleven bytes with `ü` spanning 9..11, so byte 10 is
    /// inside a character. The value arrives in a `de.mako.process.initiated`
    /// body, which makes this reachable from the wire.
    #[test]
    fn a_date_cut_inside_a_character_is_refused() {
        assert_eq!(parse("2026-10-0ü", Sparte::Strom), None);
        assert_eq!(
            parse("2026-10-01ü", Sparte::Strom),
            Some(date!(2026 - 10 - 01))
        );
        assert_eq!(parse("ÄÖÜäöüß", Sparte::Strom), None);
    }
}
