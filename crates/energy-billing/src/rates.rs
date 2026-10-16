//! `RegulatoryRates` — the statutory rates of one billing period.
//!
//! The per-kWh taxes come from dated series that end at the last year their
//! statute was read for; a year outside a series has no rate and its
//! provider refuses the period.

use rust_decimal::Decimal;
use rust_decimal::dec;
use serde::{Deserialize, Serialize};

pub use grid_billing::{RoundMoney, round_money};

// ── BEHG CO₂ price table ──────────────────────────────────────────────────────

/// BEHG CO₂ price in EUR/t by calendar year.
///
/// 2021–2025: the Festpreise of § 10 Abs. 2 Satz 2 BEHG. 2026: the Mittelwert
/// of the § 10 Abs. 2 Satz 4 BEHG 55–65 corridor, which § 4 Abs. 1 Nr. 2
/// CO2KostAufG makes the maßgeblicher Preis. No statute fixes a price for a
/// later year, so none is tabled; `cargo xtask check-behg-prices` refuses a
/// row for one.
///
/// | Year | EUR/t | ct/kWh_Hs |
/// |---|---|---|
/// | 2021 | 25   | 0.4551 |
/// | 2022 | 30   | 0.5461 |
/// | 2023 | 30   | 0.5442 |
/// | 2024 | 45   | 0.8163 |
/// | 2025 | 55   | 0.9977 |
/// | 2026 | 60   | 1.0884 |
const BEHG_EUR_PER_T: &[(i32, u32)] = &[
    (2021, 25),
    (2022, 30),
    (2023, 30),
    (2024, 45),
    (2025, 55),
    (2026, 60),
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

// ── Dated statutory series ───────────────────────────────────────────────────

pub use grid_billing::SatzZeile;

/// The row of `series` covering `year`, if any.
fn zeile_fuer(series: &'static [SatzZeile], year: i32) -> Option<&'static SatzZeile> {
    SatzZeile::fuer(series, year)
}

// ── Stromsteuer (§ 3 StromStG) ───────────────────────────────────────────────

/// The § 3 StromStG series lives in `grid-billing`, the one source `eeg-billing`
/// reads as well. The reliefs from it are **Entlastungen** (§ 9b StromStG),
/// which the customer claims from the Hauptzollamt and which never change what
/// a supplier invoices — see [`crate::steuer`].
pub use grid_billing::stromsteuer::{
    STROMSTEUER_REGELSATZ_CT_PER_KWH, STROMSTEUER_SERIE, stromsteuer_for_year,
};

// ── Energiesteuer Erdgas (§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG) ────────────────

/// The Erdgas-als-Heizstoff rate: **0,55 ct/kWh_Hs** — „für 1 MWh Erdgas und
/// 1 MWh gasförmige Kohlenwasserstoffe 5,50 EUR".
pub const ENERGIESTEUER_GAS_CT_PER_KWH: Decimal = dec!(0.55);

/// The § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG series.
///
/// Starts in 2006, the first year of the EnergieStG (in force 01.08.2006);
/// ends with 2027, the last year the reading in its `quelle` reaches: § 2
/// Abs. 3 carries no Befristung and no verkündete Änderung for 2027, and a
/// later year is not read. The time-limited Fassungen
/// of § 68 EnergieStG (2022 and 2026) reach § 2 Abs. 1 motor fuels only, never
/// § 2 Abs. 3 heating gas.
pub const ENERGIESTEUER_GAS_SERIE: &[SatzZeile] = &[SatzZeile {
    von_jahr: 2006,
    bis_jahr: 2027,
    ct_per_kwh: ENERGIESTEUER_GAS_CT_PER_KWH,
    quelle: "§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG (5,50 EUR/MWh), gelesen auf \
             gesetze-im-internet.de am 2026-10-05; Abs. 3 ohne Befristung und \
             ohne verkündete Änderung für 2027",
}];

/// The § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG heating-gas rate (ct/kWh_Hs) for a
/// calendar year, or `None` outside [`ENERGIESTEUER_GAS_SERIE`].
///
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
/// assert_eq!(energiesteuer_gas_for_year(2028), None); // not yet read for 2028
/// ```
#[must_use]
pub fn energiesteuer_gas_for_year(year: i32) -> Option<Decimal> {
    zeile_fuer(ENERGIESTEUER_GAS_SERIE, year).map(|z| z.ct_per_kwh)
}

/// Compute BEHG ct/kWh_Hs for a calendar year from the § 10 BEHG price table.
///
/// Returns `None` when the year has no tabled price or no EBeV emission
/// factor. From 2027 no statutory price exists; the caller resolves one
/// through a [`BehgRegime`] or refuses the period.
///
/// # Example
/// ```rust
/// use energy_billing::rates::behg_ct_per_kwh_for_year;
/// // 2024: 45 EUR/t × 0.18139464 kg/kWh_Hs ÷ 10
/// let ct = behg_ct_per_kwh_for_year(2024).unwrap();
/// assert!(ct > rust_decimal::dec!(0.81) && ct < rust_decimal::dec!(0.82));
/// assert!(behg_ct_per_kwh_for_year(2027).is_none());
/// ```
#[must_use]
pub fn behg_ct_per_kwh_for_year(year: i32) -> Option<Decimal> {
    behg_eur_per_t_for_year(year).and_then(|eur_per_t| behg_ct_per_kwh_from_price(eur_per_t, year))
}

/// The § 10 BEHG price (EUR/t) tabled for a year, if any.
#[must_use]
pub fn behg_eur_per_t_for_year(year: i32) -> Option<Decimal> {
    BEHG_EUR_PER_T
        .iter()
        .find(|(y, _)| *y == year)
        .map(|(_, eur_per_t)| Decimal::from(*eur_per_t))
}

/// The first year the BEHG prices: § 10 Abs. 2 Satz 2 Nr. 1 BEHG opens the
/// Festpreise „im Zeitraum vom 1. Januar 2021". A period before it carries no
/// CO₂ component — zero, not missing.
pub const BEHG_ERSTES_JAHR: i32 = 2021;

/// The legal basis of a tabled BEHG year, for the invoice trace.
#[must_use]
pub fn behg_rechtsgrundlage_for_year(year: i32) -> Option<&'static str> {
    match year {
        ..BEHG_ERSTES_JAHR => {
            Some("kein Brennstoffemissionshandel vor 2021 (§ 10 Abs. 2 Satz 2 Nr. 1 BEHG)")
        }
        2021..=2025 => Some("§ 10 Abs. 2 Satz 2 BEHG (Festpreis)"),
        2026 => Some(
            "§ 4 Abs. 1 Nr. 2 CO2KostAufG (Mittelwert des Preiskorridors nach § 10 Abs. 2 Satz 4 BEHG)",
        ),
        _ => None,
    }
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

// ── The 2027 BEHG price regime ───────────────────────────────────────────────

/// Which law prices nEHS certificates in 2027 — a deployment choice, because
/// the two candidates are incompatible and only one can bind.
///
/// | Regime | Legal basis | Supplied 2027 price |
/// |---|---|---|
/// | [`RvoBezugspreis`](Self::RvoBezugspreis) | § 10 Abs. 3 Satz 2 Nr. 4/Nr. 5 BEHG in force: the sale price „in jedem Quartal des Jahres 2027" is the „mengengewichteten Durchschnittspreis der Versteigerungen von Berechtigungen nach § 10 des Treibhausgas-Emissionshandelsgesetzes in dem jeweils vorletzten vorangegangenen Quartal" | any positive price; nothing to bound it by |
/// | [`Korridor`](Self::Korridor) | § 10 Abs. 2 Satz 4 BEHG as the 3. BEHG-ÄndG (BT-Drs. 21/7869, Art. 1 Nr. 4 a) would word it: „Für die Jahre 2026 und 2027 wird ein Preiskorridor mit einem Mindestpreis von 55 Euro … und einem Höchstpreis von 65 Euro … festgelegt" | inside 55–65 EUR/t |
///
/// `RvoBezugspreis` is the default because it is the law in force; the
/// corridor is a Gesetzentwurf. Neither regime carries a 2027 price: no
/// statute fixes one in advance, so a 2027 Gas period refuses until a price
/// is supplied, under either.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehgRegime {
    /// § 10 Abs. 3 Satz 2 Nr. 4/Nr. 5 BEHG — the enacted Verordnungsermächtigung.
    #[default]
    RvoBezugspreis,
    /// The 55–65 EUR/t corridor of the 3. BEHG-ÄndG (BT-Drs. 21/7869).
    Korridor,
}

/// Mindestpreis of the corridor regime — BT-Drs. 21/7869, Art. 1 Nr. 4 a).
pub const BEHG_KORRIDOR_MIN_EUR_PER_T: Decimal = dec!(55);
/// Höchstpreis of the corridor regime — BT-Drs. 21/7869, Art. 1 Nr. 4 a).
pub const BEHG_KORRIDOR_MAX_EUR_PER_T: Decimal = dec!(65);

impl BehgRegime {
    /// The legal basis an invoice trace names for a price set under this regime.
    #[must_use]
    pub const fn rechtsgrundlage(self) -> &'static str {
        match self {
            Self::RvoBezugspreis => {
                "§ 10 Abs. 3 Satz 2 Nr. 4/Nr. 5 BEHG (Bezugspreis der TEHG-Versteigerungen)"
            }
            Self::Korridor => "§ 10 Abs. 2 Satz 4 BEHG i. d. F. des 3. BEHG-ÄndG (BT-Drs. 21/7869)",
        }
    }

    /// Check a supplied price for a year with no tabled § 10 BEHG price.
    ///
    /// # Errors
    ///
    /// The price is not positive, or — under [`Korridor`](Self::Korridor) —
    /// lies outside 55–65 EUR/t.
    pub fn pruefe(self, eur_per_t: Decimal, year: i32) -> Result<(), RateRefusal> {
        if eur_per_t <= Decimal::ZERO {
            return Err(RateRefusal::new(
                "BEHG",
                year,
                format!("{eur_per_t} EUR/t ist kein Zertifikatspreis"),
            ));
        }
        if self == Self::Korridor
            && !(BEHG_KORRIDOR_MIN_EUR_PER_T..=BEHG_KORRIDOR_MAX_EUR_PER_T).contains(&eur_per_t)
        {
            return Err(RateRefusal::new(
                "BEHG",
                year,
                format!(
                    "{eur_per_t} EUR/t liegt außerhalb des Preiskorridors \
                     {BEHG_KORRIDOR_MIN_EUR_PER_T}–{BEHG_KORRIDOR_MAX_EUR_PER_T} EUR/t ({})",
                    self.rechtsgrundlage()
                ),
            ));
        }
        Ok(())
    }

    /// The refusal for a year with no tabled price and none supplied.
    #[must_use]
    pub fn fehlender_preis(self, year: i32) -> RateRefusal {
        let was = match self {
            Self::RvoBezugspreis => {
                "der mengengewichtete Durchschnittspreis der TEHG-Versteigerungen des \
                 vorletzten vorangegangenen Quartals"
            }
            Self::Korridor => "ein Zertifikatspreis innerhalb des Preiskorridors",
        };
        RateRefusal::new(
            "BEHG",
            year,
            format!(
                "kein gesetzlicher Preis für {year}; {was} ist nicht hinterlegt ({})",
                self.rechtsgrundlage()
            ),
        )
    }
}

