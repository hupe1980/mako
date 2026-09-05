//! `RegulatoryRates` — statutory levy rates, operator-configured.
//!
//! All rates come from `billingd.toml [rates]`. The library never hardcodes
//! statutory rates; they change with each legislative year.

use rust_decimal::Decimal;
use rust_decimal::dec;
use serde::{Deserialize, Serialize};

/// Kaufmännisches Runden (DIN 1333): round half **away from zero**.
///
/// German commercial practice — and the EN 16931 / XRechnung validation
/// ecosystem — expect half-up rounding, while `Decimal::round_dp` defaults
/// to banker's rounding (MidpointNearestEven). The two agree everywhere except
/// exact midpoints, which is where a price quoted in ct with three decimals
/// lands, so the wrong mode misstates a cent without failing any test written
/// against ordinary numbers. `cargo xtask check-rounding` refuses a bare
/// `round_dp` workspace-wide for that reason.
///
/// Away-from-zero (not literal half-up) keeps credit notes and Stornorechnungen
/// symmetric to their originals: round(-0.005) = -0.01 mirrors
/// round(0.005) = 0.01.
///
/// The mode itself is [`billing::RoundingStrategy::MidpointAwayFromZero`] — the
/// same strategy the `billing` arithmetic core applies inside every `Amount`
/// conversion, multiplication and division. One authority, two call styles:
/// typed fixed-point via `billing::Amount` where the precision is statutory,
/// and this helper where a runtime `dp` is needed on a raw `Decimal`.
#[must_use]
pub fn round_money(value: Decimal, dp: u32) -> Decimal {
    value.round_dp_with_strategy(dp, billing::RoundingStrategy::MidpointAwayFromZero.into())
}

/// Method-call form of [`round_money`] — `x.round_kfm(2)`.
pub trait RoundMoney {
    /// Round to `dp` decimal places, half away from zero (DIN 1333).
    #[must_use]
    fn round_kfm(&self, dp: u32) -> Decimal;
}

impl RoundMoney for Decimal {
    fn round_kfm(&self, dp: u32) -> Decimal {
        round_money(*self, dp)
    }
}

// ── BEHG CO₂ price table ──────────────────────────────────────────────────────

/// BEHG §10 CO₂ price in EUR/t by calendar year.
///
/// Source: Brennstoffemissionshandelsgesetz §10 BEHG (BGBl. I 2021 Nr. 37).
///
/// | Year | EUR/t | ct/kWh_Hs |
/// |---|---|---|
/// | 2021 | 25   | 0.4551 |
/// | 2022 | 30   | 0.5461 |
/// | 2023 | 30   | 0.5442 |
/// | 2024 | 45   | 0.8163 |
/// | 2025 | 55   | 0.9977 |
/// | 2026 | 65   | 1.1791 |
const BEHG_EUR_PER_T: &[(i32, u32)] = &[
    (2021, 25),
    (2022, 30),
    (2023, 30),
    (2024, 45),
    (2025, 55),
    (2026, 65),
];

// ── Erdgas emission factor (EBeV Anlage) ─────────────────────────────────────
//
// German gas is metered and billed in **kWh_Hs** (Brennwert), while the EBeV
// emission factor is *heizwertbezogen* (Hi). The ordinance closes that gap in
// its own Umrechnungsfaktor column rather than leaving it to the reader:
// Anlage 2 EBeV 2030 Nr. 6 gives Erdgas as `3,2508 GJ/MWh`, and states the
// derivation — „3,6 GJ/MWh · 0,903 GJ/GJ". Taking the bare 3,6 instead treats a
// billed Brennwert quantity as if it were a Heizwert one and overstates the
// CO₂ component by about 11 %.

/// Umrechnungsfaktor Erdgas — GJ (Hi) per MWh as billed (Hs).
///
/// Anlage 2 EBeV 2030 Nr. 6 Spalte 4, and identically Anlage 1 Teil 4 EBeV 2022.
/// `3,6 GJ/MWh · 0,903 GJ/GJ`, so the Brennwert→Heizwert step is already in it.
pub const ERDGAS_UMRECHNUNGSFAKTOR_GJ_PER_MWH: Decimal = dec!(3.2508);

