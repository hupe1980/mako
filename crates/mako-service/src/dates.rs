//! The one parser for a calendar date a caller sends.
//!
//! A date in a request is either well-formed or refused. Answering a malformed
//! one with `None` turns `"zeitraum_bis": "bald"` into an open-ended period and
//! `"beendigung_zum": "31.12."` into „now" — the request is then executed as
//! something the caller did not ask for, and nothing tells them.

use time::Date;
use time::OffsetDateTime;
use time::format_description::well_known::{Iso8601, Rfc3339};

/// A date field that is not an ISO 8601 date.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{field}` must be an ISO 8601 date (YYYY-MM-DD), got {raw:?}")]
pub struct DateError {
    /// The field the caller named, so the refusal points at it.
    pub field: String,
    /// What arrived.
    pub raw: String,
}

impl From<DateError> for crate::ApiError {
    fn from(e: DateError) -> Self {
        Self::bad_request(e.to_string())
    }
}

/// Parse `raw` as a calendar date, refusing anything else.
///
/// Accepts `YYYY-MM-DD`, and an RFC 3339 date-time — the shape a BO4E
/// `Zeitraum` carries — whose date is taken as written. Everything else,
/// including an empty string, is a [`DateError`] naming `field`.
///
/// ```rust
/// use mako_service::dates::parse_iso_date;
/// use time::macros::date;
///
/// assert_eq!(parse_iso_date("bis", "2027-06-30").unwrap(), date!(2027 - 06 - 30));
/// assert_eq!(
///     parse_iso_date("bis", "2027-06-30T00:00:00Z").unwrap(),
///     date!(2027 - 06 - 30)
/// );
/// assert!(parse_iso_date("bis", "bald").is_err());
/// ```
///
/// # Errors
///
/// [`DateError`] when `raw` is neither form.
pub fn parse_iso_date(field: &str, raw: &str) -> Result<Date, DateError> {
    let refused = || DateError {
        field: field.to_owned(),
        raw: raw.to_owned(),
    };
    if raw.len() == 10 {
        return Date::parse(raw, &Iso8601::DATE).map_err(|_| refused());
    }
    OffsetDateTime::parse(raw, &Rfc3339)
        .map(OffsetDateTime::date)
        .map_err(|_| refused())
}

#[cfg(test)]
mod tests {
    use super::parse_iso_date;
    use time::macros::date;

    #[test]
    fn a_date_and_a_date_time_are_read() {
        assert_eq!(parse_iso_date("f", "2027-06-30"), Ok(date!(2027 - 06 - 30)));
        assert_eq!(
            parse_iso_date("f", "2027-06-30T12:00:00+02:00"),
            Ok(date!(2027 - 06 - 30))
        );
    }

    /// The defect this module exists for: a malformed date is a refusal that
    /// names the field, never a silent absence.
    #[test]
    fn a_malformed_date_is_refused_and_names_the_field() {
        for raw in [
            "bald",
            "",
            "30.06.2027",
            "2027-13-01",
            "2027-06-30Tlater",
            "2027-06-3",
        ] {
            let err = parse_iso_date("zeitraum_bis", raw).unwrap_err();
            assert_eq!(err.field, "zeitraum_bis", "{raw}");
            assert!(err.to_string().contains("zeitraum_bis"), "{raw}");
        }
    }
}
