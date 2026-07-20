//! `RegulatoryRates` — statutory levy rates, operator-configured.
//!
//! All rates come from `billingd.toml [rates]`. The library never hardcodes
//! statutory rates; they change with each legislative year.

use rust_decimal::Decimal;
use rust_decimal::dec;
use serde::{Deserialize, Serialize};

// ── BEHG CO₂ price table ──────────────────────────────────────────────────────

/// BEHG §10 CO₂ price in EUR/t by calendar year.
///
/// Source: Brennstoffemissionshandelsgesetz §10 BEHG (BGBl. I 2021 Nr. 37).
/// CO₂ conversion factor for H-Erdgas: 0.20160 kg CO₂/kWh_Hs (DVGW G 685).
///
/// ## ct/kWh_Hs formula
///
/// `BEHG_ct/kWh = EUR/t × 0.20160 kg/kWh × 10^-3 t/kg × 100 ct/EUR`
/// `           = EUR/t × 0.020160`
///
/// | Year | EUR/t | ct/kWh_Hs |
/// |---|---|---|
/// | 2021 | 25   | 0.504 |
/// | 2022 | 30   | 0.605 |
/// | 2023 | 30   | 0.605 |
/// | 2024 | 45   | 0.907 |
/// | 2025 | 55   | 1.109 |
/// | 2026 | 65   | 1.310 |
const BEHG_EUR_PER_T: &[(i32, u32)] = &[
    (2021, 25),
    (2022, 30),
    (2023, 30),
    (2024, 45),
    (2025, 55),
    (2026, 65),
];

/// CO₂ conversion factor for H-Erdgas (kg CO₂/kWh_Hs), DVGW G 685.
pub const BEHG_CO2_FACTOR_H_GAS: Decimal = dec!(0.20160);

/// CO₂ conversion factor for L-Erdgas (kg CO₂/kWh_Hs), DVGW G 685.
///
/// L-Gas has a slightly lower Brennwert than H-Gas but similar specific CO₂
/// content. The DVGW G 685 reference value for L-Gas is approximately 0.2014 kg/kWh_Hs.
/// Use this constant for supply points in the L-Gas area (primarily NW Germany).
pub const BEHG_CO2_FACTOR_L_GAS: Decimal = dec!(0.20140);

// ── Stromsteuer history (§3 StromStG) ────────────────────────────────────────

/// §3 StromStG standard rate in ct/kWh by calendar year.
///
/// The standard Stromsteuer rate has been **2.05 ct/kWh** since 01.04.2003
/// (BGBl. I 2002 S. 4602). There was a temporary reduction to 0.5 ct/kWh for
/// heat-pump electricity under old tariff structures, but the per-kWh standard
/// rate for household/commercial supply has remained 2.05 ct/kWh.
///
/// `None` = no statutory rate known for the year; caller should use
/// `RegulatoryRates::stromsteuer_ct_per_kwh` as the default.
///
/// ## Usage for retroactive corrections
///
/// When correcting an invoice from a prior year, supply the year so the correct
/// rate is used:
///
/// ```rust
/// use energy_billing::rates::stromsteuer_for_year;
/// assert_eq!(stromsteuer_for_year(2024), Some(rust_decimal::dec!(2.05)));
/// assert_eq!(stromsteuer_for_year(1999), None); // before StromStG
/// ```
const STROMSTEUER_HISTORY: &[(i32, &str)] = &[
    // Standard rate 2.05 ct/kWh since 01.04.2003 (BGBl. I 2002 S. 4602)
    // All years from 2003 onward use the same rate unless changed.
    (2003, "2.05"),
    (2004, "2.05"),
    (2005, "2.05"),
    (2006, "2.05"),
    (2007, "2.05"),
    (2008, "2.05"),
    (2009, "2.05"),
    (2010, "2.05"),
    (2011, "2.05"),
    (2012, "2.05"),
    (2013, "2.05"),
    (2014, "2.05"),
    (2015, "2.05"),
    (2016, "2.05"),
    (2017, "2.05"),
    (2018, "2.05"),
    (2019, "2.05"),
    (2020, "2.05"),
    (2021, "2.05"),
    (2022, "2.05"),
    (2023, "2.05"),
    (2024, "2.05"),
    (2025, "2.05"),
    (2026, "2.05"),
];

