//! `DTM` values, interpreted through the format code that accompanies them.
//!
//! Every DVGW `DTM` is a triple — qualifier, value, **format code** — and the
//! format code is what says how to read the value:
//!
//! ```text
//! DTM+Z05:0:805'                                  ← timezone, value is an hour offset
//! DTM+137:201801011200:203'                       ← CCYYMMDDHHMM
//! DTM+Z01:201801010500201801020500:719'           ← a period: two CCYYMMDDHHMM back to back
//! ```
//!
//! The types here decode against the format code and refuse a value that does
//! not match it. `201801011200` is neither `YYYYMMDD` nor ISO 8601, so a reader
//! that guesses the shape books the wrong gas day.

use std::fmt;

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// The `DTM` C507 DE 2379 format codes DVGW uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DtmFormat {
    /// `102` — `CCYYMMDD`.
    Ccyymmdd,
    /// `203` — `CCYYMMDDHHMM`.
    Ccyymmddhhmm,
    /// `719` — `CCYYMMDDHHMMCCYYMMDDHHMM`, a start/end period.
    Period,
    /// `805` — a whole number of hours (used by `DTM+Z05` for the timezone).
    Hours,
}

impl DtmFormat {
    /// Parse a DE 2379 code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "102" => Some(Self::Ccyymmdd),
            "203" => Some(Self::Ccyymmddhhmm),
            "719" => Some(Self::Period),
            "805" => Some(Self::Hours),
            _ => None,
        }
    }

    /// The DE 2379 wire code.
    #[must_use]
    pub fn as_code(self) -> &'static str {
        match self {
            Self::Ccyymmdd => "102",
            Self::Ccyymmddhhmm => "203",
            Self::Period => "719",
            Self::Hours => "805",
        }
    }
}

/// A half-open period `[start, end)` from a format-`719` value.
///
/// A DVGW gas day is `DTM+Z01:CCYYMMDD0500CCYYMMDD0500:719` — 05:00 UTC, which is
/// the 06:00 CET gas-day boundary in winter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DvgwPeriod {
    /// Inclusive start.
    pub start: OffsetDateTime,
    /// Exclusive end.
    pub end: OffsetDateTime,
}

impl DvgwPeriod {
    /// `true` when the period runs forwards.
    ///
    /// An inverted or empty period is a message defect, not something to
    /// normalise away, so this is exposed rather than corrected.
    #[must_use]
    pub fn is_forward(&self) -> bool {
        self.start < self.end
    }

    /// The period's length.
    #[must_use]
    pub fn duration(&self) -> time::Duration {
        self.end - self.start
    }
}

impl fmt::Display for DvgwPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} .. {}", self.start, self.end)
    }
}

/// A decoded `DTM` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase", tag = "kind"))]
pub enum DtmValue {
    /// A point in time, from format `102` or `203`.
    Instant(OffsetDateTime),
    /// A period, from format `719`.
    Period(DvgwPeriod),
    /// A whole number of hours, from format `805`.
    Hours(i8),
}

impl DtmValue {
    /// The instant, when this value is one.
    #[must_use]
    pub fn as_instant(self) -> Option<OffsetDateTime> {
        match self {
            Self::Instant(t) => Some(t),
            _ => None,
        }
    }

    /// The period, when this value is one.
    #[must_use]
    pub fn as_period(self) -> Option<DvgwPeriod> {
        match self {
            Self::Period(p) => Some(p),
            _ => None,
        }
    }

    /// The hour count, when this value is one.
    #[must_use]
    pub fn as_hours(self) -> Option<i8> {
        match self {
            Self::Hours(h) => Some(h),
            _ => None,
        }
    }
}

/// Decode a `DTM` value against its format code, in the interchange's timezone.
///
/// `offset` is the zone declared by `DTM+Z05` (`0` = UTC). All DVGW timestamps
/// are wall-clock readings in that zone, so it is applied rather than assumed.
///
/// Returns `None` when the value does not match the shape its own format code
/// declares — a malformed message, reported as a validation finding rather than
/// coerced into a plausible-looking timestamp.
#[must_use]
pub fn decode(value: &str, format: DtmFormat, offset: UtcOffset) -> Option<DtmValue> {
    match format {
        // Each format is held to its own width. Sharing a length-keyed reader
        // let `DTM+137:20180101:203` decode as midnight, which is the failure
        // this module exists to prevent: the value and its declared format
        // disagree, and the message should say so rather than pick a plausible
        // reading.
        DtmFormat::Ccyymmdd => (value.len() == 8)
            .then(|| parse_datetime(value, offset))
            .flatten()
            .map(DtmValue::Instant),
        DtmFormat::Ccyymmddhhmm => (value.len() == 12)
            .then(|| parse_datetime(value, offset))
            .flatten()
            .map(DtmValue::Instant),
        DtmFormat::Period => {
            // Exactly two CCYYMMDDHHMM stamps, no separator.
            if value.len() != 24 || !value.is_ascii() {
                return None;
            }
            let start = parse_datetime(&value[..12], offset)?;
            let end = parse_datetime(&value[12..], offset)?;
            Some(DtmValue::Period(DvgwPeriod { start, end }))
        }
        DtmFormat::Hours => value.parse::<i8>().ok().map(DtmValue::Hours),
    }
}

