//! `BillingContext` — the immutable billing metadata passed to every provider.
//!
//! Separates *what we're billing* (quantities, products) from *how we're billing it*
//! (period, identifiers, invoice type, regulatory rates).

use crate::rates::RegulatoryRates;

// ── InvoiceType ───────────────────────────────────────────────────────────────

/// Whether this is an initial invoice, a correction, a cancellation, or a final settlement.
///
/// German energy suppliers frequently perform:
/// ```text
/// Initial invoice  →  Correction (corrected meter reading)
///                  →  Cancellation (full reversal)
///                  →  Final (annual Schlussabrechnung)
/// ```
///
/// ## §22 MessZV compliance
///
/// Corrections must reference the original invoice ID for the 3-year audit trail.
/// Cancellations reverse the original to EUR 0.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InvoiceType {
    /// Standard billing run (Abschlagsrechnung, periodic invoice).
    Initial,

    /// Credit note (Gutschrift) — outgoing payment to a third party.
    ///
    /// Used for:
    /// - EEG feed-in settlement (payment to generator)
    /// - EINSPEISUNG Direktvermarktung settlement
    /// - Reverse-charge scenarios
    ///
    /// `rechnungsart` = `"GUTSCHRIFT"`
    CreditNote,

    /// Correction superseding an earlier invoice (§22 MessZV).
    ///
    /// The original invoice must be referenced in the accounting system.
    /// The net effect is: `original + correction = corrected total`.
    Correction {
        /// ID of the original invoice this corrects.
        original_invoice_id: String,
        /// Human-readable reason (for audit trail).
        reason: Option<String>,
    },

    /// Full reversal of an earlier invoice (Stornorechnung).
    ///
    /// All positions are sign-inverted to bring the original to EUR 0.
    Cancellation {
        /// ID of the original invoice being cancelled.
        original_invoice_id: String,
    },

    /// Annual final settlement (Schlussabrechnung / Jahresabrechnung).
    ///
    /// Reconciles advance payments against measured consumption.
    Final,
}

impl InvoiceType {
    /// BO4E Rechnungsart string for the rechnung_json field.
    #[must_use]
    pub fn rechnungsart(&self) -> &'static str {
        match self {
            Self::Initial => "ABSCHLAGSRECHNUNG",
            Self::CreditNote => "GUTSCHRIFT",
            Self::Correction { .. } => "KORREKTURRECHNUNG",
            Self::Cancellation { .. } => "STORNORECHNUNG",
            Self::Final => "SCHLUSSRECHNUNG",
        }
    }

    /// Returns the original invoice ID for corrections and cancellations.
    #[must_use]
    pub fn original_invoice_id(&self) -> Option<&str> {
        match self {
            Self::Correction {
                original_invoice_id,
                ..
            }
            | Self::Cancellation {
                original_invoice_id,
            } => Some(original_invoice_id),
            _ => None,
        }
    }

    /// `true` when this invoice reverses all positions of the original.
    #[must_use]
    pub fn is_reversal(&self) -> bool {
        matches!(self, Self::Cancellation { .. })
    }
}

#[allow(clippy::derivable_impls)]
impl Default for InvoiceType {
    fn default() -> Self {
        Self::Initial
    }
}

// ── BillingContext ────────────────────────────────────────────────────────────

/// Immutable billing metadata — the *context* for one invoice generation run.
///
/// Every [`BillingProvider`][crate::BillingProvider] receives a reference to
/// the same `BillingContext` so all positions share identical period, party IDs,
/// and regulatory rates.
///
/// ## Design rationale
///
/// Previously these 8+ parameters were passed positionally to each `calculate_*`
/// function, making call sites error-prone and impossible to extend without
/// breaking changes. `BillingContext` is a named, extensible aggregate.
///
/// ## Example
///
/// ```rust
/// use energy_billing::{BillingContext, InvoiceType, RegulatoryRates};
/// use time::macros::date;
///
/// let ctx = BillingContext {
///     malo_id: "51238696781".to_owned(),
///     lf_mp_id: "9900000000001".to_owned(),
///     rechnungsnummer: "R2026-001".to_owned(),
///     period_from: date!(2026-01-01),
///     period_to: date!(2026-01-31),
///     invoice_type: InvoiceType::Initial,
///     regulatory_rates: RegulatoryRates::default(),
///     contract_id: None,
/// };
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BillingContext {
    /// 11-digit Marktlokations-ID of the delivery point.
    pub malo_id: String,

    /// BDEW/DVGW Codenummer of the Lieferant (invoice issuer).
    pub lf_mp_id: String,

    /// Invoice number (Rechnungsnummer) — unique per invoice.
    ///
    /// Operator's responsibility to ensure uniqueness. Recommended format:
    /// `{prefix}-{year}-{sequence}` (e.g. `"INV-2026-000001"`).
    pub rechnungsnummer: String,

    /// First day of the billing period (inclusive).
    pub period_from: time::Date,

    /// Last day of the billing period (inclusive).
    pub period_to: time::Date,

    /// Invoice type: initial, correction, cancellation, or final settlement.
    pub invoice_type: InvoiceType,

    /// Statutory levy rates (Stromsteuer, Energiesteuer, BEHG, MwSt).
    ///
    /// Sourced from `billingd.toml [rates]` — never hardcoded in the library.
    pub regulatory_rates: RegulatoryRates,

    /// Optional contract reference (for LF internal use / ERP routing).
    pub contract_id: Option<String>,
}

impl BillingContext {
    /// Number of calendar days in the billing period.
    ///
    /// Used for Grundpreis (daily rate × days) and pro-rata calculations.
    #[must_use]
    pub fn days(&self) -> i64 {
        let diff = self.period_to - self.period_from;
        diff.whole_days() + 1 // inclusive of both endpoints
    }
}