/// Heizwertbezogener Emissionsfaktor für Erdgas in t CO₂/GJ, by calendar year.
///
/// EBeV 2022 Anlage 1 Teil 4 for 2021–2022, EBeV 2030 Anlage 2 Nr. 6 from 2023.
/// The ordinance publishes **one** Erdgas row — there is no separate H-Gas and
/// L-Gas Standardwert, and § 3 Abs. 2 CO2KostAufG requires the BEHG
/// Standardwerte, so a network-specific factor has no place in this disclosure.
const ERDGAS_EMISSIONSFAKTOR_T_PER_GJ: &[(i32, Decimal)] = &[
    (2021, dec!(0.056)),
    (2022, dec!(0.056)),
    (2023, dec!(0.0558)),
    (2024, dec!(0.0558)),
    (2025, dec!(0.0558)),
    (2026, dec!(0.0558)),
    (2027, dec!(0.0558)),
    (2028, dec!(0.0558)),
    (2029, dec!(0.0558)),
    (2030, dec!(0.0558)),
];

/// The Erdgas emission factor for a year, in **kg CO₂ per kWh_Hs** — the unit
/// § 3 Abs. 1 Nr. 3 CO2KostAufG asks for, against the quantity the invoice bills.
///
/// `kg/kWh_Hs = Umrechnungsfaktor GJ/MWh × Emissionsfaktor t/GJ`, the t/kg and
/// MWh/kWh factors of 1 000 cancelling.
///
/// Returns `None` for a year outside the EBeV tables; EBeV 2030 runs to 2030.
///
/// # Example
/// ```rust
/// use energy_billing::rates::erdgas_emissionsfaktor_kg_per_kwh;
/// use rust_decimal::dec;
/// // 3,2508 GJ/MWh × 0,0558 t CO₂/GJ
/// assert_eq!(erdgas_emissionsfaktor_kg_per_kwh(2026), Some(dec!(0.18139464)));
/// ```
#[must_use]
pub fn erdgas_emissionsfaktor_kg_per_kwh(year: i32) -> Option<Decimal> {
    ERDGAS_EMISSIONSFAKTOR_T_PER_GJ
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, t_per_gj)| ERDGAS_UMRECHNUNGSFAKTOR_GJ_PER_MWH * t_per_gj)
}

// ── Stromsteuer (§ 3 StromStG) ───────────────────────────────────────────────

/// The standard § 3 StromStG rate: **2,05 ct/kWh** (20,50 EUR/MWh), unchanged
/// since 01.04.2003 (BGBl. I 2002 S. 4602).
pub const STROMSTEUER_REGELSATZ_CT_PER_KWH: Decimal = dec!(2.05);

/// First calendar year the § 3 StromStG standard rate applies to.
const STROMSTEUER_SINCE: i32 = 2003;

/// The standard § 3 StromStG rate (ct/kWh) for a calendar year.
///
/// `None` before 2003, when the rate was still climbing through the Ökosteuer
/// stages — a caller billing such a period must supply the rate itself rather
/// than inherit today's.
///
/// This replaced a 24-row table of identical values. The rate has not moved in
/// twenty-three years and the reliefs that did move are **Entlastungen**
/// (§ 9b StromStG, permanent at the EU minimum rate since 01.01.2026), which
/// the customer claims from the Hauptzollamt and which never change what a
/// supplier invoices — see [`crate::steuer`].
///
/// ## Usage for retroactive corrections
///
/// ```rust
/// use energy_billing::rates::stromsteuer_for_year;
/// assert_eq!(stromsteuer_for_year(2024), Some(rust_decimal::dec!(2.05)));
/// assert_eq!(stromsteuer_for_year(1999), None); // before the standard rate
/// ```
#[must_use]
pub fn stromsteuer_for_year(year: i32) -> Option<Decimal> {
    (year >= STROMSTEUER_SINCE).then_some(STROMSTEUER_REGELSATZ_CT_PER_KWH)
}

// ── Energiesteuer Erdgas (§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG) ────────────────

/// The Erdgas-als-Heizstoff rate: **0,55 ct/kWh_Hs** (5,50 EUR/MWh).
pub const ENERGIESTEUER_GAS_CT_PER_KWH: Decimal = dec!(0.55);

/// First calendar year the EnergieStG applies to (in force 01.08.2006).
const ENERGIESTEUER_SINCE: i32 = 2006;

