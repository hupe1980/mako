//! Summenzeitreihe aggregation — ÜNB/NB role: building the series filed with the BIKO.
//!
//! ## Background
//!
//! The **ÜNB (Übertragungsnetzbetreiber)** and **NB (Netzbetreiber)** aggregate
//! per-MaLo Lastgang time series into a **Summenzeitreihe** (aggregated time series)
//! before filing it with the **BIKO (Bilanzkoordinator)**.
//!
//! This module models the aggregation domain. The wire format is MSCONS
//! Prüfidentifikator 13003 ("Übertragung Summenzeitreihe", MSCONS AHB 3.2
//! §8.3.1) and its serialisation lives in `edi-energy`; UTILTS carries
//! Berechnungsformel and Zählzeitdefinitionen and has no Summenzeitreihe use
//! case.
//!
//! ## Direction in German MaKo
//!
//! | Message | Sender | Receiver | Content |
//! |---|---|---|---|
//! | MSCONS 13003 | ÜNB / NB | BIKO | Summenzeitreihe per Bilanzierungsgebiet |
//! | MSCONS 13003 | BIKO | BKV | Abrechnungssummenzeitreihe |
//!
//! ## Regulatory basis
//!
//! - **BK6-24-174 Anlage 3 MaBiS**: Summenzeitreihe exchange, Versionierung (§3.8.2),
//!   Datenstatus (§3.8.3) and Fristen (§3.10)
//! - **MSCONS AHB 3.2 §8.3.1**: wire format for Prüfidentifikator 13003
//! - **MaBiS (Anlage 3 zur Festlegung BK6-24-174)**: Marktregeln für die
//!   Durchführung der Bilanzkreisabrechnung Strom
//!
//! ## Position in the pipeline
//!
//! This module provides the pure domain aggregation. The `mabis-syncd` daemon
//! drives it: it queries `edmd` for per-MaLo Lastgang, aggregates with
//! [`SummenzeitreiheBuilder`], serialises via `edi-energy`, and submits through
//! `makod`.
//!
//! ## Slot resolution
//!
//! MaBiS settles electricity on a quarter-hourly grid, so the builder is
//! constructed with the slot length it expects and rejects any interval that
//! does not match. Aggregating coarser buckets would produce a Summenzeitreihe
//! whose total is right but whose shape is wrong — a settlement error the BIKO
//! cannot detect from the message alone.

use rust_decimal::Decimal;
use std::collections::HashMap;
use time::{Duration, OffsetDateTime};

/// The quarter-hourly slot length MaBiS settles electricity on.
pub const MABIS_SLOT: Duration = Duration::minutes(15);

