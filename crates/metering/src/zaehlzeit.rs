//! Zählzeitdefinition — time-variable register assignment (§14a EnWG / UTILTS).
//!
//! A Zählzeitdefinition names, per market location, when which register
//! (Zählzeitregister) counts: the NB defines the windows (e.g. §14a Modul 3
//! time-variable network tariffs, or classic HT/NT beyond the simple
//! two-band case) and communicates them via UTILTS; billing then splits
//! energy by resolving each interval's timestamp to a register.
//!
//! [`crate::tariff_window`] models the simple static HT/NT case; this module
//! models the general shape: an identified definition with validity, holding
//! ordered windows over (months × day group × time band) that resolve to a
//! register ID, plus a fallback register for uncovered times.
//!
//! Resolution is DST-correct: timestamps are converted to Europe/Berlin
//! before matching, like [`crate::tariff_window::TariffWindow::is_ht`].

use rust_decimal::Decimal;
use time::OffsetDateTime;
use time_tz::{OffsetDateTimeExt as _, timezones};

use crate::tariff_window::TariffWindowDays;

/// One resolution window of a Zählzeitdefinition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ZaehlzeitFenster {
    /// Register this window books into (e.g. `"HT"`, `"NT"`, `"ZZ1"`).
    pub register_id: String,
    /// Months the window is active in, as a bitmask (bit 0 = January).
    /// `ALL_MONTHS` for a season-independent window.
    pub months_mask: u16,
    /// Day group the window applies to.
    pub days: TariffWindowDays,
    /// Window start in Berlin local time, minutes since midnight (inclusive).
    pub from_minute: u16,
    /// Window end in Berlin local time, minutes since midnight (exclusive).
    /// Windows crossing midnight are split into two `ZaehlzeitFenster`.
    pub to_minute: u16,
}

/// All twelve months active.
pub const ALL_MONTHS: u16 = 0x0FFF;

impl ZaehlzeitFenster {
    /// `true` when the Berlin-local (month, weekday, minute) falls inside.
    #[must_use]
    fn matches(&self, month0: u8, weekday: time::Weekday, minute: u16) -> bool {
        self.months_mask & (1 << month0) != 0
            && self.days.is_ht_day(weekday)
            && minute >= self.from_minute
            && minute < self.to_minute
    }
}

/// A named Zählzeitdefinition with validity.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Zaehlzeitdefinition {
    /// NB-assigned identifier (UTILTS Zählzeitdefinitions-ID).
    pub id: String,
    /// First day the definition applies (inclusive, German calendar day).
    pub valid_from: time::Date,
    /// Last day (inclusive); `None` = open-ended.
    pub valid_to: Option<time::Date>,
    /// Ordered windows — the first match wins.
    pub windows: Vec<ZaehlzeitFenster>,
    /// Register for times no window covers (classic NT-as-rest).
    pub fallback_register: Option<String>,
}

impl Zaehlzeitdefinition {
    /// `true` when the definition is valid on the given German calendar day.
    #[must_use]
    pub fn is_valid_on(&self, date: time::Date) -> bool {
        date >= self.valid_from && self.valid_to.is_none_or(|end| date <= end)
    }

    /// Resolve a UTC timestamp to the register it books into.
    ///
    /// Conversion to Europe/Berlin happens here, so the caller passes plain
    /// UTC interval starts; DST transitions resolve to the correct local
    /// window automatically. Returns `None` when the definition is not valid
    /// on that day or no window and no fallback covers the time.
    #[must_use]
    pub fn register_for(&self, ts_utc: OffsetDateTime) -> Option<&str> {
        let berlin = ts_utc.to_timezone(timezones::db::europe::BERLIN);
        if !self.is_valid_on(berlin.date()) {
            return None;
        }
        let month0 = u8::from(berlin.month()) - 1;
        let minute = u16::from(berlin.hour()) * 60 + u16::from(berlin.minute());
        self.windows
            .iter()
            .find(|w| w.matches(month0, berlin.weekday(), minute))
            .map(|w| w.register_id.as_str())
            .or(self.fallback_register.as_deref())
    }

