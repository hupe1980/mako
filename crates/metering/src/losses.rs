//! Netzverlust — grid-loss indicator (§22 EnWG).
//!
//! §22 Abs. 1 EnWG obliges Netzbetreiber to procure the energy needed to
//! cover physical grid losses (Verlustenergie). The loss quantity itself is
//! the balance over a grid area:
//!
//! ```text
//! Verlust = Σ Einspeisung ins Netz − Σ Entnahme aus dem Netz
//! ```
//!
//! This module provides the pure balance calculation. It is an **indicator**,
//! not a settlement quantity: its accuracy is bounded by the metering
//! coverage of the summed series (unmetered infeed or offtake shows up as
//! phantom loss or gain). Settlement-grade Verlustenergie procurement uses
//! the DSO's Bilanzkreis data, not this figure.

use rust_decimal::Decimal;

/// Result of a grid-loss balance over one period.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NetworkLosses {
    /// Total energy fed into the grid area (kWh) — generation feed-in plus
    /// imports over Übergabezählpunkte.
    pub einspeisung_kwh: Decimal,
    /// Total energy taken out of the grid area (kWh) — customer offtake plus
    /// exports.
    pub entnahme_kwh: Decimal,
    /// `einspeisung − entnahme`. Positive = physical losses (plus any
    /// unmetered offtake); negative = metering-coverage gap on the infeed
    /// side.
    pub verlust_kwh: Decimal,
    /// Loss share of the infeed in percent, `None` when nothing was fed in.
    pub verlust_prozent: Option<Decimal>,
}

/// Compute the grid-loss balance from period totals.
///
/// Pure arithmetic — callers aggregate the two totals from their metering
/// data (e.g. OBIS `2.8.x`/`2.29.x` series for infeed, `1.8.x`/`1.29.x`
/// for offtake).
#[must_use]
pub fn network_losses(einspeisung_kwh: Decimal, entnahme_kwh: Decimal) -> NetworkLosses {
    let verlust_kwh = einspeisung_kwh - entnahme_kwh;
    let verlust_prozent = (einspeisung_kwh > Decimal::ZERO).then(|| {
        (verlust_kwh / einspeisung_kwh * Decimal::ONE_HUNDRED)
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
    });
    NetworkLosses {
        einspeisung_kwh,
        entnahme_kwh,
        verlust_kwh,
        verlust_prozent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn a_typical_distribution_grid_shows_single_digit_losses() {
        // 1 GWh fed in, 955 MWh delivered → 4.5 % losses.
        let l = network_losses(dec!(1_000_000), dec!(955_000));
        assert_eq!(l.verlust_kwh, dec!(45_000));
        assert_eq!(l.verlust_prozent, Some(dec!(4.50)));
    }

    #[test]
    fn negative_balance_signals_a_metering_coverage_gap() {
        let l = network_losses(dec!(100), dec!(120));
        assert_eq!(l.verlust_kwh, dec!(-20));
        assert_eq!(l.verlust_prozent, Some(dec!(-20.00)));
    }

    #[test]
    fn zero_infeed_yields_no_percentage() {
        let l = network_losses(Decimal::ZERO, dec!(10));
        assert_eq!(l.verlust_prozent, None);
    }
}
