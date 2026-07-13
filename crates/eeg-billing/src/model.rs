//! Settlement model types — the input/output contract for [`calculate_settlement`].
//!
//! [`calculate_settlement`]: crate::calculate_settlement

use crate::technology::ErzeugungsArt;
use crate::version::EegGesetz;
use rust_decimal::Decimal;
use time::Date;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// ── Enums ─────────────────────────────────────────────────────────────────────

// ── SanktionsTyp / Pflichtverstoss ────────────────────────────────────────────

/// §52 EEG ≤2021 (old regime) — sanction tier reducing the Vergütung.
///
/// Three distinct tiers based on §52 EEG 2021/2017 (via §100 Übergangsregelung).
/// For EEG 2023 plants, use [`Pflichtverstoss`] instead (separate €10/kW/month penalty).
///
/// ## Legal basis: §52 EEG 2021
///
/// ```text
/// Abs. 1: verringert sich auf null           → VerguetungAufNull
/// Abs. 2: verringert sich auf den Marktwert  → VerguetungAufMarktwert
/// Abs. 3: verringert sich um 20 Prozent      → VerguetungReduziert20Prozent
/// ```
///
/// ## §52 Abs. 3 rounding (EEG 2021)
/// "wobei das Ergebnis auf zwei Stellen nach dem Komma gerundet wird"
/// The 20% reduction result is rounded to 2 decimal places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SanktionAlt {
    /// §52 Abs. 1 EEG ≤2021: Vergütung verringert sich auf **null**.
    ///
    /// Applies to:
    /// - Nr. 1: MaStR not registered AND §71 Nr. 1 not done
    /// - Nr. 2: Capacity increase not reported AND §71 Nr. 1 not done
    /// - Nr. 2a: §10b Direktvermarktungspflicht violation
    /// - Nr. 3: §21b Abs. 2/3 violation (wrongful form change, 3 months)
    /// - Nr. 4: §27a violation for Ausschreibungsanlagen (full calendar year)
    VerguetungAufNull,
    /// §52 Abs. 2 EEG ≤2021: Vergütung verringert sich auf den **Monatsmarktwert**
    /// (= EPEX monthly average ct/kWh, same price as `PostEegSpot`).
    ///
    /// Applies to:
    /// - Nr. 1: §9 Abs. 1/1a/2/5 violation (Fernsteuerbarkeit not installed)
    /// - Nr. 1a: §9 Abs. 8 violation (Messeinrichtung not installed)
    /// - Nr. 2: §21b/§21c notification not sent
    /// - Nr. 3: Ausfallvergütung Höchstdauer exceeded
    /// - Nr. 4: §21 Abs. 2 Einspeisevergütung violation
    /// - Nr. 5: §80 Doppelvermarktungsverbot violation
    ///
    /// Requires `epex_avg_ct_kwh` in `SettleInput`. Returns `PriceMissing` if absent.
    VerguetungAufMarktwert,
    /// §52 Abs. 3 EEG ≤2021: Vergütung verringert sich um **20 Prozent**
    /// (result rounded to 2 decimal places per §52 Abs. 3).
    ///
    /// Applies to:
    /// - Nr. 1: §71 Nr. 1 was done but MaStR registration data is incomplete
    /// - Nr. 2: Capacity increase not reported, but §71 Nr. 1 was done
    VerguetungReduziert20Prozent,
}

