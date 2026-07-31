//! §51 EEG — deriving the negative-price feed-in from quarter-hour intervals.
//!
//! §51 reduces the anzulegender Wert to null for the periods in which the
//! Spotmarktpreis is negative. To settle it, the feed-in that fell into those
//! periods must be quantified. This module is the pure overlay: given each
//! quarter-hour's feed-in energy and whether its spot price was negative, it
//! returns the qualifying feed-in kWh (`kwh_during_negative_epex`) and the count
//! of qualifying quarter-hours (`negative_price_quarter_hours` for §51a).
//!
//! The **version-aware threshold** lives here, not in the settlement engine
//! (which trusts the derived kWh): EEG 2023 has no minimum duration — *any*
//! negative quarter-hour qualifies — while EEG 2021 requires the negative price
//! to persist for **≥ 4 consecutive hours** and EEG 2017 for **≥ 6** (the run is
//! measured on the actual, time-adjacent quarter-hours). Plants under §100
//! Bestandsschutz keep their commissioning-era threshold.

use crate::version::EegGesetz;
use rust_decimal::Decimal;
use time::OffsetDateTime;

/// One quarter-hour of feed-in overlaid with the sign of its spot price.
#[derive(Debug, Clone, Copy)]
pub struct NegativpreisInterval {
    /// Interval start (quarter-hour boundary, UTC).
    pub start: OffsetDateTime,
    /// Feed-in (Einspeisung) energy in this quarter-hour, kWh.
    pub feed_in_kwh: Decimal,
    /// Whether the Spotmarktpreis for this quarter-hour was negative.
    pub price_negative: bool,
}

/// Result of the §51 overlay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NegativpreisResult {
    /// Feed-in kWh in qualifying negative-price intervals — feeds
    /// `SettleInput.kwh_during_negative_epex` (§51 reduction).
    pub kwh_during_negative: Decimal,
    /// Count of qualifying quarter-hours — feeds
    /// `SettleInput.negative_price_quarter_hours` (§51a extension accrual).
    pub negative_quarter_hours: u64,
}

/// Derive the §51 negative-price feed-in from time-ordered quarter-hour intervals.
///
/// `intervals` must be sorted ascending by `start`. A negative-price *run* is a
/// maximal sequence of consecutive negative quarter-hours that are 15 minutes
/// apart; a run counts only when its length reaches the version's threshold
/// (EEG 2023: any single quarter-hour; EEG 2021: 16 QH = 4 h; EEG 2017: 24 QH =
/// 6 h). Negative feed-in values (net consumption) are floored at zero.
#[must_use]
pub fn derive_negativpreis(
    intervals: &[NegativpreisInterval],
    version: EegGesetz,
) -> NegativpreisResult {
    // No §51 regime (pre-2017) → nothing qualifies.
    if version.negativpreis_stunden_schwelle().is_none() {
        return NegativpreisResult::default();
    }
    // EEG 2023: any negative quarter-hour (the `Some(1)` threshold is a proxy for
    // "no minimum duration"). Older Fassungen: a run of ≥ threshold hours.
    let min_run_qh: usize = match version.negativpreis_stunden_schwelle() {
        Some(_) if matches!(version, EegGesetz::Eeg2023) => 1,
        Some(h) => (h as usize) * 4,
        None => return NegativpreisResult::default(),
    };

    let mut result = NegativpreisResult::default();
    let mut i = 0;
    while i < intervals.len() {
        if !intervals[i].price_negative {
            i += 1;
            continue;
        }
        // Extend a maximal run of consecutive, time-adjacent negative quarter-hours.
        let run_start = i;
        let mut j = i + 1;
        while j < intervals.len()
            && intervals[j].price_negative
            && intervals[j].start == intervals[j - 1].start + time::Duration::minutes(15)
        {
            j += 1;
        }
        if j - run_start >= min_run_qh {
            for iv in &intervals[run_start..j] {
                result.kwh_during_negative += iv.feed_in_kwh.max(Decimal::ZERO);
                result.negative_quarter_hours += 1;
            }
        }
        i = j;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::datetime;

    fn qh(n: i64, kwh: &str, neg: bool) -> NegativpreisInterval {
        NegativpreisInterval {
            start: datetime!(2026-06-01 00:00 UTC) + time::Duration::minutes(15 * n),
            feed_in_kwh: dec!(0) + kwh.parse::<Decimal>().unwrap(),
            price_negative: neg,
        }
    }

    #[test]
    fn eeg2023_any_negative_quarter_hour_qualifies() {
        // A single isolated negative QH counts under EEG 2023.
        let ivs = [qh(0, "10", false), qh(1, "12", true), qh(2, "8", false)];
        let r = derive_negativpreis(&ivs, EegGesetz::Eeg2023);
        assert_eq!(r.kwh_during_negative, dec!(12));
        assert_eq!(r.negative_quarter_hours, 1);
    }

    #[test]
    fn eeg2021_requires_four_consecutive_hours() {
        // 8 consecutive negative QH = 2 h < 4 h → does NOT qualify under EEG 2021.
        let short: Vec<_> = (0..8).map(|n| qh(n, "5", true)).collect();
        assert_eq!(
            derive_negativpreis(&short, EegGesetz::Eeg2021),
            NegativpreisResult::default()
        );
        // 16 consecutive negative QH = 4 h → qualifies; 16 × 5 = 80 kWh.
        let long: Vec<_> = (0..16).map(|n| qh(n, "5", true)).collect();
        let r = derive_negativpreis(&long, EegGesetz::Eeg2021);
        assert_eq!(r.kwh_during_negative, dec!(80));
        assert_eq!(r.negative_quarter_hours, 16);
        // The same run under EEG 2023 also qualifies (no minimum).
        assert_eq!(
            derive_negativpreis(&long, EegGesetz::Eeg2023).negative_quarter_hours,
            16
        );
    }

    #[test]
    fn a_gap_breaks_the_consecutive_run() {
        // 15 negative + a positive QH + 1 negative = no 16-long run → EEG 2021 zero.
        let mut ivs: Vec<_> = (0..15).map(|n| qh(n, "5", true)).collect();
        ivs.push(qh(15, "5", false));
        ivs.push(qh(16, "5", true));
        assert_eq!(
            derive_negativpreis(&ivs, EegGesetz::Eeg2021),
            NegativpreisResult::default()
        );
    }

    #[test]
    fn negative_feed_in_is_floored() {
        let ivs = [qh(0, "-3", true)];
        assert_eq!(
            derive_negativpreis(&ivs, EegGesetz::Eeg2023).kwh_during_negative,
            dec!(0)
        );
    }
}