// ── Refusal ──────────────────────────────────────────────────────────────────

/// Why no statutory rate is available for a period.
///
/// Carried in [`RegulatoryRates`] in place of a rate, and raised as
/// [`EngineError::RateRefused`](crate::EngineError::RateRefused) by the first
/// provider that needs it — so a Strom invoice is never refused for a missing
/// Gas levy, and a Gas invoice always is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{satz} {jahr}: {grund}")]
pub struct RateRefusal {
    /// The rate: `Stromsteuer`, `Energiesteuer Erdgas`, `BEHG`.
    pub satz: String,
    /// The calendar year the rate was asked for.
    pub jahr: i32,
    /// What is missing, and under which provision.
    pub grund: String,
}

impl RateRefusal {
    /// A refusal for `satz` in `jahr`.
    #[must_use]
    pub fn new(satz: &str, jahr: i32, grund: impl Into<String>) -> Self {
        Self {
            satz: satz.to_owned(),
            jahr,
            grund: grund.into(),
        }
    }

    /// The refusal for a year past (or before) a statutory series.
    #[must_use]
    pub fn ausserhalb_der_serie(satz: &str, series: &[SatzZeile], jahr: i32) -> Self {
        let (von, bis) = (
            series.iter().map(|z| z.von_jahr).min().unwrap_or(jahr),
            series.iter().map(|z| z.bis_jahr).max().unwrap_or(jahr),
        );
        Self::new(
            satz,
            jahr,
            format!(
                "kein gelesener Satz für {jahr}; die Reihe ist für {von}–{bis} belegt — \
                 den Satz für {jahr} aus dem Gesetz in der geltenden Fassung hinterlegen"
            ),
        )
    }
}