/// Re-export the topology ID types (defined in `crate::ids`).
pub use crate::ids::BilanzierungsgebietId;
pub use crate::ids::BilanzkreisId;
pub use crate::ids::MabisZaehlpunktId;

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
/// as an MSCONS 13003 message for balance group settlement.
///
/// ## Settlement period
///
/// The standard MaBiS billing month runs from the 1st to the last day of the
/// calendar month. Preliminary and final versions are distinguished by `version`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Summenzeitreihe {
    /// The Bilanzierungsgebiet this series belongs to — MSCONS SG6 `LOC+107`.
    pub bilanzierungsgebiet_id: BilanzierungsgebietId,
    /// The MaBiS-Zählpunkt the series is submitted under — MSCONS SG6
    /// `LOC+172` (Meldepunkt), a 33-character Zählpunktbezeichnung.
    ///
    /// Distinct from [`bilanzierungsgebiet_id`](Self::bilanzierungsgebiet_id):
    /// the AHB gives each its own qualifier, and submitting the territory EIC
    /// as the Meldepunkt misidentifies the series to the BIKO.
    pub mabis_zp_id: MabisZaehlpunktId,
    /// Start of the settlement period (UTC).
    pub period_from: OffsetDateTime,
    /// End of the settlement period (UTC).
    pub period_to: OffsetDateTime,
    /// Ascending version within (Bilanzierungsgebiet, Bilanzierungsmonat).
    ///
    /// BK6-24-174 Anlage 3 §3.8.2: "Die Version einer Summenzeitreihe ist
    /// jeweils aufsteigend zu vergeben." It is a timestamp rather than a
    /// lifecycle state — MSCONS carries it as SG6 DTM+293
    /// (Fertigstellungsdatum/-zeit) — so a correction is the same series resent
    /// under a higher version.
    pub version: OffsetDateTime,
    /// The aggregated intervals, ordered by `from`.
    pub intervals: Vec<SumInterval>,
    /// Sender MP-ID (ÜNB / NB BDEW code).
    pub sender_mp_id: String,
    /// Receiver MP-ID (BIKO BDEW code).
    pub receiver_mp_id: String,
    /// Length of one settlement slot, in minutes.
    pub slot_length_minutes: i64,
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

    /// Number of slots a gap-free series would hold for this settlement period.
    #[must_use]
    pub fn expected_slot_count(&self) -> usize {
        let secs = (self.period_to - self.period_from).whole_seconds();
        let slot = (self.slot_length_minutes * 60).max(1);
        usize::try_from(secs / slot).unwrap_or(0)
    }

    /// Slots the settlement period covers for which no MaLo reported a value.
    ///
    /// MaBiS settles against a gap-free grid, so a non-zero count means the BIKO
    /// would receive a series that silently omits energy rather than one that
    /// reports zero for those slots.
    #[must_use]
    pub fn missing_slot_count(&self) -> usize {
        self.expected_slot_count()
            .saturating_sub(self.intervals.len())
    }

    /// `true` when every slot in the settlement period carries a value.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing_slot_count() == 0
    }

    /// Convert to monthly resampled buckets using the `metering` crate.
    ///
    /// This is the canonical output for MABIS § 13 StromNZV monthly summaries.
    /// Each bucket = one calendar month. Reporting only — the filed message
    /// carries the quarter-hourly slots.
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
                    value: iv.quantity_kwh,
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
///     BilanzierungsgebietId::new("11YAPG4CTRDNZ--P").expect("a Y-type EIC"),
///     period_from,
///     period_to,
///     version, // ascending timestamp, MSCONS SG6 DTM+293
///     "9900357000004".to_owned(),
///     "9900077000006".to_owned(),  // BIKO Transnet BW
///     MABIS_SLOT,
/// );
///
/// for malo in &malos_in_bilanzierungsgebiet {
///     let lastgang: Vec<metering::MeterInterval> = edmd_client
///         .get_lastgang(malo, period_from, period_to)
///         .await?;
///     builder.add_malo(&lastgang)?;
/// }
///
/// let summenzeitreihe = builder.build();
/// ```
pub struct SummenzeitreiheBuilder {
    bilanzierungsgebiet_id: BilanzierungsgebietId,
    mabis_zp_id: MabisZaehlpunktId,
    period_from: OffsetDateTime,
    period_to: OffsetDateTime,
    version: OffsetDateTime,
    sender_mp_id: String,
    receiver_mp_id: String,
    /// The slot length every contributed interval must match.
    slot_length: Duration,
    /// Accumulated values per interval slot: (from_ns, to_ns) → (sum_kwh, malo_count, substituted)
    slots: HashMap<(i128, i128), (Decimal, u32, u32)>,
}

/// An interval whose length does not match the builder's settlement grid.
///
/// Carries the offending interval so the caller can name the MaLo and slot in
/// its own diagnostics rather than reporting an anonymous count.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "interval {from}..{to} spans {actual_minutes} min, but this Bilanzierungsgebiet settles on a {expected_minutes} min grid"
)]
pub struct SlotResolutionError {
    /// Start of the rejected interval.
    pub from: OffsetDateTime,
    /// End of the rejected interval.
    pub to: OffsetDateTime,
    /// Length the interval actually spans, in minutes.
    pub actual_minutes: i64,
    /// Length the builder requires, in minutes.
    pub expected_minutes: i64,
}