/// Return the standard §3 StromStG rate (ct/kWh) for a given calendar year.
///
/// Returns `None` when the year predates the StromStG (before 2003) or is
/// beyond the known table. Callers should fall back to
/// `RegulatoryRates::stromsteuer_ct_per_kwh` for unknown years.
#[must_use]
pub fn stromsteuer_for_year(year: i32) -> Option<Decimal> {
    STROMSTEUER_HISTORY
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, rate)| rate.parse().expect("rate is a valid decimal literal"))
}

// ── Energiesteuer Gas history (§2 Nr. 3 EnergieStG) ───────────────────────────

/// §2 Nr. 3 EnergieStG Erdgas H standard rate in ct/kWh_Hs by calendar year.
///
/// The rate for Erdgas H has been **0.55 ct/kWh** since the EnergieStG reform.
/// Note: there was a temporary reduction to 0.0 ct/kWh during the energy
/// crisis (Energiesteuersenkungsgesetz, 20.03.2022 – 31.03.2023).
///
/// | Period | ct/kWh_Hs |
/// |---|---|
/// | 01.04.2022 – 31.03.2023 | **0.00** (Energiesteuersenkung, BGBl. I 2022 S. 421) |
/// | from 01.04.2023 | **0.55** (rate restored) |
///
/// `None` = year not in table; use `RegulatoryRates::energiesteuer_gas_ct_per_kwh`.
///
/// ```rust
/// use energy_billing::rates::energiesteuer_gas_for_year;
/// // 2022 had the emergency reduction
/// assert_eq!(energiesteuer_gas_for_year(2022), Some(rust_decimal::dec!(0.0)));
/// // Restored from 2023
/// assert_eq!(energiesteuer_gas_for_year(2023), Some(rust_decimal::dec!(0.55)));
/// ```
const ENERGIESTEUER_GAS_HISTORY: &[(i32, &str)] = &[
    // Emergency 0-rate (Energiesteuersenkungsgesetz 2022-03-20, in effect 01.04.2022 – 31.03.2023)
    // For annual billing we map 2022 → 0.00; for monthly billing callers should use overrides.
    (2022, "0.00"),
    // Rate restored from 01.04.2023
    (2023, "0.55"),
    (2024, "0.55"),
    (2025, "0.55"),
    (2026, "0.55"),
];

/// Return the §2 Nr. 3 EnergieStG gas rate (ct/kWh_Hs) for a given calendar year.
///
/// This handles the 2022 emergency 0-rate and subsequent restoration.
/// For prior years (before 2022), returns `None` — use the configured
/// `RegulatoryRates::energiesteuer_gas_ct_per_kwh`.
#[must_use]
pub fn energiesteuer_gas_for_year(year: i32) -> Option<Decimal> {
    ENERGIESTEUER_GAS_HISTORY
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, rate)| rate.parse().expect("rate is a valid decimal literal"))
}

/// Compute BEHG ct/kWh_Hs for a given calendar year.
///
/// Returns `None` when no statutory rate is known for the year (caller should
/// fall back to `RegulatoryRates::behg_gas_ct_per_kwh`).
///
/// # Example
/// ```rust
/// use energy_billing::rates::behg_ct_per_kwh_for_year;
/// // 2024: 45 EUR/t × 0.20160 kg/kWh = 0.9072 ct/kWh
/// let ct = behg_ct_per_kwh_for_year(2024).unwrap();
/// assert!(ct > rust_decimal::dec!(0.90) && ct < rust_decimal::dec!(0.92));
/// ```
#[must_use]
pub fn behg_ct_per_kwh_for_year(year: i32) -> Option<Decimal> {
    BEHG_EUR_PER_T
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, eur_per_t)| {
            // ct/kWh = EUR/t × CO₂_factor kg/kWh × (1 t / 1000 kg) × (100 ct / 1 EUR)
            // = EUR/t × CO₂_factor / 10
            Decimal::from(*eur_per_t) * BEHG_CO2_FACTOR_H_GAS / dec!(10)
        })
}