/// §52 EEG 2023 compliance violation type.
///
/// Each type triggers a payment obligation to the NB of €10/kW/month (§52 Abs. 2).
/// The obligation can be retroactively reduced to €2/kW/month once fulfilled (§52 Abs. 3).
///
/// Use [`crate::foerderdauer::calculate_pflichtzahlung_§52`] to compute the penalty.
///
/// ## EEG version note
///
/// §52 EEG 2023 applies to plants under current EEG 2023 rules.
/// For old plants (commissioned before 01.01.2023) under §100 Übergangsregelung,
/// the old §47 EEG 2021 "Vergütung = 0" rule applies instead — use `sanktion: Some(SanktionAlt::VerguetungAufNull)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SanktionsTyp {
    /// §52 Abs. 1 Nr. 1 — Missing Fernsteuerbarkeit (§9 Abs. 1/2).
    ///
    /// Plant ≥25 kW does not have remote control capability installed.
    /// Obligation fulfilled → reduced to €2/kW/month retroactively.
    /// Technical defect grace: 2 months waived.
    FernsteuerbarkeitmFehlend,

    /// §52 Abs. 1 Nr. 2 — Missing Speicher / §9 Abs. 5 violation.
    ///
    /// Plant does not meet the storage requirement for certain EE/KWK plants.
    ///
    /// **Rate: €10/kW/month** (§52 Abs. 2). Not in the §52 Abs. 3 Nr. 1 reduction list
    /// — this violation is NOT retroactively reducible to €2/kW.
    SpeicherAnforderungNichtErfuellt,

    /// §52 Abs. 1 Nr. 3 — Missing iMSys Messeinrichtung (§9 Abs. 8).
    ///
    /// Plant not equipped with the required intelligent measurement system infrastructure.
    IMssAnforderungNichtErfuellt,

    /// §52 Abs. 1 Nr. 4 — §10b Direktvermarktungspflicht not met.
    ///
    /// Plant > 100 kW required to be in Direktvermarktung but still uses Einspeisevergütung.
    DirektvermarktungspflichtVerletzt,

    /// §52 Abs. 1 Nr. 11 — Plant not registered in MaStR.
    ///
    /// Required registration data not submitted per Marktstammdatenregisterverordnung.
    /// Obligation fulfilled → reduced to €2/kW/month retroactively.
    ///
    /// **EEG 2023 change**: Old §47 EEG 2021 reduced Vergütung to EUR 0.
    /// §52 EEG 2023 instead charges €10/kW/month; Vergütung remains payable.
    /// Use `sanktion: Some(SanktionAlt::VerguetungAufNull)` for old plants (EEG ≤2021, §100 Übergangsregelung).
    MastrNichtRegistriert,

    /// §52 Abs. 1 Nr. 9a — Post-commissioning violation of §37a Abs. 1a or §48 Abs. 6.
    ///
    /// Plant violates the obligations that arise after commissioning under those paragraphs
    /// (§37a Abs. 1a: iMSys Nachrüstung after commissioning; §48 Abs. 6: solar Segment obligations).
    ///
    /// **Rate: always €2/kW/month** (§52 Abs. 3 Nr. 2 EEG 2023).
    /// This is a permanently lower rate — NOT reduced from €10; starts at €2 for this type.
    /// `nachtraeglich_erfuellt` has NO effect on this type.
    InbetriebnahmeVorgabeVerletzt,

    /// §52 Abs. 1 Nr. 10 — Volleinspeisung obligation violated (§48 Abs. 2a).
    ///
    /// Plant registered for Volleinspeisung (100% grid feed-in bonus, §48 Abs. 2a EEG 2023)
    /// but does not feed all generated electricity into the grid in a calendar year.
    ///
    /// **Rate: always €2/kW/month** (§52 Abs. 3 Nr. 2 EEG 2023).
    /// `nachtraeglich_erfuellt` has NO effect on this type.
    ///
    /// ## §52 Abs. 4 Nr. 3: calendar-year scope
    ///
    /// This violation is assessed for **all calendar months of the year** in which
    /// the under-delivery occurs (not just the months of non-delivery).
    /// Include all 12 months in `monate_des_verstosses`.
    VolleinspeisungspflichtVerletzt,
}

/// §52 EEG 2023 — Pflichtverstoss input for penalty calculation.
///
/// A compliance violation that triggers a payment obligation of €10/kW/month
/// from the plant operator to the NB (§52 Abs. 2 EEG 2023).
///
/// ## Penalty calculation
///
/// ```rust
/// use eeg_billing::Pflichtverstoss;
/// use eeg_billing::SanktionsTyp;
/// use eeg_billing::foerderdauer::calculate_pflichtzahlung;
/// use rust_decimal_macros::dec;
///
/// // Missing Fernsteuerbarkeit for 3 months, 500 kW plant, obligation not yet fulfilled
/// let violation = Pflichtverstoss {
///     typ: SanktionsTyp::FernsteuerbarkeitmFehlend,
///     leistung_kw: dec!(500),
///     monate_des_verstosses: 3,
///     nachtraeglich_erfuellt: false,
/// };
/// let penalty = calculate_pflichtzahlung(&violation);
/// assert_eq!(penalty, dec!(15000)); // 500 kW × 10 EUR × 3 months
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pflichtverstoss {
    /// Type of compliance violation.
    pub typ: SanktionsTyp,
    /// Installed capacity of the plant in kW (basis for €10/kW/month).
    pub leistung_kw: Decimal,
    /// Number of calendar months during which the violation is/was in effect.
    pub monate_des_verstosses: u32,
    /// Whether the obligation has since been fulfilled.
    ///
    /// When `true`, §52 Abs. 3 reduces the penalty retroactively to €2/kW/month
    /// for violation types Nr. 1, 3, 4, 11. Has no effect for Nr. 2.
    pub nachtraeglich_erfuellt: bool,
}

