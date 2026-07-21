//! Named, annotated measurement series — grouping of intervals with provenance metadata.
//!
//! A [`MeasurementSeries`] is the semantic container that wraps a `Vec<MeterInterval>`
//! with the context required to explain every value:
//! - **who** measured it (MaLo, MeLo, meter serial, OBIS register)
//! - **how** it was produced (source, ingestion method, quality)
//! - **why** it exists (purpose, process reference)
//!
//! ## Relationship to domain objects
//!
//! ```text
//! MeasurementSeries
//!   ├── source_info: MeasurementSource   ← where data came from
//!   ├── measurement_point: Option<MeasurementPoint>  ← full location context
//!   ├── resolution: IntervalResolution   ← expected interval length
//!   ├── intervals: Vec<MeterInterval>    ← the actual data
//!   └── provenance: Vec<ProvenanceEntry> ← correction/substitution audit trail
//! ```
//!
//! ## §16 Explainability requirement
//!
//! Every stored interval should answer: *"Where did this value come from?"*
//! `MeasurementSeries` carries the answer at the series level; each
//! `MeterInterval.quality` answers it at the interval level.
//!
//! ## Legal basis
//!
//! - **§ 60 Abs. 6 MsbG**: 3-year retention with full provenance for billing data.
//! - **§ 60 Abs. 2 MsbG**: Substitute values must be traceable to their generation method.
//! - **BDEW MSCONS AHB**: Each MSCONS time series is a named series per OBIS code.

use time::OffsetDateTime;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::interval::{MeterInterval, QualityFlag};
use crate::obis::ObisCode;
use crate::resolution::IntervalResolution;

// ── MeasurementSource ─────────────────────────────────────────────────────────

/// The origin of a measurement series — how the data entered the system.
///
/// Stored per series (not per interval) since all intervals in one MSCONS
/// message share the same ingestion source.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MeasurementSource {
    /// Received via EDIFACT MSCONS (standard MaKo pipeline).
    ///
    /// The canonical source for DSO-metered customers. Follows the
    /// market-communication → master-data → metering webhook pipeline.
    Mscons {
        /// MSCONS Prüfidentifikator (e.g. 13005).
        pid: u32,
        /// EDIFACT message reference.
        message_ref: Option<String>,
        /// BDEW Codenummer of the sending NB/MSB.
        sender_mp_id: String,
    },

    /// iMSys / SMGW direct push — bypasses EDIFACT pipeline.
    ///
    /// Used for §41a EnWG dynamic tariffs where MSCONS round-trip adds latency.
    SmgwDirectPush {
        /// SMGW device ID.
        device_id: String,
        /// Session ID for idempotency.
        session_id: String,
    },

    /// Manual entry by an operator.
    ///
    /// Used for corrections after meter replacement or dispute resolution.
    ManualEntry {
        /// Operator identifier (user ID or name).
        operator_id: String,
        /// Reason for manual entry.
        reason: String,
    },

    /// Automatic § 60 Abs. 2 MsbG substitute value generation.
    ///
    /// Triggered by gap detection (V01) in the validation engine.
    AutoSubstitute {
        /// Substitute method used.
        method: crate::substitute::SubstituteMethod,
        /// Reason for substitution.
        reason: crate::substitute::SubstitutionReason,
    },

    /// Retroactive § 60 Abs. 6 MsbG correction.
    ///
    /// Applied when an earlier value was found to be wrong.
    RetroactiveCorrection {
        /// ID of the original meter_read_corrections row.
        correction_id: uuid::Uuid,
        /// Who applied the correction.
        corrected_by: String,
    },

    /// Virtual meter computation (Sum/Residual/PvSelfConsumption/GgvConstantAllocation/GgvProportionalAllocation).
    ///
    /// Derived from real measurements via `AggregationRule`.
    VirtualMeter {
        /// The aggregation rule type used.
        rule_type: String,
        /// Source MaLo IDs that contributed to this series.
        source_malo_ids: Vec<String>,
    },

    /// Redispatch 2.0 time-series import (PIDs 13020–13026).
    ///
    /// Ausfallarbeit, meteorological data, and other Redispatch quantities.
    RedispatchImport {
        /// MSCONS PID (13020–13026).
        pid: u32,
        /// Activation ID or process reference.
        activation_ref: Option<String>,
    },
}

