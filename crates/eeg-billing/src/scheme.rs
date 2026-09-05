//! Clean settlement scheme architecture — the "what" and "where" of EEG remuneration.
//!
//! This module separates three orthogonal dimensions that EEG billing depends on:
//!
//! | Dimension | Type | Question |
//! |---|---|---|
//! | **Scheme** | [`SettlementScheme`] | *How* is remuneration determined? |
//! | **Tariff source** | [`TariffSource`] | *Where* does the anzulegender Wert come from? |
//! | **Settlement type** | [`SettlementType`] | *Is this* initial, correction, or reversal? |
//!
//! ## Why this separation matters
//!
//! The `SettlementScheme + TariffSource` split separates these dimensions:
//!
//! - `Ausschreibung` is not a settlement *scheme* — it determines the AW via BNetzA tender.
//!   The *scheme* is still Marktprämie (§20 EEG); only the AW source changes.
//! - `Flexibilitaet`/`FlexibilitaetZuschlag` are *adjustments* layered on top of
//!   the main scheme, not independent settlement schemes.
//!
//! The new architecture models these dimensions separately and independently.

use crate::version::EegGesetz;
use rust_decimal::Decimal;
use time::Date;

// ── SettlementScheme ──────────────────────────────────────────────────────────

/// Settlement scheme with **embedded parameters** — the formula *and* its inputs.
///
/// Each variant carries exactly the parameters meaningful for that scheme.
/// Shared context (plant data, sanctions, metering) lives in [`crate::model::SettleInput`].
///
/// ## Design rationale
///
/// The data-bearing enum eliminates an entire class of bugs: it is now impossible to
/// construct a `SettleInput` with `kwk_max_kwh` set for a `FeedInTariff` plant,
/// or with `direktverm_aw_ct` absent for a `MarketPremium` plant. The compiler
/// enforces scheme-parameter consistency at build time.
///
/// `marktwert_ct_kwh` remains a context field on `SettleInput` because it is
/// cross-scheme: used in `MarketPremium` spread, `PostEeg` payment,
/// `SanktionAlt::VerguetungAufMarktwert`, and `§44b` excess pricing.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(tag = "scheme", rename_all = "SCREAMING_SNAKE_CASE")
)]
pub enum SettlementScheme {
    /// §21 Abs. 1 Satz 1 Nr. 1 EEG — the **Einspeisevergütung** mit gesetzlich
    /// bestimmtem anzulegenden Wert, paid by the NB to the Anlagenbetreiber.
    ///
    /// Formula: `kwh × verguetungssatz_ct / 100`
    ///
    /// **The claim ends at 100 kW installierter Leistung.** Nr. 1 grants it only
    /// „für Strom aus Anlagen mit einer installierten Leistung von bis zu 100
    /// Kilowatt", so a larger plant assigned to this scheme is owed nothing — see
    /// [`crate::direktverm::direktvermarktungspflicht`]. The engine does not
    /// enforce it, because this enum names a *formula* and the Veräußerungsform a
    /// plant is actually assigned to is register data: the caller checks it and
    /// reports [`SettlementStatus::KeinAnspruch`](crate::SettlementStatus::KeinAnspruch).
    FeedInTariff {
        /// Net feed-in tariff rate in ct/kWh (gross AW − §53 EEG deduction).
        /// Fixed at commissioning for the full 20-year Förderdauer.
        verguetungssatz_ct: Decimal,
    },

    /// §21 Abs. 1 Satz 1 Nr. 3 EEG — **Ausfallvergütung**.
    ///
    /// The fallback a plant above 100 kW falls back to when its Direktvermarkter
    /// drops out: the same formula as [`FeedInTariff`](Self::FeedInTariff), but
    /// §53 Abs. 3 reduces the anzulegender Wert by **20 %**.
    ///
    /// Supply the plant's **ordinary** rate — the engine applies the reduction
    /// and rounds to two decimals. Left to the caller, the ordinary rate passes
    /// straight through and the one scheme that exists for a plant in trouble
    /// pays it 25 % more than the statute allows.
    ///
    /// §21 Abs. 1 Satz 1 Nr. 3 also caps the Inanspruchnahme at **three
    /// consecutive calendar months and six calendar months per calendar year**;
    /// exceeding either is a §52 Abs. 1 Nr. 5 Pflichtverstoß, which the caller
    /// detects (it needs the settlement history) and passes as a
    /// [`Pflichtverstoss`](crate::Pflichtverstoss).
    TemporaryFeedInTariff {
        /// The plant's ordinary rate in ct/kWh, **before** the §53 Abs. 3 cut.
        verguetungssatz_ct: Decimal,
    },

