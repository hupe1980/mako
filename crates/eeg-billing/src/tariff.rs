//! [`billing::ScalarTariff`] implementation bridge for EEG/KWKG settlement.
//!
//! [`EegSettleTariff`] wraps a pre-computed [`SettleOutput`] and exposes it
//! via the [`billing::ScalarTariff`] trait (billing 0.8 — no `Usage`, no ignored
//! argument), enabling EEG settlement results to be used in
//! `billing::BillingDocument` generation alongside other tariffs.
//!
//! ## Workflow
//!
//! 1. Call [`crate::calculate_settlement`] to compute the settlement output.
//! 2. Handle non-billable status variants (`NoData`, `PriceMissing`).
//! 3. Wrap the output in [`EegSettleTariff`].
//! 4. Call `.settle(meta)` (billing 0.8 [`billing::ScalarTariff`]) to produce a `BillingDocument`.
//!
//! ## Tax layers
//!
//! `EegSettleTariff::tax_layers()` intentionally returns an **empty** list.
//! The VAT treatment for EEG feed-in depends on the operator's tax status:
//! - **Regelbesteuerung (19% MwSt)**: add `FixedRateTax::new("MwSt", dec!(0.19))`
//! - **Kleinunternehmer (§19 UStG)**: no VAT (common for residential rooftop PV)
//! - **§12 Abs. 3 UStG** (Photovoltaik ≤30 kWp after 01.01.2023): no VAT registration
//!
//! The caller adds the appropriate tax layer before calling `.settle()`.
//!
//! ## Example
//!
//! ```rust
//! use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus, calculate_settlement};
//! use eeg_billing::tariff::EegSettleTariff;
//! use billing::{DocumentMeta, Period};
//! use rust_decimal::dec;
//!
//! let output = calculate_settlement(&SettleInput {
//!     scheme: eeg_billing::SettlementScheme::FeedInTariff { verguetungssatz_ct: dec!(8.11) },
//!     einspeisemenge_kwh: Some(dec!(500)),
//!     ..SettleInput::default()
//! });
//!
//! assert_eq!(output.status, SettlementStatus::Calculated);
//!
//! let tariff = EegSettleTariff::new(&output);
//! use billing::ScalarTariff as _;
//! let doc = tariff.settle(
//!     DocumentMeta {
//!         invoice_number: "EEG-2026-07-001".into(),
//!         period_label:   "Juli 2026".into(),
//!         period: Some(Period::from_display("2026-07-01", "2026-07-31")),
//!         issuer_id: Some("9904234560001".into()),  // NB MP-ID
//!         issue_date: Some("2026-07-13".into()),
//!         ..Default::default()
//!     },
//! ).unwrap();
//!
//! assert_eq!(doc.net_total(), billing::Amount::parse("40.55000").unwrap());
//! ```

use billing::{BillingError, TaxLayer, tax::FixedRateTax};
use rust_decimal::dec;

use crate::model::{SettleOutput, SettlementStatus};

// ── EegSettleTariff ──────────────────────────────────────────────────────────

/// [`billing::ScalarTariff`] adapter for EEG/KWKG settlement results.
///
/// Wraps a pre-computed [`SettleOutput`] and exposes it through the `Tariff` trait
/// so EEG settlement can be composed with other billing positions and documents.
///
/// See [module-level docs](crate::tariff) for usage and VAT guidance.
pub struct EegSettleTariff<'a> {
    output: &'a SettleOutput,
}

impl<'a> EegSettleTariff<'a> {
    /// Create a new adapter from a settlement output.
    ///
    /// # Panics
    ///
    /// Does not panic. For `NoData`, `PriceMissing`, or `Sanctioned` status,
    /// `line_items()` returns an empty `Vec` (no positions on the document).
    /// For `FoerderungBeendet`, returns an empty `Vec` as well.
    ///
    /// To generate a §25-sanction audit line (EUR 0 credit), use
    /// `eeg_billing::bridge::settlement_to_line_items()` instead.
    #[must_use]
    pub fn new(output: &'a SettleOutput) -> Self {
        Self { output }
    }
}

