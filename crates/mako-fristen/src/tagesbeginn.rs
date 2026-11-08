//! A German calendar date and the UTC instant an EDIFACT process date carries
//! for it.
//!
//! Allgemeine Festlegungen V6.1d Kap. 3 (S. 28–29): every time in an EDIFACT
//! message is UTC, every point in time a process names is gesetzliche deutsche
//! Zeit, and the sender converts one into the other when it builds the message
//! and the receiver converts back. For a spartenspezifische prozessuale
//! Zeitangabe in DE 2379 format `303` the document admits exactly four clock
//! times: „2300" and „2200" in the Sparte Strom (00:00 MEZ / MESZ), „0500" and
//! „0400" in the Sparte Gas (06:00 MEZ / MESZ). Its own example: an Anmeldung
//! Netznutzung zum 01.06.2021 is 31.05.2021 22:00 UTC in Strom and 01.06.2021
//! 04:00 UTC in Gas.
//!
//! So a Lieferbeginn on 1 January is `DTM+92:202012312300?+00:303` — the UTC
//! date is the day *before*. Writing the German date with `0000` appended
//! states an instant one or two hours into the Lieferbeginn in Strom and four
//! or five hours before the Gastag in Gas, and reading the first eight digits
//! of an inbound stamp answers the day before in Strom. [`tagesbeginn_utc`]
//! and [`tag_des_beginns`] are the one conversion each way.

use core::fmt;

use time::{Date, OffsetDateTime, Time, UtcOffset};

use crate::{berlin_at, berlin_date, berlin_midnight, in_berlin};

/// The Sparte a process date belongs to — it decides where the day begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sparte {
    /// Strom: the day begins at 00:00 gesetzliche deutsche Zeit.
    Strom,
    /// Gas: the Gastag begins at 06:00 gesetzliche deutsche Zeit.
    Gas,
}

impl Sparte {
    /// The German wall-clock time at which this Sparte's day begins.
    #[must_use]
    pub const fn tagesbeginn(self) -> Time {
        match self {
            Self::Strom => Time::MIDNIGHT,
            Self::Gas => GASTAG_BEGINN,
        }
    }
}

/// 06:00 — the beginning of the Gastag in gesetzlicher deutscher Zeit.
const GASTAG_BEGINN: Time = match Time::from_hms(6, 0, 0) {
    Ok(t) => t,
    Err(_) => panic!("06:00:00 is a valid time"),
};

/// The UTC instant at which the German day `date` begins in `sparte` — the
/// value a `303` process date (`DTM+92` Beginn zum, `DTM+93` Ende zum, …)
/// carries for it.
///
/// Strom: 00:00 Europe/Berlin, i.e. `2300` (MEZ) or `2200` (MESZ) UTC of the
/// previous day. Gas: 06:00 Europe/Berlin, `0500` / `0400` UTC of the same day.
///
/// ```rust
/// use mako_fristen::{Sparte, tagesbeginn_utc};
/// use time::macros::{date, datetime};
///
/// assert_eq!(tagesbeginn_utc(date!(2027-01-01), Sparte::Strom), datetime!(2026-12-31 23:00 UTC));
/// assert_eq!(tagesbeginn_utc(date!(2027-07-01), Sparte::Strom), datetime!(2027-06-30 22:00 UTC));
/// assert_eq!(tagesbeginn_utc(date!(2027-10-01), Sparte::Gas), datetime!(2027-10-01 04:00 UTC));
/// ```
///
/// # Panics
///
/// Under the same conditions as [`berlin_at`]; neither 00:00 nor 06:00 is ever
/// inside the 02:00–03:00 DST gap.
#[must_use]
pub fn tagesbeginn_utc(date: Date, sparte: Sparte) -> OffsetDateTime {
    match sparte {
        Sparte::Strom => berlin_midnight(date),
        Sparte::Gas => berlin_at(date, GASTAG_BEGINN).to_offset(UtcOffset::UTC),
    }
}

/// The German date whose day (Strom) or Gastag (Gas) begins at `instant` —
/// the exact inverse of [`tagesbeginn_utc`].
///
/// An instant that is not such a beginning is refused, never rounded: an
/// inbound `DTM+92:202612312300?+00:303` on a Strom Anmeldung is 01.01.2027,
/// but `202612310000?+00` names no day at all in either Sparte, and taking its
/// first eight digits would settle the Lieferbeginn on a date the sender may
/// not have meant.
///
/// ```rust
/// use mako_fristen::{Sparte, tag_des_beginns};
/// use time::macros::{date, datetime};
///
/// assert_eq!(tag_des_beginns(datetime!(2026-12-31 23:00 UTC), Sparte::Strom), Ok(date!(2027-01-01)));
/// assert_eq!(tag_des_beginns(datetime!(2027-10-01 04:00 UTC), Sparte::Gas), Ok(date!(2027-10-01)));
/// assert!(tag_des_beginns(datetime!(2026-12-31 00:00 UTC), Sparte::Strom).is_err());
/// ```
///
/// # Errors
///
/// [`KeinTagesbeginn`] when `instant` is not the beginning of a German day in
/// `sparte`.
pub fn tag_des_beginns(instant: OffsetDateTime, sparte: Sparte) -> Result<Date, KeinTagesbeginn> {
    let date = match sparte {
        Sparte::Strom => berlin_date(instant),
        Sparte::Gas => in_berlin(instant).date(),
    };
    if tagesbeginn_utc(date, sparte) == instant {
        Ok(date)
    } else {
        Err(KeinTagesbeginn { instant, sparte })
    }
}

/// An instant handed to [`tag_des_beginns`] that does not begin a German day in
/// its Sparte — e.g. UTC midnight, which is 01:00 or 02:00 Berlin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeinTagesbeginn {
    /// The instant that was refused.
    pub instant: OffsetDateTime,
    /// The Sparte whose day boundary it was held against.
    pub sparte: Sparte,
}