/// The § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG heating-gas rate (ct/kWh_Hs) for a
/// calendar year, or `None` before the EnergieStG.
///
/// Constant since the 2003 Ökosteuer stage and carried over into the EnergieStG.
/// The 2022 Energiesteuersenkungsgesetz (BGBl. I 2022 S. 810) reduced
/// **motor-fuel** rates (§ 2 Abs. 1) only; the 2022/23 gas reliefs were the
/// Dezember-Soforthilfe (EWSG) and the USt cut to 7 % (§ 28 Abs. 5/6 UStG, see
/// [`mwst_rate_for_gas_waerme_period`]).
///
/// ```rust
/// use energy_billing::rates::energiesteuer_gas_for_year;
/// // No heating-gas reduction existed in 2022 (the Tankrabatt was fuels-only)
/// assert_eq!(energiesteuer_gas_for_year(2022), Some(rust_decimal::dec!(0.55)));
/// assert_eq!(energiesteuer_gas_for_year(2005), None);
/// ```
#[must_use]
pub fn energiesteuer_gas_for_year(year: i32) -> Option<Decimal> {
    (year >= ENERGIESTEUER_SINCE).then_some(ENERGIESTEUER_GAS_CT_PER_KWH)
}

/// Compute BEHG ct/kWh_Hs for a given calendar year.
///
/// Returns `None` when the year has no §10 BEHG price or no EBeV emission
/// factor (caller should fall back to `RegulatoryRates::behg_gas_ct_per_kwh`).
///
/// # Example
/// ```rust
/// use energy_billing::rates::behg_ct_per_kwh_for_year;
/// // 2024: 45 EUR/t × 0.18139464 kg/kWh_Hs ÷ 10
/// let ct = behg_ct_per_kwh_for_year(2024).unwrap();
/// assert!(ct > rust_decimal::dec!(0.81) && ct < rust_decimal::dec!(0.82));
/// ```
#[must_use]
pub fn behg_ct_per_kwh_for_year(year: i32) -> Option<Decimal> {
    let eur_per_t = BEHG_EUR_PER_T
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, eur_per_t)| Decimal::from(*eur_per_t))?;
    behg_ct_per_kwh_from_price(eur_per_t, year)
}

/// Convert an nEHS certificate price (EUR/t CO₂) into the Gas CO₂ cost
/// component in ct/kWh_Hs, at the year's EBeV emission factor.
///
/// Since 2026 nEHS certificates are **auctioned** (§10 Abs. 1 BEHG: weekly EEX
/// auctions from 01.07.2026 inside the 55–65 EUR/t corridor of §10 Abs. 2,
/// followed by a Verkaufsphase at 68 EUR/t), so the CO₂ component is
/// market-formed rather than a statutory fixed price. Callers supply the
/// supplier's actual acquisition price (CO2KostAufG §3 passes through the
/// **tatsächlich aufgewendete** CO₂ costs) — e.g. from a dated market-price
/// series.
///
/// The factor is the year's, not the caller's: § 3 Abs. 2 CO2KostAufG requires
/// the BEHG Standardwerte, and the EBeV publishes one Erdgas row for the whole
/// market.
///
/// `ct/kWh_Hs = EUR/t × Emissionsfaktor kg/kWh_Hs ÷ 10`
///
/// Returns `None` for a year outside the EBeV tables.
#[must_use]
pub fn behg_ct_per_kwh_from_price(eur_per_t: Decimal, year: i32) -> Option<Decimal> {
    erdgas_emissionsfaktor_kg_per_kwh(year).map(|factor| eur_per_t * factor / dec!(10))
}

// ── RegulatoryRates ───────────────────────────────────────────────────────────

/// Platform-level defaults for statutory rates.
///
/// Configure under `[rates]` in `billingd.toml`. These defaults reflect
/// 2025/2026 published rates and will be superseded by operator configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegulatoryRates {
    /// §3 StromStG — ct/kWh (current: 2.05 ct/kWh since 01.04.2003,
    /// BGBl. I 2002 S. 4602; see [`stromsteuer_for_year`]).
    pub stromsteuer_ct_per_kwh: Decimal,
    /// § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG, Erdgas als Heizstoff —
    /// ct/kWh_Hs (current: 0.55 ct/kWh = 5,50 EUR/MWh).
    pub energiesteuer_gas_ct_per_kwh: Decimal,
    /// BEHG CO₂ levy for Erdgas H — ct/kWh_Hs.
    /// = CO₂-Preis EUR/t × Emissionsfaktor kg_CO₂/kWh_Hs ÷ 10
    /// (see [`erdgas_emissionsfaktor_kg_per_kwh`])
    pub behg_gas_ct_per_kwh: Decimal,
    /// Standard MwSt rate (fraction, e.g. `0.19`).
    pub mwst_rate: Decimal,
    /// Reduced MwSt rate (§ 12 Abs. 2 UStG, fraction, e.g. `0.07`).
    ///
    /// Used for the Anlage-2 supplies this engine bills — Trinkwasser (Nr. 34).
    /// Carried as a rate rather than a literal so a statutory change is one
    /// configuration edit rather than a grep across the providers.
    #[serde(default = "default_mwst_reduced")]
    pub mwst_rate_reduced: Decimal,
}

