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

    /// The delivery period is governed by an Entgelt regime whose methodology
    /// cannot be computed yet.
    ///
    /// StromNEV/GasNEV and the ARegV lapse with the end of 2028; the successor
    /// system AgNeS (GBK-25-01) is draft only — the Rahmenfestlegung is
    /// expected end-2026, and its parameter tables follow as configuration
    /// once they become binding. Refusing is deliberate: computing under the
    /// lapsed Verordnung math and merely tagging the result AgNeS would be
    /// wrong under both regimes. See
    /// [`crate::regulatory::RegulatoryRegime::ensure_berechenbar`].
    #[error(
        "cannot price Netzentgelte for Tarifjahr {tarifjahr}: the period is governed by the \
         AgNeS Entgelt regime (GBK-25-01), whose parameter tables are not yet festgelegt — \
         the Rahmenfestlegung is expected end-2026, and the methodology follows as \
         configuration once binding"
    )]
    UnsupportedEntgeltRegime {
        /// The calendar year whose rates the settlement would have priced.
        tarifjahr: i32,
    },
}
