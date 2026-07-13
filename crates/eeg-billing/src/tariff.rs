//! [`billing::Tariff`] implementation bridge for EEG/KWKG settlement.
//!
//! [`EegSettleTariff`] wraps a pre-computed [`SettleOutput`] and exposes it
//! via the [`billing::Tariff`] trait, enabling EEG settlement results to be
//! used in `billing::BillingDocument` generation alongside other tariffs.
//!
//! ## Workflow
//!
//! 1. Call [`crate::calculate_settlement`] to compute the settlement output.
//! 2. Handle non-billable status variants (`NoData`, `PriceMissing`).
//! 3. Wrap the output in [`EegSettleTariff`].
//! 4. Call `.bill(meta, &())` to produce a `BillingDocument`.
//!
//! ## Tax layers
//!
//! `EegSettleTariff::tax_layers()` intentionally returns an **empty** list.
//! The VAT treatment for EEG feed-in depends on the operator's tax status:
//! - **Regelbesteuerung (19% MwSt)**: add `FixedRateTax::new("MwSt", dec!(0.19))`
//! - **Kleinunternehmer (§19 UStG)**: no VAT (common for residential rooftop PV)
//! - **§12 Abs. 3 UStG** (Photovoltaik ≤30 kWp after 01.01.2023): no VAT registration
//!
//! The caller adds the appropriate tax layer before calling `.bill()`.
//!
//! ## Example
//!
//! ```rust
//! use eeg_billing::{SettleInput, SettlementModel, SettlementStatus, calculate_settlement};
//! use eeg_billing::tariff::EegSettleTariff;
//! use billing::{DocumentMeta, Period};
//! use rust_decimal_macros::dec;
//!
//! let output = calculate_settlement(&SettleInput {
//!     model: SettlementModel::Verguetung,
//!     einspeisemenge_kwh: Some(dec!(500)),
//!     verguetungssatz_ct: dec!(8.11),
//!     ..SettleInput::default()
//! });
//!
//! assert_eq!(output.status, SettlementStatus::Calculated);
//!
//! let tariff = EegSettleTariff::new(&output);
//! use billing::Tariff as _;
//! let doc = tariff.bill(
//!     DocumentMeta {
//!         invoice_number: "EEG-2026-07-001".into(),
//!         period_label:   "Juli 2026".into(),
//!         period: Some(Period::new("2026-07-01", "2026-07-31")),
//!         issuer_id: Some("9904234560001".into()),  // NB MP-ID
//!         issue_date: Some("2026-07-13".into()),
//!         ..Default::default()
//!     },
//!     &(),
//! ).unwrap();
//!
//! assert_eq!(doc.net_total(), billing::Amount::parse("40.55000").unwrap());
//! ```

use billing::{BillingError, LineItem, TaxLayer};

use crate::model::{SettleOutput, SettlementStatus};

// ── EegSettleTariff ──────────────────────────────────────────────────────────

/// [`billing::Tariff`] adapter for EEG/KWKG settlement results.
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

impl billing::Tariff for EegSettleTariff<'_> {
    /// `Usage = ()` — the settlement positions are already computed in `SettleOutput`.
    type Usage = ();

    /// Use `BillingError` directly for compatibility with `BillingDocument` construction.
    type Error = BillingError;

    fn line_items(&self, _usage: &()) -> Result<Vec<LineItem>, BillingError> {
        // NoData/PriceMissing → empty, Sanctioned → EUR 0 audit line, etc.
        Ok(crate::bridge::settlement_to_line_items(self.output))
    }

    /// Returns empty — the caller adds the USt layer via [`crate::ust::ust_tax_layers`].
    ///
    /// Use [`EegSettleTariffRegelbesteuerung`] for the common 19 % case.
    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        vec![]
    }
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
///
/// # Deprecated name
///
/// Previously called `EegSettleTariffMitMwSt` — renamed to use the legal term
/// "Umsatzsteuer" and the correct tax-status name.
pub type EegSettleTariffMitMwSt<'a> = EegSettleTariffRegelbesteuerung<'a>;

/// See [`EegSettleTariffMitMwSt`] (now an alias for `EegSettleTariffRegelbesteuerung`).
pub struct EegSettleTariffRegelbesteuerung<'a> {
    inner: EegSettleTariff<'a>,
    ust_rate: rust_decimal::Decimal,
}

impl<'a> EegSettleTariffRegelbesteuerung<'a> {
    /// Create a tariff adapter with 19 % Umsatzsteuer (standard German VAT).
    #[must_use]
    pub fn new(output: &'a SettleOutput) -> Self {
        Self {
            inner: EegSettleTariff::new(output),
            ust_rate: rust_decimal_macros::dec!(0.19),
        }
    }

    /// Create a tariff adapter with a custom USt rate (e.g. 7 % reduced).
    #[must_use]
    pub fn with_rate(output: &'a SettleOutput, ust_rate: rust_decimal::Decimal) -> Self {
        Self {
            inner: EegSettleTariff::new(output),
            ust_rate,
        }
    }
}

impl billing::Tariff for EegSettleTariffRegelbesteuerung<'_> {
    type Usage = ();
    type Error = BillingError;

    fn line_items(&self, usage: &()) -> Result<Vec<LineItem>, BillingError> {
        self.inner.line_items(usage)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        use billing::tax::FixedRateTax;
        let rate_pct = self.ust_rate * rust_decimal::Decimal::from(100u32);
        vec![Box::new(FixedRateTax::new(
            format!("Umsatzsteuer {rate_pct:.0}\u{202f}%"),
            self.ust_rate,
        ))]
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