/// Serde default for [`RegulatoryRates::mwst_rate_reduced`].
fn default_mwst_reduced() -> Decimal {
    dec!(0.07)
}

impl Default for RegulatoryRates {
    fn default() -> Self {
        Self {
            stromsteuer_ct_per_kwh: dec!(2.05),
            energiesteuer_gas_ct_per_kwh: dec!(0.55),
            // 65 EUR/t (2026, BEHG §10) × 0.18139464 kg_CO₂/kWh_Hs ÷ 10
            behg_gas_ct_per_kwh: dec!(1.17906516),
            mwst_rate: dec!(0.19),
            mwst_rate_reduced: dec!(0.07),
        }
    }
}

impl RegulatoryRates {
    // ── Typed product helpers (used by Product::build_engine) ─────────────────

    /// Effective MwSt for an [`ElectricityProduct`](crate::ElectricityProduct).
    ///
    /// A retail electricity **supply** is standard-rated regardless of whether
    /// the customer operates their own PV plant: §12 Abs. 3 UStG zero-rates the
    /// *supply of the PV system* (modules, storage, installation) — not the
    /// electricity delivered over the grid. Only an explicit `mwst_rate_override`
    /// departs from the default (e.g. an intra-community reverse-charge B2B case
    /// the caller has already assessed).
    #[must_use]
    pub fn effective_mwst_electricity(&self, p: &crate::tariff::ElectricityProduct) -> Decimal {
        p.mwst_rate_override.unwrap_or(self.mwst_rate)
    }

    /// Effective MwSt for a [`SolarProduct`](crate::SolarProduct) feed-in Gutschrift.
    ///
    /// A feed-in operator who has opted for the **Kleinunternehmerregelung
    /// (§19 UStG)** — the common case for small rooftop plants since the
    /// §12 Abs. 3 UStG Nullsteuersatz removed the input-tax incentive to choose
    /// Regelbesteuerung — issues no USt, so the Gutschrift is 0 %. It is the
    /// operator's *election*, not a function of plant size, so it is carried as
    /// an explicit flag rather than derived from `anlage_kwp`.
    #[must_use]
    pub fn effective_mwst_solar(&self, p: &crate::tariff::SolarProduct) -> Decimal {
        if let Some(r) = p.mwst_rate_override {
            return r;
        }
        if p.kleinunternehmer_19_ustg {
            return Decimal::ZERO;
        }
        self.mwst_rate
    }