/// EEG/KWKG settlement model.
///
/// Determines which regulatory formula is applied during settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SettlementModel {
    /// §21 EEG — fixed Einspeisevergütung paid by NB to Anlagenbetreiber.
    ///
    /// Formula: `kwh × verguetungssatz_ct / 100`
    ///
    /// Rate fixed at commissioning for the full 20-year Förderdauer.
    /// Historical rates differ significantly by EEG year and technology:
    /// - EEG 2000 solar PV ≤30 kWp: 50.62 ct/kWh
    /// - EEG 2012 solar PV ≤10 kWp: 24.43 ct/kWh
    /// - EEG 2023 solar PV ≤10 kWp: 8.11 ct/kWh (initial) / 8.51 ct/kWh (after Solarpaket I 2024)
    /// Use `eeg_billing::rates::solar_pv_lookup()` or `einsd`'s rate table for
    /// the correct historical rate.
    Verguetung,

    /// §38a EEG — community solar Mieterstrom surcharge on top of Vergütung.
    ///
    /// Formula: `kwh × (verguetungssatz_ct + mieter_zuschlag_ct) / 100`
    ///
    /// Available only for plants commissioned under EEG 2017 or later.
    /// Maximum plant size: 100 kWp (§38a Abs. 3 EEG 2023).
    Mieterstrom,

    /// §20 EEG — Gleitende Marktprämie: NB pays the spread between
    /// Anzulegender Wert and EPEX monthly average, plus Managementprämie.
    ///
    /// Formula: `max(0, AW − EPEX) × kwh / 100 + managementpraemie_ct × kwh / 100`
    ///
    /// Mandatory for plants >100 kW commissioned after 01.01.2016.
    Direktvermarktung,

    /// §§22a, 28 EEG — BNetzA tender plants. Same formula as
    /// `Direktvermarktung`; the `direktverm_aw_ct` is the tender-awarded value.
    ///
    /// The Ausschreibungswert is set by BNetzA tender and does NOT automatically
    /// change with the statutory reference value degression.
    Ausschreibung,

    /// Post-20-year-Förderung: plant feeds in at EPEX monthly spot average.
    ///
    /// Formula: `kwh × epex_avg_ct_kwh / 100`
    ///
    /// No price floor — negative EPEX produces negative settlement (plant owes NB).
    /// Triggered automatically when `billing_date > foerderendedatum` if the
    /// original model was `Verguetung` or `Mieterstrom`.
    PostEegSpot,

    /// §38a EEG — self-consumption. No Einspeisevergütung is paid.
    ///
    /// Formula: EUR 0
    ///
    /// Used for plants with `Überschusseinspeisung` metering where the owner
    /// consumes the majority of generation on-site.
    Eigenverbrauch,

    /// §7 KWKG 2023 — KWK-Zuschlag for combined heat-and-power plants.
    ///
    /// Formula: `eligible_kwh × verguetungssatz_ct / 100`
    ///
    /// Subject to Förderdauer hour-limit enforcement (§8 KWKG 2023 for plants >2 MW):
    /// eligible kWh is prorated when `kwk_strom_kwh_gesamt + kwh > kwk_max_kwh`.
    KwkgZuschlag,

    /// §50b EEG — Flexibilitätsprämie for **existing** biomass plants (bestehende Anlagen).
    ///
    /// Formula: `kwh × (verguetungssatz_ct + flex_praemie_ct_kwh) / 100`
    ///
    /// Applies only to biomass and biogas plants already receiving Vergütung that
    /// install additional flexible peak capacity (§50b EEG 2023 + Anlage 3).
    Flexibilitaet,

    /// §50a EEG 2023 — Flexibilitätszuschlag for **new** biomass plants (neue Anlagen).
    ///
    /// A capacity-based payment of €100/kW/year for new biomass plants that are
    /// commissioned with additional flexible installed capacity (>50% of
    /// Bemessungsleistung as additional peak capacity).
    ///
    /// Distinct from §50b `Flexibilitaet`: §50a is for NEW plants, §50b for EXISTING.
    ///
    /// ## Input fields for this model
    ///
    /// - `leistung_kwp` = additional flexible capacity in kW (installed peak above base)
    /// - `verguetungssatz_ct` = annual rate in EUR/kW (statutory: 100 EUR/kW/year)
    /// - `einspeisemenge_kwh` = irrelevant — set to `Some(Decimal::ZERO)` or `None`
    ///
    /// ## Output
    ///
    /// One position with `eur = leistung_kwp × rate / 12` (monthly payment).
    FlexibilitaetZuschlag,
}

/// The metering concept (§2 Nr. 20 EEG 2023 / §3 MessZV).
///
/// Documents how Einspeisemenge is measured. Affects which tariff rules apply
/// and which MaLo/MeLo combination is used for billing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Messkonzept {
    /// **Volleinspeisung** — 100 % of generation is fed into the grid.
    ///
    /// Einspeisemenge = Erzeugungsmenge. No self-consumption occurs.
    /// Higher EEG tariff typically applies for Volleinspeisung
    /// (e.g. solar PV ≤10 kWp: 8.51 ct/kWh Volleinspeisung vs. 8.11 ct/kWh Überschuss, EEG 2023 initial).
    Volleinspeisung,

    /// **Überschusseinspeisung** — surplus after self-consumption is fed in.
    ///
    /// Einspeisemenge < Erzeugungsmenge.
    /// The Einspeisemenge measured by the bidirectional meter is what is billed.
    /// Slightly lower EEG tariff applies in some EEG versions.
    Ueberschusseinspeisung,

    /// **Direktlieferung** — direct delivery to a local customer (§42a EEG).
    ///
    /// Power goes directly to a nearby buyer without flowing through the grid.
    /// Used for Gemeinschaftliche Gebäudeversorgung and Mieterstrom models.
    Direktlieferung,
}

