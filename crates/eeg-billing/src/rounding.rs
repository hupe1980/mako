//! Kaufmännisches Runden — the one rounding mode this crate uses.

use rust_decimal::{Decimal, RoundingStrategy};

/// Kaufmännisches Runden (DIN 1333): round half **away from zero**.
///
/// `Decimal::round_dp` rounds half to even. The two modes agree everywhere
/// except exact midpoints — the values a price quoted in ct with three decimals
/// produces — so the wrong one misstates a cent without failing any test
/// written against ordinary numbers. Away-from-zero rather than literal half-up
/// keeps a Storno symmetric to what it reverses: round(-0.005) = -0.01 mirrors
/// round(0.005) = 0.01, and it is the strategy the `billing` arithmetic core
/// applies inside every `Amount` operation.
///
/// `cargo xtask check-rounding` refuses a bare `round_dp` workspace-wide, so no
/// call site can fall back to banker's rounding silently.
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
