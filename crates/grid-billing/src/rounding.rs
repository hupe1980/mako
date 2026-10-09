//! Kaufmännisches Runden — the one rounding helper of the billing stack.
//!
//! `grid-billing` is the lowest crate of that stack, so `eeg-billing`,
//! `energy-billing` and `productd` take the helper from here and re-export it
//! rather than define their own. `cargo xtask check-rounding` refuses a second
//! definition anywhere in the workspace.

use rust_decimal::Decimal;

/// Kaufmännisches Runden (DIN 1333): round half **away from zero**.
///
/// `Decimal::round_dp` rounds half to even. The two modes agree everywhere
/// except exact midpoints — the values a price quoted in ct with three decimals
/// produces — so the wrong one misstates a cent without failing any test
/// written against ordinary numbers. Away-from-zero rather than literal half-up
/// keeps a Storno symmetric to what it reverses: round(-0.005) = -0.01 mirrors
/// round(0.005) = 0.01.
///
/// The mode is [`billing::RoundingStrategy::MidpointAwayFromZero`] — the
/// strategy the `billing` arithmetic core applies inside every `Amount`
/// conversion, multiplication and division. Use `billing::Amount` where the
/// precision is statutory; this helper is for a runtime `dp` on a raw
/// `Decimal`.
///
/// ```rust
/// use grid_billing::rounding::round_money;
/// use rust_decimal::dec;
/// assert_eq!(round_money(dec!(0.005), 2), dec!(0.01));
/// assert_eq!(round_money(dec!(-0.005), 2), dec!(-0.01));
/// assert_eq!(round_money(dec!(0.015), 2), dec!(0.02)); // half-even would give 0.02 too
/// assert_eq!(round_money(dec!(0.025), 2), dec!(0.03)); // half-even would give 0.02
/// ```
#[must_use]
pub fn round_money(value: Decimal, dp: u32) -> Decimal {
    value.round_dp_with_strategy(dp, ::billing::RoundingStrategy::MidpointAwayFromZero.into())
}

/// Method-call form of [`round_money`] — `x.round_kfm(2)`.
pub trait RoundMoney {
    /// Round to `dp` decimal places, half away from zero (DIN 1333).
    #[must_use]
    fn round_kfm(&self, dp: u32) -> Decimal;
}

impl RoundMoney for Decimal {
    fn round_kfm(&self, dp: u32) -> Decimal {
        round_money(*self, dp)
    }
}