// ── CapacityBlock ─────────────────────────────────────────────────────────────

/// A single capacity block for §24 EEG Anlagenerweiterung (plant extension).
///
/// When an existing EEG plant is extended with additional capacity
/// (e.g. adding 5 kWp to an existing 10 kWp installation), the extension
/// receives its own:
/// - Feed-in tariff rate (the statutory rate at the **extension** date, which
///   is typically lower due to annual degression)
/// - 20-year Förderdauer starting from the extension commissioning date
///
/// The settlement engine allocates the measured Einspeisemenge proportionally
/// across all blocks by installed capacity (§24 Abs. 1 EEG 2023).
///
/// ## Zusammenlegung vs. Erweiterung
///
/// - **Zusammenlegung** (§24 EEG): two legally separate plants merged into one
///   entity. Both plants contribute their original rates and end dates.
///   Model via two `CapacityBlock`s.
///
/// - **Erweiterung**: capacity added to an existing plant at a later date.
///   New capacity block gets current statutory rate from extension date.
///   Model via one primary block (in `SettleInput`) + one `CapacityBlock`.
///
/// ## Example
///
/// ```rust
/// use eeg_billing::CapacityBlock;
/// use rust_decimal_macros::dec;
/// use time::macros::date;
///
/// // Original 10 kWp at 9.25 ct/kWh (EEG 2020)
/// let original = CapacityBlock {
///     leistung_kwp:     dec!(10),
///     verguetungssatz_ct: dec!(9.25),
///     inbetriebnahme:   date!(2020-03-15),
///     foerderendedatum: date!(2040-03-15),
/// };
///
/// // Extension: +5 kWp at 8.11 ct/kWh (EEG 2023)
/// let extension = CapacityBlock {
///     leistung_kwp:     dec!(5),
///     verguetungssatz_ct: dec!(8.11),
///     inbetriebnahme:   date!(2024-06-01),
///     foerderendedatum: date!(2044-06-01),
/// };
/// ```
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CapacityBlock {
    /// Installed capacity for this block in kWp (or kW_el for KWKG).
    pub leistung_kwp: Decimal,
    /// EEG feed-in tariff rate for this block in ct/kWh.
    ///
    /// Fixed at the commissioning date of **this block** for its full Förderdauer.
    pub verguetungssatz_ct: Decimal,
    /// Commissioning date for this block (Inbetriebnahmedatum).
    pub inbetriebnahme: Date,
    /// Subsidy end date for this block (`inbetriebnahme + 20 years`).
    ///
    /// When the billing period start date exceeds this, the block is expired
    /// and contributes EUR 0 (or EPEX spot price for `PostEegSpot` transition).
    pub foerderendedatum: Date,
}

// ── SettleInput ───────────────────────────────────────────────────────────────