    /// §23a EEG i.V.m. Anlage 1 — **Gleitende Marktprämie**.
    ///
    /// Formula (Anlage 1 Nr. 3.1.2 / 4.1.2): `MP = max(0, AW − MW)`, settled as
    /// `MP × kwh / 100`, where `AW = direktverm_aw_ct × wind_korrekturfaktor`.
    ///
    /// There is **no additive Managementprämie**. Anlage 1 defines `MP = AW – MW`
    /// and nothing else; §20 EEG 2023 has no Absätze at all, let alone the
    /// "+0,4 ct" one. Since EEG 2014 the marketing cost is folded *into* the
    /// anzulegender Wert — its mirror image is the §53 Abs. 1 deduction of
    /// 0,4 / 0,2 ct that the Einspeisevergütung route takes off the same AW.
    ///
    /// `marktwert_ct_kwh` (context field on `SettleInput`) provides the market reference
    /// price. Use `TariffSource::Auction(…)` for BNetzA tender plants — same formula,
    /// different AW source and billing-position label.
    MarketPremium {
        /// Anzulegender Wert in ct/kWh — statutory or BNetzA-tendered.
        /// For Ausschreibungsanlagen: the tender-awarded value.
        direktverm_aw_ct: Decimal,

        /// §36h EEG — certified wind-onshore Korrekturfaktor.
        /// Multiplied into `direktverm_aw_ct` before computing the spread.
        /// Takes precedence over `wind_standort` when both are set.
        wind_korrekturfaktor: Option<Decimal>,

        /// §36h EEG — wind site quality model for auto-deriving `korrekturfaktor`.
        /// Ignored when `wind_korrekturfaktor` is explicitly set.
        wind_standort: Option<crate::wind::WindStandort>,
    },

    /// §21 Abs. 3 EEG 2023 — **Mieterstrom** surcharge on top of FeedInTariff.
    ///
    /// Formula: `kwh × (verguetungssatz_ct + mieter_zuschlag_ct) / 100`
    TenantElectricity {
        /// Base Vergütung rate in ct/kWh.
        verguetungssatz_ct: Decimal,
        /// §21 Abs. 3 Mieterstrom-Zuschlag in ct/kWh (on top of base rate).
        mieter_zuschlag_ct: Option<Decimal>,
    },

    /// §21 EEG post-Förderung — plant fed in at **market spot reference price**.
    ///
    /// Formula: `kwh × marktwert_ct_kwh / 100` (no floor; negative EPEX → plant pays).
    /// §23b EEG 2023 cap: market price capped at 10 ct/kWh for ausgeförderte Anlagen.
    ///
    /// `marktwert_ct_kwh` (context field on `SettleInput`) provides the EPEX spot price.
    PostEeg {
        /// Optional price floor in ct/kWh. Contract-defined; not a statutory rule.
        /// `None` = full market exposure.
        /// `Some(0)` = operator cannot be charged for negative EPEX.
        /// `Some(x)` = contract-defined minimum (e.g. bilateral agreement).
        price_floor: Option<Decimal>,
    },

