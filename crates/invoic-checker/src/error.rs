//! Errors from the `invoic-checker` pipeline.

use thiserror::Error;

/// Errors that can occur during INVOIC validation.
#[derive(Debug, Error)]
pub enum CheckError {
    /// No tariff entry found for the given GLN and billing period.
    #[error("tariff not found for sender GLN '{mp_id}' on date '{date}'")]
    TariffNotFound { mp_id: String, date: String },
}
