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

    /// `Invoice::allocate_proportionally` was given weights it cannot normalise.
    ///
    /// A negative weight is not an allocation share, and weights summing to
    /// zero name no recipient at all. Both are caller mistakes rather than
    /// arithmetic failures, so they are refused before the split runs.
    #[error("allocation weights must be non-negative and sum above zero, got sum {sum}")]
    AllocationWeightsInvalid {
        /// The sum of the supplied weights.
        sum: Decimal,
    },

    /// A §42b EEG Nutzungsplan whose shares do not describe an allocation.
    ///
    /// The plan's fractions are caller-supplied and must partition the plant's
    /// generation exactly once — a plan entered as percentages allocates a
    /// hundred times the generation, and one summing short leaves kWh
    /// unallocated. Refused before the split rather than distributed.
    #[error("GGV Nutzungsplan shares must sum to 1, got {sum}")]
    NutzungsplanSharesInvalid {
        /// The sum of the supplied shares.
        sum: Decimal,
    },

    /// A value the EN 16931 semantic model cannot represent.
    ///
    /// A rendered e-invoice is a legally binding document: a line amount
    /// saturated to `0.00`, or a BT-2 Ausstellungsdatum taken from a fallback
    /// constant, states as fact something § 14 Abs. 4 UStG requires the document
    /// to get right. Both are refused rather than emitted.
    #[error("{field} cannot be represented in the EN 16931 model: {value}")]
    Unrepresentable {
        /// The business term that could not be represented (e.g. `"BT-131"`).
        field: String,
        /// The value that could not be represented.
        value: String,
    },

    /// EN 16931 reconciliation (BG-22/BG-23, BR-CO-10..16) failed.
    ///
    /// The breakdown and the totals of an e-invoice are *derived*, and a
    /// derivation that fails leaves the document carrying whichever totals the
    /// builder happened to hold — a document that states amounts nothing
    /// computed. Refused rather than emitted.
    #[error("EN 16931 reconciliation failed: {reason}")]
    ReconciliationFailed {
        /// What the reconciler reported.
        reason: String,
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
            Self::AllocationWeightsInvalid { .. } => "ALLOCATION_WEIGHTS_INVALID",
            Self::NutzungsplanSharesInvalid { .. } => "NUTZUNGSPLAN_SHARES_INVALID",
            Self::Unrepresentable { .. } => "UNREPRESENTABLE",
            Self::ReconciliationFailed { .. } => "RECONCILIATION_FAILED",
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
                    code: "MODUL3_AND_FLAT_NNE",
                    severity: WarningSeverity::Error,
                    message: "both configured".into(),
                },
            ],
        };
        let s = err.to_string();
        assert!(s.contains("MODUL3_AND_FLAT_NNE: both configured"), "{s}");
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