    /// § 7 KWKG — **KWK-Zuschlag** for combined heat-and-power plants.
    ///
    /// Formula: `eligible_kwh × verguetungssatz_ct / 100`, where `eligible_kwh`
    /// is bounded by both § 8 limits: the lifetime Vollbenutzungsstunden of
    /// Abs. 1–3 and the Abs. 4 cap on the calendar year.
    KwkSurcharge {
        /// KWK-Zuschlag rate in ct/kWh.
        ///
        /// § 7 prices per Leistungsanteil, so this is the blended Mischsatz from
        /// [`crate::kwkg::zuschlag_ct_kwh`], not one band's rate.
        verguetungssatz_ct: Decimal,
        /// Cumulative kWh already paid over the plant's life (§ 8 Abs. 1–3).
        /// `None` → no lifetime limit enforced.
        kwh_paid_gesamt: Option<Decimal>,
        /// Lifetime kWh limit = `kwk_leistung_kw × Vollbenutzungsstunden`
        /// (§ 8 Abs. 1–3). `None` → no lifetime cap applied.
        max_kwh: Option<Decimal>,
        /// § 8 Abs. 4 — the kWh still payable in this calendar year: the year's
        /// `kwk_leistung_kw × Jahreshöchstbetrag` less what the year has already
        /// been paid for.
        ///
        /// `None` → the annual cap is not enforced. It binds independently of the
        /// lifetime limit, and it is the one that decides what a single year can
        /// be paid.
        jahres_restkontingent_kwh: Option<Decimal>,
    },

    /// §50b EEG 2023 — **Flexibilitätsprämie** for *existing* biomass plants.
    ///
    /// Formula: `kwh × (verguetungssatz_ct + flex_praemie_ct_kwh) / 100`
    FlexibilityPremium {
        /// Base Vergütung rate in ct/kWh.
        verguetungssatz_ct: Decimal,
        /// Flexibilitätsprämie rate in ct/kWh (§50b EEG 2023 + Anlage 3).
        flex_praemie_ct_kwh: Option<Decimal>,
    },

    /// §50a EEG 2023 — **Flexibilitätszuschlag** for *new* biomass plants.
    ///
    /// Capacity-based payment: `€100/kW/year ÷ 12` per month (kWh-independent).
    /// Formula: `leistung_kwp_flex × rate_eur_per_kw_year / 12`
    FlexibilitySurcharge {
        /// Annual capacity payment rate in EUR/kW/year (statutory: 100 EUR/kW/year).
        /// Note: this is EUR/kW/year, NOT ct/kWh.
        rate_eur_per_kw_year: Decimal,
    },

    /// §21 Abs. 3 EEG — **Eigenverbrauch**: self-consumption, no grid feed-in payment.
    ///
    /// Formula: EUR 0 always. No NB payment.
    Eigenverbrauch,

    /// §21a EEG 2023 — **Sonstige Direktvermarktung**: direct third-party sale.
    ///
    /// No EEG payment from NB. Records the period in settlement history.
    SonstigeDirektvermarktung,
}

impl Default for SettlementScheme {
    fn default() -> Self {
        Self::FeedInTariff {
            verguetungssatz_ct: Decimal::ZERO,
        }
    }
}

impl SettlementScheme {
    /// Returns `true` for schemes that require a market reference price (`marktwert_ct_kwh`).
    #[must_use]
    pub fn requires_marktwert(&self) -> bool {
        matches!(self, Self::MarketPremium { .. } | Self::PostEeg { .. })
    }

    /// Returns `true` for schemes that pay remuneration based on feed-in kWh.
    #[must_use]
    pub fn is_kwh_based(&self) -> bool {
        !matches!(
            self,
            Self::FlexibilitySurcharge { .. }
                | Self::Eigenverbrauch
                | Self::SonstigeDirektvermarktung
        )
    }

    /// Returns `true` when §51 Negativpreisregel potentially applies to this scheme.
    ///
    /// §51 Abs. 1 reduces *the anzulegender Wert* to zero, and the AW is what
    /// Anlage 1 Nr. 1 feeds into `MP = AW − MW` ("der anzulegende Wert unter
    /// Berücksichtigung der §§ 19 bis 54"). The Marktprämie is therefore §51's
    /// primary object, not an exception to it.
    ///
    /// Does NOT apply to `PostEeg` (no AW left to reduce — the plant is
    /// ausgefördert), `KwkSurcharge` (KWKG, a different law), `Eigenverbrauch`,
    /// `SonstigeDirektvermarktung` (§21a: no EEG payment at all), or
    /// `FlexibilitySurcharge` (§50a is capacity- not energy-based).
    #[must_use]
    pub fn negativpreis_rule_applicable(&self) -> bool {
        matches!(
            self,
            Self::FeedInTariff { .. }
                | Self::MarketPremium { .. }
                | Self::TenantElectricity { .. }
                | Self::TemporaryFeedInTariff { .. }
                | Self::FlexibilityPremium { .. }
        )
    }