    /// Split a set of intervals into per-register energy sums.
    ///
    /// Non-billable intervals are excluded; intervals outside the definition's
    /// validity or coverage land in the `None` bucket so the caller can see
    /// unassigned energy instead of silently losing it.
    #[must_use]
    pub fn split_energy(
        &self,
        intervals: &[crate::interval::MeterInterval],
    ) -> std::collections::BTreeMap<Option<String>, Decimal> {
        let mut sums: std::collections::BTreeMap<Option<String>, Decimal> =
            std::collections::BTreeMap::new();
        for iv in intervals.iter().filter(|iv| iv.quality.is_billable()) {
            let register = self.register_for(iv.from).map(str::to_owned);
            *sums.entry(register).or_insert(Decimal::ZERO) += iv.value_kwh;
        }
        sums
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::{MeterInterval, QualityFlag};
    use rust_decimal::dec;
    use time::macros::{date, datetime};

    /// Winter-HT weekdays 06:00–22:00; rest NT.
    fn winter_ht() -> Zaehlzeitdefinition {
        Zaehlzeitdefinition {
            id: "ZZD-TEST-1".to_owned(),
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            windows: vec![ZaehlzeitFenster {
                register_id: "HT".to_owned(),
                // November–March
                months_mask: 0b0000_0110_0000_0111,
                days: TariffWindowDays::WeekdaysOnly,
                from_minute: 6 * 60,
                to_minute: 22 * 60,
            }],
            fallback_register: Some("NT".to_owned()),
        }
    }

    #[test]
    fn winter_weekday_morning_is_ht_summer_is_nt() {
        let zzd = winter_ht();
        // Thursday 2026-01-15 08:00 Berlin = 07:00 UTC.
        assert_eq!(
            zzd.register_for(datetime!(2026-01-15 07:00 UTC)),
            Some("HT")
        );
        // Same clock time in July (CEST): month not in mask → fallback NT.
        assert_eq!(
            zzd.register_for(datetime!(2026-07-16 06:00 UTC)),
            Some("NT")
        );
        // Winter Sunday → fallback NT.
        assert_eq!(
            zzd.register_for(datetime!(2026-01-18 07:00 UTC)),
            Some("NT")
        );
    }

    #[test]
    fn dst_boundary_resolves_in_berlin_local_time() {
        let zzd = winter_ht();
        // 2026-03-29 (spring forward): 05:30 UTC = 07:30 CEST → March, weekday
        // (Sunday!) — 2026-03-29 is a Sunday, so NT. Use Monday 03-30: but
        // March 30 is in the mask; 05:30 UTC = 07:30 CEST → HT despite the
        // UTC hour being before 06:00.
        assert_eq!(
            zzd.register_for(datetime!(2026-03-30 05:30 UTC)),
            Some("HT")
        );
    }

    #[test]
    fn energy_split_books_every_billable_interval() {
        let zzd = winter_ht();
        let iv = |from: OffsetDateTime, kwh| MeterInterval {
            from,
            to: from + time::Duration::minutes(15),
            value_kwh: kwh,
            quality: QualityFlag::Measured,
            obis_code: None,
        };
        let split = zzd.split_energy(&[
            iv(datetime!(2026-01-15 07:00 UTC), dec!(2)), // HT
            iv(datetime!(2026-01-15 23:30 UTC), dec!(1)), // NT (00:30 Berlin next day)
            iv(datetime!(2026-01-18 07:00 UTC), dec!(4)), // Sunday → NT
        ]);
        assert_eq!(split.get(&Some("HT".to_owned())), Some(&dec!(2)));
        assert_eq!(split.get(&Some("NT".to_owned())), Some(&dec!(5)));
        assert!(!split.contains_key(&None), "everything was covered");
    }

    #[test]
    fn validity_bounds_are_inclusive() {
        let mut zzd = winter_ht();
        zzd.valid_to = Some(date!(2026 - 06 - 30));
        assert!(zzd.register_for(datetime!(2026-07-01 10:00 UTC)).is_none());
        assert!(zzd.register_for(datetime!(2026-06-30 10:00 UTC)).is_some());
    }
}