impl billing::ScalarTariff for EegSettleTariff<'_> {
    /// Use `BillingError` directly for compatibility with `BillingDocument` construction.
    type Error = BillingError;

    /// `Infallible` — the billing layer always renders *some* document. The
    /// domain-level not-billable reason (NoData / PriceMissing / Sanctioned /
    /// FoerderungBeendet) lives on [`SettleOutput::status`], which is richer than a
    /// two-state billing reason and is what callers already inspect.
    type NotBillable = std::convert::Infallible;

    /// `ScalarTariff` (billing 0.8): the positions are already computed in
    /// `SettleOutput`, so there is no `Usage` and no ignored `_usage` argument.
    fn positions(&self) -> Result<billing::Positions<std::convert::Infallible>, BillingError> {
        // NoData/PriceMissing → empty, Sanctioned → EUR 0 audit line, etc.
        Ok(crate::bridge::settlement_to_line_items(self.output).into())
    }

    // Tax layers default to empty — the caller adds USt via [`crate::ust::ust_tax_layers`].
    // Use [`EegSettleTariffRegelbesteuerung`] for the common 19 % case.
}

// ── EegSettleTariffRegelbesteuerung ──────────────────────────────────────────

/// Convenience wrapper that includes 19 % Umsatzsteuer (Regelbesteuerung).
///
/// Correct for:
/// - Commercial operators (Gewerbetreibende)
/// - Operators who opted into Regelbesteuerung
/// - Plants > 30 kWp or commissioned before 01.01.2023
///
/// NOT for Kleinunternehmer (§19 UStG) or §12 Abs. 3 exempt plants (PV ≤30 kWp, post-2023).
/// Use [`EegSettleTariff12Abs3`] or [`EegSettleTariff`] for those.
pub struct EegSettleTariffRegelbesteuerung<'a> {
    inner: EegSettleTariff<'a>,
    ust: FixedRateTax,
}

impl<'a> EegSettleTariffRegelbesteuerung<'a> {
    /// Create a tariff adapter with 19 % Umsatzsteuer (standard German VAT).
    #[must_use]
    pub fn new(output: &'a SettleOutput) -> Self {
        Self {
            inner: EegSettleTariff::new(output),
            ust: ust_layer(dec!(0.19)).expect("19 % is a valid rate"),
        }
    }

    /// Create a tariff adapter with a custom USt rate (e.g. 7 % reduced).
    ///
    /// The layer is built here rather than in `tax_layers` so that a rate read
    /// from configuration is rejected at construction, where the caller can still
    /// act on it.
    ///
    /// # Errors
    ///
    /// Returns [`BillingError::InvalidInput`] if `ust_rate` is negative.
    pub fn with_rate(
        output: &'a SettleOutput,
        ust_rate: rust_decimal::Decimal,
    ) -> Result<Self, BillingError> {
        Ok(Self {
            inner: EegSettleTariff::new(output),
            ust: ust_layer(ust_rate)?,
        })
    }
}

/// Build the Umsatzsteuer layer, naming it with the rate as a percentage.
fn ust_layer(rate: rust_decimal::Decimal) -> Result<FixedRateTax, BillingError> {
    let pct = rate * rust_decimal::Decimal::from(100u32);
    FixedRateTax::new(format!("Umsatzsteuer {pct:.0}\u{202f}%"), rate)
}

impl billing::ScalarTariff for EegSettleTariffRegelbesteuerung<'_> {
    type Error = BillingError;
    type NotBillable = std::convert::Infallible;

    fn positions(&self) -> Result<billing::Positions<std::convert::Infallible>, BillingError> {
        billing::ScalarTariff::positions(&self.inner)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        // billing 0.8: `.boxed()` replaces `Box::new(_) as Box<dyn TaxLayer>`.
        vec![self.ust.clone().boxed()]
    }
}

// ── Status check helpers ──────────────────────────────────────────────────────

/// Convenience alias: `EegSettleTariff` for plants exempt under **§12 Abs. 3 UStG**
/// (Solar PV ≤ 30 kWp, commissioned after 01.01.2023).
///
/// Produces the same document as `EegSettleTariff` (empty tax layers),
/// but the name makes the VAT reasoning explicit in calling code.
pub type EegSettleTariff12Abs3<'a> = EegSettleTariff<'a>;

/// Convenience alias: `EegSettleTariff` for Kleinunternehmer operators (§19 UStG).
pub type EegSettleTariffKleinunternehmer<'a> = EegSettleTariff<'a>;

/// Return `true` when the settlement output can be turned into a billing document.
///
/// `NoData` and `PriceMissing` produce empty documents (no positions to bill);
/// `Sanctioned` produces a EUR 0 audit line via `settlement_to_line_items()`.
/// `FoerderungBeendet` produces an empty document.
/// `Calculated` produces the normal positions.
#[must_use]
pub fn is_billable(output: &SettleOutput) -> bool {
    !matches!(
        output.status,
        SettlementStatus::NoData | SettlementStatus::PriceMissing
    )
}