/// A rate as resolved for one period: known, or refused with its reason.
pub type ResolvedRate = Result<Decimal, RateRefusal>;

// ── RegulatoryRates ───────────────────────────────────────────────────────────

/// The statutory rates resolved for one billing period.
///
/// Each per-kWh tax is a [`ResolvedRate`]: the rate, or the reason there is
/// none for the period. The provider that bills the tax refuses on the
/// reason; nothing substitutes another year's rate.
///
/// [`Default`] is the **2026** set — the rates for an example or a fixture
/// dated 2026, not a fallback for any other year.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegulatoryRates {
    /// § 3 StromStG — ct/kWh (see [`stromsteuer_for_year`]).
    pub stromsteuer_ct_per_kwh: ResolvedRate,
    /// § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG, Erdgas als Heizstoff — ct/kWh_Hs
    /// (see [`energiesteuer_gas_for_year`]).
    pub energiesteuer_gas_ct_per_kwh: ResolvedRate,
    /// BEHG CO₂ cost component for Erdgas — ct/kWh_Hs
    /// = CO₂-Preis EUR/t × Emissionsfaktor kg_CO₂/kWh_Hs ÷ 10
    /// (see [`erdgas_emissionsfaktor_kg_per_kwh`]).
    pub behg_gas_ct_per_kwh: ResolvedRate,
    /// The legal basis the BEHG price was set under — named on the CO₂ line
    /// and in its trace.
    pub behg_rechtsgrundlage: String,
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
            stromsteuer_ct_per_kwh: Ok(STROMSTEUER_REGELSATZ_CT_PER_KWH),
            energiesteuer_gas_ct_per_kwh: Ok(ENERGIESTEUER_GAS_CT_PER_KWH),
            // 60 EUR/t — § 4 Abs. 1 Nr. 2 CO2KostAufG bills the *Mittelwert* of
            // the § 10 Abs. 2 BEHG 55–65 corridor, not its Höchstpreis —
            // × 0.18139464 kg_CO₂/kWh_Hs ÷ 10
            behg_gas_ct_per_kwh: Ok(dec!(1.08836784)),
            behg_rechtsgrundlage: behg_rechtsgrundlage_for_year(2026)
                .unwrap_or_default()
                .to_owned(),
            mwst_rate: dec!(0.19),
            mwst_rate_reduced: dec!(0.07),
        }
    }
}