/// Parse `CCYYMMDD` or `CCYYMMDDHHMM` at `offset`.
///
/// ASCII is checked before any slicing: the value is untrusted wire data and a
/// byte-index split through a multi-byte character would panic.
fn parse_datetime(s: &str, offset: UtcOffset) -> Option<OffsetDateTime> {
    if !s.is_ascii() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (date_part, time_part) = match s.len() {
        8 => (s, "0000"),
        12 => (&s[..8], &s[8..]),
        _ => return None,
    };
    let year: i32 = date_part[..4].parse().ok()?;
    let month = Month::try_from(date_part[4..6].parse::<u8>().ok()?).ok()?;
    let day: u8 = date_part[6..8].parse().ok()?;
    let hour: u8 = time_part[..2].parse().ok()?;
    let minute: u8 = time_part[2..].parse().ok()?;

    let date = Date::from_calendar_date(year, month, day).ok()?;
    // Hour 24 is a legal EDIFACT end-of-day; `time` refuses it, so it is
    // normalised to 00:00 of the following day — the same instant.
    let (date, hour) = if hour == 24 && minute == 0 {
        (date.next_day()?, 0)
    } else {
        (date, hour)
    };
    let clock = Time::from_hms(hour, minute, 0).ok()?;
    Some(PrimitiveDateTime::new(date, clock).assume_offset(offset))
}

/// Render an instant as a format-`203` value (`CCYYMMDDHHMM`) in `offset`.
#[must_use]
pub fn format_instant(value: OffsetDateTime, offset: UtcOffset) -> String {
    let v = value.to_offset(offset);
    format!(
        "{:04}{:02}{:02}{:02}{:02}",
        v.year(),
        v.month() as u8,
        v.day(),
        v.hour(),
        v.minute()
    )
}

/// Render a period as a format-`719` value in `offset`.
#[must_use]
pub fn format_period(period: DvgwPeriod, offset: UtcOffset) -> String {
    let mut s = format_instant(period.start, offset);
    s.push_str(&format_instant(period.end, offset));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn decodes_the_gas_day_period_from_a_real_alocat() {
        let v = decode(
            "201801010500201801020500",
            DtmFormat::Period,
            UtcOffset::UTC,
        )
        .expect("format 719 must decode");
        let p = v.as_period().unwrap();
        assert_eq!(p.start, datetime!(2018-01-01 05:00 UTC));
        assert_eq!(p.end, datetime!(2018-01-02 05:00 UTC));
        assert!(p.is_forward());
        assert_eq!(p.duration(), time::Duration::hours(24));
    }

    #[test]
    fn decodes_the_message_timestamp() {
        let v = decode("201801011200", DtmFormat::Ccyymmddhhmm, UtcOffset::UTC).unwrap();
        assert_eq!(v.as_instant().unwrap(), datetime!(2018-01-01 12:00 UTC));
    }

    #[test]
    fn decodes_the_timezone_declaration() {
        assert_eq!(
            decode("0", DtmFormat::Hours, UtcOffset::UTC)
                .unwrap()
                .as_hours(),
            Some(0)
        );
        assert_eq!(
            decode("1", DtmFormat::Hours, UtcOffset::UTC)
                .unwrap()
                .as_hours(),
            Some(1)
        );
    }

    /// The old reader guessed `YYYY-MM-DD` then `YYYYMMDD` and fell back to
    /// today. Refusing is the whole point: a wrong gas day is silent corruption.
    #[test]
    fn refuses_values_that_do_not_match_their_format_code() {
        assert_eq!(
            decode("2018-01-01", DtmFormat::Ccyymmdd, UtcOffset::UTC),
            None
        );
        assert_eq!(
            decode("201801011200", DtmFormat::Period, UtcOffset::UTC),
            None
        );
        assert_eq!(
            decode("20180132", DtmFormat::Ccyymmdd, UtcOffset::UTC),
            None
        );
        assert_eq!(decode("", DtmFormat::Ccyymmddhhmm, UtcOffset::UTC), None);
    }

    /// Untrusted wire bytes must never panic the parser on a byte-index split.
    #[test]
    fn non_ascii_values_are_rejected_without_panicking() {
        assert_eq!(
            decode("2018ü101", DtmFormat::Ccyymmdd, UtcOffset::UTC),
            None
        );
        let twelve_wide = "ü".repeat(12);
        assert_eq!(
            decode(&twelve_wide, DtmFormat::Ccyymmddhhmm, UtcOffset::UTC),
            None
        );
        let twentyfour_wide = "ü".repeat(12);
        assert_eq!(
            decode(&twentyfour_wide, DtmFormat::Period, UtcOffset::UTC),
            None
        );
    }

    #[test]
    fn hour_24_is_the_next_midnight() {
        let v = decode("201801012400", DtmFormat::Ccyymmddhhmm, UtcOffset::UTC).unwrap();
        assert_eq!(v.as_instant().unwrap(), datetime!(2018-01-02 00:00 UTC));
    }

    #[test]
    fn rendering_round_trips_through_decoding() {
        let period = DvgwPeriod {
            start: datetime!(2026-03-01 05:00 UTC),
            end: datetime!(2026-03-02 05:00 UTC),
        };
        let wire = format_period(period, UtcOffset::UTC);
        assert_eq!(wire, "202603010500202603020500");
        assert_eq!(
            decode(&wire, DtmFormat::Period, UtcOffset::UTC)
                .unwrap()
                .as_period(),
            Some(period)
        );
    }

    #[test]
    fn a_declared_offset_is_applied_not_assumed() {
        let plus_one = UtcOffset::from_hms(1, 0, 0).unwrap();
        let v = decode("202603010600", DtmFormat::Ccyymmddhhmm, plus_one).unwrap();
        assert_eq!(v.as_instant().unwrap(), datetime!(2026-03-01 05:00 UTC));
    }
}
