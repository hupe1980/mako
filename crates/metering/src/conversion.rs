//! Gas unit conversion: m³ → kWh_Hs.
//!
//! ## Legal basis
//!
//! A gas meter registers m³. The kWh billed is derived, never measured, so the
//! conversion rests on the Eichrecht exceptions to §33 MessEG:
//!
//! - **§33 Abs. 1 MessEG**: a value for a Messgröße may only be used if it was
//!   determined with a Messgerät.
//! - **§25 Nr. 4 MessEV**: permits Brennwert values *"wenn sie nach den
//!   anerkannten Regeln der Technik ermittelt worden sind"*.
//! - **§25 Nr. 7 MessEV**: permits a value formed as a *"Produkt"* of measured
//!   values, which is what V × Z × Hs is.
//! - **DVGW G 685**: the anerkannte Regel der Technik referenced by §25 Nr. 4.
//! - **DVGW G 260**: Gasbeschaffenheit, Hs-Bereich für Erdgas H/L.
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
    /// Source: Messstellenbetreiber / NB monthly data per supply area.
    pub hs_kwh_per_m3: Decimal,
    /// Volume conversion factor (Zustandszahl, dimensionless).
    ///
    /// Accounts for pressure and temperature at the meter.
    /// Neutral default when not separately metered: 1.0.
    pub zustandszahl: Decimal,
}

impl GasConversionParams {
    /// Default conversion parameters when no measurement data is available.
    ///
    /// Uses `Hs = 10.55 kWh/m³` (typical German Erdgas H average) and
    /// `Zustandszahl = 1.0` (neutral).
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
    use rust_decimal::dec;

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

// ── Warm water → heat energy (HeizkostenV §9 Abs. 2) ─────────────────────────

/// Adjustments applied to a §9 Abs. 2 result.
///
/// §9 Abs. 2 Satz 6 applies these to the result of *either* Zahlenwertgleichung
/// and does not make them exclusive, so more than one may hold at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WarmWaterAdjustments {
    /// *"bei brennwertbezogener Abrechnung von Erdgas mit 1,11 zu multiplizieren"*.
    pub brennwert_erdgas: bool,
    /// *"bei eigenständiger gewerblicher Wärmelieferung durch 1,15 zu dividieren"*.
    ///
    /// **Eigenständig** is a term of art (cf. §1 Abs. 1 Nr. 2); ordinary
    /// commercial heat supply does not qualify.
    pub eigenstaendige_gewerbliche_waermelieferung: bool,
    /// *"bei dem Betrieb einer monovalenten Wärmepumpe mit 0,30 zu multiplizieren"*.
    pub monovalente_waermepumpe: bool,
}

impl WarmWaterAdjustments {
    /// No adjustment.
    pub const NONE: Self = Self {
        brennwert_erdgas: false,
        eigenstaendige_gewerbliche_waermelieferung: false,
        monovalente_waermepumpe: false,
    };

    fn apply(self, base: Decimal) -> Decimal {
        let mut q = base;
        if self.brennwert_erdgas {
            q *= Decimal::from_str_exact("1.11").unwrap_or(Decimal::ONE);
        }
        if self.eigenstaendige_gewerbliche_waermelieferung {
            q /= Decimal::from_str_exact("1.15").unwrap_or(Decimal::ONE);
        }
        if self.monovalente_waermepumpe {
            q *= Decimal::from_str_exact("0.30").unwrap_or(Decimal::ONE);
        }
        q
    }
}

/// Heat attributable to a central warm-water system from the **metered volume**,
/// per HeizkostenV §9 Abs. 2 Satz 2.
///
/// ```text
/// Q [kWh/a] = 2.5 × V [m³] × (t_w [°C] − 10)
/// ```
///
/// §9 Abs. 2 Satz 1 requires the heat quantity to be **measured with a
/// Wärmezähler**. This equation is the fallback admitted only where measurement
/// *"nur mit einem unzumutbar hohen Aufwand"* is possible.
///
/// It is a *Zahlenwertgleichung* — a numerical-value equation, not dimensionally
/// consistent — so 2.5 carries no unit. §9 Abs. 2 Satz 3 Nr. 1 defines it as
/// covering the Erzeugeraufwandszahl des Wärmeerzeugers, the mittlere spezifische
/// Wärmekapazität des Wassers, the Wärmeverluste für Warmwasserspeicher,
/// Verteilung einschließlich Zirkulation, and Messdatenerhebungen zum
/// Warmwasserverbrauch. Because the Erzeugeraufwandszahl is inside the constant,
/// **Q is generator-input heat, not delivered useful heat**.
///
/// `mean_temp_c` is *"die gemessene oder geschätzte mittlere Temperatur"* — the
/// regulation permits an estimate and prescribes neither a default nor a cap.
///
/// # Example
///
/// ```rust
/// use metering::{warm_water_heat_kwh, WarmWaterAdjustments};
/// use rust_decimal::Decimal;
///
/// // 40 m³ of warm water at 60 °C
/// let q = warm_water_heat_kwh(
///     Decimal::from(40u32),
///     Decimal::from(60u32),
///     WarmWaterAdjustments::NONE,
/// );
/// assert_eq!(q, Decimal::from(5000u32)); // 2.5 × 40 × 50
/// ```
#[must_use]
pub fn warm_water_heat_kwh(
    volume_m3: Decimal,
    mean_temp_c: Decimal,
    adjustments: WarmWaterAdjustments,
) -> Decimal {
    let factor = Decimal::from_str_exact("2.5").unwrap_or(Decimal::from(2u32));
    let cold_inlet = Decimal::from(10u32);
    adjustments.apply(factor * volume_m3 * (mean_temp_c - cold_inlet))
}

