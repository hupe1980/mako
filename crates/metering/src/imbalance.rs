//! Mehr-/Mindermengensaldo (imbalance) calculation.
//!
//! ## Legal basis
//!
//! - **§27 MessZV**: Abrechnung der Mehr-/Mindermengensaldo.
//! - **GPKE BK6-22-024 §7**: Mehr-/Mindermengensaldo-Abrechnung zwischen LF und NB.
//! - **GeLi Gas BK7-24-01-009 §6**: Gas Mehr-/Mindermengensaldo.
//!
//! ## Definition
//!
//! The imbalance (`Mehr-/Mindermengensaldo`) compares the **actual metered energy**
//! against the **contracted/profile energy** for a billing period:
//!
//! ```text
//! Mehr-Menge  = max(0, actual_kwh − contracted_kwh)   [LF owes NB]
//! Minder-Menge = max(0, contracted_kwh − actual_kwh)  [NB owes LF]
//! ```
//!
//! Only one of `mehr_kwh` or `minder_kwh` is positive in any period.
//!
//! ## Note on current `edmd` implementation
//!
//! The current `PgTimeSeriesRepository::imbalance()` returns `delta_kwh = ZERO`
//! because `contracted_kwh` is not stored in `edmd` — it lives in the LF's
//! INVOIC/billing system.  The correct pattern is for the ERP to supply
//! `contracted_kwh` when calling `compute_imbalance`, which `edmd` then computes.

use rust_decimal::Decimal;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Result of a Mehr-/Mindermengensaldo calculation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ImbalanceSaldo {
    /// Actual metered energy in kWh.
    pub actual_kwh: Decimal,
    /// Contracted / profile energy in kWh.
    pub contracted_kwh: Decimal,
    /// Mehr-Menge: max(0, actual − contracted). LF owes NB.
    pub mehr_kwh: Decimal,
    /// Minder-Menge: max(0, contracted − actual). NB owes LF.
    pub minder_kwh: Decimal,
    /// Signed delta: actual − contracted (positive = Mehr, negative = Minder).
    pub delta_kwh: Decimal,
}

impl ImbalanceSaldo {
    /// `true` when there is a Mehrmengen position (LF owes NB).
    #[must_use]
    pub fn is_mehr(&self) -> bool {
        self.mehr_kwh > Decimal::ZERO
    }

    /// `true` when there is a Mindermengen position (NB owes LF).
    #[must_use]
    pub fn is_minder(&self) -> bool {
        self.minder_kwh > Decimal::ZERO
    }

    /// `true` when actual == contracted (balanced period).
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.delta_kwh.is_zero()
    }

    /// Absolute imbalance magnitude in kWh.
    #[must_use]
    pub fn magnitude_kwh(&self) -> Decimal {
        self.delta_kwh.abs()
    }

    /// Imbalance as a percentage of contracted quantity.
    ///
    /// Returns `None` when `contracted_kwh` is zero.
    #[must_use]
    pub fn delta_pct(&self) -> Option<Decimal> {
        if self.contracted_kwh.is_zero() {
            None
        } else {
            Some(self.delta_kwh / self.contracted_kwh * Decimal::from(100u32))
        }
    }
}

/// Compute the Mehr-/Mindermengensaldo for a billing period.
///
/// # Example
/// ```rust
/// use metering::compute_imbalance;
/// use rust_decimal::Decimal;
///
/// // LF delivered 1050 kWh against 1000 kWh contracted → Mehr-Menge 50 kWh
/// let saldo = compute_imbalance(
///     Decimal::from(1050u32),
///     Decimal::from(1000u32),
/// );
/// assert_eq!(saldo.mehr_kwh, Decimal::from(50u32));
/// assert!(saldo.is_mehr());
/// assert!(!saldo.is_minder());
/// ```
#[must_use]
pub fn compute_imbalance(actual_kwh: Decimal, contracted_kwh: Decimal) -> ImbalanceSaldo {
    let delta = actual_kwh - contracted_kwh;
    let mehr = delta.max(Decimal::ZERO);
    let minder = (-delta).max(Decimal::ZERO);
    ImbalanceSaldo {
        actual_kwh,
        contracted_kwh,
        mehr_kwh: mehr,
        minder_kwh: minder,
        delta_kwh: delta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn mehr_menge_lf_owes_nb() {
        // LF delivered more than contracted
        let s = compute_imbalance(dec!(1050), dec!(1000));
        assert_eq!(s.mehr_kwh, dec!(50));
        assert_eq!(s.minder_kwh, Decimal::ZERO);
        assert_eq!(s.delta_kwh, dec!(50));
        assert!(s.is_mehr());
        assert!(!s.is_minder());
    }

    #[test]
    fn minder_menge_nb_owes_lf() {
        // LF delivered less than contracted
        let s = compute_imbalance(dec!(950), dec!(1000));
        assert_eq!(s.mehr_kwh, Decimal::ZERO);
        assert_eq!(s.minder_kwh, dec!(50));
        assert_eq!(s.delta_kwh, dec!(-50));
        assert!(!s.is_mehr());
        assert!(s.is_minder());
    }

    #[test]
    fn balanced_period() {
        let s = compute_imbalance(dec!(1000), dec!(1000));
        assert!(s.is_balanced());
        assert_eq!(s.magnitude_kwh(), Decimal::ZERO);
        assert_eq!(s.delta_pct(), Some(Decimal::ZERO));
    }

    #[test]
    fn delta_pct_calculation() {
        // 50 kWh excess on 1000 contracted = 5%
        let s = compute_imbalance(dec!(1050), dec!(1000));
        assert_eq!(s.delta_pct(), Some(dec!(5)));
    }

    #[test]
    fn delta_pct_zero_contracted() {
        let s = compute_imbalance(dec!(100), Decimal::ZERO);
        assert_eq!(s.delta_pct(), None);
    }

    #[test]
    fn mess_zv_mehr_minder_mutually_exclusive() {
        // §27 MessZV: Mehr and Minder are mutually exclusive
        for (actual, contracted) in [
            (dec!(900), dec!(1000)),
            (dec!(1100), dec!(1000)),
            (dec!(1000), dec!(1000)),
        ] {
            let s = compute_imbalance(actual, contracted);
            // Never both mehr and minder simultaneously
            assert!(
                !(s.is_mehr() && s.is_minder()),
                "mehr and minder cannot both be true"
            );
        }
    }
}
