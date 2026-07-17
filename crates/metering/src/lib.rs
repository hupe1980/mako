//! German energy metering domain library.
//!
//! A **standalone**, **pure** library for meter data calculations required by
//! BDEW MaKo, MessZV, GasGVV, and EnWG.  Zero I/O, no async, no float money.
//!
//! This crate supersedes the `meter-quality` crate, which has been folded in.
//!
//! # Modules
//!
//! | Module | Contents |
//! |---|---|
//! | [`interval`] | `MeterInterval`, `Sparte`, `QualityFlag`, `demand_kw()` |
//! | [`conversion`] | Gas m³ → kWh_Hs (§24 GasGVV / DVGW G 685) |
//! | [`aggregation`] | Billing period: `arbeitsmenge_kwh`, `spitzenleistung_kw`, HT/NT |
//! | [`classification`] | SLP/RLM/iMSys detection, interval length |
//! | [`imbalance`] | Mehr-/Mindermengensaldo (§27 MessZV, compute_imbalance) |
//! | [`quality`] | Hampel-filter quality scoring (M7), `score_intervals_raw` for f64 |
//!
//! # Quick start — billing period
//!
//! ```rust
//! use metering::{MeterInterval, QualityFlag, aggregate, AggregationConfig};
//! use rust_decimal::Decimal;
//! use time::macros::datetime;
//!
//! let iv = MeterInterval {
//!     from: datetime!(2026-06-01 0:00 UTC),
//!     to:   datetime!(2026-06-01 0:15 UTC),
//!     value_kwh: Decimal::from_str_exact("2.345").unwrap(),
//!     quality: QualityFlag::Measured,
//!     obis_code: None,
//! };
//! let period = aggregate(&[iv], AggregationConfig::rlm_strom());
//! assert!(period.arbeitsmenge_kwh > Decimal::ZERO);
//! ```
//!
//! # Quick start — Gas m³ → kWh_Hs
//!
//! ```rust
//! use metering::gas_m3_to_kwh_hs;
//! use rust_decimal::Decimal;
//!
//! let kwh = gas_m3_to_kwh_hs(
//!     Decimal::from(100u32),
//!     Decimal::from_str_exact("10.55").unwrap(),
//!     Decimal::from_str_exact("0.9764").unwrap(),
//! );
//! assert!(kwh > Decimal::from(1000u32));
//! ```
//!
//! # Quick start — quality scoring (f64 API, e.g. from DB)
//!
//! ```rust
//! use metering::{score_intervals_raw, QualityGrade};
//!
//! let values = vec![2.3_f64, 2.4, 2.3, 2.5, 2.2, 2.4, 2.3];
//! let grade = score_intervals_raw(&values, 3, 3.0);
//! assert_eq!(grade, QualityGrade::A);
//! ```
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod aggregation;
pub mod aggregation_rule;
pub mod classification;
pub mod conversion;
pub mod demand;
pub mod forecast;
pub mod imbalance;
pub mod interval;
pub mod lifecycle;
pub mod load_profile;
pub mod measurement_point;
pub mod measurement_series;
pub mod obis;
pub mod power_quality;
pub mod quality;
pub mod register;
pub mod resample;
pub mod resolution;
pub mod smgw;
pub mod substitute;
pub mod tariff_window;
pub mod validation;
pub mod virtual_meter;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use aggregation::{AggregationConfig, BillingPeriod, HtNtSplit, aggregate};
pub use aggregation_rule::AggregationRule;
pub use classification::{IntervalLengthClass, Messtyp, classify_messtyp, detect_interval_length};
pub use conversion::{GasConversionParams, gas_m3_to_kwh_hs, normalize_interval_to_kwh};
pub use demand::{DemandInterval, DemandWindow};
pub use forecast::{
    AnnualForecast, ForecastMethod, SubstituteValueEntry, prior_period_substitutes,
    project_annual_consumption,
};
pub use imbalance::{ImbalanceSaldo, compute_imbalance};
pub use interval::{MeterInterval, QualityFlag, Sparte};
pub use lifecycle::{
    MeterExchangeEvent, MeterLifecycleEvent, MeterLifecycleEventType, MeterStatus,
};
pub use load_profile::LoadProfile;
pub use measurement_point::{EnergyFlow, MarktRolle, MeasurementPoint};
pub use measurement_series::{
    MeasurementSeries, MeasurementSource, ProvenanceEntry, ProvenanceEventType,
};
pub use obis::{ObisCode, ObisParseError};
pub use power_quality::PowerQualityInterval;
pub use quality::{
    QualityConfig, QualityGrade, QualityReport, hampel_filter, score_intervals,
    score_intervals_f64, score_intervals_raw,
};
pub use register::{EnergyDirection, MeterRegister, RegisterUnit};
pub use resample::{ResampleConfig, ResampledBucket, resample};
pub use resolution::IntervalResolution;
pub use smgw::{
    CertificateType, ClsChannel, ClsChannelStatus, ClsDeviceType, GatewayCertificate,
    GatewayStatus, SmgwSession,
};
pub use substitute::{
    FillGapsConfig, SubstituteMethod, SubstitutionReason, fill_gaps, fill_gaps_with_config,
    linear_interpolation,
};
pub use tariff_window::{HtNtSchedule, TariffWindow, TariffWindowDays};
pub use validation::{
    ValidationConfig, ValidationIssue, ValidationResult, ValidationRuleId, ValidationSeverity,
    validate_intervals,
};
pub use virtual_meter::{VirtualMeterError, compute_virtual_meter};
