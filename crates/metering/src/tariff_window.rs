//! HT/NT tariff window calendar for German electricity meters.
//!
//! ## Purpose
//!
//! German residential and small commercial electricity meters with two tariff
//! registers (Zweitarif) use time-of-use windows to distinguish:
//!
//! - **HT (Hochtarif / High Tariff)** — daytime hours when grid is under higher load
//! - **NT (Niedertarif / Low Tariff)** — nighttime hours when grid load is lower
//!
//! The exact HT/NT schedule is defined by the local DSO (NB) and published in
//! their Preisblatt. There is no national standard — each NB sets their own windows.
//!
//! ## Relationship to `metering::aggregation`
//!
//! [`metering::aggregation`] uses pre-defined HT/NT schedules from `HtNtSchedule`
//! to split 15-min interval energy into HT vs NT registers for billing.
//!
//! ## DST handling
//!
//! All window boundaries are expressed in **German local time** (CET = UTC+1,
//! CEST = UTC+2). The `contains_utc()` method converts the UTC timestamp to
//! German local time before checking the window boundaries.
//!
//! ## Common German schedules
//!
//! | Region / DSO | HT window (local time) | Source |
//! |---|---|---|
//! | Most German DSOs | Mon–Fri 06:00–22:00 | BDEW Musterleistungsbeschreibung |
//! | Some east German DSOs | Mon–Fri 06:00–21:00 | Regional variant |
//! | Industry tariffs | Mon–Sat 06:00–22:00 | Includes Saturday |

use time::{OffsetDateTime, Weekday};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// ── TariffWindowDay ───────────────────────────────────────────────────────────

/// Which days-of-week apply to a tariff window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TariffWindowDays {
    /// Monday through Friday only.
    WeekdaysOnly,
    /// Monday through Saturday.
    WeekdaysAndSaturday,
    /// All seven days (no NT/HT distinction by day).
    AllDays,
}

impl TariffWindowDays {
    /// `true` when `weekday` is an HT day for this schedule.
    #[must_use]
    pub fn is_ht_day(self, weekday: Weekday) -> bool {
        match self {
            Self::WeekdaysOnly => matches!(
                weekday,
                Weekday::Monday
                    | Weekday::Tuesday
                    | Weekday::Wednesday
                    | Weekday::Thursday
                    | Weekday::Friday
            ),
            Self::WeekdaysAndSaturday => !matches!(weekday, Weekday::Sunday),
            Self::AllDays => true,
        }
    }
}

// ── TariffWindow ─────────────────────────────────────────────────────────────

/// A single HT time window: `[hour_from, hour_to)` in German local time.
///
/// Both bounds are hours in 24h notation (0–23):
/// - `hour_from = 6, hour_to = 22` → HT from 06:00 to 22:00 (22:00 is NT)
/// - Minute-level precision: an interval starting at 06:00:00 local is HT,
///   one starting at 05:45:00 local is NT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TariffWindow {
    /// Start of HT period (inclusive, 0–23).
    pub hour_from: u8,
    /// End of HT period (exclusive, 1–24).
    pub hour_to: u8,
    /// Which days count as HT days.
    pub days: TariffWindowDays,
}

impl TariffWindow {
    /// Standard BDEW Musterleistungsbeschreibung: Mon–Fri 06:00–22:00 local.
    pub const BDEW_STANDARD: Self = Self {
        hour_from: 6,
        hour_to: 22,
        days: TariffWindowDays::WeekdaysOnly,
    };

    /// Extended variant including Saturday: Mon–Sat 06:00–22:00 local.
    pub const MON_SAT_0600_2200: Self = Self {
        hour_from: 6,
        hour_to: 22,
        days: TariffWindowDays::WeekdaysAndSaturday,
    };

    /// `true` when `ts` (UTC) falls within this HT window.
    ///
    /// Converts `ts` to German local time (CET/CEST) before checking the window.
    /// Handles DST transitions correctly.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use metering::tariff_window::TariffWindow;
    /// use time::macros::datetime;
    ///
    /// let window = TariffWindow::BDEW_STANDARD;
    ///
    /// // Monday 09:00 CET = Monday 08:00 UTC → HT
    /// assert!(window.is_ht(datetime!(2026-01-05 8:00 UTC)));
    ///
    /// // Monday 22:00 CET = Monday 21:00 UTC → NT (exclusive upper bound)
    /// assert!(!window.is_ht(datetime!(2026-01-05 21:00 UTC)));
    ///
    /// // Sunday 12:00 CET = Sunday 11:00 UTC → NT (weekdays only)
    /// assert!(!window.is_ht(datetime!(2026-01-04 11:00 UTC)));
    /// ```
    #[must_use]
    pub fn is_ht(&self, ts: OffsetDateTime) -> bool {
        use time_tz::{OffsetDateTimeExt, timezones};
        let berlin = timezones::db::europe::BERLIN;
        let local = ts.to_timezone(berlin);
        let hour = local.hour();
        let weekday = local.weekday();
        self.days.is_ht_day(weekday) && hour >= self.hour_from && hour < self.hour_to
    }

