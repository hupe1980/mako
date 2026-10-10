//! `edmd`'s own domain vocabulary.
//!
//! The two repository traits, their DTOs and the error type are consumed only by
//! `edmd`, so they live here alongside the service that owns them.
//!
//! - [`model`] — `MeterRead` / `MeterDataReceipt` / `Typ2Read` / correction and
//!   billing-period DTOs, the MSCONS PID tables, and `IngestionSource`.
//! - [`register`] — which OBIS registers may be folded into one energy figure.
//! - [`repository`] — the `TimeSeriesRepository` + `Typ2Repository` traits.
//! - [`validation`] — the one V-rule pass every ingest door, the MCP tool and
//!   the § 60 Abs. 1 substitute path share.
//! - [`vee`] — one register's readings as a `metering` series, validated over
//!   the window a caller asked about.
//! - [`virtual_meter`] — what a virtual Marktlokation is computed from.
//! - [`error`] — `EdmError`.

pub mod error;
pub mod model;
pub mod register;
pub mod repository;
pub mod validation;
pub mod vee;
pub mod virtual_meter;

pub use error::EdmError;
pub use model::*;
pub use register::{
    EnergyDirection, billable_share_pct, energy_intervals, energy_intervals_from,
    normalise_obis_code, register_groups, worst_quality,
};
pub use repository::{TimeSeriesRepository, Typ2Repository};
pub use validation::{IngestContext, ValidatedReads, batch_period};