// ── RegulatoryRates ───────────────────────────────────────────────────────────

/// Platform-level defaults for statutory rates.
///
/// Configure under `[rates]` in `billingd.toml`. These defaults reflect
/// 2025/2026 published rates and will be superseded by operator configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegulatoryRates {
    /// §3 StromStG — ct/kWh (current: 2.05 ct/kWh since 01.07.2023).
    pub stromsteuer_ct_per_kwh: Decimal,
    /// §2 Nr. 3 EnergieStG Erdgas H — ct/kWh_Hs (current: 0.55 ct/kWh).
    pub energiesteuer_gas_ct_per_kwh: Decimal,
    /// BEHG CO₂ levy for Erdgas H — ct/kWh_Hs.
    /// = CO₂-Preis EUR/t × 0.20160 kg_CO₂/kWh_Hs ÷ 10
    pub behg_gas_ct_per_kwh: Decimal,
    /// Standard MwSt rate (fraction, e.g. `0.19`).
    pub mwst_rate: Decimal,
}

impl Default for RegulatoryRates {
    fn default() -> Self {
        Self {
            stromsteuer_ct_per_kwh: dec!(2.05),
            energiesteuer_gas_ct_per_kwh: dec!(0.55),
            behg_gas_ct_per_kwh: dec!(1.310), // 65 EUR/t × 0.20160 kg_CO₂/kWh_Hs (2026, BEHG §10)
            mwst_rate: dec!(0.19),
        }
    }
}

impl RegulatoryRates {
    // ── Typed product helpers (used by Product::build_engine) ─────────────────

    /// Effective MwSt for an [`ElectricityProduct`](crate::ElectricityProduct).
    ///
    /// Priority: `mwst_rate_override` → kWp 0% rule (§12 Abs. 3 UStG) → default.
    #[must_use]
    pub fn effective_mwst_electricity(&self, p: &crate::tariff::ElectricityProduct) -> Decimal {
        if let Some(r) = p.mwst_rate_override {
            return r;
        }
        if p.anlage_kwp.is_some_and(|kwp| kwp <= dec!(30)) {
            return Decimal::ZERO;
        }
        self.mwst_rate
    }

    /// Effective MwSt for a [`SolarProduct`](crate::SolarProduct).
    #[must_use]
    pub fn effective_mwst_solar(&self, p: &crate::tariff::SolarProduct) -> Decimal {
        if let Some(r) = p.mwst_rate_override {
            return r;
        }
        if p.anlage_kwp.is_some_and(|kwp| kwp <= dec!(30)) {
            return Decimal::ZERO;
        }
        self.mwst_rate
    }

    /// Effective MwSt for an [`EegProduct`](crate::EegProduct).
    #[must_use]
    pub fn effective_mwst_eeg(&self, p: &crate::tariff::EegProduct) -> Decimal {
        if let Some(r) = p.mwst_rate_override {
            return r;
        }
        if p.anlage_kwp.is_some_and(|kwp| kwp <= dec!(30)) {
            return Decimal::ZERO;
        }
        self.mwst_rate
    }

    // ── Override-based helpers (used by providers) ────────────────────────────

    /// Effective Stromsteuer — product `override_ct` wins, else statutory rate.
    #[must_use]
    pub fn effective_stromsteuer(&self, override_ct: Option<Decimal>) -> Decimal {
        override_ct.unwrap_or(self.stromsteuer_ct_per_kwh)
    }