    /// Effective MwSt for an [`EegProduct`](crate::EegProduct) feed-in Gutschrift.
    ///
    /// 0 % when the operator has elected the Kleinunternehmerregelung
    /// (§19 UStG); otherwise the standard rate. See [`Self::effective_mwst_solar`].
    #[must_use]
    pub fn effective_mwst_eeg(&self, p: &crate::tariff::EegProduct) -> Decimal {
        if let Some(r) = p.mwst_rate_override {
            return r;
        }
        if p.kleinunternehmer_19_ustg {
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
    /// Heating gas has been 0.55 ct/kWh throughout (§2 Abs. 3 Nr. 4 EnergieStG) —
    /// the 2022 Energiesteuersenkungsgesetz cut motor fuels only, never heating gas.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behg_year_table_2026_matches_expected() {
        let ct = behg_ct_per_kwh_for_year(2026).unwrap();
        // 65 EUR/t × 0.18139464 kg/kWh_Hs ÷ 10 = 1.17906516 ct/kWh
        let expected = dec!(65) * dec!(0.18139464) / dec!(10);
        assert_eq!(ct, expected);
        assert_eq!(ct, dec!(1.17906516));
    }

    #[test]
    fn behg_year_table_2024_matches_expected() {
        let ct = behg_ct_per_kwh_for_year(2024).unwrap();
        // 45 EUR/t × 0.18139464 kg/kWh_Hs ÷ 10 = 0.81627588 ct/kWh
        let expected = dec!(45) * dec!(0.18139464) / dec!(10);
        assert_eq!(ct, expected);
    }

    #[test]
    fn behg_unknown_year_returns_none() {
        assert!(behg_ct_per_kwh_for_year(2020).is_none());
        assert!(behg_ct_per_kwh_for_year(2031).is_none());
    }

    /// The EBeV factor is heizwertbezogen while the invoice bills Brennwert, so
    /// the ordinance's own Umrechnungsfaktor carries the conversion. Using the
    /// bare 3,6 GJ/MWh instead of 3,2508 overstates the component by ~11 %.
    #[test]
    fn the_emission_factor_is_brennwert_based() {
        let f = erdgas_emissionsfaktor_kg_per_kwh(2026).expect("2026 is tabled");
        assert_eq!(f, dec!(3.2508) * dec!(0.0558));
        assert_eq!(f, dec!(0.18139464));

        let heizwert_basis = dec!(3.6) * dec!(0.0558);
        assert!(
            heizwert_basis > f,
            "the Heizwert basis is the larger number — it is not what is billed"
        );
    }

    /// EBeV 2022 and EBeV 2030 differ in the Emissionsfaktor, not the
    /// Umrechnungsfaktor, so the step falls at the 2022/2023 boundary.
    #[test]
    fn the_factor_steps_when_the_ordinance_does() {
        assert_eq!(
            erdgas_emissionsfaktor_kg_per_kwh(2022),
            Some(dec!(3.2508) * dec!(0.056))
        );
        assert_eq!(
            erdgas_emissionsfaktor_kg_per_kwh(2023),
            Some(dec!(3.2508) * dec!(0.0558))
        );
        assert_ne!(
            erdgas_emissionsfaktor_kg_per_kwh(2022),
            erdgas_emissionsfaktor_kg_per_kwh(2023)
        );
        // EBeV 2030 runs to 2030; nothing is tabled beyond it.
        assert!(erdgas_emissionsfaktor_kg_per_kwh(2031).is_none());
    }

    /// The EBeV publishes one Erdgas row, so an L-Gas supply point is priced on
    /// the same Standardwert as an H-Gas one.
    #[test]
    fn there_is_one_erdgas_factor_for_the_whole_market() {
        assert_eq!(
            behg_ct_per_kwh_from_price(dec!(65), 2026),
            Some(dec!(65) * dec!(0.18139464) / dec!(10))
        );
        assert!(behg_ct_per_kwh_from_price(dec!(65), 2031).is_none());
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
        assert_eq!(ct, dec!(45) * dec!(0.18139464) / dec!(10));
    }
}

/// The Umsatzsteuer rate in force for a billing period.
///
/// Germany has had one departure from 19 % since 2007: the COVID reduction of
/// 01.07.2020 – 31.12.2020, at 16 % (§28 Abs. 1 UStG in the then-current
/// Fassung). A period wholly inside that window bills 16 %; wholly outside,
/// 19 %.
///
/// Returns `None` when the period *straddles* the window boundary: no single
/// rate is correct for such a period, and picking one silently would misbill
/// part of it. The caller splits the period — the same discipline the grid
/// engine applies to regulatory-regime turnovers.
#[must_use]
pub fn mwst_rate_for_period(from: time::Date, to: time::Date) -> Option<Decimal> {
    const SENKUNG_VON: time::Date = time::macros::date!(2020 - 07 - 01);
    const SENKUNG_BIS: time::Date = time::macros::date!(2020 - 12 - 31);

    let inside = from >= SENKUNG_VON && to <= SENKUNG_BIS;
    let outside = to < SENKUNG_VON || from > SENKUNG_BIS;
    match (inside, outside) {
        (true, _) => Some(dec!(0.16)),
        (_, true) => Some(dec!(0.19)),
        _ => None, // straddles the boundary — split the period
    }
}

/// The Umsatzsteuer rate in force for a **gas or Fernwärme** billing period.
///
/// Gas delivered via the Erdgasnetz and heat via a Wärmenetz had two statutory
/// departures from 19 %:
///
/// - 01.07.2020 – 31.12.2020: **16 %** (COVID reduction, §28 Abs. 1–3 UStG a.F.)
/// - 01.10.2022 – 31.03.2024: **7 %** (Gesetz zur temporären Senkung des
///   Umsatzsteuersatzes auf Gaslieferungen über das Erdgasnetz, §28 Abs. 5/6
///   UStG — extended to Fernwärme by the Finanzausschuss)
///
/// Returns `None` when the period straddles a window boundary: no single rate
/// is correct for such a period and picking one silently would misbill part of
/// it. The caller splits the period at the Stichtag and merges the invoices.
#[must_use]
pub fn mwst_rate_for_gas_waerme_period(from: time::Date, to: time::Date) -> Option<Decimal> {
    const WINDOWS: &[(time::Date, time::Date, Decimal)] = &[
        (
            time::macros::date!(2020 - 07 - 01),
            time::macros::date!(2020 - 12 - 31),
            dec!(0.16),
        ),
        (
            time::macros::date!(2022 - 10 - 01),
            time::macros::date!(2024 - 03 - 31),
            dec!(0.07),
        ),
    ];
    for (von, bis, rate) in WINDOWS {
        let inside = from >= *von && to <= *bis;
        let overlaps = from <= *bis && to >= *von;
        if inside {
            return Some(*rate);
        }
        if overlaps {
            return None; // straddles this window's boundary — split the period
        }
    }
    Some(dec!(0.19))
}

#[cfg(test)]
mod mwst_period_tests {
    use super::*;
    use time::macros::date;

