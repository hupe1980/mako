//! Monetary amount type for INVOIC plausibility checks.
//!
//! `EuroAmount` is [`billing::Amount<5>`] — a fixed-point integer that stores
//! amounts in units of 10⁻⁵ EUR (1/100 000 EUR), giving five decimal places of
//! precision. Parsing and addition are `billing`'s; this module adds the
//! checker's working-resolution conversion, its one exact product and the
//! relative-tolerance comparison every plausibility stage runs.

pub use billing::Amount;
pub use billing::Rounding;
use billing::{AmountScale, Decimal};

/// A monetary amount in euro at the checker's 5-decimal working resolution.
///
/// `billing` is currency-agnostic and names no currency in its own types. The
/// INVOIC domain *is* euro-denominated (BDEW MaKo settles in EUR), so the alias
/// is correct here — it belongs to the domain crate, not to the engine.
pub type EuroAmount = Amount<5>;

/// The checker's working resolution: five decimals, half away from zero
/// (DIN 1333).
const WORKING: AmountScale = AmountScale {
    decimals: 5,
    rounding: Rounding::HalfAwayFromZero,
};

/// Convert a `Decimal` into a 5-dp [`EuroAmount`], rounding to the checker's
/// working resolution half away from zero; `None` only when the value
/// overflows the representable range.
///
/// `billing`'s own `Amount::from_decimal` is *exact* — it errors on excess
/// precision rather than rounding. A plausibility checker works at 5 dp, so
/// rounding an over-precise INVOIC amount to that resolution is the correct
/// model; this helper makes that explicit via `from_decimal_rounded`. Kept as a
/// free fn so it still passes to `.and_then` / `.filter_map`.
#[must_use]
pub fn euro_from_decimal(d: Decimal) -> Option<EuroAmount> {
    EuroAmount::from_decimal_rounded(d, Rounding::HalfAwayFromZero).ok()
}

/// `amount × factor`, rounded **once** to the working resolution.
///
/// The product is taken exactly in `Decimal` and reduced in one step, so a
/// percentage (`basis × satz / 100`) is one expression here, never a product
/// rounded and then divided and rounded again. `None` when the product leaves
/// `Decimal` or the representable range — both operands arrive off the wire
/// unchecked, and an unrepresentable product is a finding about the document,
/// not a fault in the checker.
#[must_use]
pub fn reduce_product(amount: EuroAmount, factor: Decimal) -> Option<EuroAmount> {
    let exact = amount.to_decimal().checked_mul(factor)?;
    WORKING.reduce(exact).ok()
}

/// Relative-tolerance comparison in parts per million.
pub trait WithinTolerance {
    /// `true` when `|self − expected| ≤ |expected| × ppm / 1 000 000`; against
    /// an `expected` of zero only an exact zero is within tolerance.
    ///
    /// Taken in integer arithmetic over the raw 10⁻⁵ units, widened to `i128`:
    /// two valid amounts at opposite extremes have a difference outside `i64`,
    /// and a comparison that could fail would surface at every call site as
    /// „outside tolerance" — an arithmetic fault reported as a finding.
    fn within_tolerance_ppm(self, expected: Self, ppm: u32) -> bool;
}

impl WithinTolerance for EuroAmount {
    fn within_tolerance_ppm(self, expected: Self, ppm: u32) -> bool {
        if expected.is_zero() {
            return self.is_zero();
        }
        let diff = i128::from(self.to_raw()) - i128::from(expected.to_raw());
        let lhs = diff.unsigned_abs() * 1_000_000_u128;
        let rhs = i128::from(expected.to_raw()).unsigned_abs() * u128::from(ppm);
        lhs <= rhs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn tolerance_is_relative_to_the_expected_amount() {
        let stated = EuroAmount::parse("100.00000").unwrap();
        let computed = EuroAmount::parse("100.50000").unwrap();
        assert!(stated.within_tolerance_ppm(computed, 10_000));
        assert!(!stated.within_tolerance_ppm(computed, 4_000));
        assert!(stated.within_tolerance_ppm(stated, 0));
        assert!(
            !EuroAmount::from_raw(i64::MAX)
                .within_tolerance_ppm(EuroAmount::from_raw(i64::MIN), 1_000)
        );
        assert!(!stated.within_tolerance_ppm(EuroAmount::ZERO, 1_000_000));
    }

    /// A percentage of an amount is rounded once. 0.00003 EUR at 16.5 % is
    /// exactly 0.00000495 EUR, which is 0.00000 at five decimals; rounding the
    /// product with the rate first (0.000495 → 0.00050) and the division by 100
    /// second (0.000005 → 0.00001) takes two reductions and lands one unit off.
    #[test]
    fn a_percentage_is_reduced_once() {
        let basis = EuroAmount::parse("0.00003").unwrap();
        assert_eq!(
            reduce_product(basis, dec!(16.5) / dec!(100)).unwrap(),
            EuroAmount::ZERO
        );
        let basis = EuroAmount::parse("10.02629").unwrap();
        assert_eq!(
            reduce_product(basis, dec!(19) / dec!(100)).unwrap(),
            EuroAmount::parse("1.90500").unwrap()
        );
        assert!(reduce_product(basis, rust_decimal::Decimal::MAX).is_none());
    }

    /// `billing` 0.16 reads `[+-]digits[.digits]` only: a decimal comma — the
    /// German spelling of an INVOIC amount a caller may hand through — is
    /// refused as malformed, never read as a decimal point. „1,000" read the
    /// other way is one euro where a thousand were stated.
    #[test]
    fn a_decimal_comma_is_refused() {
        let err = EuroAmount::parse("1,000").unwrap_err();
        assert!(matches!(err.kind(), billing::ErrorKind::Malformed { .. }));
        assert!(serde_json::from_str::<EuroAmount>("\"12,345\"").is_err());
    }
}
