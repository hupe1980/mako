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

use rust_decimal::Decimal;
use time::Date;

// ── SettlementScheme ──────────────────────────────────────────────────────────

/// How remuneration for a feed-in plant is computed (§21, §20, §38a, §7 EEG/KWKG).
///
/// `SettlementScheme + TariffSource` provides clean separation of concerns:
/// the **scheme** determines the *formula* used; the **tariff source** ([`TariffSource`])
/// determines how the *anzulegender Wert* is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SettlementScheme {
    /// §21 EEG — Fixed **Einspeisevergütung** paid by NB to Anlagenbetreiber.
    ///
    /// Formula: `kwh × verguetungssatz_ct / 100`
    #[default]
    ///
    /// Rate is fixed at commissioning for the full 20-year Förderdauer.
    /// Use `tariff_source = TariffSource::Statutory` for normal plants or
    /// `TariffSource::Transitional(rule)` for §100 Übergangsregelung plants.
    FeedInTariff,

    /// §20 EEG — **Gleitende Marktprämie**: NB pays the spread between
    /// Anzulegender Wert and EPEX monthly average, plus Managementprämie.
    ///
    /// Formula: `max(0, AW − EPEX) × kwh / 100 + Managementprämie × kwh / 100`
    ///
    /// `tariff_source = TariffSource::Statutory` for statutory AW plants;
    /// `tariff_source = TariffSource::Auction(meta)` for BNetzA tender plants.
    /// Both use the same formula — only the AW source and position label differ.
    MarketPremium,

    /// §38a EEG — **Mieterstrom** surcharge on top of FeedInTariff.
    ///
    /// Formula: `kwh × (verguetungssatz_ct + mieter_zuschlag_ct) / 100`
    ///
    /// The base rate is the statutory FeedInTariff for the capacity class;
    /// the Mieterstrom-Zuschlag is an additional ct/kWh on top.
    TenantElectricity,

    /// §21 EEG post-Förderung — plant feeds in at **EPEX monthly spot average**.
    ///
    /// Formula: `kwh × epex_avg_ct_kwh / 100` (no floor; negative EPEX → plant pays)
    ///
    /// Triggered when `billing_date > foerderendedatum`.
    /// §23b EEG 2023 cap: EPEX capped at 10 ct/kWh for ausgeförderte Anlagen.
    PostEeg,

    /// §7 KWKG 2023 — **KWK-Zuschlag** for combined heat-and-power plants.
    ///
    /// Formula: `eligible_kwh × verguetungssatz_ct / 100`
    ///
    /// `eligible_kwh` is prorated when the §8 KWKG hour-limit is approached.
    KwkSurcharge,

    /// §21 Abs. 1 Nr. 2 EEG — **Ausfallvergütung** (failsafe tariff).
    ///
    /// Temporary fallback when Direktvermarktung is not yet possible or has
    /// ended. Same formula as `FeedInTariff` but at a *reduced rate*:
    /// typically 80% of the statutory Vergütungssatz (§21 Abs. 1 Nr. 2 EEG 2023).
    ///
    /// Applies for a maximum of 3 consecutive months. The billing system should
    /// flag and escalate plants remaining in Ausfallvergütung beyond that period.
    FailsafeTariff,

    /// §38a EEG — **Eigenverbrauch** (self-consumption): no grid feed-in payment.
    ///
    /// Formula: EUR 0 always. No payment from NB.
    ///
    /// Used for plants with 100% self-consumption (Eigenversorgung).
    Eigenverbrauch,

    /// §50b EEG 2023 — **Flexibilitätsprämie** for *existing* biomass plants.
    ///
    /// Formula: `kwh × (verguetungssatz_ct + flex_praemie_ct_kwh) / 100`
    ///
    /// The base Vergütung is paid alongside the flexibility premium.
    /// Only for biomass/biogas plants already receiving FeedInTariff that
    /// install additional flexible peak capacity (§50b EEG 2023 + Anlage 3).
    FlexibilityPremium,

    /// §50a EEG 2023 — **Flexibilitätszuschlag** for *new* biomass plants.
    ///
    /// A capacity-based payment of €100/kW/year for new biomass plants commissioned
    /// with >50% additional flexible installed capacity (§50a EEG 2023 + Anlage 2).
    ///
    /// Formula: `leistung_kwp × rate / 12` (monthly capacity payment, kWh-independent).
    FlexibilitySurcharge,
}