    /// Return the `verguetungssatz_ct` for schemes that have a fixed tariff rate.
    /// Returns `None` for market-based or capacity-based schemes.
    #[must_use]
    pub fn verguetungssatz_ct(&self) -> Option<Decimal> {
        match self {
            Self::FeedInTariff { verguetungssatz_ct }
            | Self::TemporaryFeedInTariff { verguetungssatz_ct }
            | Self::TenantElectricity {
                verguetungssatz_ct, ..
            }
            | Self::KwkSurcharge {
                verguetungssatz_ct, ..
            }
            | Self::FlexibilityPremium {
                verguetungssatz_ct, ..
            } => Some(*verguetungssatz_ct),
            _ => None,
        }
    }
}

// ── TariffSource ──────────────────────────────────────────────────────────────

/// How the **Anzulegender Wert (AW)** was determined for a plant.
///
/// The AW is the statutory or tendered rate that drives the Marktprämie spread
/// and serves as the reference for all other payment types.
///
/// This is *orthogonal* to [`SettlementScheme`]: the same `MarketPremium` scheme
/// can be used for both statutory-AW plants (`Statutory`) and BNetzA tender plants
/// (`Auction`). Only the AW source — and the billing position label — differ.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "source")
)]
pub enum TariffSource {
    /// §21 EEG — Statutory AW, set by law at commissioning date (§48 EEG 2023).
    ///
    /// Rate is fixed for the 20-year Förderdauer. For solar, the §49 EEG 2023
    /// semi-annual degression selects the value from the commissioning date —
    /// see [`crate::rates::solar_pv_ueberschuss_aw_ct`]. The caller supplies the
    /// resolved rate in `direktverm_aw_ct` / `verguetungssatz_ct`.
    Statutory,

    /// §§22a, 28 EEG — BNetzA **tender award**: AW set by sealed-bid auction.
    ///
    /// The award value (`award_ct` in `AusschreibungMetadata`) replaces the
    /// statutory AW for the full Förderdauer. Subsequent statutory degression
    /// does NOT apply to Ausschreibungsanlagen.
    Auction(AusschreibungMetadata),

    /// §100 EEG — **Transitional rule**: old plant uses old AW from prior EEG version.
    ///
    /// Plants commissioned before 01.01.2023 may settle under the rules of the
    /// EEG version in force at commissioning, not EEG 2023.
    /// The specific rule is identified by [`Paragraph100Rule`].
    Transitional(Paragraph100Rule),
}

#[allow(clippy::derivable_impls)]
impl Default for TariffSource {
    fn default() -> Self {
        Self::Statutory
    }
}

impl TariffSource {
    /// Returns `true` for BNetzA tender plants.
    #[must_use]
    pub fn is_auction(&self) -> bool {
        matches!(self, Self::Auction(_))
    }

    /// Returns `true` for plants using the §100 Übergangsregelung.
    #[must_use]
    pub fn is_transitional(&self) -> bool {
        matches!(self, Self::Transitional(_))
    }

    /// Returns `true` for §51b biogas Ausschreibungsanlagen.
    ///
    /// When `true`, §51/§51a do NOT apply, and the AW is zero for periods
    /// where `epex_avg_ct_kwh ≤ 2 ct/kWh` (§51b EEG 2023).
    #[must_use]
    pub fn is_biogas_sect51b(&self) -> bool {
        matches!(self, Self::Auction(m) if m.is_biogas_sect51b)
    }

    /// Returns `true` for §39n Innovationsausschreibung awards.
    ///
    /// These plants receive a **fixed** market premium (feste Marktprämie =
    /// the Zuschlagswert per kWh, §3 InnAusV) rather than the *gleitende*
    /// Marktprämie `max(0, AW − Marktwert)` — so the payout does not shrink as
    /// the Monatsmarktwert rises.
    #[must_use]
    pub fn is_innovation_auction(&self) -> bool {
        matches!(self, Self::Auction(m) if m.innovation_auction)
    }
}

