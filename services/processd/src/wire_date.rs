//! Parsing the date shapes a `de.mako.process.initiated` payload can carry.
//!
//! | Shape | Where it comes from |
//! |---|---|
//! | `YYYYMMDD` | an ERP command through `makod`'s REST API |
//! | `YYYY-MM-DD` | a JSON payload written by hand or by a connector |
//! | `CCYYMMDDHHMM±ZZ` | **EDIFACT** — DE 2379 format code `303` |
//!
//! Every EDI@Energy MIG gives its dates as `303`, so an `SG4 DTM+92` „Beginn
//! zum" reaches the decision modules as `202610010000+00`. One parser rather
//! than one per module, so all three shapes stay recognised everywhere.

use time::Date;

/// Parse a date from any shape a process payload carries.
///
/// The time and zone of a `303` stamp are deliberately discarded: the decision
/// modules compare *calendar* dates against Fristen, and the German market's
/// day boundary is already baked into the value by the sender.
#[must_use]
pub fn parse(raw: &str) -> Option<Date> {
    let raw = raw.trim();
    // `CCYYMMDDHHMM±ZZ` — DE 2379 `303`. Recognised by its zone sign rather
    // than by length alone, so a malformed value falls through to a failure
    // instead of being silently truncated to its first eight digits.
    if raw.len() >= 13
        && raw.is_char_boundary(12)
        && matches!(raw.as_bytes()[12], b'+' | b'-')
        && raw[..12].bytes().all(|b| b.is_ascii_digit())
    {
        return Date::parse(
            &raw[..8],
            time::macros::format_description!("[year][month][day]"),
        )
        .ok();
    }
    if raw.len() == 8 {
        return Date::parse(raw, time::macros::format_description!("[year][month][day]")).ok();
    }
    // `YYYY-MM-DD`, optionally with a time suffix a JSON producer appended.
    Date::parse(
        &raw[..raw.len().min(10)],
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::parse;
    use time::macros::date;

    #[test]
    fn the_edifact_wire_shape_parses() {
        assert_eq!(parse("202610010000+00"), Some(date!(2026 - 10 - 01)));
        assert_eq!(parse("202512312300-01"), Some(date!(2025 - 12 - 31)));
    }

    #[test]
    fn the_rest_and_json_shapes_still_parse() {
        assert_eq!(parse("20261001"), Some(date!(2026 - 10 - 01)));
        assert_eq!(parse("2026-10-01"), Some(date!(2026 - 10 - 01)));
        assert_eq!(parse("2026-10-01T00:00:00Z"), Some(date!(2026 - 10 - 01)));
    }

    /// An absent date stays a failure — it is the signal that something
    /// upstream dropped it.
    #[test]
    fn nothing_is_not_a_date() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
        assert_eq!(parse("not-a-date"), None);
    }

    /// A 15-character value that is not a `303` stamp must not be truncated
    /// into a plausible-looking date.
    #[test]
    fn a_malformed_stamp_is_refused() {
        assert_eq!(parse("2026100100000000"), None);
        assert_eq!(parse("20261001abcd+00"), None);
    }
}
