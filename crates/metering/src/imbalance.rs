//! Mehr-/Mindermengensaldo (imbalance) calculation.
//!
//! ## Legal basis
//!
//! - **GPKE (BK6-24-174) Teil 1, Kap. 8.4** — Jahresmehr- und Jahresmindermengen
//!   (Strom). Historically §13 Abs. 3 StromNZV, repealed with effect from the end
//!   of 31.12.2025.
//! - **GaBi Gas 2.1 (BK7-24-01-008), Ziff. 3a** — Mehr-/Mindermengen Gas.
//!   Historically §25 GasNZV, repealed on the same date.
//!
//! ## Definition
//!
//! Both quantities are named from the **network operator's** side, which inverts
//! the intuitive reading. GPKE Kap. 8.4 Nr. 3:
//!
//! > Unterschreitet die Summe der in einem Zeitraum ermittelten elektrischen
//! > Arbeit die Summe der Arbeit, die den bilanzierten Profilen zu Grunde gelegt
//! > wurde (ungewollte Mehrmenge), so vergütet der Netzbetreiber dem Lieferanten
//! > oder dem Kunden diese Differenzmenge.
//!
//! ```text
//! Mehr-Menge   = max(0, profiled_kwh − actual_kwh)   [NB vergütet → NB owes LF]
//! Minder-Menge = max(0, actual_kwh − profiled_kwh)   [NB stellt in Rechnung → LF owes NB]
//! ```
//!
//! The customer consuming *less* than the profile leaves surplus energy the
//! network operator absorbed — that surplus is the Mehrmenge, and it is credited.
//!
//! Only one of `mehr_kwh` or `minder_kwh` is positive in any period.
//!
//! `contracted_kwh` is a parameter because the contracted quantity is a
//! commercial figure held in the supplier's billing system, not a measured one.
//! The caller supplies it alongside the measured total; this module owns the
//! arithmetic and the sign convention.

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
    /// Mehr-Menge: `max(0, contracted − actual)`. NB vergütet, so NB owes LF.
    pub mehr_kwh: Decimal,
    /// Minder-Menge: `max(0, actual − contracted)`. NB invoices, so LF owes NB.
    pub minder_kwh: Decimal,
    /// Signed delta: `actual − contracted`. Positive is a **Minder**menge.
    pub delta_kwh: Decimal,
}

impl ImbalanceSaldo {
    /// `true` when there is a Mehrmengen position (NB owes LF, a credit).
    #[must_use]
    pub fn is_mehr(&self) -> bool {
        self.mehr_kwh > Decimal::ZERO
    }

    /// `true` when there is a Mindermengen position (LF owes NB, a charge).
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
/// // 1050 kWh measured against a 1000 kWh profile → Mindermenge 50 kWh,
/// // which the network operator invoices.
/// let saldo = compute_imbalance(
///     Decimal::from(1050u32),
///     Decimal::from(1000u32),
/// );
/// assert_eq!(saldo.minder_kwh, Decimal::from(50u32));
/// assert!(saldo.is_minder());
/// assert!(!saldo.is_mehr());
/// ```
#[must_use]
pub fn compute_imbalance(actual_kwh: Decimal, contracted_kwh: Decimal) -> ImbalanceSaldo {
    let delta = actual_kwh - contracted_kwh;
    // Under-consumption is the Mehrmenge; over-consumption the Mindermenge.
    let mehr = (-delta).max(Decimal::ZERO);
    let minder = delta.max(Decimal::ZERO);
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

    /// Consuming above the profile is an ungewollte **Minder**menge: the NB
    /// supplied the shortfall and invoices it.
    #[test]
    fn over_consumption_is_a_mindermenge() {
        let s = compute_imbalance(dec!(1050), dec!(1000));
        assert_eq!(s.minder_kwh, dec!(50));
        assert_eq!(s.mehr_kwh, Decimal::ZERO);
        assert_eq!(s.delta_kwh, dec!(50));
        assert!(s.is_minder());
        assert!(!s.is_mehr());
    }

    /// Consuming below the profile is an ungewollte **Mehr**menge: the NB took
    /// the surplus and reimburses it.
    #[test]
    fn under_consumption_is_a_mehrmenge() {
        let s = compute_imbalance(dec!(950), dec!(1000));
        assert_eq!(s.mehr_kwh, dec!(50));
        assert_eq!(s.minder_kwh, Decimal::ZERO);
        assert_eq!(s.delta_kwh, dec!(-50));
        assert!(s.is_mehr());
        assert!(!s.is_minder());
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
