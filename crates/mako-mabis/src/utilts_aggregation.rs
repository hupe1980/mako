//! UTILTS aggregation — ÜNB role: building Summenzeitreihe for BIKO.
//!
//! ## Background
//!
//! The **ÜNB (Übertragungsnetzbetreiber)** and **NB (Netzbetreiber)** aggregate
//! per-MaLo Lastgang time series into a **Summenzeitreihe** (aggregated time series)
//! before submitting it to the **BIKO (Bilanzkoordinator)** via UTILTS message.
//!
//! This module models the aggregation domain for the UTILTS exchange. It does NOT
//! implement the EDIFACT UTILTS serialisation — that lives in `edi-energy`.
//!
//! ## UTILTS in German MaKo
//!
//! | Message direction | Sender | Receiver | Content |
//! |---|---|---|---|
//! | UTILTS | ÜNB / NB | BIKO | Summenzeitreihe per Bilanzierungsgebiet |
//! | UTILTS | BIKO | BKV | Abrechnungssummenzeitreihe |
//!
//! ## Regulatory basis
//!
//! - **BK6-22-024 Anlage 3 MaBiS**: defines Summenzeitreihe exchange protocol
//! - **UTILTS AHB**: BDEW format specification for UTILTS S1.0 / S2.0
//! - **MaBiS (Anlage 3 zur Festlegung BK6-24-174)**: Marktregeln für die
//!   Durchführung der Bilanzkreisabrechnung Strom
//!
//! ## Architecture note
//!
//! A complete `mabis-syncd` daemon (not yet built) would:
//! 1. Query `edmd` for per-MaLo Lastgang via `GET /api/v1/lastgang/{malo_id}`
//! 2. Aggregate using this module's `SummenzeitreiheBuilder`
//! 3. Serialise via `edi-energy` UTILTS encoder
//! 4. Submit via AS4 through `makod`
//!
//! This module provides only the pure domain aggregation logic (step 2).
//!
//! ## Breaking change (2026-07-15)
//!
//! `add_malo()` now accepts `&[metering::MeterInterval]` instead of raw tuples.
//! `BilanzierungsgebietId` and `BilanzkreisId` are re-exported from `mako-edm`
//! (single canonical definition).

use rust_decimal::Decimal;
use std::collections::HashMap;
use time::OffsetDateTime;

// Re-export canonical topology ID types from mako-edm (single source of truth)
pub use mako_edm::BilanzierungsgebietId;
pub use mako_edm::BilanzkreisId;

// ── Interval aggregation ──────────────────────────────────────────────────────

/// A single aggregated interval in a Summenzeitreihe.
///
/// Represents the sum of all individual MaLo interval values within a
/// Bilanzierungsgebiet for one time slot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SumInterval {
    /// Interval start (UTC).
    pub from: OffsetDateTime,
    /// Interval end (UTC).
    pub to: OffsetDateTime,
    /// Aggregated energy in kWh (sum of all contributing MaLo values).
    pub quantity_kwh: Decimal,
    /// Number of MaLos contributing to this interval.
    pub malo_count: u32,
    /// Number of MaLos with non-measured (estimated/substituted) values.
    pub substituted_count: u32,
}

// ── Summenzeitreihe ───────────────────────────────────────────────────────────

/// An aggregated time series for one Bilanzierungsgebiet over a settlement period.
///
/// Built by the ÜNB/NB from individual MaLo Lastgänge and submitted to the BIKO
/// as a UTILTS message for balance group settlement.
///
/// ## Settlement period
///
/// The standard MaBiS billing month runs from the 1st to the last day of the
/// calendar month. Preliminary and final versions are distinguished by `version`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Summenzeitreihe {
    /// The Bilanzierungsgebiet this series belongs to.
    pub bilanzierungsgebiet_id: BilanzierungsgebietId,
    /// Start of the settlement period (UTC).
    pub period_from: OffsetDateTime,
    /// End of the settlement period (UTC).
    pub period_to: OffsetDateTime,
    /// Version: `"vorlaeufig"` (preliminary) or `"endgueltig"` (final).
    pub version: String,
    /// The aggregated intervals, ordered by `from`.
    pub intervals: Vec<SumInterval>,
    /// Sender MP-ID (ÜNB / NB BDEW code).
    pub sender_mp_id: String,
    /// Receiver MP-ID (BIKO BDEW code).
    pub receiver_mp_id: String,
}

impl Summenzeitreihe {
    /// Total energy in kWh across all intervals.
    #[must_use]
    pub fn total_kwh(&self) -> Decimal {
        self.intervals.iter().map(|i| i.quantity_kwh).sum()
    }

    /// Number of time slots in the series.
    #[must_use]
    pub fn interval_count(&self) -> usize {
        self.intervals.len()
    }

    /// `true` when any interval contains substituted (non-measured) values.
    #[must_use]
    pub fn has_substituted_values(&self) -> bool {
        self.intervals.iter().any(|i| i.substituted_count > 0)
    }