    /// The COVID window bills 16 %, everything else 19 %.
    #[test]
    fn the_covid_window_and_its_edges() {
        assert_eq!(
            mwst_rate_for_period(date!(2020 - 07 - 01), date!(2020 - 12 - 31)),
            Some(dec!(0.16))
        );
        assert_eq!(
            mwst_rate_for_period(date!(2020 - 01 - 01), date!(2020 - 06 - 30)),
            Some(dec!(0.19))
        );
        assert_eq!(
            mwst_rate_for_period(date!(2026 - 01 - 01), date!(2026 - 01 - 31)),
            Some(dec!(0.19))
        );
    }

    /// A straddling period has no single correct rate.
    #[test]
    fn a_straddling_period_yields_none() {
        assert_eq!(
            mwst_rate_for_period(date!(2020 - 06 - 15), date!(2020 - 07 - 15)),
            None
        );
        assert_eq!(
            mwst_rate_for_period(date!(2020 - 12 - 15), date!(2021 - 01 - 15)),
            None
        );
    }

    /// Gas/Wärme: the §28 Abs. 5/6 UStG 7 % window (01.10.2022 – 31.03.2024).
    #[test]
    fn the_gas_waerme_seven_percent_window() {
        // Wholly inside the 7 % window
        assert_eq!(
            mwst_rate_for_gas_waerme_period(date!(2022 - 10 - 01), date!(2023 - 09 - 30)),
            Some(dec!(0.07))
        );
        assert_eq!(
            mwst_rate_for_gas_waerme_period(date!(2024 - 01 - 01), date!(2024 - 03 - 31)),
            Some(dec!(0.07))
        );
        // COVID window still yields 16 %
        assert_eq!(
            mwst_rate_for_gas_waerme_period(date!(2020 - 07 - 01), date!(2020 - 12 - 31)),
            Some(dec!(0.16))
        );
        // Outside every window → 19 %
        assert_eq!(
            mwst_rate_for_gas_waerme_period(date!(2026 - 01 - 01), date!(2026 - 12 - 31)),
            Some(dec!(0.19))
        );
        // Straddling the window end (Q1/Q2 2024) → split required
        assert_eq!(
            mwst_rate_for_gas_waerme_period(date!(2024 - 03 - 01), date!(2024 - 04 - 30)),
            None
        );
        // A gas annual bill Oct 2022 – Sep 2023 straddling the window start
        assert_eq!(
            mwst_rate_for_gas_waerme_period(date!(2022 - 09 - 01), date!(2023 - 08 - 31)),
            None
        );
    }
}

// ── Steuerliche Stichtage inside a billing period ────────────────────────────

/// The dates inside `[from, to]` on which a statutory rate changes, for the
/// given product category.
///
/// # Why a period must be split rather than billed at one rate
///
/// [`mwst_rate_for_period`] and [`mwst_rate_for_gas_waerme_period`] return
/// `None` for a straddling period because **no single rate is correct** for it.
/// Answering that `None` with a default silently bills part of the period at
/// the wrong rate — for a gas period crossing 31.03.2024 that is the whole
/// period at 19 % when the earlier portion was legally 7 %, i.e. a customer
/// overcharge that no downstream check can distinguish from a correct invoice.
///
/// This returns the **first day of each new regime** inside the period, so a
/// caller can split `[from, stichtag-1]` and `[stichtag, to]` and bill each at
/// its own rate. An empty result means one rate governs the whole period.
///
/// Two families of Stichtag exist:
///
/// - **Umsatzsteuer** — the COVID window (01.07.2020–31.12.2020, 16 %) for every
///   commodity, plus the gas/Fernwärme window (01.10.2022–31.03.2024, 7 %,
///   §28 Abs. 5/6 UStG) for `GAS` and `WAERME`.
/// - **BEHG** — the CO₂ price of §10 BEHG steps at each calendar-year boundary,
///   so a `GAS` period spanning a year-end has two levy rates. A Fernwärme
///   period does not: its CO₂ cost is the heat product's own CO2KostAufG § 3
///   figure, which no calendar boundary moves.
///
/// # Example
///
/// ```rust
/// use energy_billing::rates::steuer_stichtage_im_zeitraum;
/// use time::macros::date;
///
/// // A gas period crossing the end of the 7 % window splits at 01.04.2024.
/// let s = steuer_stichtage_im_zeitraum("GAS", date!(2024-03-01), date!(2024-04-30));
/// assert_eq!(s, vec![date!(2024-04-01)]);
///
/// // Electricity never had the 7 % window, so the same period is uniform.
/// assert!(steuer_stichtage_im_zeitraum("STROM", date!(2024-03-01), date!(2024-04-30)).is_empty());
/// ```
#[must_use]
pub fn steuer_stichtage_im_zeitraum(
    category: &str,
    from: time::Date,
    to: time::Date,
) -> Vec<time::Date> {
    let mut stichtage = Vec::new();
    if from > to {
        return stichtage;
    }
    let gas_or_waerme = matches!(category, "GAS" | "WAERME");

    // Each entry is the first day of a new USt regime. A boundary counts only
    // when it falls strictly inside the period: a period starting exactly on a
    // Stichtag is already uniform.
    let mut ust: Vec<time::Date> = vec![
        time::macros::date!(2020 - 07 - 01),
        time::macros::date!(2021 - 01 - 01),
    ];
    if gas_or_waerme {
        ust.push(time::macros::date!(2022 - 10 - 01));
        ust.push(time::macros::date!(2024 - 04 - 01));
    }
    for d in ust {
        if d > from && d <= to {
            stichtage.push(d);
        }
    }

    // The § 10 BEHG CO₂ price steps every 1 January, and it is the **Gas**
    // levy: a Fernwärme invoice states the CO₂ cost of the fuel burned to
    // produce the heat, which CO2KostAufG § 3 measures as the cost the supplier
    // actually bore, so it is the heat product's own rate and is the same on
    // both sides of a year boundary. Splitting a heat period there yields two
    // legs priced identically.
    //
    // Both years must be in the § 10 BEHG table. `Some(rate) != None` is not a
    // rate change: a year pair the table does not cover cannot be split into
    // two known rates, so it is left to the operator.
    if category == "GAS" {
        for year in (from.year() + 1)..=to.year() {
            let Ok(jan1) = time::Date::from_calendar_date(year, time::Month::January, 1) else {
                continue;
            };
            if jan1 <= from || jan1 > to {
                continue;
            }
            let (Some(before), Some(after)) = (
                behg_ct_per_kwh_for_year(year - 1),
                behg_ct_per_kwh_for_year(year),
            ) else {
                continue;
            };
            if before != after {
                stichtage.push(jan1);
            }
        }
    }

    stichtage.sort_unstable();
    stichtage.dedup();
    stichtage
}

#[cfg(test)]
mod stichtag_tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn a_uniform_period_has_no_stichtag() {
        assert!(
            steuer_stichtage_im_zeitraum("STROM", date!(2026 - 01 - 01), date!(2026 - 01 - 31))
                .is_empty()
        );
        assert!(
            steuer_stichtage_im_zeitraum("GAS", date!(2026 - 05 - 01), date!(2026 - 05 - 31))
                .is_empty()
        );
    }

