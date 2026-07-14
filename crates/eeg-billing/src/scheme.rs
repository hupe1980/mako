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
    /// §21 EEG — Fixed **Einspeisevergütung** paid by NB to Anlagenbetreiber.
    ///
    /// Formula: `kwh × verguetungssatz_ct / 100`
    FeedInTariff {
        /// Net feed-in tariff rate in ct/kWh (gross AW − §53 EEG deduction).
        /// Fixed at commissioning for the full 20-year Förderdauer.
        verguetungssatz_ct: Decimal,
    },

    /// §21 Abs. 1 Nr. 2 EEG — **Ausfallvergütung** (temporary feed-in tariff).
    ///
    /// Same formula as `FeedInTariff` but at the statutory reduced rate (typically
    /// 80 % of the normal Vergütungssatz). Caller must supply the already-reduced rate.
    TemporaryFeedInTariff {
        /// Reduced rate ct/kWh per §21 Abs. 1 Nr. 2 EEG 2023.
        verguetungssatz_ct: Decimal,
    },

    /// §20 EEG — **Gleitende Marktprämie**.
    ///
    /// Formula: `max(0, eff_AW − marktwert) × kwh / 100`
    /// where `eff_AW = direktverm_aw_ct × wind_korrekturfaktor + managementpraemie_ct`.
    ///
    /// `marktwert_ct_kwh` (context field on `SettleInput`) provides the market reference
    /// price. Use `TariffSource::Auction(…)` for BNetzA tender plants — same formula,
    /// different AW source and billing-position label.
    MarketPremium {
        /// Anzulegender Wert in ct/kWh — statutory or BNetzA-tendered.
        /// For Ausschreibungsanlagen: the tender-awarded value.
        direktverm_aw_ct: Decimal,

        /// §20 Abs. 3 EEG 2023 Managementprämie in ct/kWh.
        /// `None` → auto-computed from `SettleInput.leistung_kwp`
        /// (0.4 ct/kWh for ≤100 MW, 0.2 ct/kWh for >100 MW).
        managementpraemie_ct: Option<Decimal>,

        /// §36k EEG — certified wind-onshore Korrekturfaktor.
        /// Multiplied into `direktverm_aw_ct` before computing the spread.
        /// Takes precedence over `wind_standort` when both are set.
        wind_korrekturfaktor: Option<Decimal>,

        /// §36k EEG — wind site quality model for auto-deriving `korrekturfaktor`.
        /// Ignored when `wind_korrekturfaktor` is explicitly set.
        wind_standort: Option<crate::wind::WindStandort>,
    },

    /// §38a EEG 2023 — **Mieterstrom** surcharge on top of FeedInTariff.
    ///
    /// Formula: `kwh × (verguetungssatz_ct + mieter_zuschlag_ct) / 100`
    TenantElectricity {
        /// Base Vergütung rate in ct/kWh.
        verguetungssatz_ct: Decimal,
        /// §38a Mieterstrom-Zuschlag in ct/kWh (on top of base rate).
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

    /// §7 KWKG 2023 — **KWK-Zuschlag** for combined heat-and-power plants.
    ///
    /// Formula: `eligible_kwh × verguetungssatz_ct / 100`
    /// `eligible_kwh` is prorated when the §8 KWKG hour-limit is approached.
    KwkSurcharge {
        /// KWK-Zuschlag rate in ct/kWh (§7 Abs. 1 KWKG 2023).
        verguetungssatz_ct: Decimal,
        /// Cumulative kWh already paid in prior periods (for §8 KWKG hour-limit).
        /// `None` → no hour-limit enforcement.
        kwh_paid_gesamt: Option<Decimal>,
        /// Maximum total eligible kWh = rated_kW_el × kwk_foerderdauer_h.
        /// `None` → no hour-limit cap applied.
        max_kwh: Option<Decimal>,
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

    /// §38a EEG — **Eigenverbrauch**: self-consumption, no grid feed-in payment.
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
    /// Does NOT apply to `MarketPremium`/`PostEeg` (market risk borne by Direktvermarkter),
    /// `KwkSurcharge`, `Eigenverbrauch`, or `SonstigeDirektvermarktung`.
    #[must_use]
    pub fn negativpreis_rule_applicable(&self) -> bool {
        matches!(
            self,
            Self::FeedInTariff { .. }
                | Self::TenantElectricity { .. }
                | Self::TemporaryFeedInTariff { .. }
                | Self::FlexibilityPremium { .. }
        )
    }

    /// Returns `true` for schemes where §53b regional Grünstromkennzeichnung reduction applies.
    #[must_use]
    pub fn sect53b_applicable(&self) -> bool {
        matches!(
            self,
            Self::FeedInTariff { .. }
                | Self::TenantElectricity { .. }
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
    /// Rate is fixed for the 20-year Förderdauer. Quarterly degression applies
    /// from commissioning month (§23a EEG 2023 — not computed here; caller provides
    /// the net rate in `direktverm_aw_ct` / `verguetungssatz_ct`).
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
/// - Innovation auctions (§39j EEG 2023) have additional technology bonuses.
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
    /// Innovation auction (§39j EEG 2023) — additional technology bonus applies.
    pub innovation_auction: bool,
    /// **§22b EEG 2023** — Bürgerenergiegesellschaft (citizen energy cooperative).
    ///
    /// Qualifying cooperatives receive special treatment:
    /// - Exemption from certain Pönalen (§55 EEG 2023)
    /// - Preferential bidding conditions in auctions (§36g EEG 2023)
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
    /// §38a Mieterstrom rules for the remaining Förderdauer.
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
/// Tracking the settlement type is essential for §22 MessZV-compliant bookkeeping.
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