// ── AusschreibungMetadata ─────────────────────────────────────────────────────

/// BNetzA tender auction metadata for Ausschreibungsanlagen.
///
/// Stores the full lifecycle of the BNetzA award from tender to possible expiry.
/// This data is needed because auction plants have special rules:
///
/// - The AW is the `award_ct`, NOT the statutory rate from §48 EEG.
/// - A second tender is required when the first award expires (§33 EEG 2023).
/// - Bürgerenergiegesellschaften have reduced requirements (§22b EEG 2023).
/// - Innovationsausschreibungen (§39n EEG 2023) pay a fixed rather than a
///   sliding market premium.
/// - Biogas auction plants use §51b rules (AW = 0 when EPEX ≤ 2 ct/kWh).
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AusschreibungMetadata {
    /// BNetzA Zuschlag-ID (e.g. `"SEE-2024-001234"`).
    pub zuschlag_id: Option<String>,
    /// Tendered AW in ct/kWh (the legally binding value from the tender result).
    pub award_ct: Option<Decimal>,
    /// Date of the BNetzA tender award notification.
    pub award_date: Option<Date>,
    /// Whether the award has expired (§33 EEG 2023: unbuilt plant after N years).
    pub award_expired: bool,
    /// Innovationsausschreibung (§39n EEG 2023) — fixed market premium instead of
    /// the sliding one, awarded for combinations of installation and storage.
    pub innovation_auction: bool,
    /// **§22b EEG 2023** — Bürgerenergiegesellschaft (§3 Nr. 15).
    ///
    /// Such a plant is exempt from the requirement of a *valid Zuschlag*
    /// (§22 Abs. 2 Satz 2 Nr. 3 for Wind an Land, §22 Abs. 3 Satz 2 Nr. 2 for
    /// Solaranlagen), so it is settled at the statutory rate despite falling in
    /// an auction-eligible size class. The exemption is conditional on
    /// notification to the Bundesnetzagentur within three weeks and on the
    /// company having commissioned no other plant of the same kind in the
    /// preceding three years; §22b Abs. 4 requires the status to be re-proven to
    /// the Netzbetreiber every five years.
    pub is_buergerenergie: bool,
    /// **§51b EEG 2023** — Biogas Ausschreibungsanlage with slightly-positive price rule.
    ///
    /// For biogas plants (excluding biomethane) whose AW was determined by auction:
    /// the AW reduces to **zero** when `epex_avg_ct_kwh ≤ 2 ct/kWh`.
    /// **§51 and §51a do NOT apply** to these plants (§51b Satz 2 EEG 2023).
    ///
    /// Legal basis: §51b EEG 2023.
    /// Source: EEG 2023, Clearingstelle EEG|KWKG Working Text 23.12.2025.
    pub is_biogas_sect51b: bool,
}

// ── Paragraph100Rule ──────────────────────────────────────────────────────────