    /// Classify a UTC timestamp as `"HT"` or `"NT"`.
    ///
    /// Used for invoice display and register assignment.
    #[must_use]
    pub fn register_name(&self, ts: OffsetDateTime) -> &'static str {
        if self.is_ht(ts) { "HT" } else { "NT" }
    }
}

// ── HtNtSchedule ─────────────────────────────────────────────────────────────

/// A complete HT/NT schedule: one or more windows that together define all HT periods.
///
/// Multiple windows are ORed: a timestamp is HT if it falls in ANY window.
/// This allows modeling of split schedules (e.g. morning + evening HT peaks).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HtNtSchedule {
    /// The DSO or NB who published this schedule.
    pub dsop_mp_id: Option<String>,
    /// One or more HT windows. Empty = no HT (everything is NT).
    pub windows: Vec<TariffWindow>,
}

impl HtNtSchedule {
    /// The standard BDEW Musterleistungsbeschreibung schedule.
    #[must_use]
    pub fn bdew_standard() -> Self {
        Self {
            dsop_mp_id: None,
            windows: vec![TariffWindow::BDEW_STANDARD],
        }
    }

    /// `true` when `ts` falls in any HT window.
    #[must_use]
    pub fn is_ht(&self, ts: OffsetDateTime) -> bool {
        self.windows.iter().any(|w| w.is_ht(ts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn bdew_standard_monday_morning_is_ht() {
        let window = TariffWindow::BDEW_STANDARD;
        // Monday 2026-01-05 09:00 CET = 08:00 UTC (winter, CET=UTC+1)
        assert!(
            window.is_ht(datetime!(2026-01-05 8:00 UTC)),
            "Mon 09:00 CET should be HT"
        );
    }

    #[test]
    fn bdew_standard_monday_night_is_nt() {
        let window = TariffWindow::BDEW_STANDARD;
        // Monday 23:00 CET = 22:00 UTC (winter)
        assert!(
            !window.is_ht(datetime!(2026-01-05 22:00 UTC)),
            "Mon 23:00 CET should be NT"
        );
    }

    #[test]
    fn bdew_standard_22_00_is_nt() {
        let window = TariffWindow::BDEW_STANDARD;
        // Monday 22:00 CET = 21:00 UTC — exclusive upper bound, so NT
        assert!(
            !window.is_ht(datetime!(2026-01-05 21:00 UTC)),
            "Mon 22:00 CET exclusive = NT"
        );
    }

    #[test]
    fn bdew_standard_sunday_is_nt() {
        let window = TariffWindow::BDEW_STANDARD;
        // Sunday 12:00 CET = 11:00 UTC
        assert!(
            !window.is_ht(datetime!(2026-01-04 11:00 UTC)),
            "Sunday should be NT"
        );
    }

    #[test]
    fn bdew_standard_saturday_is_nt() {
        let window = TariffWindow::BDEW_STANDARD;
        // Saturday 10:00 CET = 09:00 UTC — weekdays only, Saturday = NT
        assert!(
            !window.is_ht(datetime!(2026-01-03 9:00 UTC)),
            "Saturday should be NT"
        );
    }

    #[test]
    fn mon_sat_schedule_saturday_morning_is_ht() {
        let window = TariffWindow::MON_SAT_0600_2200;
        // Saturday 10:00 CET = 09:00 UTC
        assert!(
            window.is_ht(datetime!(2026-01-03 9:00 UTC)),
            "Sat 10:00 CET should be HT"
        );
    }

    #[test]
    fn dst_summer_time_is_handled() {
        // 2026-06-15 (CEST = UTC+2)
        // Monday 09:00 CEST = 07:00 UTC → HT
        let window = TariffWindow::BDEW_STANDARD;
        assert!(
            window.is_ht(datetime!(2026-06-15 7:00 UTC)),
            "Mon 09:00 CEST should be HT"
        );
        // Monday 22:00 CEST = 20:00 UTC → NT (22:00 exclusive)
        assert!(
            !window.is_ht(datetime!(2026-06-15 20:00 UTC)),
            "Mon 22:00 CEST should be NT"
        );
    }

    #[test]
    fn schedule_is_ht_delegates_to_windows() {
        let schedule = HtNtSchedule::bdew_standard();
        // Mon 09:00 CET = 08:00 UTC
        assert!(schedule.is_ht(datetime!(2026-01-05 8:00 UTC)));
        assert!(!schedule.is_ht(datetime!(2026-01-04 11:00 UTC)));
    }

    #[test]
    fn register_name_returns_correct_string() {
        let window = TariffWindow::BDEW_STANDARD;
        assert_eq!(window.register_name(datetime!(2026-01-05 8:00 UTC)), "HT");
        assert_eq!(window.register_name(datetime!(2026-01-04 11:00 UTC)), "NT");
    }
}
