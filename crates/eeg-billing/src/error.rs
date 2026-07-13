//! Error type for `eeg-billing`.

use thiserror::Error;

/// Errors that can be returned by `eeg-billing` functions.
#[derive(Debug, Error)]
pub enum SettlementError {
    /// Settlement amount exceeded the representable range (> ±92 233 720 368 EUR).
    ///
    /// This is impossible in practice for any real EEG/KWKG plant.
    #[error("settlement amount out of representable range: {0}")]
    AmountOutOfRange(rust_decimal::Decimal),
}
