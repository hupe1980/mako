//! Gas unit conversion: m³ → kWh_Hs.
//!
//! ## Legal basis
//!
//! - **§24 GasGVV** (Gasgrundversorgungsverordnung): Abrechnungsmenge in kWh.
//! - **DVGW G 685** §10: Umrechnung Volumen → Energie.
//! - **DVGW G 260** (Gasbeschaffenheit): Hs-Bereich für Erdgas H/L.
//!
//! ## Formula
//!
//! ```text
//! kWh_Hs = V_m3 × Hs_kWh_per_m3 × Zustandszahl
//! ```
//!
//! where:
//! - `V_m3` — metered volume in m³ (at meter measurement conditions)
//! - `Hs_kWh_per_m3` — superior calorific value (Brennwert Ho / Hs) in kWh/m³
//!   as determined by the gas distributor for the supply area
//! - `Zustandszahl` — volume conversion factor (dimensionless, typically 0.95–1.05)
//!   accounting for pressure and temperature at the meter
//!
//! ## Typical values for German natural gas (Erdgas H)
//!
//! | Parameter | Typical range | Unit |
//! |---|---|---|
//! | `hs_kwh_per_m3` | 9.5 – 12.0 | kWh/m³ |
//! | `zustandszahl` | 0.92 – 1.06 | dimensionless |
//!
//! ## Accuracy note
//!
//! All arithmetic uses [`rust_decimal::Decimal`] for exact decimal precision.
//! Never use `f64` for energy quantities — a 0.001% billing error on a 10 GWh/year
//! industrial customer is 100 kWh/year or ~EUR 10.

use rust_decimal::Decimal;

use crate::interval::MeterInterval;

/// Parameters for Gas m³ → kWh_Hs conversion.
#[derive(Debug, Clone)]
pub struct GasConversionParams {
    /// Superior calorific value (Brennwert Ho / Hs) in kWh/m³.
    ///
    /// Published monthly by the gas distributor per supply area.
    /// Source: Messstellenbetreiber / NB monthly data per §24 GasGVV.
    pub hs_kwh_per_m3: Decimal,
    /// Volume conversion factor (Zustandszahl, dimensionless).
    ///
    /// Accounts for pressure and temperature at the meter.
    /// Default per §24 GasGVV when not metered: 1.0.
    pub zustandszahl: Decimal,
}

impl GasConversionParams {
    /// Default conversion parameters when no measurement data is available.
    ///
    /// Uses `Hs = 10.55 kWh/m³` (typical German Erdgas H average) and
    /// `Zustandszahl = 1.0` (neutral) per §24 GasGVV default.
    #[must_use]
    pub fn default_erdgas_h() -> Self {
        Self {
            hs_kwh_per_m3: Decimal::from_str_exact("10.55").unwrap_or(Decimal::from(10u32)),
            zustandszahl: Decimal::ONE,
        }
    }
}

/// Convert a Gas volume reading in m³ to energy in kWh_Hs.
///
/// Formula: `kWh_Hs = m3 × hs_kwh_per_m3 × zustandszahl`
///
/// # Example
/// ```rust
/// use metering::gas_m3_to_kwh_hs;
/// use rust_decimal::Decimal;
///
/// // 100 m³ × 10.55 kWh/m³ × 0.9764 = 1029.90 kWh_Hs (rounded)
/// let kwh = gas_m3_to_kwh_hs(
///     Decimal::from(100u32),
///     Decimal::from_str_exact("10.55").unwrap(),
///     Decimal::from_str_exact("0.9764").unwrap(),
/// );
/// assert!(kwh > Decimal::from(1000u32));
/// ```
#[must_use]
pub fn gas_m3_to_kwh_hs(
    volume_m3: Decimal,
    hs_kwh_per_m3: Decimal,
    zustandszahl: Decimal,
) -> Decimal {
    volume_m3 * hs_kwh_per_m3 * zustandszahl
}

/// Normalize a raw meter interval to kWh.
///
/// For Strom intervals already in kWh: returns `interval.value_kwh` unchanged.
/// For Gas intervals in m³: applies the Hs conversion.
///
/// The `unit` field on the interval indicates the raw unit:
/// - `"kWh"` → no conversion
/// - `"m3"` or `"m³"` → multiply by `hs × z`
/// - `"kW"` → instantaneous demand; multiply by duration_h to get kWh
#[must_use]
pub fn normalize_interval_to_kwh(
    interval: &MeterInterval,
    unit: &str,
    gas_params: Option<&GasConversionParams>,
) -> Decimal {
    match unit.to_lowercase().as_str() {
        "m3" | "m³" => {
            let p = gas_params
                .map(|p| (p.hs_kwh_per_m3, p.zustandszahl))
                .unwrap_or_else(|| {
                    let d = GasConversionParams::default_erdgas_h();
                    (d.hs_kwh_per_m3, d.zustandszahl)
                });
            gas_m3_to_kwh_hs(interval.value_kwh, p.0, p.1)
        }
        "kw" => {
            // kW → kWh: multiply by duration in hours
            let dur_h = Decimal::from(interval.duration_secs()) / Decimal::from(3600u32);
            interval.value_kwh * dur_h
        }
        _ => interval.value_kwh, // assume already kWh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn gas_conversion_exact() {
        // 100 m³ × 10.55 kWh/m³ × 1.0 = 1055.00 kWh_Hs
        let kwh = gas_m3_to_kwh_hs(dec!(100), dec!(10.55), dec!(1.0));
        assert_eq!(kwh, dec!(1055.00));
    }

    #[test]
    fn gas_conversion_with_zustandszahl() {
        // 50 m³ × 10.80 kWh/m³ × 0.9800 = 529.20 kWh_Hs
        let kwh = gas_m3_to_kwh_hs(dec!(50), dec!(10.80), dec!(0.9800));
        assert_eq!(kwh, dec!(529.2000));
    }

    #[test]
    fn gas_conversion_zero_volume() {
        assert_eq!(gas_m3_to_kwh_hs(dec!(0), dec!(10.55), dec!(1.0)), dec!(0));
    }

    #[test]
    fn default_erdgas_h_params() {
        let p = GasConversionParams::default_erdgas_h();
        assert_eq!(p.hs_kwh_per_m3, dec!(10.55));
        assert_eq!(p.zustandszahl, Decimal::ONE);
    }
}
