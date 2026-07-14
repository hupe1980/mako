//! Bridge to [`billing`] document types.
//!
//! Since [`SettleOutput`] now carries `positions: Vec<SettlePosition>`, this
//! module is a thin adapter — it calls `SettlePosition::to_line_item` on each
//! position and handles the special cases (`NoData`, `PriceMissing`, `Sanctioned`).
//!
//! # Why not `billing::Tariff`?
//!
//! The `Tariff` trait generates documents from usage data.  EEG settlement is a
//! **scalar calculation** with status variants (`NoData`, `PriceMissing`, `Sanctioned`,
//! `FoerderungBeendet`) that cannot be represented as `Err(BillingError)`, and with
//! stateful KWKG hour-limit tracking.  The positions are already computed by
//! [`crate::calculate_settlement`] — this module only converts them to `billing::LineItem`.
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

use billing::{Amount, LineItem};
use rust_decimal::Decimal;

use crate::model::{SettleOutput, SettlementStatus};

/// Convert a settlement result into [`billing::LineItem`] positions.
///
/// Returns an **empty** `Vec` for `NoData` and `PriceMissing` — nothing to bill yet.
///
/// For `Sanctioned`, returns a single EUR 0 line tagged `"§25-sanctioned"` for audit
/// trail.
///
/// For all other statuses, delegates to `SettlePosition::to_line_item` on each
/// position already computed by [`crate::calculate_settlement`].
pub fn settlement_to_line_items(output: &SettleOutput) -> Vec<LineItem> {
    match output.status {
        // Nothing to bill — no document positions issued
        SettlementStatus::NoData | SettlementStatus::PriceMissing => vec![],

        // §25 EEG: payment suspended. Emit a EUR 0 audit line.
        SettlementStatus::Sanctioned => {
            let kwh = output.eligible_kwh.unwrap_or(Decimal::ZERO);
            vec![
                LineItem::credit_fixed(
                    "Einspeisevergütung gesperrt – ausstehende MaStR-Registrierung §25 EEG 2023",
                    Amount::<5>::ZERO,
                )
                .meta("legal_basis", "§25 EEG 2023")
                .meta("kwh", kwh.to_string())
                .tag("§25-sanctioned")
                .tag("eeg")
                .build()
                .expect("static description always non-empty"),
            ]
        }

        // Positions already computed in SettleOutput — delegate directly
        SettlementStatus::Calculated | SettlementStatus::FoerderungBeendet => {
            output.positions.iter().map(|p| p.to_line_item()).collect()
        }
    }
}