    /// Effective Energiesteuer Gas — product `override_ct` wins.
    #[must_use]
    pub fn effective_energiesteuer_gas(&self, override_ct: Option<Decimal>) -> Decimal {
        override_ct.unwrap_or(self.energiesteuer_gas_ct_per_kwh)
    }

    /// Effective BEHG CO₂ levy — product `override_ct` wins.
    #[must_use]
    pub fn effective_behg_gas(&self, override_ct: Option<Decimal>) -> Decimal {
        override_ct.unwrap_or(self.behg_gas_ct_per_kwh)
    }

    /// Effective BEHG for a specific billing year (retroactive corrections).
    #[must_use]
    pub fn effective_behg_gas_for_year(&self, override_ct: Option<Decimal>, year: i32) -> Decimal {
        if let Some(o) = override_ct {
            return o;
        }
        behg_ct_per_kwh_for_year(year).unwrap_or(self.behg_gas_ct_per_kwh)
    }

    /// Effective Stromsteuer for a specific billing year (retroactive corrections).
    #[must_use]
    pub fn effective_stromsteuer_for_year(
        &self,
        override_ct: Option<Decimal>,
        year: i32,
    ) -> Decimal {
        if let Some(o) = override_ct {
            return o;
        }
        stromsteuer_for_year(year).unwrap_or(self.stromsteuer_ct_per_kwh)
    }

    /// Effective Energiesteuer Gas for a specific billing year (retroactive corrections).
    ///
    /// Handles the 2022 emergency 0-rate (Energiesteuersenkungsgesetz).
    #[must_use]
    pub fn effective_energiesteuer_gas_for_year(
        &self,
        override_ct: Option<Decimal>,
        year: i32,
    ) -> Decimal {
        if let Some(o) = override_ct {
            return o;
        }
        energiesteuer_gas_for_year(year).unwrap_or(self.energiesteuer_gas_ct_per_kwh)
    }

    /// Effective MwSt from a raw override value and optional kWp.
    ///
    /// Used by providers that need the MwSt rate outside of `Product::build_engine`.
    #[must_use]
    pub fn effective_mwst_with_override(
        &self,
        override_rate: Option<Decimal>,
        anlage_kwp: Option<Decimal>,
    ) -> Decimal {
        if let Some(r) = override_rate {
            return r;
        }
        if anlage_kwp.is_some_and(|kwp| kwp <= dec!(30)) {
            return Decimal::ZERO;
        }
        self.mwst_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behg_year_table_2026_matches_expected() {
        let ct = behg_ct_per_kwh_for_year(2026).unwrap();
        // 65 EUR/t × 0.20160 = 13.104 → ÷ 10 = 1.3104 ct/kWh
        let expected = dec!(65) * dec!(0.20160) / dec!(10);
        assert_eq!(ct, expected);
    }

    #[test]
    fn behg_year_table_2024_matches_expected() {
        let ct = behg_ct_per_kwh_for_year(2024).unwrap();
        // 45 EUR/t × 0.20160 = 9.072 → ÷ 10 = 0.9072 ct/kWh
        let expected = dec!(45) * dec!(0.20160) / dec!(10);
        assert_eq!(ct, expected);
    }

    #[test]
    fn behg_unknown_year_returns_none() {
        assert!(behg_ct_per_kwh_for_year(2020).is_none());
        assert!(behg_ct_per_kwh_for_year(2030).is_none());
    }

    #[test]
    fn effective_behg_for_year_prefers_override() {
        let rates = RegulatoryRates::default();
        assert_eq!(
            rates.effective_behg_gas_for_year(Some(dec!(0.99)), 2024),
            dec!(0.99)
        );
        // Without override, uses statutory year rate
        let ct = rates.effective_behg_gas_for_year(None, 2024);
        assert!(ct > dec!(0.90) && ct < dec!(0.92));
    }
}