    /// The gas 7 % window ends 31.03.2024, so 01.04.2024 opens a new regime.
    #[test]
    fn the_gas_vat_window_end_splits_a_gas_period() {
        assert_eq!(
            steuer_stichtage_im_zeitraum("GAS", date!(2024 - 03 - 01), date!(2024 - 04 - 30)),
            vec![date!(2024 - 04 - 01)]
        );
        assert_eq!(
            steuer_stichtage_im_zeitraum("WAERME", date!(2024 - 03 - 01), date!(2024 - 04 - 30)),
            vec![date!(2024 - 04 - 01)]
        );
    }

    /// Electricity kept 19 % throughout — the gas window is not its boundary.
    #[test]
    fn electricity_does_not_see_the_gas_window() {
        assert!(
            steuer_stichtage_im_zeitraum("STROM", date!(2022 - 09 - 01), date!(2022 - 10 - 31))
                .is_empty()
        );
        assert!(
            steuer_stichtage_im_zeitraum("STROM", date!(2024 - 03 - 01), date!(2024 - 04 - 30))
                .is_empty()
        );
    }

    /// The COVID window applies to every commodity.
    #[test]
    fn the_covid_window_splits_any_commodity() {
        assert_eq!(
            steuer_stichtage_im_zeitraum("STROM", date!(2020 - 06 - 01), date!(2020 - 07 - 31)),
            vec![date!(2020 - 07 - 01)]
        );
        assert_eq!(
            steuer_stichtage_im_zeitraum("STROM", date!(2020 - 12 - 01), date!(2021 - 01 - 31)),
            vec![date!(2021 - 01 - 01)]
        );
    }