/// Input for a single settlement period calculation.
///
/// All monetary rates are in **ct/kWh** (Cent per kWh), not EUR/kWh.
/// Supply `Default::default()` for fields not applicable to the model.
///
/// ## Multi-EEG-version support
///
/// EEG has been revised many times (2000, 2004, 2009, 2012, 2014, 2017, 2021, 2023).
/// The correct `verguetungssatz_ct` is fixed at the plant's commissioning date and
/// does not change over the 20-year Förderdauer.  Supply the rate that was valid
/// when the plant was commissioned — use `eeg_billing::rates` or `einsd`'s
/// rate lookup table for historical rates.
///
/// The formula logic (which model is applicable, whether §27 applies, etc.)
/// differs by EEG version and commissioning date. Supply `inbetriebnahme` so
/// the engine can apply the correct version-specific guards automatically.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SettleInput {
    /// Which regulatory formula to apply.
    pub model: SettlementModel,

    /// Einspeisemenge kWh for the billing period.
    /// `None` → output status = [`SettlementStatus::NoData`].
    pub einspeisemenge_kwh: Option<Decimal>,

    /// Monthly average EPEX Spot Day-Ahead price in ct/kWh.
    ///
    /// Required for [`Direktvermarktung`], [`Ausschreibung`], [`PostEegSpot`].
    /// `None` → output status = [`SettlementStatus::PriceMissing`] for those models.
    ///
    /// [`Direktvermarktung`]: SettlementModel::Direktvermarktung
    /// [`Ausschreibung`]: SettlementModel::Ausschreibung
    /// [`PostEegSpot`]: SettlementModel::PostEegSpot
    pub epex_avg_ct_kwh: Option<Decimal>,

    /// Fixed feed-in tariff rate in ct/kWh — **NET amount after §53 EEG deduction**.
    ///
    /// This field holds the **actual Vergütungssatz** the operator receives, which is
    /// the statutory `anzulegender Wert` (AW) **minus the §53 EEG deduction**:
    ///
    /// | Technology | §53 deduction | Example: AW → net `verguetungssatz_ct` |
    /// |---|---|---|
    /// | Solar PV, Wind | −0.4 ct/kWh | 8.51 ct AW → **8.11 ct net** |
    /// | Biomasse, Wasserkraft, Gas variants | −0.2 ct/kWh | 14.67 ct AW → **14.47 ct net** |
    ///
    /// Use [`crate::rates::sect53_deduction`] to compute the deduction from
    /// `crate::rates::solar_pv_lookup()` / `wind_onshore_lookup()` etc.
    ///
    /// §53 applies to: `Verguetung`, `Mieterstrom`, `Flexibilitaet`.
    /// §53 does NOT apply to: `Direktvermarktung`, `Ausschreibung`, `PostEegSpot`, `KwkgZuschlag`.
    ///
    /// Ignored when `capacity_blocks` is non-empty (each block carries its own rate).
    pub verguetungssatz_ct: Decimal,

    /// Anzulegender Wert in ct/kWh (Direktvermarktung / Ausschreibung).
    ///
    /// For `Ausschreibung`: the BNetzA tender-awarded value.
    pub direktverm_aw_ct: Option<Decimal>,

    /// Mieterstrom-Zuschlag in ct/kWh (§38a EEG 2023).
    ///
    /// Required when `model = Mieterstrom`.
    pub mieter_zuschlag_ct: Option<Decimal>,

    /// Flexibilitätsprämie rate in ct/kWh (§50b EEG 2023, Biomasse only).
    ///
    /// Required when `model = Flexibilitaet` (§50b, bestehende Anlagen).
    /// Not used for `FlexibilitaetZuschlag` (§50a, neue Anlagen).
    pub flex_praemie_ct_kwh: Option<Decimal>,

    /// §20 Abs. 3 EEG Managementprämie in ct/kWh.
    ///
    /// Paid by NB to Direktvermarkter for market integration administration.
    /// Statutory values:
    /// - `0.4 ct/kWh` for plants ≤100 MW
    /// - `0.2 ct/kWh` for plants >100 MW (§20 Abs. 3 Nr. 1 EEG 2023)
    ///
    /// Only applies to [`Direktvermarktung`] and [`Ausschreibung`].
    /// Supply `None` to omit Managementprämie from the calculation.
    /// Alternatively, when `leistung_kwp` is set, the engine computes this
    /// automatically from the statutory thresholds.
    ///
    /// [`Direktvermarktung`]: SettlementModel::Direktvermarktung
    /// [`Ausschreibung`]: SettlementModel::Ausschreibung
    pub managementpraemie_ct: Option<Decimal>,

    /// KWKG only: accumulated kWh already paid out in previous periods.
    ///
    /// Used for hour-limit enforcement (§8 KWKG 2023, plants >2 MW).
    /// `None` → no hour-limit tracking.
    pub kwk_strom_kwh_gesamt: Option<Decimal>,

    /// KWKG only: total maximum eligible kWh = `rated_kW × kwk_foerderdauer_h`.
    ///
    /// `None` → no hour-limit cap applied.
    pub kwk_max_kwh: Option<Decimal>,

    /// **§47 EEG (weggefallen/deleted in EEG 2023) / old EEG rules via §100 Übergangsregelung.**
    ///
    /// §52 EEG ≤2021 sanction tier (old regime via §100 Übergangsregelung).
    ///
    /// §52 EEG 2021/2017 has **three tiers** that each reduce the Vergütung differently.
    /// These are for plants governed by EEG ≤2021 rules (commissioned before 01.01.2023).
    ///
    /// | `SanktionAlt` | §52 EEG ≤2021 | Vergütung effect |
    /// |---|---|---|
    /// | `VerguetungAufNull` | Abs. 1 Nr. 1: MaStR not registered | **EUR 0** |
    /// | `VerguetungAufNull` | Abs. 1 Nr. 2a: §10b Direktvermarktungspflicht | **EUR 0** |
    /// | `VerguetungAufNull` | Abs. 1 Nr. 4: §27a Eigenversorgung (Ausschreibung) | **EUR 0** |
    /// | `VerguetungAufMarktwert` | Abs. 2 Nr. 1: §9 Abs. 1/2/5 Fernsteuerbarkeit | **→ EPEX Marktwert** |
    /// | `VerguetungAufMarktwert` | Abs. 2 Nr. 1a: §9 Abs. 8 Messeinrichtung | **→ EPEX Marktwert** |
    /// | `VerguetungReduziert20Prozent` | Abs. 3 Nr. 1: MaStR partial/late | **× 0.80** |
    ///
    /// For EEG 2023 plants, use `pflichtverstoss` instead — §52 EEG 2023 charges
    /// €10/kW/month without suspending Vergütung.
    ///
    /// `None` = no sanction (normal settlement).
    pub sanktion: Option<SanktionAlt>,

    /// §52 EEG 2023 — Pflichtverstöße (compliance violations).
    ///
    /// §52 applies to plants governed by **EEG 2023 rules** (commissioned after 01.01.2023,
    /// or old plants for violations introduced in EEG 2023).
    ///
    /// Each violation results in a **separate payment obligation from the plant operator
    /// to the NB** of €10/kW/month (§52 Abs. 2 EEG 2023). This is NOT a reduction of
    /// the Vergütung — the operator still receives the full Vergütung AND must separately
    /// pay the §52 penalty to the NB (§52 Abs. 6: the NB may net these).
    ///
    /// ## Common violation types
    ///
    /// | `SanktionsTyp` | §52 Abs. 1 Nr. | Trigger |
    /// |---|---|---|
    /// | `FernsteuerbarkeitmFehlend` | Nr. 1 | §9 Abs. 1/2: no remote-control equipment |
    /// | `SpeicherAnforderungNichtErfuellt` | Nr. 2 | §9 Abs. 5: missing storage requirement |
    /// | `MastrNichtRegistriert` | Nr. 11 | Plant not registered in MaStR |
    ///
    /// Use [`crate::foerderdauer::calculate_pflichtzahlung_§52`] to compute the penalty amount.
    ///
    /// When `pflichtverstoss` is `Some`, the settlement formula still computes the
    /// **full Vergütung** — the penalty is returned separately in the output's
    /// `pflichtzahlung_eur` field.
    ///
    /// Default: `None` (no violation).
    pub pflichtverstoss: Option<Pflichtverstoss>,

    /// §27 EEG — kWh produced during negative EPEX hours (to be excluded).
    ///
    /// Under §27 EEG 2023 (formerly §51 EEG 2021), for plants **≥100 kWp
    /// commissioned after 01.01.2016**, EEG Vergütung is zero during hours
    /// when the hourly EPEX Spot price is negative AND the consecutive run of
    /// negative hours is ≥6.
    ///
    /// When `inbetriebnahme` and `leistung_kwp` are both set, the engine
    /// automatically guards this rule:
    /// - Plants commissioned **before** 2016-01-01 → §27 not applied
    /// - Plants **< 100 kWp** → §27 not applied
    ///
    /// When either field is absent, the caller's decision is trusted.
    ///
    /// **Applies to**: `Verguetung`, `Mieterstrom`, `Flexibilitaet`.
    /// **Not for**: `Direktvermarktung`, `Ausschreibung` (market risk borne by Direktvermarkter),
    /// `KwkgZuschlag`, `PostEegSpot`, `Eigenverbrauch`.
    ///
    /// Default: `None` (rule not applied).
    pub kwh_during_negative_epex: Option<Decimal>,

    // ── Commissioning & Förderdauer ──────────────────────────────────────────
    /// Plant commissioning date (Inbetriebnahmedatum).
    ///
    /// When set, enables automatic EEG-version-aware rule enforcement:
    /// - **§51 EEG Negativpreisregel**: threshold and kW exemption depend on EEG version
    ///   derived from commissioning year (see `eeg_gesetz`).
    ///   Key boundary: §66 EEG 2017 exempts plants commissioned **before 01.01.2016**.
    ///   Plants from 2016-01-01 onwards are subject to §51 EEG 2017 (6h, 500 kW/3 MW).
    /// - **Audit position labels**: include the commissioning year for traceability.
    ///
    /// For multi-block plants (§24 Anlagenerweiterung), the commissioning dates
    /// live on each `CapacityBlock` instead.
    pub inbetriebnahme: Option<Date>,

    /// Installed peak power in kWp (or kW_el for KWKG).
    ///
    /// Used for:
    /// - §27 EEG guard (threshold: 100 kWp)
    /// - Automatic `managementpraemie_ct` when set to `None`
    ///   (auto: 0.4 ct/kWh for ≤100 MW, 0.2 ct/kWh for >100 MW)
    ///
    /// Ignored when `capacity_blocks` is non-empty.
    pub leistung_kwp: Option<Decimal>,

    /// EEG subsidy end date from the plant registry.
    ///
    /// When set together with `billing_date`, the engine automatically returns
    /// `FoerderungBeendet` when `billing_date > foerderendedatum`.
    ///
    /// For KWKG plants, this is the **calendar-year** fallback (§8 Abs. 4 KWKG):
    /// Förderung ends at `min(kwk_hour_limit, inbetriebnahme + 15y)`.
    pub foerderendedatum: Option<Date>,

    /// First day of the billing period (ISO 8601 month-start, e.g. 2026-07-01).
    ///
    /// Used together with `foerderendedatum` for automatic `FoerderungBeendet`
    /// detection. When omitted, the caller must check FoerderungBeendet manually.
    pub billing_date: Option<Date>,

    // ── §24 Anlagenerweiterung / Zusammenlegung ───────────────────────────────
    /// Additional capacity blocks for §24 EEG Anlagenerweiterung / Zusammenlegung.
    ///
    /// When non-empty, the engine performs multi-block settlement:
    /// 1. Each block receives a proportional share of `einspeisemenge_kwh`
    ///    (proportional to `leistung_kwp` of each block).
    /// 2. The primary block uses `SettleInput.verguetungssatz_ct` and
    ///    `SettleInput.inbetriebnahme` / `foerderendedatum`.
    /// 3. Blocks whose `foerderendedatum < billing_date` are expired (EUR 0).
    /// 4. The §27 Negativpreisregel is applied per-block based on each block's
    ///    commissioning date and capacity.
    ///
    /// Leave empty for single-block plants (the vast majority).
    pub capacity_blocks: Vec<CapacityBlock>,

    // ── Metering concept ─────────────────────────────────────────────────────
    /// Metering concept (§2 Nr. 20 EEG 2023) — for audit trail and validation.
    ///
    /// Does not affect the settlement formula itself: the engine always uses
    /// the measured `einspeisemenge_kwh` as input. The `messkonzept` is
    /// recorded in position metadata for regulatory audit transparency.
    pub messkonzept: Option<Messkonzept>,

    /// EEG law year applicable to this plant (Gesetz-Jahr des anzuwendenden EEG).
    ///
    /// Determines which version-specific rules the engine applies:
    ///
    /// EEG law version governing this plant.
    ///
    /// Determines which version-specific §51/§52 rules apply:
    ///
    /// | `eeg_gesetz` | §51 threshold | §51 kW exemption |
    /// |---|---|---|
    /// | `Kwkg` / `Eeg2000`–`Eeg2012` | none (§66 EEG 2017 Bestandsschutz) | — |
    /// | `Eeg2017` | ≥ **6** consecutive hours | Wind <3 MW; other <500 kW |
    /// | `Eeg2021` | ≥ **4** consecutive hours | all plants < 500 kW |
    /// | `Eeg2023` (default) | **any** negative period | < 100 kW (until iMSys) |
    ///
    /// Use [`EegGesetz::from_db_year`] to convert the `eeg_gesetz` DB column, or
    /// [`EegGesetz::from_inbetriebnahme_year`] as a fallback.
    pub eeg_gesetz: EegGesetz,

    /// Plant technology type (optional, used for §51 EEG 2017 wind exemption).
    ///
    /// Under **EEG 2017**, wind turbines get a separate 3 MW kW exemption
    /// (§51 Abs. 3 Nr. 1); other plants get the 500 kW exemption (Nr. 2).
    /// Derive from `einsd` `erzeugungsart` column via [`ErzeugungsArt::from_db_str`].
    ///
    /// `None` is treated as non-wind (conservative: 500 kW exemption under EEG 2017).
    pub erzeugungsart: Option<ErzeugungsArt>,
}