impl RegulatoryRates {
    /// The statutory per-kWh taxes for a calendar year, straight from the
    /// series: Stromsteuer, Energiesteuer Erdgas and the tabled § 10 BEHG
    /// price, each refused where the year has no row. The VAT fields are the
    /// [`Default`] ones; a caller resolving a real period sets them.
    #[must_use]
    pub fn for_year(year: i32) -> Self {
        let regime = BehgRegime::default();
        Self {
            stromsteuer_ct_per_kwh: stromsteuer_for_year(year).ok_or_else(|| {
                RateRefusal::ausserhalb_der_serie("Stromsteuer", STROMSTEUER_SERIE, year)
            }),
            energiesteuer_gas_ct_per_kwh: energiesteuer_gas_for_year(year).ok_or_else(|| {
                RateRefusal::ausserhalb_der_serie(
                    "Energiesteuer Erdgas",
                    ENERGIESTEUER_GAS_SERIE,
                    year,
                )
            }),
            behg_gas_ct_per_kwh: behg_ct_per_kwh_for_year(year)
                .or_else(|| (year < BEHG_ERSTES_JAHR).then_some(Decimal::ZERO))
                .ok_or_else(|| regime.fehlender_preis(year)),
            behg_rechtsgrundlage: behg_rechtsgrundlage_for_year(year)
                .unwrap_or(regime.rechtsgrundlage())
                .to_owned(),
            ..Self::default()
        }
    }

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

