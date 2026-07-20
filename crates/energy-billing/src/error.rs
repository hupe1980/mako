//! `EngineError` — the typed error surface of the billing engine.
//!
//! Every failure mode a caller can act on differently is its own variant:
//! a blocked regulatory validation carries the warnings that blocked it, a
//! price out of monetary range names the tariff field, an invalid period
//! carries both dates. The arithmetic core (`billing` crate) keeps its own
//! error type; it passes through as [`EngineError::Arithmetic`].
//!
//! Each variant maps to a stable machine-readable [`code`](EngineError::code)
//! so services can build structured error responses without parsing display
//! strings.

use rust_decimal::Decimal;

use crate::position::BillingWarning;

/// Errors returned by [`BillingEngine::bill`](crate::BillingEngine::bill) and
/// the invoice assembly functions.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EngineError {
    /// One or more providers raised `Error`-severity regulatory warnings
    /// during validation — the run must not produce an invoice.
    ///
    /// Carries **all** warnings collected up to and including the blocking
    /// provider, so the caller sees every violation at once. The blocking
    /// ones are those with [`WarningSeverity::Error`](crate::WarningSeverity::Error).
    #[error(
        "billing blocked by regulatory validation: {}",
        blocking_summary(warnings)
    )]
    ValidationBlocked {
        /// All warnings collected before the run was aborted.
        warnings: Vec<BillingWarning>,
    },

    /// A tariff price could not be represented in the monetary type.
    ///
    /// Raised when a configured ct/kWh price exceeds the `Amount` range —
    /// in practice always a corrupt tariff, never a real price.
    #[error("tariff field {field} out of monetary range: {value}")]
    PriceOutOfRange {
        /// The tariff field holding the offending value (e.g. `"arbeitspreis_ht_ct_per_kwh"`).
        field: String,
        /// The value that could not be represented.
        value: Decimal,
    },

    /// A billing period whose end precedes its start.
    ///
    /// Unreachable through [`BillingPeriod::new`](crate::BillingPeriod::new) —
    /// this is what the constructor returns, making the invalid pair
    /// unrepresentable everywhere downstream.
    #[error("invalid billing period: {from} is after {to}")]
    InvalidPeriod {
        /// The requested first day.
        from: time::Date,
        /// The requested last day, before `from`.
        to: time::Date,
    },

    /// `Invoice::allocate_proportionally` was called with mismatched shapes.
    #[error("allocation mismatch: {fractions} fractions vs {contexts} contexts")]
    AllocationMismatch {
        /// Number of allocation fractions supplied.
        fractions: usize,
        /// Number of recipient contexts supplied.
        contexts: usize,
    },

    /// An arithmetic or document error from the `billing` core —
    /// monetary overflow, invalid schedule, tax-layer failure.
    #[error(transparent)]
    Arithmetic(#[from] billing::BillingError),
}

impl EngineError {
    /// Stable machine-readable code for structured error responses.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ValidationBlocked { .. } => "VALIDATION_BLOCKED",
            Self::PriceOutOfRange { .. } => "PRICE_OUT_OF_RANGE",
            Self::InvalidPeriod { .. } => "INVALID_PERIOD",
            Self::AllocationMismatch { .. } => "ALLOCATION_MISMATCH",
            Self::Arithmetic(_) => "ARITHMETIC",
        }
    }

    /// The `Error`-severity warnings that blocked the run, when this is a
    /// [`ValidationBlocked`](Self::ValidationBlocked).
    #[must_use]
    pub fn blocking_warnings(&self) -> &[BillingWarning] {
        match self {
            Self::ValidationBlocked { warnings } => warnings,
            _ => &[],
        }
    }
}

/// Display helper: the blocking warnings as `CODE: message; CODE: message`.
fn blocking_summary(warnings: &[BillingWarning]) -> String {
    warnings
        .iter()
        .filter(|w| w.severity == crate::position::WarningSeverity::Error)
        .map(|w| format!("{}: {}", w.code, w.message))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::WarningSeverity;

    /// The display shows only the blocking warnings, prefixed with their codes.
    #[test]
    fn validation_blocked_displays_codes() {
        let err = EngineError::ValidationBlocked {
            warnings: vec![
                BillingWarning {
                    code: "ESTIMATED_READING",
                    severity: WarningSeverity::Warning,
                    message: "reading estimated".into(),
                },
                BillingWarning {
                    code: "MODUL2_AND_FLAT_NNE",
                    severity: WarningSeverity::Error,
                    message: "both configured".into(),
                },
            ],
        };
        let s = err.to_string();
        assert!(s.contains("MODUL2_AND_FLAT_NNE: both configured"), "{s}");
        assert!(!s.contains("ESTIMATED_READING"), "{s}");
        assert_eq!(err.code(), "VALIDATION_BLOCKED");
        assert_eq!(err.blocking_warnings().len(), 2);
    }

    /// Arithmetic errors pass through transparently, keeping their message.
    #[test]
    fn arithmetic_passthrough() {
        let inner = billing::BillingError::InvalidInput {
            reason: "negative quantity".into(),
        };
        let err: EngineError = inner.into();
        assert_eq!(err.code(), "ARITHMETIC");
        assert!(err.to_string().contains("negative quantity"));
    }
}