impl Default for SettleInput {
    fn default() -> Self {
        Self {
            model: SettlementModel::Verguetung,
            einspeisemenge_kwh: None,
            epex_avg_ct_kwh: None,
            verguetungssatz_ct: Decimal::ZERO,
            direktverm_aw_ct: None,
            mieter_zuschlag_ct: None,
            flex_praemie_ct_kwh: None,
            managementpraemie_ct: None,
            kwk_strom_kwh_gesamt: None,
            kwk_max_kwh: None,
            sanktion: None,
            kwh_during_negative_epex: None,
            inbetriebnahme: None,
            leistung_kwp: None,
            foerderendedatum: None,
            billing_date: None,
            capacity_blocks: vec![],
            messkonzept: None,
            pflichtverstoss: None,
            eeg_gesetz: EegGesetz::default(), // EEG 2023 — safe default for new plants
            erzeugungsart: None,
        }
    }
}

// ── SettleOutput ──────────────────────────────────────────────────────────────

/// Output of a settlement calculation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SettleOutput {
    /// Total settlement amount in EUR (sum of all `positions`).
    ///
    /// `None` when `status` is [`NoData`] or [`PriceMissing`].
    /// `Some(Decimal::ZERO)` when `status` is [`Sanctioned`] or [`Eigenverbrauch`].
    ///
    /// [`NoData`]: SettlementStatus::NoData
    /// [`PriceMissing`]: SettlementStatus::PriceMissing
    /// [`Sanctioned`]: SettlementStatus::Sanctioned
    pub settlement_eur: Option<Decimal>,

    /// Effective kWh used in the calculation.
    ///
    /// - May be less than `einspeisemenge_kwh` when KWKG hour-limit is approached.
    /// - Excludes `kwh_during_negative_epex` when the §27 negative-price rule applies.
    pub eligible_kwh: Option<Decimal>,

    /// Individual billing positions that make up `settlement_eur`.
    ///
    /// Empty when `status` is `NoData`, `PriceMissing`, `Sanctioned`, or `Eigenverbrauch`.
    ///
    /// Multi-component models produce multiple positions:
    /// - `Mieterstrom`: base Vergütung + §38a Zuschlag
    /// - `Direktvermarktung`/`Ausschreibung`: Gleitende Marktprämie + §20 Abs. 3 Managementprämie
    /// - `Flexibilitaet`: base Vergütung + §50 Flex-Prämie
    /// - Multi-block plants (§24 Anlagenerweiterung): one position per active block
    ///
    /// Use `.to_line_item()` on each position to convert to [`billing::LineItem`]
    /// for invoice / `BillingDocument` generation.
    pub positions: Vec<SettlePosition>,

    /// Computation outcome.
    pub status: SettlementStatus,

    /// §52 EEG 2023 penalty amount owed by plant operator to NB (separate from Vergütung).
    ///
    /// `None` when `input.pflichtverstoss` was not set.
    /// `Some(Decimal::ZERO)` when there is no violation.
    /// Positive = operator owes NB (the NB may net this against Vergütung per §52 Abs. 6).
    ///
    /// This amount is NOT deducted from `settlement_eur` — the Vergütung is computed
    /// independently. The caller is responsible for netting if desired.
    pub pflichtzahlung_eur: Option<Decimal>,
}