/// §100 EEG 2023 — Übergangsbestimmungen (transition rules).
///
/// Plants commissioned before 01.01.2023 often settle under the rules of the
/// EEG version in force when they were commissioned (§100 Abs. 1 EEG 2023).
/// This enum identifies which specific §100 subparagraph applies.
///
/// ## Important caveat
///
/// §100 EEG 2023 has 36+ numbered subsections. This enum covers the most
/// commonly encountered transition rules. For plant types not covered here,
/// the caller must determine the applicable rule and supply the corresponding
/// `verguetungssatz_ct` and `eeg_gesetz` directly.
///
/// Per §100 Abs. 1 EEG 2023, the applicable rules are determined by the
/// transition provisions in force at the time — not a single universal rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Paragraph100Rule {
    /// §100 Abs. 1 EEG 2023: old plants (commissioned before 01.01.2023) keep the
    /// EEG rules as of 31.12.2022 (= EEG 2021 rules) for the remaining Förderdauer.
    OldPlantBeforeEeg2023,

    /// §100 Abs. 2 Nr. 13 EEG 2021: EEG 2017 plants keep the 6-hour §51 threshold
    /// (not EEG 2021's 4-hour threshold) per §100 EEG 2021 Abs. 2 Nr. 13.
    ///
    /// Used for plants commissioned 2016-01-01 to 2020-12-31.
    Eeg2017Negativpreis6h,

    /// §100 Abs. 3 EEG 2023: biomass transition — plants with biomass fuel
    /// changes after 01.01.2023 may use transitional fuel-class rules.
    BiomassTransition,

    /// §100 Abs. 9 EEG 2023: Solarpaket I transitional — plants whose legal
    /// classification changed under the Solarpaket I (BGBl I 2024 Nr. 107)
    /// amendments. Use for Balkonkraftwerk / Stecker-PV reclassifications.
    SolarpaketITransition,

    /// §100 Abs. 1 Satz 4 EEG 2017 Bestandsschutz: plants commissioned before 01.01.2016
    /// are permanently exempt from §51 Negativpreisregel.
    Pre2016Bestandsschutz,

    /// §100 KWKG: KWKG plants use the transitional rule from KWKG 2017 → 2023.
    KwkgTransition,

    /// §100 Abs. 6 EEG 2023: biomass plants that received their EEG support under
    /// old §42–§44 rules continue at their original rates and with original fuel-class
    /// restrictions for the remainder of their Förderdauer.
    ///
    /// Commonly applies to solid-biomass and biogas plants commissioned 2012–2020.
    BiomassOldFuelClassContinuation,

    /// §100 Abs. 7 EEG 2023: hydropower plants that underwent ecological improvements
    /// retain extended Förderdauer from the modernization date rather than the
    /// original commissioning date.
    HydropowerEcologicalModernization,

    /// §100 Abs. 11 EEG 2023: small biomass plants (≤150 kW) that are not subject
    /// to mandatory Direktvermarktung continue under old EEG 2017 feed-in tariff rules.
    SmallBiomassBelow150kw,

    /// §100 Abs. 15/16 EEG 2023: auction-built plants whose commissioning deadline
    /// falls under transitional provisions receive extended Pönalen grace periods.
    AuctionPoenalTransition,

    /// §100 Abs. 26 EEG 2023: Solarpaket I — existing Mieterstrom buildings reclassified
    /// to Gemeinschaftliche Gebäudeversorgung (§42b) may continue under the old
    /// §21 Abs. 3 Mieterstrom rules for the remaining Förderdauer.
    MieterstromToGgvTransition,

    /// §100 Abs. 2 Nr. 4 EEG 2021: EEG 2012/2014 plants retain the old §23 Abs. 4
    /// degression schedule (not EEG 2017 §49 quarterly degression).
    Eeg2012DegressionSchedule,
}

impl Paragraph100Rule {
    /// Returns the [`EegGesetz`] version implied by this §100 transition rule.
    ///
    /// When `Some`, `calculate_settlement` uses this version for §51/§52 dispatch
    /// **instead of** the caller-supplied `SettleInput.eeg_gesetz`, preventing
    /// silent miscalculation when a `Transitional` rule is set without the
    /// matching `eeg_gesetz` being updated.
    ///
    /// Returns `None` for rules that do not imply a specific EEG version — the
    /// caller's `eeg_gesetz` is then used as-is.
    ///
    /// | `Paragraph100Rule` | Implied `EegGesetz` | Reason |
    /// |---|---|---|
    /// | `Pre2016Bestandsschutz` | `Eeg2012` | §100 Abs. 1 Satz 4 EEG 2017 — §51 exempt forever |
    /// | `Eeg2017Negativpreis6h` | `Eeg2017` | 6h threshold, 500kW/3MW exemption |
    /// | `BiomassOldFuelClassContinuation` | `Eeg2017` | old §42–§44 fuel rules |
    /// | `SmallBiomassBelow150kw` | `Eeg2017` | small biomass keeps EEG 2017 FiT |
    /// | `OldPlantBeforeEeg2023` | `Eeg2021` | §100 Abs. 1 EEG 2023 → EEG 2021 rules |
    /// | all others | `None` | caller's `eeg_gesetz` applies |
    #[must_use]
    pub fn implied_eeg_gesetz(self) -> Option<EegGesetz> {
        match self {
            // §100 Abs. 1 Satz 4 EEG 2017: plants commissioned before 01.01.2016 are
            // permanently exempt from §51 Negativpreisregel.
            Self::Pre2016Bestandsschutz => Some(EegGesetz::Eeg2012),
            // EEG 2017 plants: 6h consecutive-hour threshold,
            // wind <3 MW exempt / other <500 kW exempt (§51 Abs. 3 EEG 2017).
            Self::Eeg2017Negativpreis6h
            | Self::BiomassOldFuelClassContinuation
            | Self::SmallBiomassBelow150kw => Some(EegGesetz::Eeg2017),
            // §100 Abs. 1 EEG 2023: old plants keep rules as of 31.12.2022
            // = EEG 2021 rules (4h threshold, 500 kW exemption, all types).
            Self::OldPlantBeforeEeg2023 => Some(EegGesetz::Eeg2021),
            // All other rules: caller's eeg_gesetz applies.
            _ => None,
        }
    }
}