impl SummenzeitreiheBuilder {
    /// Create a new builder for a Bilanzierungsgebiet settlement period.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bilanzierungsgebiet_id: BilanzierungsgebietId,
        mabis_zp_id: MabisZaehlpunktId,
        period_from: OffsetDateTime,
        period_to: OffsetDateTime,
        version: OffsetDateTime,
        sender_mp_id: impl Into<String>,
        receiver_mp_id: impl Into<String>,
        slot_length: Duration,
    ) -> Self {
        Self {
            bilanzierungsgebiet_id,
            mabis_zp_id,
            period_from,
            period_to,
            version,
            sender_mp_id: sender_mp_id.into(),
            receiver_mp_id: receiver_mp_id.into(),
            slot_length,
            slots: HashMap::new(),
        }
    }

    /// Number of slots a complete series covers for this settlement period.
    #[must_use]
    pub fn expected_slot_count(&self) -> usize {
        let span = self.period_to - self.period_from;
        usize::try_from(span.whole_seconds() / self.slot_length.whole_seconds().max(1)).unwrap_or(0)
    }

    /// Add one MaLo's typed [`metering::MeterInterval`] slice to the aggregation.
    ///
    /// Each interval's value is accumulated into the running total for its time slot.
    /// Quality is tracked: `Substituted`, `Estimated`, `Preliminary`, and `Faulty`
    /// intervals count toward `substituted_count` in the output.
    ///
    /// Call this once per MaLo. The builder accumulates the cross-MaLo sum.
    ///
    /// # Errors
    ///
    /// Returns [`SlotResolutionError`] for the first interval whose length does
    /// not match the builder's grid, leaving the accumulator untouched. The MaLo
    /// is then absent from the series rather than folded in at the wrong shape,
    /// so a caller that ignores this error under-reports rather than mis-reports.
    pub fn add_malo(
        &mut self,
        intervals: &[metering::MeterInterval],
    ) -> Result<(), SlotResolutionError> {
        for iv in intervals {
            let actual = iv.to - iv.from;
            if actual != self.slot_length {
                return Err(SlotResolutionError {
                    from: iv.from,
                    to: iv.to,
                    actual_minutes: actual.whole_minutes(),
                    expected_minutes: self.slot_length.whole_minutes(),
                });
            }
        }
        for iv in intervals {
            let key = (iv.from.unix_timestamp_nanos(), iv.to.unix_timestamp_nanos());
            let is_substituted = !matches!(
                iv.quality,
                metering::QualityFlag::Measured | metering::QualityFlag::Calculated
            );
            let entry = self.slots.entry(key).or_insert((Decimal::ZERO, 0, 0));
            entry.0 += iv.value;
            entry.1 += 1;
            if is_substituted {
                entry.2 += 1;
            }
        }
        Ok(())
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
            mabis_zp_id: self.mabis_zp_id,
            bilanzierungsgebiet_id: self.bilanzierungsgebiet_id,
            period_from: self.period_from,
            period_to: self.period_to,
            version: self.version,
            intervals,
            sender_mp_id: self.sender_mp_id,
            receiver_mp_id: self.receiver_mp_id,
            slot_length_minutes: self.slot_length.whole_minutes(),
        }
    }
}

#[cfg(test)]
mod tests {
    /// A Bilanzierungsgebiet EIC for tests — panics on a malformed one, which
    /// is the point: a fixture that is not an EIC would not be a territory.
    fn bg(code: &str) -> BilanzierungsgebietId {
        BilanzierungsgebietId::new(code).expect("a valid Y-type Bilanzierungsgebiet EIC")
    }

