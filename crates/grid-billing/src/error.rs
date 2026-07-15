//! Error types for `mako-nne`.

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
    #[error("monetary overflow: amount too large for EuroAmount representation")]
    MonetaryOverflow,
}
