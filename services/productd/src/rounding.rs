//! Kaufmännisches Runden — the one rounding mode this service uses.

use rust_decimal::{Decimal, RoundingStrategy};

/// Kaufmännisches Runden (DIN 1333): round half **away from zero**.
///
/// `Decimal::round_dp` rounds half to even. The two modes agree everywhere
/// except exact midpoints — the values a price quoted in ct with three decimals
/// produces — so the wrong one misstates a cent on a quoted Jahreskosten
/// without failing any test written against ordinary numbers.
///
/// `cargo xtask check-rounding` refuses a bare `round_dp` workspace-wide.
pub trait RoundMoney {
    /// Round to `dp` decimal places, half away from zero (DIN 1333).
    #[must_use]
    fn round_kfm(&self, dp: u32) -> Decimal;
}

impl RoundMoney for Decimal {
    fn round_kfm(&self, dp: u32) -> Decimal {
        self.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
    }
}