    use super::*;
    use metering::{MeterInterval, QualityFlag};
    use rust_decimal::dec;
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
            value: kwh,
            quality,
            obis_code: None,
        }
    }

    #[test]
    fn empty_builder_produces_empty_series() {
        let (from, to) = period();
        let builder = SummenzeitreiheBuilder::new(
            bg("11YMAKO-TEST-01U"),
            MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap(),
            from,
            to,
            datetime!(2026-07-03 05:00 UTC),
            "9900357000004",
            "9900077000006",
            MABIS_SLOT,
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
            bg("11YMAKO-TEST-01U"),
            MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap(),
            from,
            to,
            datetime!(2026-07-03 05:00 UTC),
            "9900357000004",
            "9900077000006",
            MABIS_SLOT,
        );

        // MaLo A: 2.5 kWh measured
        builder
            .add_malo(&[make_iv(iv_start, dec!(2.5), QualityFlag::Measured)])
            .unwrap();
        // MaLo B: 3.0 kWh (substituted)
        builder
            .add_malo(&[make_iv(iv_start, dec!(3.0), QualityFlag::Substituted)])
            .unwrap();

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
            bg("11YMAKO-TEST-01U"),
            MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap(),
            from,
            to,
            datetime!(2026-07-08 05:00 UTC),
            "SENDER",
            "BIKO",
            MABIS_SLOT,
        );
        // Before-period interval: from = 23:45, to = 00:00 — to == period_from, so excluded
        builder
            .add_malo(&[
                MeterInterval {
                    from: before_period,
                    to: from, // to == period_from → excluded (not strictly inside)
                    value: dec!(10.0),
                    quality: QualityFlag::Measured,
                    obis_code: None,
                },
                make_iv(in_period, dec!(5.0), QualityFlag::Measured),
            ])
            .unwrap();

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
            bg("11YMAKO-QUALIT-5"),
            MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap(),
            from,
            to,
            datetime!(2026-07-03 05:00 UTC),
            "SENDER",
            "BIKO",
            MABIS_SLOT,
        );
        builder
            .add_malo(&[make_iv(iv_start, dec!(1.0), QualityFlag::Estimated)])
            .unwrap();

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
            bg("11YMAKO-MONTHL-Q"),
            MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap(),
            from,
            to,
            datetime!(2026-07-08 05:00 UTC),
            "SENDER",
            "BIKO",
            MABIS_SLOT,
        );
        // Add one interval in June
        builder
            .add_malo(&[make_iv(
                datetime!(2026-06-15 10:00 UTC),
                dec!(2.0),
                QualityFlag::Measured,
            )])
            .unwrap();

        let series = builder.build();
        let monthly = series.monthly_totals();
        assert_eq!(
            monthly.len(),
            1,
            "all intervals in June should produce one bucket"
        );
        assert_eq!(monthly[0].total, dec!(2.0));
    }

    #[test]
    fn a_monthly_bucket_is_rejected_rather_than_settled_as_a_slot() {
        let (from, to) = period();
        let mut builder = SummenzeitreiheBuilder::new(
            bg("11YMAKO-TEST-01U"),
            MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap(),
            from,
            to,
            datetime!(2026-07-03 05:00 UTC),
            "SENDER",
            "BIKO",
            MABIS_SLOT,
        );

        // One bucket spanning the whole settlement month, as a resampled
        // endpoint would return it.
        let err = builder
            .add_malo(&[MeterInterval {
                from,
                to,
                value: dec!(1234.5),
                quality: QualityFlag::Measured,
                obis_code: None,
            }])
            .expect_err("a month-long bucket is not a quarter-hourly slot");

        assert_eq!(err.expected_minutes, 15);
        assert_eq!(err.actual_minutes, 30 * 24 * 60);
        assert_eq!(
            builder.build().total_kwh(),
            dec!(0),
            "a rejected MaLo must contribute nothing"
        );
    }

    #[test]
    fn a_partially_covered_period_reports_its_missing_slots() {
        let (from, to) = period();
        let mut builder = SummenzeitreiheBuilder::new(
            bg("11YMAKO-TEST-01U"),
            MabisZaehlpunktId::new("DE0004030099000000000000000012345").unwrap(),
            from,
            to,
            datetime!(2026-07-03 05:00 UTC),
            "SENDER",
            "BIKO",
            MABIS_SLOT,
        );
        builder
            .add_malo(&[
                make_iv(from, dec!(1.0), QualityFlag::Measured),
                make_iv(
                    from + Duration::minutes(15),
                    dec!(1.0),
                    QualityFlag::Measured,
                ),
            ])
            .unwrap();

        let series = builder.build();
        // June has 30 days → 2 880 quarter-hours, of which 2 are filled.
        assert_eq!(series.expected_slot_count(), 30 * 96);
        assert_eq!(series.missing_slot_count(), 30 * 96 - 2);
        assert!(!series.is_complete());
    }
}