    /// Effective Stromsteuer — a product `override_ct` wins, else the period's
    /// statutory rate.
    ///
    /// # Errors
    ///
    /// No override and no statutory rate for the period.
    pub fn effective_stromsteuer(
        &self,
        override_ct: Option<Decimal>,
    ) -> Result<Decimal, crate::EngineError> {
        resolve(override_ct, &self.stromsteuer_ct_per_kwh)
    }

    /// Effective Energiesteuer Erdgas — a product `override_ct` wins, else the
    /// period's statutory rate.
    ///
    /// # Errors
    ///
    /// No override and no statutory rate for the period.
    pub fn effective_energiesteuer_gas(
        &self,
        override_ct: Option<Decimal>,
    ) -> Result<Decimal, crate::EngineError> {
        resolve(override_ct, &self.energiesteuer_gas_ct_per_kwh)
    }

    /// Effective BEHG CO₂ component — a product `override_ct` wins **only
    /// where the period has a lawful price**.
    ///
    /// Unlike the two taxes, an override does not stand in for a missing
    /// BEHG price: a product price for a year no law has priced yet is the
    /// same unsourced figure the refusal exists to stop, with a product name
    /// on it.
    ///
    /// # Errors
    ///
    /// The period has no lawful BEHG price, override or not.
    pub fn effective_behg_gas(
        &self,
        override_ct: Option<Decimal>,
    ) -> Result<Decimal, crate::EngineError> {
        let statutory = self
            .behg_gas_ct_per_kwh
            .clone()
            .map_err(crate::EngineError::RateRefused)?;
        Ok(override_ct.unwrap_or(statutory))
    }
}

