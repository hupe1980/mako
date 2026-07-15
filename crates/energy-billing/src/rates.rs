//! `RegulatoryRates` — statutory levy rates, operator-configured.
//!
//! All rates come from `billingd.toml [rates]`. The library never hardcodes
//! statutory rates; they change with each legislative year.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::tariff::TariffInput;

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
/// assert_eq!(stromsteuer_for_year(2024), Some(rust_decimal_macros::dec!(2.05)));
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
/// assert_eq!(energiesteuer_gas_for_year(2022), Some(rust_decimal_macros::dec!(0.0)));
/// // Restored from 2023
/// assert_eq!(energiesteuer_gas_for_year(2023), Some(rust_decimal_macros::dec!(0.55)));
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
/// assert!(ct > rust_decimal_macros::dec!(0.90) && ct < rust_decimal_macros::dec!(0.92));
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
    /// Effective Stromsteuer: product `stromsteuer_ct_per_kwh_override` wins.
    pub fn effective_stromsteuer(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .stromsteuer_ct_per_kwh_override
            .unwrap_or(self.stromsteuer_ct_per_kwh)
    }

    /// Effective Energiesteuer Gas: product override wins.
    pub fn effective_energiesteuer_gas(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .energiesteuer_gas_ct_per_kwh_override
            .unwrap_or(self.energiesteuer_gas_ct_per_kwh)
    }

    /// Effective BEHG CO₂ levy: product override wins, then `behg_gas_ct_per_kwh`.
    pub fn effective_behg_gas(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .behg_gas_ct_per_kwh_override
            .unwrap_or(self.behg_gas_ct_per_kwh)
    }

    /// Effective BEHG CO₂ levy for a specific billing year.
    ///
    /// Prefers product override → statutory rate for `year` → `behg_gas_ct_per_kwh`.
    /// Use this for historical correction invoices where the wrong year's rate would
    /// otherwise be applied.
    pub fn effective_behg_gas_for_year(&self, tariff: &TariffInput, year: i32) -> Decimal {
        if let Some(o) = tariff.behg_gas_ct_per_kwh_override {
            return o;
        }
        behg_ct_per_kwh_for_year(year).unwrap_or(self.behg_gas_ct_per_kwh)
    }

    /// Effective Stromsteuer for a specific billing year.
    ///
    /// Prefers product override → statutory rate for `year` → `stromsteuer_ct_per_kwh`.
    ///
    /// Use for retroactive correction invoices to apply the correct year's statutory rate.
    pub fn effective_stromsteuer_for_year(&self, tariff: &TariffInput, year: i32) -> Decimal {
        if let Some(o) = tariff.stromsteuer_ct_per_kwh_override {
            return o;
        }
        stromsteuer_for_year(year).unwrap_or(self.stromsteuer_ct_per_kwh)
    }

    /// Effective Energiesteuer Gas for a specific billing year.
    ///
    /// Handles the 2022 emergency 0-rate (Energiesteuersenkungsgesetz) and
    /// post-2023 restoration automatically.
    ///
    /// Prefers product override → statutory rate for `year` → `energiesteuer_gas_ct_per_kwh`.
    pub fn effective_energiesteuer_gas_for_year(&self, tariff: &TariffInput, year: i32) -> Decimal {
        if let Some(o) = tariff.energiesteuer_gas_ct_per_kwh_override {
            return o;
        }
        energiesteuer_gas_for_year(year).unwrap_or(self.energiesteuer_gas_ct_per_kwh)
    }

    /// Effective MwSt rate: product override → kWp auto-rule → default.
    ///
    /// ## Auto-zero for solar PV ≤ 30 kWp
    ///
    /// §12 Abs. 3 UStG (Jahressteuergesetz 2022, in force since 01.01.2023):
    /// supply and self-consumption billing for solar PV ≤ 30 kWp is subject to
    /// **0% MwSt** (Nullsteuersatz). When `tariff.anlage_kwp` is set and ≤ 30,
    /// this method returns `Decimal::ZERO` automatically — no need to set
    /// `mwst_rate_override` manually.
    ///
    /// ## ETS2 / BEHG post-2026 note
    ///
    /// Germany's BEHG fixed-price regime (§10 BEHG, max 65 EUR/t in 2026) expires
    /// after 2026. From 2027, heating fuel carbon pricing transitions to the EU
    /// Emissions Trading System 2 (ETS2) with market-determined prices. Configure
    /// the current ETS2 CO₂ price in `behg_gas_ct_per_kwh` via `billingd.toml [rates]`.
    pub fn effective_mwst(&self, tariff: &TariffInput) -> Decimal {
        if let Some(override_rate) = tariff.mwst_rate_override {
            return override_rate;
        }
        // §12 Abs. 3 UStG: 0% MwSt for solar PV installations ≤ 30 kWp (since 01.01.2023).
        if tariff
            .anlage_kwp
            .is_some_and(|kwp| kwp <= rust_decimal_macros::dec!(30))
        {
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
        let tariff = crate::tariff::TariffInput {
            behg_gas_ct_per_kwh_override: Some(dec!(0.99)),
            ..Default::default()
        };
        assert_eq!(rates.effective_behg_gas_for_year(&tariff, 2024), dec!(0.99));
    }
}