// ── Identifier integrity ──────────────────────────────────────────────────────
// The identifier confusion this module exists to prevent is now
// **unrepresentable**, not merely checked.
//
// MSCONS SG6 carries three `LOC` qualifiers as free text — `172` the
// Meldepunkt, `107` the Bilanzierungsgebiet, `237` the Bilanzkreis — so a
// message that files the territory EIC as the Meldepunkt parses, validates and
// is accepted by the BIKO, which then settles against the wrong point.
//
// A `validate_identifiers()` method used to refuse that at build time, because
// `BilanzierungsgebietId` was an unchecked `String` newtype and a caller could
// put a 33-character Zählpunktbezeichnung in it. With the territory typed as a
// 16-character Y-type EIC and the Meldepunkt as a 33-character
// Zählpunktbezeichnung, the two cannot hold the same value — the check could
// no longer fail, and a control that cannot fire is worse than none, because
// review reads it as protection. The types are the protection now;
// `identifier_tests` pins that.

#[cfg(test)]
mod identifier_tests {
    use super::*;

    const ZP: &str = "DE0004030099000000000000000012345";

    /// See the note on `tests::bg`: a fixture that is not an EIC is not a
    /// territory, so the helper refuses rather than papering over it.
    fn bg(code: &str) -> BilanzierungsgebietId {
        BilanzierungsgebietId::new(code).expect("a valid Y-type Bilanzierungsgebiet EIC")
    }

    fn series(mabis_zp_id: MabisZaehlpunktId, gebiet: &str) -> Summenzeitreihe {
        SummenzeitreiheBuilder::new(
            bg(gebiet),
            mabis_zp_id,
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH + Duration::days(1),
            OffsetDateTime::UNIX_EPOCH,
            "9900000000001",
            "9900000000002",
            Duration::minutes(15),
        )
        .build()
    }

    /// A malformed Meldepunkt no longer reaches this check — it cannot be
    /// constructed. The 16-character territory EIC, the empty string and every
    /// other wrong shape fail at `MabisZaehlpunktId::new`, which `Deserialize`
    /// also runs.
    ///
    /// This is the difference between the type and the runtime guard it
    /// replaced: the earlier version could only refuse a bad value *after* a
    /// caller had already put it in a Summenzeitreihe.
    #[test]
    fn a_malformed_meldepunkt_cannot_reach_a_summenzeitreihe() {
        assert!(MabisZaehlpunktId::new("11XSWISSGRIDBGX8").is_err());
        assert!(MabisZaehlpunktId::new("").is_err());
    }

    /// The Meldepunkt/Bilanzierungsgebiet swap cannot be built.
    ///
    /// This replaces a runtime equality check. A Zählpunktbezeichnung is 33
    /// characters and a Bilanzierungsgebiet is a 16-character EIC, so neither
    /// can be parsed into the other's type — the substitution the BIKO cannot
    /// detect is refused before a Summenzeitreihe exists.
    #[test]
    fn the_meldepunkt_territory_swap_is_unrepresentable() {
        // A Meldepunkt offered as a territory: wrong length.
        assert!(BilanzierungsgebietId::new(ZP).is_err());
        // A territory offered as a Meldepunkt: also wrong length.
        assert!(MabisZaehlpunktId::new("11YMAKO-TEST-01U").is_err());
    }

    /// A Bilanzkreis is not a Bilanzierungsgebiet, and the EIC says so.
    ///
    /// ENTSO-E types a Bilanzkreis as a **Party** (`X`) — it is held by a
    /// Bilanzkreisverantwortlicher — and a Bilanzierungsgebiet as an **Area**
    /// (`Y`). Both are 16 characters, so length cannot separate them; the
    /// object type can, and it is the only thing that can.
    #[test]
    fn a_bilanzkreis_is_refused_where_a_territory_belongs() {
        // A published Bilanzkreis code (Party).
        assert!(BilanzierungsgebietId::new("11XSUEDWESTSTRO8").is_err());
        // …and the mirror: an area is not a balance group.
        assert!(BilanzkreisId::new("11YMAKO-TEST-01U").is_err());
    }

    /// A proper Zählpunktbezeichnung against its own territory builds.
    #[test]
    fn a_well_formed_pair_is_accepted() {
        let s = series(MabisZaehlpunktId::new(ZP).unwrap(), "11YMAKO-TEST-01U");
        assert_eq!(s.mabis_zp_id.as_str(), ZP);
        assert_eq!(s.bilanzierungsgebiet_id.as_ref(), "11YMAKO-TEST-01U");
    }
}