/// Override wins; else the resolved rate, or its refusal as an engine error.
fn resolve(
    override_ct: Option<Decimal>,
    rate: &ResolvedRate,
) -> Result<Decimal, crate::EngineError> {
    match override_ct {
        Some(o) => Ok(o),
        None => rate.clone().map_err(crate::EngineError::RateRefused),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026 bills the corridor **midpoint**, not its ceiling.
    ///
    /// § 4 Abs. 1 Nr. 2 CO2KostAufG: „Dem **Mittelwert** des Preiskorridors nach
    /// § 10 Absatz 2 Satz 4 des Brennstoffemissionshandelsgesetzes". § 10 Abs. 2
    /// BEHG sets that corridor at „einem Mindestpreis von 55 Euro … und einem
    /// Höchstpreis von 65 Euro", so the price a supplier must bill is 60 EUR/t.
    /// Taking 65 — the price a supplier may *pay* at auction — overcharged the
    /// CO₂ line of every 2026 gas invoice by 8.33 %.
    #[test]
    fn behg_year_table_2026_is_the_corridor_midpoint_not_the_ceiling() {
        let ct = behg_ct_per_kwh_for_year(2026).unwrap();
        // 60 EUR/t × 0.18139464 kg/kWh_Hs ÷ 10 = 1.08836784 ct/kWh
        let expected = dec!(60) * dec!(0.18139464) / dec!(10);
        assert_eq!(ct, expected);
        assert_eq!(ct, dec!(1.08836784));
        assert_ne!(
            ct,
            dec!(65) * dec!(0.18139464) / dec!(10),
            "65 EUR/t is the § 10 Abs. 2 BEHG Höchstpreis, not the § 4 Abs. 1 \
             Nr. 2 CO2KostAufG Mittelwert"
        );
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

    /// Each series ends at the last year the statute was read for, so the
    /// year after it has no rate. Answering it with the last row's rate bills
    /// a year nobody checked as if somebody had.
    #[test]
    fn the_tax_series_end_where_they_were_read() {
        let last_strom = STROMSTEUER_SERIE.iter().map(|z| z.bis_jahr).max().unwrap();
        let last_gas = ENERGIESTEUER_GAS_SERIE
            .iter()
            .map(|z| z.bis_jahr)
            .max()
            .unwrap();
        assert_eq!(stromsteuer_for_year(last_strom), Some(dec!(2.05)));
        assert_eq!(stromsteuer_for_year(last_strom + 1), None);
        assert_eq!(energiesteuer_gas_for_year(last_gas), Some(dec!(0.55)));
        assert_eq!(energiesteuer_gas_for_year(last_gas + 1), None);
        assert_eq!(stromsteuer_for_year(2002), None);
        assert_eq!(energiesteuer_gas_for_year(2005), None);
        for z in STROMSTEUER_SERIE.iter().chain(ENERGIESTEUER_GAS_SERIE) {
            assert!(
                z.quelle.contains("gelesen"),
                "a row names when it was read: {z:?}"
            );
        }
    }

    /// A year outside the series is refused by the provider that bills it,
    /// naming the tax and the year — and a product override still wins.
    #[test]
    fn an_unread_year_refuses_and_an_override_still_wins() {
        let rates = RegulatoryRates::for_year(2028);
        let err = rates
            .effective_stromsteuer(None)
            .expect_err("2028 is unread");
        let msg = err.to_string();
        assert!(msg.contains("Stromsteuer") && msg.contains("2028"), "{msg}");
        assert_eq!(err.code(), "RATE_REFUSED");
        assert!(rates.effective_energiesteuer_gas(None).is_err());
        assert_eq!(
            rates.effective_stromsteuer(Some(dec!(1.5))).unwrap(),
            dec!(1.5)
        );
        assert_eq!(
            RegulatoryRates::for_year(2026)
                .effective_stromsteuer(None)
                .unwrap(),
            dec!(2.05)
        );
    }

    /// 2027 has no § 10 BEHG price, and a product override does not stand in
    /// for one: it is the same unsourced figure with a product name on it.
    #[test]
    fn a_2027_behg_price_is_refused_override_or_not() {
        let rates = RegulatoryRates::for_year(2027);
        let err = rates
            .effective_behg_gas(None)
            .expect_err("2027 has no price");
        assert!(err.to_string().contains("2027"), "{err}");
        assert!(rates.effective_behg_gas(Some(dec!(1.2))).is_err());
        assert_eq!(
            RegulatoryRates::for_year(2024)
                .effective_behg_gas(Some(dec!(0.99)))
                .unwrap(),
            dec!(0.99)
        );
    }

    /// The corridor regime bounds a supplied price; the RVO regime has
    /// nothing to bound it by.
    #[test]
    fn the_corridor_regime_bounds_the_price_and_the_rvo_regime_does_not() {
        let k = BehgRegime::Korridor;
        assert!(k.pruefe(dec!(54), 2027).is_err());
        assert!(k.pruefe(dec!(66), 2027).is_err());
        assert!(k.pruefe(dec!(55), 2027).is_ok());
        assert!(k.pruefe(dec!(60), 2027).is_ok());
        assert!(k.pruefe(dec!(65), 2027).is_ok());
        let r = BehgRegime::RvoBezugspreis;
        assert!(r.pruefe(dec!(80), 2027).is_ok());
        assert!(r.pruefe(dec!(0), 2027).is_err());
        assert_eq!(BehgRegime::default(), BehgRegime::RvoBezugspreis);
        assert!(
            r.fehlender_preis(2027)
                .grund
                .contains("vorletzten vorangegangenen Quartals")
        );
    }

    /// Before 2021 there was no Brennstoffemissionshandel: the CO₂ component
    /// of a 2020 correction is zero, not a refusal and not today's price.
    #[test]
    fn a_period_before_the_behg_carries_no_co2_component() {
        assert_eq!(
            RegulatoryRates::for_year(2020)
                .effective_behg_gas(None)
                .unwrap(),
            Decimal::ZERO
        );
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
/// Three families of Stichtag exist:
///
/// - **Umsatzsteuer** — the COVID window (01.07.2020–31.12.2020, 16 %) for every
///   commodity, plus the gas/Fernwärme window (01.10.2022–31.03.2024, 7 %,
///   §28 Abs. 5/6 UStG) for `GAS` and `WAERME`.
/// - **BEHG** — the CO₂ price of §10 BEHG steps at each calendar-year boundary,
///   so a `GAS` period spanning a year-end has two levy rates. A Fernwärme
///   period does not: its CO₂ cost is the heat product's own CO2KostAufG § 3
///   figure, which no calendar boundary moves.
/// - **End of a series** — a 1 January after which a per-year rate (BEHG,
///   Energiesteuer Erdgas for `GAS`; Stromsteuer for the electricity
///   categories) has no row: the leg after it refuses rather than inherit
///   the year before's rate.
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
    // A year whose price is not tabled is a boundary too: billing the leg
    // after it at the earlier year's price is the substitution the refusal
    // exists to stop, so the period splits and the untabled leg refuses on
    // its own. The same holds where a tax series ends.
    if category == "GAS" {
        stichtage.extend(jahresgrenzen(from, to, behg_ct_per_kwh_for_year));
        stichtage.extend(jahresgrenzen(from, to, energiesteuer_gas_for_year));
    }
    if STROMSTEUER_KATEGORIEN.contains(&category) {
        stichtage.extend(jahresgrenzen(from, to, stromsteuer_for_year));
    }

    stichtage.sort_unstable();
    stichtage.dedup();
    stichtage
}

/// The product categories whose providers bill § 3 StromStG.
const STROMSTEUER_KATEGORIEN: &[&str] = &[
    "STROM",
    "WAERMEPUMPE",
    "WALLBOX",
    "SOLAR",
    "HEMS",
    "EMOBILITY",
    "SHARING",
];

/// Each 1 January inside `(from, to]` where a per-year rate changes or where
/// either side has none.
fn jahresgrenzen(
    from: time::Date,
    to: time::Date,
    rate: fn(i32) -> Option<Decimal>,
) -> Vec<time::Date> {
    ((from.year() + 1)..=to.year())
        .filter_map(|year| {
            let jan1 = time::Date::from_calendar_date(year, time::Month::January, 1).ok()?;
            let (before, after) = (rate(year - 1), rate(year));
            (jan1 > from && jan1 <= to && (before.is_none() || after.is_none() || before != after))
                .then_some(jan1)
        })
        .collect()
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

    /// Where the § 3 StromStG series ends, the year after has no rate; a
    /// period running into it splits so that leg refuses on its own.
    #[test]
    fn the_end_of_the_stromsteuer_series_splits_an_electricity_period() {
        let last = STROMSTEUER_SERIE.iter().map(|z| z.bis_jahr).max().unwrap();
        let von = time::Date::from_calendar_date(last, time::Month::December, 1).unwrap();
        let bis = time::Date::from_calendar_date(last + 1, time::Month::January, 31).unwrap();
        let jan1 = time::Date::from_calendar_date(last + 1, time::Month::January, 1).unwrap();
        assert_eq!(steuer_stichtage_im_zeitraum("STROM", von, bis), vec![jan1]);
        assert!(steuer_stichtage_im_zeitraum("WASSER", von, bis).is_empty());
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

    /// A year with no tabled § 10 BEHG price is a boundary: the leg after it
    /// cannot be billed at the year before's price, so the period splits there
    /// and that leg refuses on its own.
    #[test]
    fn a_year_the_behg_table_does_not_cover_is_a_stichtag() {
        let last_tabled = BEHG_EUR_PER_T
            .iter()
            .map(|(y, _)| *y)
            .max()
            .expect("the § 10 BEHG table is not empty");
        let von = time::Date::from_calendar_date(last_tabled, time::Month::December, 1)
            .expect("1 December exists");
        let bis = time::Date::from_calendar_date(last_tabled + 1, time::Month::January, 31)
            .expect("31 January exists");
        let jan1 = time::Date::from_calendar_date(last_tabled + 1, time::Month::January, 1)
            .expect("1 January exists");
        assert_eq!(steuer_stichtage_im_zeitraum("GAS", von, bis), vec![jan1]);
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