impl SettlementScheme {
    /// Returns `true` for schemes that require an EPEX monthly average price.
    #[must_use]
    pub fn requires_epex_price(self) -> bool {
        matches!(self, Self::MarketPremium | Self::PostEeg)
    }

    /// Returns `true` for schemes that pay a Vergütung based on feed-in kWh.
    #[must_use]
    pub fn is_kwh_based(self) -> bool {
        !matches!(self, Self::FlexibilitySurcharge | Self::Eigenverbrauch)
    }

    /// Returns `true` when §51 Negativpreisregel potentially applies.
    ///
    /// §51 reduces Vergütung to zero for negative-price intervals.
    /// Does NOT apply to MarketPremium/PostEeg (market risk borne by Direktvermarkter)
    /// or KwkSurcharge.
    #[must_use]
    pub fn negativpreis_rule_applicable(self) -> bool {
        matches!(
            self,
            Self::FeedInTariff
                | Self::TenantElectricity
                | Self::FailsafeTariff
                | Self::FlexibilityPremium
        )
    }

    /// Returns `true` for schemes where §53b regional reduction can apply.
    #[must_use]
    pub fn sect53b_applicable(self) -> bool {
        matches!(
            self,
            Self::FeedInTariff | Self::TenantElectricity | Self::FlexibilityPremium
        )
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
}

// ── AusschreibungMetadata ─────────────────────────────────────────────────────

/// BNetzA tender auction metadata for Ausschreibungsanlagen.
///
/// Stores the full lifecycle of the BNetzA award from tender to possible expiry.
/// This data is needed because auction plants have special rules:
///
/// - The AW is the `award_ct`, NOT the statutory rate from §48 EEG.
/// - A second tender is required when the first award expires (§33 EEG 2023).
/// - Citizen energy cooperatives have reduced requirements (§36g EEG 2023).
/// - Innovation auctions (§39j EEG 2023) have additional technology bonuses.
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
    /// Citizen energy (§36g EEG 2023) — reduced requirements.
    pub citizen_energy: bool,
}

// ── Paragraph100Rule ──────────────────────────────────────────────────────────

/// §100 EEG 2023 — Übergangsbestimmungen (transition rules).
///
/// Plants commissioned before 01.01.2023 often settle under the rules of the
/// EEG version in force when they were commissioned (§100 Abs. 1 EEG 2023).
/// This enum identifies which specific §100 subparagraph applies.
///
/// ## Legal context
///
/// §100 EEG 2023 has many sub-paragraphs addressing:
/// - Plants commissioned under EEG 2012 / 2014 / 2017 / 2021
/// - Specific technology transitions (biomass, offshore)
/// - Solarpaket I amendments
/// - KWKG transition rules
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    /// §66 EEG 2017 Satz 4 Bestandsschutz: plants commissioned before 01.01.2016
    /// are permanently exempt from §51 Negativpreisregel.
    Pre2016Bestandsschutz,

    /// §100 KWKG: KWKG plants use the transitional rule from KWKG 2017 → 2023.
    KwkgTransition,
}

// ── SettlementType ────────────────────────────────────────────────────────────

/// Whether this is an initial settlement, correction, or reversal.
///
/// DSOs perform settlement corrections and retroactive adjustments frequently:
/// corrected meter readings, changed tariffs, regulatory reprocessing.
/// Tracking the settlement type is essential for §22 MessZV-compliant bookkeeping.
#[derive(Debug, Clone, PartialEq)]
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
/// BNetzA Marktwert data portal: https://www.bundesnetzagentur.de/EEG-Marktwerte
///
/// ## Billing note
/// The EPEX monthly average (`epex_avg_ct_kwh`) in `SettleInput` should match
/// the Marktwert category appropriate for the plant's `ErzeugungsArt`.
/// This enum serves as documentation and validation aid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
