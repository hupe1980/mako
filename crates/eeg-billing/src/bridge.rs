//! Bridge to [`billing`] document types.
//!
//! [`SettleOutput`] carries `positions: Vec<SettlePosition>`, so this module is
//! a thin adapter — it calls `SettlePosition::to_line_item` on each position and
//! handles the special cases (`NoData`, `PriceMissing`, `Sanctioned`).
//!
//! # A `PricingModel` with no usage
//!
//! EEG settlement is a **scalar calculation** — the positions are already computed
//! by [`crate::calculate_settlement`], with no usage input. `billing` 0.12 folded
//! the old `ScalarTariff` into [`billing::PricingModel`] rather than keeping two
//! traits, so that shape is now expressed as `type Usage = ()`;
//! [`crate::tariff::EegSettleTariff`] implements it. The domain not-billable
//! reasons (`NoData` / `PriceMissing` / `Sanctioned` / `FoerderungBeendet`) live on
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

use billing::{LineItem, Quantity, UnitPrice};
use rust_decimal::Decimal;

use crate::model::{SettleOutput, SettlementStatus};

/// Convert a settlement result into [`billing::LineItem`] positions.
///
/// Returns an **empty** `Vec` for `NoData` and `PriceMissing` — nothing to bill yet.
///
/// For `Sanctioned`, returns a single EUR 0 line tagged `"§25-sanctioned"` for audit
/// trail. `KeinAnspruch` carries its own EUR 0 position and passes straight through.
///
/// For all other statuses, delegates to `SettlePosition::to_line_item` on each
/// position already computed by [`crate::calculate_settlement`].
pub fn settlement_to_line_items(output: &SettleOutput) -> Vec<LineItem> {
    match output.status {
        // Nothing to bill — no document positions issued
        SettlementStatus::NoData | SettlementStatus::PriceMissing => vec![],

        // §25 EEG: payment suspended. Emit a EUR 0 audit line stating the
        // eligible quantity at a zero rate. `credit_for_usage` (quantity × price)
        // gives the line BT-129/BT-130/BT-146 (BR-22/BR-23/BR-26), so the €0 stub
        // is a valid EN 16931 invoice line — `credit_fixed` states an amount only
        // and would be rejected if the Gutschrift is rendered to XRechnung.
        SettlementStatus::Sanctioned => {
            let kwh = output.eligible_kwh.unwrap_or(Decimal::ZERO);
            vec![
                LineItem::credit_for_usage(
                    "Einspeisevergütung gesperrt – ausstehende MaStR-Registrierung §25 EEG 2023",
                    Quantity::new(kwh, "kWh").with_code("KWH"),
                    UnitPrice::new(Decimal::ZERO, "EUR/kWh"),
                )
                .meta("legal_basis", "§25 EEG 2023")
                .tag("§25-sanctioned")
                .tag("eeg")
                .build()
                .expect("static description always non-empty"),
            ]
        }

        // Positions already computed in SettleOutput — delegate directly.
        // `KeinAnspruch` carries its own EUR 0 position naming §21 Abs. 1 Satz 1
        // Nr. 1, so the Gutschrift states why nothing is owed rather than being
        // silently empty.
        SettlementStatus::Calculated
        | SettlementStatus::FoerderungBeendet
        | SettlementStatus::KeinAnspruch => {
            output.positions.iter().map(|p| p.to_line_item()).collect()
        }
    }
}
