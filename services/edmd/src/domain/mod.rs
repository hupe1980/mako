//! `edmd`'s own domain vocabulary.
//!
//! The two repository traits, their DTOs and the error type are consumed only by
//! `edmd`, so they live here alongside the service that owns them.
//!
//! - [`model`] — `MeterRead` / `MeterDataReceipt` / `Typ2Read` / correction and
//!   billing-period DTOs, the MSCONS PID tables, and `IngestionSource`.
//! - [`repository`] — the `TimeSeriesRepository` + `Typ2Repository` traits.
//! - [`error`] — `EdmError`.

pub mod error;
pub mod model;
pub mod repository;
pub mod validation;

pub use error::EdmError;
pub use model::*;
pub use repository::{TimeSeriesRepository, Typ2Repository};