    /// Convert to monthly resampled buckets using the `metering` crate.
    ///
    /// This is the canonical output for MABIS §27 MessZV monthly summaries.
    /// Each bucket = one calendar month, suitable for UTILTS billing period.
    #[must_use]
    pub fn monthly_totals(&self) -> Vec<metering::ResampledBucket> {
        use metering::{MeterInterval, QualityFlag, ResampleConfig, resample};
        let intervals: Vec<MeterInterval> = self
            .intervals
            .iter()
            .map(|iv| {
                let quality = if iv.substituted_count > 0 {
                    QualityFlag::Substituted
                } else {
                    QualityFlag::Measured
                };
                MeterInterval {
                    from: iv.from,
                    to: iv.to,
                    value_kwh: iv.quantity_kwh,
                    quality,
                    obis_code: None,
                }
            })
            .collect();
        resample(&intervals, &ResampleConfig::to_monthly())
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Builds a [`Summenzeitreihe`] by aggregating per-MaLo interval data.
///
/// ## Usage
///
/// ```rust,ignore
/// let mut builder = SummenzeitreiheBuilder::new(
///     BilanzierungsgebietId("11YAPG4CTRDNZ--A".to_owned()),
///     period_from,
///     period_to,
///     "vorlaeufig".to_owned(),
///     "9900357000004".to_owned(),
///     "9900077000006".to_owned(),  // BIKO Transnet BW
/// );
///
/// for malo in &malos_in_bilanzierungsgebiet {
///     let lastgang: Vec<metering::MeterInterval> = edmd_client
///         .get_lastgang(malo, period_from, period_to)
///         .await?;
///     builder.add_malo(&lastgang);
/// }
///
/// let summenzeitreihe = builder.build();
/// ```
pub struct SummenzeitreiheBuilder {
    bilanzierungsgebiet_id: BilanzierungsgebietId,
    period_from: OffsetDateTime,
    period_to: OffsetDateTime,
    version: String,
    sender_mp_id: String,
    receiver_mp_id: String,
    /// Accumulated values per interval slot: (from_ns, to_ns) → (sum_kwh, malo_count, substituted)
    slots: HashMap<(i128, i128), (Decimal, u32, u32)>,
}

impl SummenzeitreiheBuilder {
    /// Create a new builder for a Bilanzierungsgebiet settlement period.
    #[must_use]
    pub fn new(
        bilanzierungsgebiet_id: BilanzierungsgebietId,
        period_from: OffsetDateTime,
        period_to: OffsetDateTime,
        version: impl Into<String>,
        sender_mp_id: impl Into<String>,
        receiver_mp_id: impl Into<String>,
    ) -> Self {
        Self {
            bilanzierungsgebiet_id,
            period_from,
            period_to,
            version: version.into(),
            sender_mp_id: sender_mp_id.into(),
            receiver_mp_id: receiver_mp_id.into(),
            slots: HashMap::new(),
        }
    }

    /// Add one MaLo's typed [`metering::MeterInterval`] slice to the aggregation.
    ///
    /// Each interval's value is accumulated into the running total for its time slot.
    /// Quality is tracked: `Substituted`, `Estimated`, `Preliminary`, and `Faulty`
    /// intervals count toward `substituted_count` in the output.
    ///
    /// Call this once per MaLo. The builder accumulates the cross-MaLo sum.
    ///
    /// ## Breaking change (2026-07-15)
    ///
    /// Previously accepted `impl IntoIterator<Item = (OffsetDateTime, OffsetDateTime, Decimal, bool)>`.
    /// Now accepts `&[metering::MeterInterval]` for type-safe integration.
    pub fn add_malo(&mut self, intervals: &[metering::MeterInterval]) {
        for iv in intervals {
            let key = (iv.from.unix_timestamp_nanos(), iv.to.unix_timestamp_nanos());
            let is_substituted = !matches!(
                iv.quality,
                metering::QualityFlag::Measured | metering::QualityFlag::Calculated
            );
            let entry = self.slots.entry(key).or_insert((Decimal::ZERO, 0, 0));
            entry.0 += iv.value_kwh;
            entry.1 += 1;
            if is_substituted {
                entry.2 += 1;
            }
        }
    }

    /// Build the [`Summenzeitreihe`] from accumulated MaLo data.
    ///
    /// The result is sorted chronologically and bounded to `[period_from, period_to]`.
    #[must_use]
    pub fn build(self) -> Summenzeitreihe {
        let mut intervals: Vec<SumInterval> = self
            .slots
            .into_iter()
            .filter_map(|((from_ns, to_ns), (kwh, count, sub))| {
                let from = OffsetDateTime::from_unix_timestamp_nanos(from_ns).ok()?;
                let to = OffsetDateTime::from_unix_timestamp_nanos(to_ns).ok()?;
                // Clamp to settlement period
                if from >= self.period_to || to <= self.period_from {
                    return None;
                }
                Some(SumInterval {
                    from,
                    to,
                    quantity_kwh: kwh,
                    malo_count: count,
                    substituted_count: sub,
                })
            })
            .collect();
        intervals.sort_by_key(|i| i.from);

        Summenzeitreihe {
            bilanzierungsgebiet_id: self.bilanzierungsgebiet_id,
            period_from: self.period_from,
            period_to: self.period_to,
            version: self.version,
            intervals,
            sender_mp_id: self.sender_mp_id,
            receiver_mp_id: self.receiver_mp_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metering::{MeterInterval, QualityFlag};
    use rust_decimal_macros::dec;
    use time::Duration;
    use time::macros::datetime;

    fn period() -> (OffsetDateTime, OffsetDateTime) {
        (
            datetime!(2026-06-01 0:00 UTC),
            datetime!(2026-07-01 0:00 UTC),
        )
    }

    fn make_iv(
        from: OffsetDateTime,
        kwh: rust_decimal::Decimal,
        quality: QualityFlag,
    ) -> MeterInterval {
        MeterInterval {
            from,
            to: from + Duration::minutes(15),
            value_kwh: kwh,
            quality,
            obis_code: None,
        }
    }

    #[test]
    fn empty_builder_produces_empty_series() {
        let (from, to) = period();
        let builder = SummenzeitreiheBuilder::new(
            BilanzierungsgebietId("TEST".to_owned()),
            from,
            to,
            "vorlaeufig",
            "9900357000004",
            "9900077000006",
        );
        let series = builder.build();
        assert_eq!(series.interval_count(), 0);
        assert_eq!(series.total_kwh(), Decimal::ZERO);
    }

    #[test]
    fn two_malos_aggregated_correctly() {
        let (from, to) = period();
        let iv_start = datetime!(2026-06-01 0:00 UTC);

        let mut builder = SummenzeitreiheBuilder::new(
            BilanzierungsgebietId("TEST".to_owned()),
            from,
            to,
            "vorlaeufig",
            "9900357000004",
            "9900077000006",
        );

        // MaLo A: 2.5 kWh measured
        builder.add_malo(&[make_iv(iv_start, dec!(2.5), QualityFlag::Measured)]);
        // MaLo B: 3.0 kWh (substituted)
        builder.add_malo(&[make_iv(iv_start, dec!(3.0), QualityFlag::Substituted)]);

        let series = builder.build();
        assert_eq!(series.interval_count(), 1);
        assert_eq!(series.intervals[0].quantity_kwh, dec!(5.5)); // 2.5 + 3.0
        assert_eq!(series.intervals[0].malo_count, 2);
        assert_eq!(series.intervals[0].substituted_count, 1);
        assert!(series.has_substituted_values());
    }

    #[test]
    fn intervals_outside_period_are_excluded() {
        let (from, to) = period();
        let before_period = datetime!(2026-05-31 23:45 UTC);
        let in_period = datetime!(2026-06-01 0:00 UTC);

        let mut builder = SummenzeitreiheBuilder::new(
            BilanzierungsgebietId("TEST".to_owned()),
            from,
            to,
            "endgueltig",
            "SENDER",
            "BIKO",
        );
        // Before-period interval: from = 23:45, to = 00:00 — to == period_from, so excluded
        builder.add_malo(&[
            MeterInterval {
                from: before_period,
                to: from, // to == period_from → excluded (not strictly inside)
                value_kwh: dec!(10.0),
                quality: QualityFlag::Measured,
                obis_code: None,
            },
            make_iv(in_period, dec!(5.0), QualityFlag::Measured),
        ]);

        let series = builder.build();
        assert_eq!(
            series.interval_count(),
            1,
            "outside interval must be excluded"
        );
        assert_eq!(series.total_kwh(), dec!(5.0));
    }

    #[test]
    fn estimated_quality_counts_as_substituted() {
        let (from, to) = period();
        let iv_start = datetime!(2026-06-15 12:00 UTC);

        let mut builder = SummenzeitreiheBuilder::new(
            BilanzierungsgebietId("QUALITY_TEST".to_owned()),
            from,
            to,
            "vorlaeufig",
            "SENDER",
            "BIKO",
        );
        builder.add_malo(&[make_iv(iv_start, dec!(1.0), QualityFlag::Estimated)]);

        let series = builder.build();
        assert_eq!(
            series.intervals[0].substituted_count, 1,
            "Estimated must be counted as substituted"
        );
    }

    #[test]
    fn monthly_totals_uses_resample() {
        let (from, to) = period();
        let mut builder = SummenzeitreiheBuilder::new(
            BilanzierungsgebietId("MONTHLY_TEST".to_owned()),
            from,
            to,
            "endgueltig",
            "SENDER",
            "BIKO",
        );
        // Add one interval in June
        builder.add_malo(&[make_iv(
            datetime!(2026-06-15 10:00 UTC),
            dec!(2.0),
            QualityFlag::Measured,
        )]);

        let series = builder.build();
        let monthly = series.monthly_totals();
        assert_eq!(
            monthly.len(),
            1,
            "all intervals in June should produce one bucket"
        );
        assert_eq!(monthly[0].total_kwh, dec!(2.0));
    }
}
