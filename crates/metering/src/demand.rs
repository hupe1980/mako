//! Demand window modeling for RLM Spitzenleistung billing.
//!
//! ## Legal basis
//!
//! - **§18 Abs. 1 StromNEV**: Grid tariffs for RLM customers include a power-demand
//!   component (Leistungspreis). The billing basis is the highest 15-minute average
//!   demand (Spitzenleistung) within the billing period.
//! - **§2 Nr. 17 MessZV**: "Spitzenleistung" — the maximum arithmetic mean of the
//!   active power in any 15-minute interval during the billing period.
//! - **BDEW Metering Code 2.0**: defines how 15-min intervals are computed from
//!   SMGW or RLM read-out data.
//!
//! ## Demand window concept
//!
//! A `DemandWindow` tracks the **rolling peak demand** over an interval resolution:
//!
//! ```text
//! interval 00:00–00:15 : 35.2 kW avg demand
//! interval 00:15–00:30 : 48.7 kW avg demand  ← peak so far
//! interval 00:30–00:45 : 42.1 kW avg demand
//! ...
//! end of billing period
//!   → Spitzenleistung = 48.7 kW  → used for Leistungspreis billing
//! ```
//!
//! The demand value in kW is computed from an energy interval as:
//! `power_kw = energy_kwh / (interval_duration_hours)`

use rust_decimal::Decimal;
use time::OffsetDateTime;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A single demand measurement over a fixed-length interval.
///
/// Used to track 15-minute average power demand (Leistungsmittelwert) for
/// RLM Spitzenleistung billing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DemandInterval {
    /// Interval start (UTC).
    pub from: OffsetDateTime,
    /// Interval end (UTC).
    pub to: OffsetDateTime,
    /// Average power demand during this interval (kW).
    pub demand_kw: Decimal,
    /// OBIS code of the demand measurement (typically `1-0:1.29.0*255`).
    pub obis_code: Option<crate::obis::ObisCode>,
}

impl DemandInterval {
    /// Compute the demand (kW) from energy (kWh) and interval duration.
    ///
    /// Formula: `kW = kWh / (duration_seconds / 3600)`
    ///
    /// ## Example — 15-min interval with 12 kWh
    ///
    /// ```rust
    /// use metering::demand::DemandInterval;
    /// use rust_decimal_macros::dec;
    ///
    /// let kw = DemandInterval::energy_to_demand_kw(dec!(12.0), 900);
    /// // 12 kWh / (900s / 3600s/h) = 12 / 0.25 = 48 kW
    /// assert_eq!(kw, dec!(48.0));
    /// ```
    #[must_use]
    pub fn energy_to_demand_kw(energy_kwh: Decimal, interval_secs: u32) -> Decimal {
        if interval_secs == 0 {
            return Decimal::ZERO;
        }
        let hours = Decimal::from(interval_secs) / Decimal::from(3600u32);
        energy_kwh / hours
    }
}

/// Tracks the peak demand (Spitzenleistung) over a billing period.
///
/// Per §2 Nr. 17 MessZV and §18 StromNEV: the Spitzenleistung is the **maximum**
/// 15-minute average demand during the billing period. This type accumulates
/// demand intervals and exposes the peak.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DemandWindow {
    /// All demand intervals accumulated for this window.
    intervals: Vec<DemandInterval>,
}

impl DemandWindow {
    /// Create an empty demand window.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a demand interval to the window.
    pub fn push(&mut self, interval: DemandInterval) {
        self.intervals.push(interval);
    }

    /// Peak demand (Spitzenleistung) in kW — maximum across all intervals.
    ///
    /// Returns `None` when no intervals have been added.
    ///
    /// Per §2 Nr. 17 MessZV, this is the billing-relevant Spitzenleistung.
    #[must_use]
    pub fn peak_kw(&self) -> Option<Decimal> {
        self.intervals
            .iter()
            .map(|i| i.demand_kw)
            .reduce(Decimal::max)
    }

    /// Interval containing the peak demand.
    #[must_use]
    pub fn peak_interval(&self) -> Option<&DemandInterval> {
        self.intervals.iter().max_by_key(|i| i.demand_kw)
    }

    /// Number of intervals in this window.
    #[must_use]
    pub fn len(&self) -> usize {
        self.intervals.len()
    }

    /// `true` when the window has no intervals.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    /// Average demand over the billing period (kW).
    ///
    /// Returns `None` when no intervals exist.
    #[must_use]
    pub fn average_kw(&self) -> Option<Decimal> {
        if self.intervals.is_empty() {
            return None;
        }
        let sum: Decimal = self.intervals.iter().map(|i| i.demand_kw).sum();
        Some(sum / Decimal::from(self.intervals.len()))
    }

    /// Build a `DemandWindow` from energy intervals.
    ///
    /// Each `MeterInterval` with a demand-type OBIS code (C=1, D=29) or
    /// with an expected interval length is converted to an average kW value.
    ///
    /// For standard 15-min RLM intervals: `demand_kw = energy_kwh × 4`.
    #[must_use]
    pub fn from_intervals(
        intervals: &[crate::interval::MeterInterval],
        interval_secs: u32,
    ) -> Self {
        let mut window = Self::new();
        for iv in intervals {
            let demand_kw = DemandInterval::energy_to_demand_kw(iv.value_kwh, interval_secs);
            window.push(DemandInterval {
                from: iv.from,
                to: iv.to,
                demand_kw,
                obis_code: iv.parsed_obis_code(),
            });
        }
        window
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::{MeterInterval, QualityFlag};
    use rust_decimal_macros::dec;
    use time::macros::datetime;

    #[test]
    fn energy_to_demand_kw_15min() {
        // 15 kWh in 15 min = 15 / 0.25h = 60 kW
        assert_eq!(DemandInterval::energy_to_demand_kw(dec!(15), 900), dec!(60));
    }

    #[test]
    fn energy_to_demand_kw_hourly() {
        // 10 kWh in 1 hour = 10 kW
        assert_eq!(
            DemandInterval::energy_to_demand_kw(dec!(10), 3600),
            dec!(10)
        );
    }

    #[test]
    fn peak_kw_returns_maximum() {
        let mut window = DemandWindow::new();
        let base = datetime!(2026-01-01 0:00 UTC);

        for (i, kw) in [dec!(30), dec!(48), dec!(35), dec!(22)].iter().enumerate() {
            let from = base + time::Duration::minutes(i as i64 * 15);
            window.push(DemandInterval {
                from,
                to: from + time::Duration::minutes(15),
                demand_kw: *kw,
                obis_code: None,
            });
        }

        // §2 Nr. 17 MessZV: peak = maximum 15-min average demand
        assert_eq!(window.peak_kw(), Some(dec!(48)));
        assert_eq!(window.len(), 4);
    }

    #[test]
    fn from_intervals_15min_conversion() {
        let base = datetime!(2026-01-01 0:00 UTC);
        let intervals = vec![MeterInterval {
            from: base,
            to: base + time::Duration::minutes(15),
            value_kwh: dec!(12), // 12 kWh in 15 min = 48 kW
            quality: QualityFlag::Measured,
            obis_code: None,
        }];
        let window = DemandWindow::from_intervals(&intervals, 900);
        assert_eq!(window.peak_kw(), Some(dec!(48)));
    }

    #[test]
    fn empty_window_returns_none() {
        let window = DemandWindow::new();
        assert!(window.peak_kw().is_none());
        assert!(window.average_kw().is_none());
        assert!(window.is_empty());
    }
}
