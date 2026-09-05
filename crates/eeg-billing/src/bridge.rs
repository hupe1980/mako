//! Bridge to [`billing`] document types.
//!
//! [`SettleOutput`] carries `positions: Vec<SettlePosition>`, so this module is
//! a thin adapter — it calls `SettlePosition::to_line_item` on each position and
//! issues no lines for the two statuses that have nothing to bill yet.
//!
//! # A `PricingModel` with no usage
//!
//! EEG settlement is a **scalar calculation** — the positions are already computed
//! by [`crate::calculate_settlement`], with no usage input. `billing` 0.12 folded
//! the old `ScalarTariff` into [`billing::PricingModel`] rather than keeping two
//! traits, so that shape is now expressed as `type Usage = ()`;
//! [`crate::tariff::EegSettleTariff`] implements it. The domain not-billable
//! reasons (`NoData` / `PriceMissing`) live on
//! [`crate::SettleOutput::status`], which is richer than a billing-layer reason and
//! is what callers inspect. This module only converts positions to
//! `billing::LineItem`.
//!
//! # Example
//!
//! ```rust
//! use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement};
//! use eeg_billing::bridge::settlement_to_line_items;
//! use rust_decimal::Decimal;
//!
//! let output = calculate_settlement(&SettleInput {
//!     scheme: eeg_billing::SettlementScheme::FeedInTariff { verguetungssatz_ct: Decimal::from_str_exact("8.11").unwrap() },
//!     einspeisemenge_kwh: Some(Decimal::from(500u32)),
//!     ..SettleInput::default()
//! });
//! let items = settlement_to_line_items(&output);
//! assert_eq!(items.len(), 1);
//! assert!(items[0].description.contains("EEG"));
//! ```

use billing::LineItem;

use crate::model::{SettleOutput, SettlementStatus};

/// Convert a settlement result into [`billing::LineItem`] positions.
///
/// Returns an **empty** `Vec` for `NoData` and `PriceMissing` — nothing to bill yet.
///
/// Every other status delegates to `SettlePosition::to_line_item` on the
/// positions [`crate::calculate_settlement`] already computed. That includes
/// `Sanctioned`: only §52 Abs. 1 EEG 2021 reduces the Vergütung to zero, while
/// Abs. 2 pays the Monatsmarktwert and Abs. 3 pays 80 % of the ordinary
/// Vergütung, and each of the three states its own §52 Absatz in the position it
/// carries. `KeinAnspruch` likewise carries the position naming the provision
/// that leaves nothing owed, and `JahreskontingentErschoepft` — a KWK plant that
/// has drawn its § 8 Abs. 4 KWKG hours for the calendar year — carries no
/// position, so it bills nothing until January.
pub fn settlement_to_line_items(output: &SettleOutput) -> Vec<LineItem> {
    match output.status {
        // Nothing to bill — no document positions issued.
        SettlementStatus::NoData | SettlementStatus::PriceMissing => vec![],

        SettlementStatus::Calculated
        | SettlementStatus::FoerderungBeendet
        | SettlementStatus::JahreskontingentErschoepft
        | SettlementStatus::Sanctioned
        | SettlementStatus::KeinAnspruch => {
            output.positions.iter().map(|p| p.to_line_item()).collect()
        }
    }
}
