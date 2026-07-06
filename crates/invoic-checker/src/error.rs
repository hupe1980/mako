//! Errors from the `invoic-checker` pipeline.

use thiserror::Error;

/// Errors that can occur during INVOIC validation.
#[derive(Debug, Error)]
pub enum CheckError {
    /// No tariff entry found for the given GLN and billing period.
    #[error("tariff not found for sender GLN '{gln}' on date '{date}'")]
    TariffNotFound { gln: String, date: String },
}
