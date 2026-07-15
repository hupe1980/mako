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
pub mod classification;
pub mod conversion;
pub mod imbalance;
pub mod interval;
pub mod quality;
pub mod substitute;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use aggregation::{AggregationConfig, BillingPeriod, HtNtSplit, aggregate};
pub use classification::{IntervalLengthClass, Messtyp, classify_messtyp, detect_interval_length};
pub use conversion::{GasConversionParams, gas_m3_to_kwh_hs, normalize_interval_to_kwh};
pub use imbalance::{ImbalanceSaldo, compute_imbalance};
pub use interval::{MeterInterval, QualityFlag, Sparte};
pub use quality::{
    QualityConfig, QualityGrade, QualityReport, hampel_filter, score_intervals, score_intervals_raw,
};
pub use substitute::{
    FillGapsConfig, SubstituteMethod, fill_gaps, fill_gaps_with_config, linear_interpolation,
};