impl MeasurementSource {
    /// Short human-readable label (German).
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Mscons { .. } => "MSCONS",
            Self::SmgwDirectPush { .. } => "SMGW-Direktpush",
            Self::ManualEntry { .. } => "Manuelle Eingabe",
            Self::AutoSubstitute { .. } => "§ 60 Abs. 2 MsbG Auto-Ersatzwert",
            Self::RetroactiveCorrection { .. } => "§ 60 Abs. 6 MsbG Korrektur",
            Self::VirtualMeter { .. } => "Virtueller Zähler",
            Self::RedispatchImport { .. } => "Redispatch 2.0",
        }
    }

    /// `true` when this source produces billable intervals per § 60 Abs. 2 MsbG.
    #[must_use]
    pub fn is_billable_source(&self) -> bool {
        matches!(
            self,
            Self::Mscons { .. }
                | Self::SmgwDirectPush { .. }
                | Self::AutoSubstitute { .. }
                | Self::RetroactiveCorrection { .. }
        )
    }
}

// ── ProvenanceEntry ───────────────────────────────────────────────────────────

/// An immutable audit record for a change applied to a series or interval.
///
/// Provenance entries are append-only — they record what happened and when,
/// but never overwrite earlier entries. This satisfies the § 60 Abs. 6 MsbG
/// 3-year audit trail requirement.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ProvenanceEntry {
    /// When this event occurred (UTC).
    pub occurred_at: OffsetDateTime,
    /// What kind of event this was.
    pub event_type: ProvenanceEventType,
    /// Who or what triggered this event.
    pub actor: String,
    /// Free-text note for regulatory audit trail.
    pub note: Option<String>,
}

/// Type of provenance event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ProvenanceEventType {
    /// Initial ingest of the series.
    Ingested,
    /// Quality assessment run (V01-V10 validation).
    QualityAssessed,
    /// Gap detected and substitute value generated.
    SubstituteGenerated,
    /// Retroactive correction applied.
    Corrected,
    /// Archive to cold tier (Iceberg/S3).
    Archived,
    /// GDPR erasure request applied.
    Anonymised,
}

// ── MeasurementSeries ─────────────────────────────────────────────────────────

/// A named, annotated time series of meter intervals with full provenance.
///
/// This is the richest representation of meter data in the `metering` crate.
/// It combines all context required to answer § 60 Abs. 6 MsbG audit questions.
///
/// ## Usage
///
/// For most processing (aggregation, validation, resampling), use `Vec<MeterInterval>`
/// directly. Use `MeasurementSeries` at system boundaries where the full context
/// must be preserved: persistence, tool responses, and ERP handoffs.
///
/// ## Relationship to `MeasurementPoint`
///
/// `MeasurementPoint` describes the **physical and regulatory binding** (MaLo,
/// MeLo, OBIS, MarktRolle). `MeasurementSeries` describes the **data series**
/// with its provenance. One `MeasurementPoint` can produce many
/// `MeasurementSeries` (e.g. one per MSCONS delivery).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MeasurementSeries {
    /// 11-digit MaLo-ID.
    pub malo_id: String,

    /// 33-character MeLo-ID (if available).
    pub melo_id: Option<String>,

    /// OBIS code identifying this measurement channel.
    pub obis_code: Option<ObisCode>,

    /// Expected interval resolution for this series.
    ///
    /// Derived from `obis_code.default_resolution()` when not explicitly set.
    pub resolution: Option<IntervalResolution>,

    /// How the data entered the system.
    pub source: MeasurementSource,

    /// The interval data, ordered by `from` ascending.
    pub intervals: Vec<MeterInterval>,

    /// Worst quality flag across all intervals.
    ///
    /// Pre-computed for fast filtering — recalculate after modifying `intervals`.
    pub worst_quality: QualityFlag,

    /// Audit trail for this series (ordered chronologically).
    pub provenance: Vec<ProvenanceEntry>,
}