/// Heat attributable to a central warm-water system from **floor area**, per
/// HeizkostenV §9 Abs. 2 Satz 4: `Q [kWh/a] = 32 × A_Wohn [m²]`.
///
/// Admitted only *"in Ausnahmefällen"* where **neither** the heat quantity **nor**
/// the warm-water volume can be measured — a narrower trigger than an unmetered
/// volume alone.
///
/// `flaeche_m2` is the *"Wohn- oder Nutzfläche"* supplied with warm water by the
/// central system. §9 Abs. 2 Satz 5 Nr. 1 defines 32 as covering the
/// Nutzwärmebedarf für Warmwasser, the Erzeugeraufwandszahl and
/// Messdatenerhebungen — note this is a **different** bundle from the 2.5 of
/// Satz 2, excluding Speicher-, Verteilungs- und Zirkulationsverluste.
///
/// Separate from [`warm_water_heat_kwh`] rather than an `Option` parameter: a
/// metered volume and a floor-area estimate are different evidentiary categories,
/// so the caller states which it holds.
#[must_use]
pub fn warm_water_heat_kwh_unmetered(
    flaeche_m2: Decimal,
    adjustments: WarmWaterAdjustments,
) -> Decimal {
    adjustments.apply(Decimal::from(32u32) * flaeche_m2)
}

#[cfg(test)]
mod warm_water_tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).unwrap()
    }

    /// The worked identity from HeizkostenV §9 Abs. 2 Satz 2.
    #[test]
    fn metered_warm_water_follows_the_statutory_formula() {
        // 2.5 × 40 m³ × (60 − 10) = 5000 kWh
        assert_eq!(
            warm_water_heat_kwh(
                Decimal::from(40u32),
                Decimal::from(60u32),
                WarmWaterAdjustments::NONE
            ),
            Decimal::from(5000u32)
        );
    }

    /// At the assumed cold-inlet temperature there is no apportionable heat.
    /// Below it the result stays negative, signalling a bad temperature input.
    #[test]
    fn at_and_below_cold_inlet_temperature() {
        assert_eq!(
            warm_water_heat_kwh(
                Decimal::from(40u32),
                Decimal::from(10u32),
                WarmWaterAdjustments::NONE
            ),
            Decimal::ZERO
        );
        assert!(
            warm_water_heat_kwh(
                Decimal::from(40u32),
                Decimal::from(5u32),
                WarmWaterAdjustments::NONE
            ) < Decimal::ZERO
        );
    }

    #[test]
    fn adjustments_match_the_statutory_factors() {
        let v = Decimal::from(40u32);
        let t = Decimal::from(60u32);
        let base = Decimal::from(5000u32);

        let brennwert = WarmWaterAdjustments {
            brennwert_erdgas: true,
            ..WarmWaterAdjustments::NONE
        };
        assert_eq!(warm_water_heat_kwh(v, t, brennwert), base * d("1.11"));

        let wp = WarmWaterAdjustments {
            monovalente_waermepumpe: true,
            ..WarmWaterAdjustments::NONE
        };
        assert_eq!(warm_water_heat_kwh(v, t, wp), base * d("0.30"));

        // Eigenständige gewerbliche Wärmelieferung divides.
        let gewerblich = WarmWaterAdjustments {
            eigenstaendige_gewerbliche_waermelieferung: true,
            ..WarmWaterAdjustments::NONE
        };
        assert_eq!(warm_water_heat_kwh(v, t, gewerblich), base / d("1.15"));
    }

    /// §9 Abs. 2 Satz 6 does not make the three grounds exclusive, so a
    /// heat-pump system supplied under eigenständige gewerbliche Wärmelieferung
    /// takes both adjustments.
    #[test]
    fn adjustments_compose() {
        let both = WarmWaterAdjustments {
            eigenstaendige_gewerbliche_waermelieferung: true,
            monovalente_waermepumpe: true,
            ..WarmWaterAdjustments::NONE
        };
        let q = warm_water_heat_kwh(Decimal::from(40u32), Decimal::from(60u32), both);
        assert_eq!(q, Decimal::from(5000u32) / d("1.15") * d("0.30"));
    }

    /// The adjustments apply to the floor-area equation too ("Satz 2 oder 4").
    #[test]
    fn unmetered_fallback_uses_floor_area_and_takes_adjustments() {
        // 32 × 75 m² = 2400 kWh
        assert_eq!(
            warm_water_heat_kwh_unmetered(Decimal::from(75u32), WarmWaterAdjustments::NONE),
            Decimal::from(2400u32)
        );
        let brennwert = WarmWaterAdjustments {
            brennwert_erdgas: true,
            ..WarmWaterAdjustments::NONE
        };
        assert_eq!(
            warm_water_heat_kwh_unmetered(Decimal::from(75u32), brennwert),
            Decimal::from(2400u32) * d("1.11")
        );
    }
}