// ── SettlementType ────────────────────────────────────────────────────────────

/// Whether this is an initial settlement, correction, or reversal.
///
/// DSOs perform settlement corrections and retroactive adjustments frequently:
/// corrected meter readings, changed tariffs, regulatory reprocessing.
/// Tracking the settlement type is essential for § 147 AO / GoBD-compliant bookkeeping.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SettlementType {
    /// First settlement for this plant / billing period.
    Initial,
    /// Correction of a prior settlement (replaces the original).
    ///
    /// `original_id` references the `settlement_receipt.id` being corrected.
    Correction {
        /// ID of the original settlement receipt being corrected.
        original_id: String,
        /// Reason for the correction (for audit trail).
        reason: CorrectionReason,
    },
    /// Complete reversal of a prior settlement (cancels the original to EUR 0).
    ///
    /// Used for regulatory revocations, MaStR retroactive deregistrations, etc.
    Reversal {
        /// ID of the original settlement receipt to reverse.
        original_id: String,
    },
}

#[allow(clippy::derivable_impls)]
impl Default for SettlementType {
    fn default() -> Self {
        Self::Initial
    }
}

/// Reason for a settlement correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum CorrectionReason {
    /// Corrected meter reading (Zählernachlesung).
    MeterDataCorrected,
    /// Tariff correction (wrong rate applied).
    TariffCorrected,
    /// MaStR registration retroactively confirmed (§52 sanction removed).
    MastrRegistrationConfirmed,
    /// Plant capacity correction (wrong kWp applied).
    CapacityCorrected,
    /// Regulatory reprocessing (BNetzA ruling changed billing basis).
    RegulatoryReprocessing,
    /// Foerderendedatum corrected (§25 Abs. 1 Satz 2 date recalculated).
    FoerderendedatumCorrected,
    /// Other/manual correction.
    Other,
}

// ── Marktwertserie (Anlage 1 Nr. 2) ──────────────────────────────────────────

/// Which of the two Marktwert series Anlage 1 Nr. 2 EEG 2023 gives a plant.
///
/// The Marktprämie is `max(0, AW − MW)`, and using the wrong `MW` misprices
/// every kWh — so the choice is the plant's vintage, never the operator's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Marktwertserie {
    /// Anlage 1 Nr. 3 — the energieträgerspezifische **Monats**marktwert. Final
    /// when published, and per calendar month.
    Monatsmarktwert,
    /// Anlage 1 Nr. 4 — the energieträgerspezifische **Jahres**marktwert. It has
    /// no month, and the binding figure exists only once the year is over; the
    /// ÜNB publish a running estimate before that.
    Jahresmarktwert,
}

impl Marktwertserie {
    /// The stored/wire token.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Monatsmarktwert => "MONATSMARKTWERT",
            Self::Jahresmarktwert => "JAHRESMARKTWERT",
        }
    }

    /// Parse a stored/wire token.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "MONATSMARKTWERT" => Some(Self::Monatsmarktwert),
            "JAHRESMARKTWERT" => Some(Self::Jahresmarktwert),
            _ => None,
        }
    }
}