impl MeasurementSeries {
    /// Construct a new series from intervals.
    ///
    /// Automatically computes `worst_quality` from the interval set.
    /// Adds an `Ingested` provenance entry with the current UTC time.
    #[must_use]
    pub fn new(
        malo_id: impl Into<String>,
        obis_code: Option<ObisCode>,
        intervals: Vec<MeterInterval>,
        source: MeasurementSource,
    ) -> Self {
        let worst_quality = worst_quality_of(&intervals);
        let resolution = obis_code.and_then(|o| o.default_resolution());
        Self {
            malo_id: malo_id.into(),
            melo_id: None,
            obis_code,
            resolution,
            source: source.clone(),
            intervals,
            worst_quality,
            provenance: vec![ProvenanceEntry {
                occurred_at: OffsetDateTime::now_utc(),
                event_type: ProvenanceEventType::Ingested,
                actor: source.label().to_owned(),
                note: None,
            }],
        }
    }

    /// Number of intervals in this series.
    #[must_use]
    pub fn interval_count(&self) -> usize {
        self.intervals.len()
    }

    /// `true` when the series has no intervals (e.g. empty MSCONS delivery).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    /// Total energy (kWh) across all billable intervals.
    #[must_use]
    pub fn total_kwh(&self) -> rust_decimal::Decimal {
        self.intervals
            .iter()
            .filter(|iv| iv.quality.is_billable())
            .map(|iv| iv.value_kwh)
            .sum()
    }

    /// Earliest interval start in this series.
    #[must_use]
    pub fn period_from(&self) -> Option<OffsetDateTime> {
        self.intervals.iter().map(|iv| iv.from).min()
    }

    /// Latest interval end in this series.
    #[must_use]
    pub fn period_to(&self) -> Option<OffsetDateTime> {
        self.intervals.iter().map(|iv| iv.to).max()
    }

    /// Recompute `worst_quality` from current `intervals`.
    ///
    /// Call this after modifying `intervals` directly.
    pub fn recompute_quality(&mut self) {
        self.worst_quality = worst_quality_of(&self.intervals);
    }