    /// A period starting exactly on a Stichtag is already uniform.
    #[test]
    fn a_period_starting_on_the_stichtag_is_uniform() {
        assert!(
            steuer_stichtage_im_zeitraum("GAS", date!(2024 - 04 - 01), date!(2024 - 04 - 30))
                .is_empty()
        );
    }

    /// §10 BEHG steps every 1 January, so a year-crossing gas period splits.
    #[test]
    fn a_year_crossing_gas_period_splits_on_the_behg_step() {
        let s = steuer_stichtage_im_zeitraum("GAS", date!(2023 - 12 - 01), date!(2024 - 01 - 31));
        assert!(
            s.contains(&date!(2024 - 01 - 01)),
            "the BEHG CO₂ price changes at the year boundary: {s:?}"
        );
    }

    /// Electricity carries no BEHG levy, so a year crossing does not split it.
    #[test]
    fn a_year_crossing_electricity_period_does_not_split() {
        assert!(
            steuer_stichtage_im_zeitraum("STROM", date!(2025 - 12 - 01), date!(2026 - 01 - 31))
                .is_empty()
        );
    }

    /// A Fernwärme CO₂ cost is the heat product's own CO2KostAufG § 3 figure,
    /// not the § 10 BEHG gas price, so a year boundary changes nothing about it
    /// — and a Fernwärme year, which is what an annual heat invoice covers,
    /// stays one leg.
    #[test]
    fn a_year_crossing_waerme_period_does_not_split() {
        assert!(
            steuer_stichtage_im_zeitraum("WAERME", date!(2025 - 12 - 01), date!(2026 - 01 - 31))
                .is_empty(),
            "a heat period has one CO₂ rate on both sides of 1 January"
        );
        assert!(
            steuer_stichtage_im_zeitraum("WAERME", date!(2025 - 07 - 01), date!(2026 - 06 - 30))
                .is_empty(),
            "the ordinary Fernwärme Jahresabrechnung is billable whole"
        );
    }

    /// Both years must carry a § 10 BEHG price. Past the end of the table there
    /// is no second rate to bill the other leg at, so there is nothing to split.
    #[test]
    fn a_year_pair_the_behg_table_does_not_cover_is_no_stichtag() {
        let last_tabled = BEHG_EUR_PER_T
            .iter()
            .map(|(y, _)| *y)
            .max()
            .expect("the § 10 BEHG table is not empty");
        let von = time::Date::from_calendar_date(last_tabled, time::Month::December, 1)
            .expect("1 December exists");
        let bis = time::Date::from_calendar_date(last_tabled + 1, time::Month::January, 31)
            .expect("31 January exists");
        assert!(
            steuer_stichtage_im_zeitraum("GAS", von, bis).is_empty(),
            "an untabled year is left to the operator, not split at a rate that does not exist"
        );
    }

    /// A long gas period can carry both a VAT and a BEHG Stichtag, in order.
    #[test]
    fn several_stichtage_come_back_sorted_and_unique() {
        let s = steuer_stichtage_im_zeitraum("GAS", date!(2023 - 11 - 01), date!(2024 - 04 - 30));
        assert!(
            s.contains(&date!(2024 - 01 - 01)) && s.contains(&date!(2024 - 04 - 01)),
            "{s:?}"
        );
        let mut sorted = s.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(s, sorted, "must be sorted and deduplicated");
    }

    #[test]
    fn a_reversed_period_yields_nothing_rather_than_panicking() {
        assert!(
            steuer_stichtage_im_zeitraum("GAS", date!(2024 - 04 - 30), date!(2024 - 03 - 01))
                .is_empty()
        );
    }
}
