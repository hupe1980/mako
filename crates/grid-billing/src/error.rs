//! Error types for `grid-billing`.

/// Errors returned by the billing calculation functions.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BillingError {
    /// The billing input contains an invalid or inconsistent value.
    #[error("invalid billing input: {reason}")]
    InvalidInput {
        /// Human-readable explanation. Dynamic `String` so callers can include runtime context.
        reason: String,
    },

    /// Monetary precision overflow — the calculated amount exceeds `i64` range.
    ///
    /// This can only happen for unrealistically large billing amounts (> ~92 million EUR).
    /// `input_value` carries the `Decimal` that caused the overflow so callers can log it.
    #[error("monetary overflow: amount {input_value:?} too large for EuroAmount representation")]
    MonetaryOverflow {
        /// The value that caused the overflow, if available.
        input_value: Option<rust_decimal::Decimal>,
    },
}
