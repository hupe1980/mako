//! Bridge to [`billing`] document types.
//!
//! [`SettleOutput`] carries `positions: Vec<SettlePosition>`, so this module is
//! a thin adapter: [`settlement_to_lines`] calls `SettlePosition::to_line` on
//! each position and issues no lines for the statuses that have nothing to bill
//! yet, and [`settlement_document`] builds the `billing::Document` those lines
//! and the operator's VAT treatment make.
//!
//! EEG settlement is a **scalar calculation** — the positions are already
//! computed by [`crate::calculate_settlement`], with no usage input. The domain
//! not-billable reasons (`NoData` / `PriceMissing`) live on
//! [`crate::SettleOutput::status`], which is richer than a billing-layer reason
//! and is what callers inspect ([`is_billable`]).
//!
//! # Example
//!
//! ```rust
//! use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement};
//! use eeg_billing::bridge::{settlement_document, settlement_to_lines};
//! use eeg_billing::ust::VatStatus;
//! use rust_decimal::dec;
//!
//! let output = calculate_settlement(&SettleInput {
//!     scheme: eeg_billing::SettlementScheme::FeedInTariff { verguetungssatz_ct: dec!(8.11) },
//!     einspeisemenge_kwh: Some(dec!(500)),
//!     ..SettleInput::default()
//! });
//! let lines = settlement_to_lines(&output);
//! assert_eq!(lines.len(), 1);
//! assert!(lines[0].description().contains("EEG"));
//!
//! let doc = settlement_document(&output, VatStatus::Regelbesteuerung).unwrap();
//! assert_eq!(doc.tax_exclusive_total().to_string(), "40.55000");
//! assert_eq!(doc.vat_total().to_string(), "7.70000"); // 40.55 × 19 % = 7.7045 → 7.70
//! ```

use billing::{AmountScale, Currency, Document, Line};

use crate::model::{SettleOutput, SettlementStatus};
use crate::ust::{VatStatus, ust_vat};

/// Convert a settlement result into [`billing::Line`]s.
///
/// Returns an **empty** `Vec` for `NoData`, `PriceMissing` and
/// `FassungNichtTabelliert` — nothing to bill yet.
///
/// Every other status delegates to `SettlePosition::to_line` on the
/// positions [`crate::calculate_settlement`] already computed. That includes
/// `Sanctioned`: only §52 Abs. 1 EEG 2021 reduces the Vergütung to zero, while
/// Abs. 2 pays the Monatsmarktwert and Abs. 3 pays 80 % of the ordinary
/// Vergütung, and each of the three states its own §52 Absatz in the position it
/// carries. `KeinAnspruch` likewise carries the position naming the provision
/// that leaves nothing owed, and `JahreskontingentErschoepft` — a KWK plant that
/// has drawn its § 8 Abs. 4 KWKG hours for the calendar year — carries no
/// position, so it bills nothing until January.
#[must_use]
pub fn settlement_to_lines(output: &SettleOutput) -> Vec<Line> {
    match output.status {
        // Nothing to bill — no document positions issued.
        SettlementStatus::NoData
        | SettlementStatus::PriceMissing
        | SettlementStatus::FassungNichtTabelliert => vec![],

        SettlementStatus::Calculated
        | SettlementStatus::FoerderungBeendet
        | SettlementStatus::JahreskontingentErschoepft
        | SettlementStatus::Sanctioned
        | SettlementStatus::KeinAnspruch => output.positions.iter().map(|p| p.to_line()).collect(),
    }
}

/// The settlement as a `billing::Document` in EUR at the EN 16931 scale (two
/// decimals, half away from zero), every line under the operator's VAT
/// treatment ([`ust_vat`]).
///
/// Every total is derived once by `billing`: each line's net rounded once from
/// its exact `kwh × rate`, the VAT group's tax rounded once from its exact
/// base. A settlement with no billable positions is a document with no lines.
///
/// # Errors
///
/// `billing`'s refusal when a line cannot be represented (an amount outside
/// the `Amount` range).
pub fn settlement_document(
    output: &SettleOutput,
    vat: VatStatus,
) -> Result<Document, billing::Error> {
    Document::builder(Currency::EUR, AmountScale::EN16931)
        .default_vat(ust_vat(vat))
        .lines(settlement_to_lines(output))
        .build()
}

/// Return `true` when the settlement output can be turned into a billing document.
///
/// `NoData`, `PriceMissing` and `FassungNichtTabelliert` produce empty
/// documents (no positions to bill);
/// `Sanctioned` produces a EUR 0 audit line via [`settlement_to_lines`].
/// `FoerderungBeendet` produces an empty document.
/// `Calculated` produces the normal positions.
#[must_use]
pub fn is_billable(output: &SettleOutput) -> bool {
    !matches!(
        output.status,
        SettlementStatus::NoData
            | SettlementStatus::PriceMissing
            | SettlementStatus::FassungNichtTabelliert
    )
}
