#![deny(unsafe_code)]
#![allow(clippy::doc_markdown)]
//! Energy Data Management library for the German energy market (MaKo/LF role).
//!
//! Handles the full lifecycle of metered energy quantity data sourced from
//! inbound MSCONS messages:
//!
//! ```text
//! NB sends MSCONS (meter reads) ─► edmd webhook
//!                                       │
//!                              validate + parse
//!                                       │
//!                         ┌─────────────▼──────────────┐
//!                         │  TimeSeriesRepository       │
//!                         │  (TimescaleDB in prod)      │
//!                         └─────────────┬──────────────┘
//!                                       │
//!              ┌────────────────────────┼──────────────────────────┐
//!              ▼                        ▼                          ▼
//!        query(malo_id,          billing_period(malo_id,    imbalance(malo_id,
//!          from, to)               period_from,               period_from,
//!                                  period_to)                 period_to)
//! ```
//!
//! ## MSCONS PIDs handled
//!
//! | PID   | Description                                    | Direction |
//! |-------|------------------------------------------------|-----------|
//! | 13005 | EEG-Überführungszeitreihe                      | NB → LF   |
//! | 13006 | Stornierung von Messwerten                     | NB → LF   |
//! | 13015 | Bewegungsdaten vor Lieferbeginn                | NB → LF   |
//! | 13016 | Messwerte Berechnungsformel                    | NB → LF   |
//! | 13017 | Messwerte Allokation                           | NB → LF   |
//! | 13018 | Messwerte Prognose                             | NB → LF   |
//! | 13019 | Messwerte Zählerstand (RLM)                    | NB → LF   |
//! | 13025 | Messwerte Jahreswert Energiemenge              | NB → LF   |
//! | 13027 | Messwerte Grundversorgung                      | NB → LF   |
//!
//! Source: GPKE BK6-24-174, WiM BK6-24-174; direction NB → LF.
//!
//! ## M15 — `MeterBillingPeriod`
//!
//! [`domain::MeterBillingPeriod`] aggregates meter reads for one MaLo over a
//! billing period.  It is the input for:
//!
//! - `invoicd` RLM plausibility (M16): `spitzenleistung_kw` (Leistungspreisanteil)
//! - `netzbilanzd` NNE invoice generation (N4): all billing positions
//! - Gas energy conversion: `arbeitsmenge_kwh = m³ × brennwert × zustandszahl`
//!
//! | Module | Content |
//! |---|---|
//! | [`domain`] | `MeterDataReceipt`, `MeterRead`, `ImbalanceReport`, `MeterBillingPeriod`, `Messtyp`, `BillingPeriodQuery` |
//! | [`repository`] | `TimeSeriesRepository` trait incl. `billing_period()` |
//! | [`error`] | `EdmError` |
//! | [`testing`] | In-memory impls (behind `testing` feature) |

pub mod archive;
pub mod domain;
pub mod error;
pub mod repository;

#[cfg(feature = "testing")]
pub mod testing;

// ── Root re-exports ───────────────────────────────────────────────────────────

pub use domain::{
    ALL_MSCONS_PIDS, BilanzierungsgebietId, BilanzkreisId, BilanzzuordnungRecord,
    BillingPeriodQuery, CorrectionRecord, CorrectionRequest, CorrectionResponse, CorrectionSource,
    GAS_MMMA_PIDS, GAS_QUALITY_PIDS, GasQualityData, ImbalanceReport, IngestionSource, MSCONS_PIDS,
    Messtyp, MeterBillingPeriod, MeterDataReceipt, MeterRead, QualityFlag, REDISPATCH_MSCONS_PIDS,
    Sparte, TimeSeriesQuery, mscons_pid_description,
};
pub use error::EdmError;
pub use repository::TimeSeriesRepository;