impl fmt::Display for KeinTagesbeginn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let erwartet = match self.sparte {
            Sparte::Strom => "00:00 (2300/2200 UTC)",
            Sparte::Gas => "06:00 (0500/0400 UTC)",
        };
        let u = self.instant.to_offset(UtcOffset::UTC);
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}Z is not the beginning of a German day in the Sparte \
             {:?}: a process date must fall on {erwartet} gesetzliche deutsche Zeit (Allgemeine \
             Festlegungen V6.1d Kap. 3.1)",
            u.year(),
            u.month() as u8,
            u.day(),
            u.hour(),
            u.minute(),
            self.sparte
        )
    }
}

impl std::error::Error for KeinTagesbeginn {}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    /// Both DST changes of 2026 and 2027, the days either side of each, and
    /// the AF's own year boundary.
    const DAYS: [(Date, OffsetDateTime, OffsetDateTime); 10] = [
        // date, Strom start, Gas start
        (
            date!(2026 - 03 - 28),
            datetime!(2026-03-27 23:00 UTC),
            datetime!(2026-03-28 05:00 UTC),
        ),
        // 29.03.2026: the clock jumps at 02:00 — midnight is still MEZ, 06:00 is MESZ.
        (
            date!(2026 - 03 - 29),
            datetime!(2026-03-28 23:00 UTC),
            datetime!(2026-03-29 04:00 UTC),
        ),
        (
            date!(2026 - 03 - 30),
            datetime!(2026-03-29 22:00 UTC),
            datetime!(2026-03-30 04:00 UTC),
        ),
        // 25.10.2026: back at 03:00 — midnight is still MESZ, 06:00 is MEZ.
        (
            date!(2026 - 10 - 25),
            datetime!(2026-10-24 22:00 UTC),
            datetime!(2026-10-25 05:00 UTC),
        ),
        (
            date!(2026 - 10 - 26),
            datetime!(2026-10-25 23:00 UTC),
            datetime!(2026-10-26 05:00 UTC),
        ),
        (
            date!(2027 - 01 - 01),
            datetime!(2026-12-31 23:00 UTC),
            datetime!(2027-01-01 05:00 UTC),
        ),
        (
            date!(2027 - 03 - 28),
            datetime!(2027-03-27 23:00 UTC),
            datetime!(2027-03-28 04:00 UTC),
        ),
        (
            date!(2027 - 03 - 29),
            datetime!(2027-03-28 22:00 UTC),
            datetime!(2027-03-29 04:00 UTC),
        ),
        (
            date!(2027 - 10 - 31),
            datetime!(2027-10-30 22:00 UTC),
            datetime!(2027-10-31 05:00 UTC),
        ),
        (
            date!(2027 - 11 - 01),
            datetime!(2027-10-31 23:00 UTC),
            datetime!(2027-11-01 05:00 UTC),
        ),
    ];

    #[test]
    fn tagesbeginn_across_both_dst_changes() {
        for (day, strom, gas) in DAYS {
            assert_eq!(tagesbeginn_utc(day, Sparte::Strom), strom, "Strom {day}");
            assert_eq!(tagesbeginn_utc(day, Sparte::Gas), gas, "Gas {day}");
        }
    }

    #[test]
    fn tag_des_beginns_inverts_tagesbeginn() {
        for (day, strom, gas) in DAYS {
            assert_eq!(
                tag_des_beginns(strom, Sparte::Strom),
                Ok(day),
                "Strom {day}"
            );
            assert_eq!(tag_des_beginns(gas, Sparte::Gas), Ok(day), "Gas {day}");
        }
    }

    /// Every day of 2026 and 2027 round-trips in both Sparten.
    #[test]
    fn round_trip_every_day() {
        let mut day = date!(2026 - 01 - 01);
        while day <= date!(2027 - 12 - 31) {
            for sparte in [Sparte::Strom, Sparte::Gas] {
                let instant = tagesbeginn_utc(day, sparte);
                assert_eq!(instant.offset(), UtcOffset::UTC);
                assert_eq!(tag_des_beginns(instant, sparte), Ok(day));
            }
            day = day.next_day().expect("in range");
        }
    }

    /// UTC midnight — the shape a padded `CCYYMMDD` produces — and the other
    /// Sparte's boundary are refused, not rounded.
    #[test]
    fn a_non_boundary_is_refused() {
        for (instant, sparte) in [
            (datetime!(2026-12-31 00:00 UTC), Sparte::Strom),
            (datetime!(2027-07-01 00:00 UTC), Sparte::Strom),
            (datetime!(2027-10-01 00:00 UTC), Sparte::Gas),
            // the Strom boundary in the Gas Sparte, and vice versa
            (datetime!(2027-09-30 22:00 UTC), Sparte::Gas),
            (datetime!(2027-10-01 04:00 UTC), Sparte::Strom),
            // one minute off
            (datetime!(2026-12-31 23:01 UTC), Sparte::Strom),
        ] {
            assert_eq!(
                tag_des_beginns(instant, sparte),
                Err(KeinTagesbeginn { instant, sparte }),
                "{instant} {sparte:?}"
            );
        }
    }

    /// The Allgemeine Festlegungen' own example: Anmeldung zum 01.06.2021.
    #[test]
    fn the_festlegungen_example() {
        let day = date!(2021 - 06 - 01);
        assert_eq!(
            tagesbeginn_utc(day, Sparte::Strom),
            datetime!(2021-05-31 22:00 UTC)
        );
        assert_eq!(
            tagesbeginn_utc(day, Sparte::Gas),
            datetime!(2021-06-01 04:00 UTC)
        );
    }
}