    /// Append a provenance entry.
    pub fn record_event(&mut self, event_type: ProvenanceEventType, actor: impl Into<String>) {
        self.provenance.push(ProvenanceEntry {
            occurred_at: OffsetDateTime::now_utc(),
            event_type,
            actor: actor.into(),
            note: None,
        });
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn quality_rank(q: QualityFlag) -> u8 {
    match q {
        QualityFlag::Faulty | QualityFlag::Unknown => 5,
        QualityFlag::Preliminary => 4,
        QualityFlag::Estimated => 3,
        QualityFlag::Corrected | QualityFlag::Substituted => 2,
        QualityFlag::Calculated => 1,
        QualityFlag::Measured => 0,
    }
}

fn worst_quality_of(intervals: &[MeterInterval]) -> QualityFlag {
    intervals
        .iter()
        .max_by_key(|iv| quality_rank(iv.quality))
        .map(|iv| iv.quality)
        .unwrap_or(QualityFlag::Unknown)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::QualityFlag;
    use crate::obis::ObisCode;
    use rust_decimal::dec;
    use time::{Duration, macros::datetime};
    use uuid::Uuid;

    fn make_interval(from: time::OffsetDateTime, kwh: rust_decimal::Decimal) -> MeterInterval {
        MeterInterval {
            from,
            to: from + Duration::minutes(15),
            value_kwh: kwh,
            quality: QualityFlag::Measured,
            obis_code: None,
        }
    }

    #[test]
    fn new_series_computes_worst_quality() {
        let base = datetime!(2026-01-01 0:00 UTC);
        let mut intervals = vec![
            make_interval(base, dec!(1.0)),
            make_interval(base + Duration::minutes(15), dec!(1.0)),
        ];
        intervals[1].quality = QualityFlag::Estimated;

        let series = MeasurementSeries::new(
            "51238696780",
            Some(ObisCode::STROM_BEZUG_TOTAL),
            intervals,
            MeasurementSource::Mscons {
                pid: 13005,
                message_ref: None,
                sender_mp_id: "9900357000004".to_owned(),
            },
        );
        assert_eq!(series.worst_quality, QualityFlag::Estimated);
        assert_eq!(series.interval_count(), 2);
    }

    #[test]
    fn total_kwh_sums_billable_only() {
        let base = datetime!(2026-01-01 0:00 UTC);
        let mut intervals = vec![
            make_interval(base, dec!(2.0)),
            make_interval(base + Duration::minutes(15), dec!(1.0)),
        ];
        intervals[1].quality = QualityFlag::Faulty; // not billable

        let series = MeasurementSeries::new(
            "51238696780",
            None,
            intervals,
            MeasurementSource::ManualEntry {
                operator_id: "ops-001".to_owned(),
                reason: "test".to_owned(),
            },
        );
        assert_eq!(series.total_kwh(), dec!(2.0)); // only Measured interval
    }

    #[test]
    fn period_bounds_from_intervals() {
        let base = datetime!(2026-06-01 0:00 UTC);
        let intervals = vec![
            make_interval(base, dec!(1.0)),
            make_interval(base + Duration::minutes(15), dec!(1.0)),
        ];
        let series = MeasurementSeries::new(
            "51238696780",
            None,
            intervals,
            MeasurementSource::SmgwDirectPush {
                device_id: "SMGW-001".to_owned(),
                session_id: "sess-001".to_owned(),
            },
        );
        assert_eq!(series.period_from(), Some(base));
        assert_eq!(series.period_to(), Some(base + Duration::minutes(30)));
    }

    #[test]
    fn empty_series_reports_correctly() {
        let series = MeasurementSeries::new(
            "51238696780",
            None,
            vec![],
            MeasurementSource::AutoSubstitute {
                method: crate::substitute::SubstituteMethod::ZeroFill,
                reason: crate::substitute::SubstitutionReason::NoMeasurementAvailable,
            },
        );
        assert!(series.is_empty());
        assert_eq!(series.total_kwh(), rust_decimal::Decimal::ZERO);
        assert!(series.period_from().is_none());
    }

    #[test]
    fn source_labels_non_empty() {
        let sources = [
            MeasurementSource::Mscons {
                pid: 13005,
                message_ref: None,
                sender_mp_id: "x".into(),
            },
            MeasurementSource::SmgwDirectPush {
                device_id: "d".into(),
                session_id: "s".into(),
            },
            MeasurementSource::ManualEntry {
                operator_id: "o".into(),
                reason: "r".into(),
            },
            MeasurementSource::AutoSubstitute {
                method: crate::substitute::SubstituteMethod::LinearInterpolation,
                reason: crate::substitute::SubstitutionReason::MeterFault,
            },
            MeasurementSource::RetroactiveCorrection {
                correction_id: Uuid::new_v4(),
                corrected_by: "op".into(),
            },
            MeasurementSource::VirtualMeter {
                rule_type: "Sum".into(),
                source_malo_ids: vec![],
            },
            MeasurementSource::RedispatchImport {
                pid: 13022,
                activation_ref: None,
            },
        ];
        for s in &sources {
            assert!(!s.label().is_empty());
        }
    }

    #[test]
    fn obis_default_resolution_wires_into_series() {
        let series = MeasurementSeries::new(
            "51238696780",
            Some(ObisCode::STROM_BEZUG_TOTAL),
            vec![],
            MeasurementSource::Mscons {
                pid: 13005,
                message_ref: None,
                sender_mp_id: "x".into(),
            },
        );
        assert_eq!(
            series.resolution,
            Some(crate::resolution::IntervalResolution::QuarterHour)
        );
    }
}