/// Anlage 1 Nr. 2 EEG 2023 — which Marktwert series a plant's Marktprämie is
/// computed from.
///
/// Satz 1 sends plants „die vor dem 1. Januar 2023 in Betrieb genommen worden
/// sind **oder** deren Zuschlag vor dem 1. Januar 2023 erteilt worden ist" to
/// the Monatsmarktwert; Satz 2 sends „Strom aus anderen Anlagen" to the
/// Jahresmarktwert. Satz 3 then moves a Satz-1 plant onto the Jahresmarktwert
/// too, „wenn der Anspruch nach der Abgrenzungs- oder der Pauschaloption nach
/// § 19 Absatz 3b oder 3c geltend gemacht wird".
///
/// `zuschlag_datum` is the BNetzA award date where the plant has one; a plant
/// commissioned in 2024 on a 2022 award takes the **Monats**marktwert, which is
/// the case a bare Inbetriebnahme test gets wrong.
///
/// # Example
///
/// ```rust
/// use eeg_billing::{Marktwertserie, marktwertserie};
/// use time::macros::date;
///
/// // Commissioned 2021 — Satz 1.
/// assert_eq!(
///     marktwertserie(date!(2021-06-01), None, false),
///     Marktwertserie::Monatsmarktwert
/// );
/// // Commissioned 2024 on a 2022 Zuschlag — still Satz 1.
/// assert_eq!(
///     marktwertserie(date!(2024-06-01), Some(date!(2022-11-01)), false),
///     Marktwertserie::Monatsmarktwert
/// );
/// // Commissioned 2024, no earlier award — Satz 2.
/// assert_eq!(
///     marktwertserie(date!(2024-06-01), None, false),
///     Marktwertserie::Jahresmarktwert
/// );
/// // A Satz-1 plant claiming under §19 Abs. 3b/3c — Satz 3.
/// assert_eq!(
///     marktwertserie(date!(2021-06-01), None, true),
///     Marktwertserie::Jahresmarktwert
/// );
/// ```
#[must_use]
pub fn marktwertserie(
    inbetriebnahme: Date,
    zuschlag_datum: Option<Date>,
    speicher_abgrenzungs_oder_pauschaloption: bool,
) -> Marktwertserie {
    if speicher_abgrenzungs_oder_pauschaloption {
        return Marktwertserie::Jahresmarktwert;
    }
    let vor_2023 = |d: Date| d < ANLAGE1_NR2_STICHTAG;
    if vor_2023(inbetriebnahme) || zuschlag_datum.is_some_and(vor_2023) {
        Marktwertserie::Monatsmarktwert
    } else {
        Marktwertserie::Jahresmarktwert
    }
}

/// Anlage 1 Nr. 2 Satz 1 EEG 2023 — „vor dem 1. Januar 2023".
pub const ANLAGE1_NR2_STICHTAG: Date = time::macros::date!(2023 - 01 - 01);

// ── MarktpreisKategorie ───────────────────────────────────────────────────────

/// Technology-specific EPEX monthly market value (Marktwert) category.
///
/// The BNetzA publishes separate Marktwert tables per technology type each month.
/// For Direktvermarktung, the correct Marktwert must be used — using the wrong
/// category produces incorrect Marktprämie calculations.
///
/// ## Source
/// BNetzA Marktwert data portal: <https://www.bundesnetzagentur.de/EEG-Marktwerte>
///
/// ## Billing note
/// The EPEX monthly average (`epex_avg_ct_kwh`) in `SettleInput` should match
/// the Marktwert category appropriate for the plant's `ErzeugungsArt`.
/// This enum serves as documentation and validation aid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MarktpreisKategorie {
    /// Marktwert Solar (PV) — published monthly by BNetzA.
    Solar,
    /// Marktwert Wind onshore — published monthly by BNetzA.
    WindOnshore,
    /// Marktwert Wind offshore — published monthly by BNetzA.
    WindOffshore,
    /// Marktwert Biomasse (biogenic feedstocks including biogas, biomethane).
    Biomasse,
    /// Marktwert Wasserkraft.
    Wasserkraft,
    /// Marktwert Geothermie / sonstige EE.
    Sonstige,
    /// EPEX Day-Ahead monthly average (used for PostEEG ausgeförderte Anlagen).
    EpexDayAhead,
}