// ── SettlePosition ────────────────────────────────────────────────────────────

/// A single billing component of a settlement calculation.
///
/// Each position represents one regulatory charge line:
/// `net_eur = kwh × rate_ct_kwh / 100`.
///
/// Convert to a [`billing::LineItem`] for invoice generation via [`.to_line_item()`].
///
/// [`.to_line_item()`]: SettlePosition::to_line_item
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SettlePosition {
    /// Human-readable description of this charge line.
    pub description: String,

    /// Legal basis for audit trail (e.g. `"§21 EEG 2023"`, `"§20 Abs. 3 EEG 2023"`).
    pub legal_basis: String,

    /// Energy quantity this position applies to (kWh).
    pub kwh: Decimal,

    /// Rate in ct/kWh. May be negative (e.g. `PostEegSpot` at negative EPEX).
    pub rate_ct_kwh: Decimal,

    /// Net amount in EUR (`kwh × rate_ct_kwh / 100`, rounded to 5dp).
    ///
    /// Positive = NB owes Anlagenbetreiber (typical).
    /// Negative = Anlagenbetreiber owes NB (post-EEG at negative EPEX).
    pub eur: Decimal,
}

impl SettlePosition {
    /// Convert this position to a [`billing::LineItem`] for use in
    /// [`billing::BillingDocument`] generation (invoice, settlement receipt).
    ///
    /// Uses `billing::LineItem::for_usage()` with the signed rate — negative
    /// EPEX prices produce a negative `net_amount` on a `Sign::Debit` item,
    /// correctly modelling the post-EEG scenario where the plant owes the NB.
    pub fn to_line_item(&self) -> billing::LineItem {
        use billing::LineItem;

        let rate_eur = self.rate_ct_kwh / rust_decimal::Decimal::from(100);
        let mut builder =
            LineItem::for_usage(&self.description, self.kwh, "kWh", rate_eur, "EUR/kWh")
                .meta("legal_basis", self.legal_basis.as_str());

        // Category tags for ERP filtering
        if self.legal_basis.contains("EEG") || self.legal_basis.contains("post-F\u{00f6}rderung") {
            builder = builder.tag("eeg");
        }
        if self.legal_basis.contains("KWKG") {
            builder = builder.tag("kwkg");
        }
        builder = match self.legal_basis.as_str() {
            b if b.starts_with("\u{00a7}20 Abs. 3") => builder.tag("managementpraemie"),
            b if b.starts_with("\u{00a7}20") || b.starts_with("\u{00a7}\u{00a7}22a") => {
                builder.tag("marktpraemie")
            }
            b if b == "\u{00a7}38a EEG 2023" => builder.tag("mieterstrom"),
            b if b == "\u{00a7}50b EEG 2023" || b == "\u{00a7}50 EEG 2023" => {
                builder.tag("flexibilitaet")
            }
            b if b.contains("post-F\u{00f6}rderung") => builder.tag("post-eeg-spot"),
            b if b == "\u{00a7}7 KWKG 2023" => builder.tag("kwk-zuschlag"),
            b if b == "\u{00a7}21 EEG 2023" => builder.tag("verguetung"),
            _ => builder,
        };

        builder
            .build()
            .expect("SettlePosition always has a non-empty static description")
    }
}

// ── SettlementStatus ──────────────────────────────────────────────────────────

/// Outcome of a settlement calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SettlementStatus {
    /// Amount calculated successfully.
    Calculated,
    /// No meter data for the billing period. Try again once data arrives.
    NoData,
    /// Required price data (EPEX monthly average) is missing.
    PriceMissing,
    /// Förderdauer has ended (KWKG hour-limit exhausted or EEG 20-year period expired).
    FoerderungBeendet,
    /// §25 / §47 EEG: MaStR registration missing — payment suspended.
    Sanctioned,
}
