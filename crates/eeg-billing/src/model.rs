//! Settlement model types — the input/output contract for [`calculate_settlement`].
//!
//! [`calculate_settlement`]: crate::calculate_settlement

use crate::scheme::{SettlementScheme, SettlementType, TariffSource};
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
#[non_exhaustive]
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
/// Use [`crate::foerderdauer::calculate_pflichtzahlung`] to compute the penalty.
///
/// ## EEG version note
///
/// §52 EEG 2023 applies to plants under current EEG 2023 rules.
/// For old plants (commissioned before 01.01.2023) under §100 Übergangsregelung,
/// the old §47 EEG 2021 "Vergütung = 0" rule applies instead — use `sanktion: Some(SanktionAlt::VerguetungAufNull)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SanktionsTyp {
    /// §52 Abs. 1 Nr. 1 — Missing Fernsteuerbarkeit (§9 Abs. 1/2).
    ///
    /// Plant ≥25 kW does not have remote control capability installed.
    /// Obligation fulfilled → reduced to €2/kW/month retroactively.
    /// Technical defect grace: 2 months waived.
    FernsteuerbarkeitFehlend,

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

    /// §52 Abs. 1 Nr. 9a — Post-commissioning violation of §37 Abs. 1a or §48 Abs. 6.
    ///
    /// Plant violates the obligations that arise after commissioning under those paragraphs
    /// (§37 Abs. 1a: iMSys Nachrüstung after commissioning; §48 Abs. 6: solar Segment obligations).
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

    // ── §52 Abs. 1 Nr. 5–12 — additional violations ──────────────────────────────
    /// §52 Abs. 1 Nr. 5 — Ausfallvergütung Höchstdauer exceeded
    /// (§21 Abs. 1 Satz 1 Nr. 3).
    ///
    /// Plant in Ausfallvergütung exceeds the statutory 3-month maximum.
    ///
    /// The §10/kW Pflichtzahlung is owed for the months of the violation only —
    /// §52 Abs. 4 grants *additional* months solely to Nr. 7 (+3), Nr. 9 (+1),
    /// Nr. 10 (full calendar year) and Nr. 12 (+6). Nr. 5 is **not** listed there,
    /// so no extra months are added (adding +3 here over-charged the operator).
    AusfallverguetungHoechstdauerUeberschritten,

    /// §52 Abs. 1 Nr. 6 — Unzulässige Inanspruchnahme von Einspeisevergütung (§21 Abs. 2).
    ///
    /// Plant claims Einspeisevergütung while violating the conditions of §21 Abs. 2
    /// (e.g., plant participates in Regelenergiemarkt while on Einspeisevergütung).
    EinspeiseverguetungUnzulaessigeNutzung,

    /// §52 Abs. 1 Nr. 7 — Unzulässiger Veräußerungsform-Wechsel (§21b Abs. 2 Satz 1 zweiter Halbsatz).
    ///
    /// Operator performs an impermissible switch of Veräußerungsform (e.g., switching
    /// when mandatory Direktvermarktung applies and return to Einspeisevergütung is blocked).
    ///
    /// ## §52 Abs. 4 Nr. 1: +3 extra months
    ///
    /// Payment is also owed for the **3 calendar months following** the violation period.
    /// Callers should add these 3 months to `monate_des_verstosses`.
    VeraeusserungsformWechselUngueltig,

    /// §52 Abs. 1 Nr. 8 — Pflichtnachweis-Verletzung (§21b Abs. 3).
    ///
    /// Operator fails to provide required evidence/documentation after a Veräußerungsform
    /// switch (§21b Abs. 3 documentation obligations).
    ///
    /// ## §52 Abs. 3 Satz 2: Technical defect grace
    ///
    /// When `technischer_defekt = true` and violation occurred after 31 Dec 2023:
    /// payment waived for the violation month and the following month.
    VeraeusserungsformNachweispflichtVerletzt,

    /// §52 Abs. 1 Nr. 9 — Zuordnungs-/Wechselmeldung nicht übermittelt (§21c).
    ///
    /// Operator did not notify the NB of a Veräußerungsform assignment or switch
    /// within the deadline per §21c EEG 2023.
    ///
    /// ## §52 Abs. 4 Nr. 2: +1 extra month
    ///
    /// Payment is also owed for the **1 calendar month following** the violation period.
    /// Callers should add this 1 month to `monate_des_verstosses`.
    ZuordnungsWechselNichtGemeldet,

    /// §52 Abs. 1 Nr. 12 — Doppelvermarktungsverbot verletzt (§80 EEG 2023).
    ///
    /// Strom was claimed for EEG payment AND simultaneously used in another subsidised
    /// scheme (e.g., EEG + EEG, or EEG + KWKG, or EEG + HKN). §80 prohibits double-counting.
    ///
    /// ## §52 Abs. 4 Nr. 4: +6 extra months
    ///
    /// Payment is also owed for the **6 calendar months following** the violation period.
    /// Callers should add these 6 months to `monate_des_verstosses`.
    DoppelvermarktungsverbotVerletzt,
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
/// use rust_decimal::dec;
///
/// // Missing Fernsteuerbarkeit for 3 months, 500 kW plant, obligation not yet fulfilled
/// let violation = Pflichtverstoss {
///     typ: SanktionsTyp::FernsteuerbarkeitFehlend,
///     leistung_kw: dec!(500),
///     monate_des_verstosses: 3,
///     nachtraeglich_erfuellt: false,
///     technischer_defekt: false,
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
    ///
    /// **Include §52 Abs. 4 extra months** for the following types:
    /// - Nr. 5 (Ausfallvergütung Höchstdauer), Nr. 7 (§21b Wechsel ungültig): add 3 months
    /// - Nr. 9 (§21c Zuordnungsmeldung fehlt): add 1 month
    /// - Nr. 10 (Volleinspeisung): pass 12 months (all months of the calendar year)
    /// - Nr. 12 (§80 Doppelvermarktung): add 6 months
    pub monate_des_verstosses: u32,
    /// Whether the obligation has since been fulfilled.
    ///
    /// When `true`, §52 Abs. 3 reduces the penalty retroactively to €2/kW/month
    /// for violation types Nr. 1, 3, 4, 11. Has no effect for Nr. 2, 9a, 10.
    pub nachtraeglich_erfuellt: bool,
    /// Whether the violation was caused by a **technical defect** of plant equipment.
    ///
    /// Per §52 Abs. 3 Satz 2 EEG 2023 (in force from 01.01.2024):
    /// For violations of Nr. 1 (Fernsteuerbarkeit), Nr. 3 (iMSys), Nr. 4 (§10b), Nr. 8 (§21b Abs. 3)
    /// caused by a technical defect, the penalty is **waived for the defect month and
    /// the following calendar month**.
    ///
    /// - Only applies to violations occurring **after 31 December 2023**.
    /// - The operator bears the burden of proof for the defect (Darlegungs- und Beweislast).
    /// - Does **not** apply to Nr. 2, 5, 6, 7, 9, 10, 11, 12.
    ///
    /// When `true`: effective months = `max(0, monate_des_verstosses - 2)` for eligible types.
    pub technischer_defekt: bool,
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
/// use rust_decimal::dec;
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
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SettleInput {
    // ── Settlement scheme — HOW remuneration is determined ────────────────
    /// Which regulatory formula to apply.
    ///
    /// - `FeedInTariff` → §21 EEG Einspeisevergütung
    /// - `MarketPremium` → §20 EEG Gleitende Marktprämie (incl. Ausschreibung via `tariff_source`)
    /// - `TenantElectricity` → §21 Abs. 3 Mieterstrom
    /// - `PostEeg` → post-Förderung spot (configurable `post_eeg_price_floor`)
    /// - `KwkSurcharge` → §7 KWKG
    /// - `TemporaryFeedInTariff` → §21 Abs. 1 Nr. 2 Ausfallvergütung
    /// - `Eigenverbrauch` → no payment
    /// - `FlexibilityPremium` → §50b bestehende Biomasseanlagen
    /// - `FlexibilitySurcharge` → §50a neue Biomasseanlagen (capacity payment)
    pub scheme: SettlementScheme,

    /// Where the Anzulegender Wert (AW) comes from.
    ///
    /// - `Statutory` → §48 EEG statutory tables (default)
    /// - `Auction(meta)` → BNetzA tender award (Ausschreibung: same formula as MarketPremium)
    /// - `Transitional(rule)` → §100 EEG Übergangsregelung
    pub tariff_source: TariffSource,

    /// Settlement type: initial, correction, or reversal.
    pub settlement_type: SettlementType,

    /// Einspeisemenge kWh for the billing period.
    /// `None` → output status = [`SettlementStatus::NoData`].
    pub einspeisemenge_kwh: Option<Decimal>,

    /// Monthly EPEX Spot Day-Ahead average **or** technology-specific Jahresmarktwert
    /// (§20 Abs. 2 + Anlage 1 EEG 2023) in ct/kWh.
    ///
    /// This is the **market reference price** used for all calculations that compare
    /// EEG payment against the market:
    /// - `MarketPremium`: spread = `max(0, eff_AW - marktwert_ct_kwh)`
    /// - `PostEeg`: plant paid at `marktwert_ct_kwh` per kWh
    /// - `SanktionAlt::VerguetungAufMarktwert`: old-regime sanction uses this as rate
    /// - `§44b` biogas excess: excess kWh paid at `marktwert_ct_kwh`
    ///
    /// The library does not distinguish between EPEX monthly average and Jahresmarktwert —
    /// the caller resolves which value applies and passes it here.
    pub marktwert_ct_kwh: Option<Decimal>,

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
    /// | `FernsteuerbarkeitFehlend` | Nr. 1 | §9 Abs. 1/2: no remote-control equipment |
    /// | `SpeicherAnforderungNichtErfuellt` | Nr. 2 | §9 Abs. 5: missing storage requirement |
    /// | `MastrNichtRegistriert` | Nr. 11 | Plant not registered in MaStR |
    ///
    /// Use [`crate::foerderdauer::calculate_pflichtzahlung`] to compute the penalty amount.
    ///
    /// When `pflichtverstoss` is `Some`, the settlement formula still computes the
    /// **full Vergütung** — the penalty is returned separately in the output's
    /// `pflichtzahlung_eur` field.
    ///
    /// Default: empty Vec (no violations).
    pub pflichtverstoss: Vec<Pflichtverstoss>,

    /// §§53b–54 EEG 2023 — reductions that act on the anzulegender Wert.
    ///
    /// §53b Regionalnachweise, §53c Stromsteuerbefreiung and §54 solar
    /// first-segment auction defects. Each reduces the AW *before* the
    /// settlement formula, which matters for the gleitende Marktprämie: its
    /// `max(0, …)` floor must absorb the reduction rather than the result being
    /// pushed negative afterwards. See [`crate::aw_reductions`].
    pub aw_reductions: crate::aw_reductions::AwReductionContext,

    /// §51a EEG 2023 — quarter-hours during which §51 reduced Vergütung to zero.
    ///
    /// When provided, the engine computes `SettleOutput.verlaengerungsanspruch_qh`:
    /// Solar PV: `ceil(qh / 2)` · Others: 1:1 factor.
    pub negative_price_quarter_hours: Option<u64>,

    /// §13a EnWG (Redispatch 2.0) — kWh curtailed by the NB (Einspeisemanagement compensation).
    ///
    /// §51 Negativpreisregel does NOT apply to these kWh (§19 Abs. 2 EEG 2023).
    pub einspeisemanagement_kwh: Option<Decimal>,

    /// §§42–44 EEG 2023 — Biomass/biogas fuel composition for settlement enforcement.
    ///
    /// When set for biomass or biogas plants, the engine enforces:
    ///
    /// - **§43 Abs. 1 Nr. 2 substrate cap** (max 40 % Energiepflanzen vom Acker):
    ///   when `substrate_cap_ok = false`, settlement returns `Sanctioned` (EUR 0,
    ///   legal_basis = "§43 Abs. 1 Nr. 2 EEG 2023").
    /// - **§44 Güllekleinanlage** eligibility is recorded in the position label for
    ///   audit transparency — it does **not** change the formula here; the caller
    ///   must supply the correct Güllekleinanlage `verguetungssatz_ct`
    ///   (use [`crate::rates::guellekleinanlage_rate`]).
    ///
    /// Use [`crate::biomasse::BiomassSettlementData::new`] to derive from fuel
    /// composition data.  `None` = plant is not biomass/biogas (cap not enforced).
    pub biomasse: Option<crate::biomasse::BiomassSettlementData>,

    /// **§25 Abs. 1 Satz 3 EEG** — Fraction of the billing month with entitlement.
    ///
    /// When `None`, the library auto-computes from `billing_date`, `inbetriebnahme`,
    /// and `foerderendedatum` via `foerderdauer::compute_billing_days_fraction()`.
    /// When `Some(x)`, the provided value is used directly (override).
    ///
    /// Set explicitly only when the auto-computed value would be wrong for your
    /// settlement scenario (rare edge cases). For standard plant lifecycles,
    /// leave as `None` and ensure `billing_date`, `inbetriebnahme`, and
    /// `foerderendedatum` are set.
    pub billing_days_fraction: Option<Decimal>,

    /// §51 EEG — kWh produced during negative EPEX hours (to be excluded).
    ///
    /// Under §51 EEG 2023, for plants **≥100 kWp commissioned after 01.01.2016**,
    /// EEG Vergütung is zero during hours when the hourly EPEX Spot price is
    /// negative AND the consecutive run of negative hours meets the version-specific
    /// threshold (§51 EEG 2023: any period; §51 EEG 2017: ≥6h; §51 EEG 2021: ≥4h).
    ///
    /// When `inbetriebnahme` and `leistung_kwp` are both set, the engine
    /// automatically guards this rule based on `eeg_gesetz`.
    ///
    /// **Does NOT apply to §51b biogas Ausschreibungsanlagen** — those plants
    /// use a different rule (AW = 0 when EPEX ≤ 2 ct/kWh).
    ///
    /// Default: `None` (rule not applied).
    pub kwh_during_negative_epex: Option<Decimal>,

    // ── Commissioning & Förderdauer ──────────────────────────────────────────
    /// Plant commissioning date (Inbetriebnahmedatum).
    ///
    /// When set, enables automatic EEG-version-aware rule enforcement:
    /// - **§51 EEG Negativpreisregel**: threshold and kW exemption depend on EEG version
    ///   derived from commissioning year (see `eeg_gesetz`).
    ///   Key boundary: §100 Abs. 1 Satz 4 EEG 2017 exempts plants commissioned **before 01.01.2016**.
    ///   Plants from 2016-01-01 onwards are subject to §51 EEG 2017 (6h, 500 kW/3 MW).
    /// - **Audit position labels**: include the commissioning year for traceability.
    ///
    /// For multi-block plants (§24 Anlagenerweiterung), the commissioning dates
    /// live on each `CapacityBlock` instead.
    pub inbetriebnahme: Option<Date>,

    /// Type of commissioning event — for audit trail and Förderdauer rules.
    ///
    /// Determines whether the Förderdauer clock resets (only `Repowering` resets it)
    /// and which lifecycle state the plant is in. Stored in `einsd`'s
    /// `eeg_anlagen.inbetriebnahme_typ` column.
    ///
    /// | `InbetriebnahmeTyp` | F\u00f6rderdauer | Audit relevance |
    /// |---|---|---|
    /// | `Erstinbetriebnahme` (default) | starts at `inbetriebnahme` | Normal plant |
    /// | `Wiederinbetriebnahme` | continues from original | Restart after shutdown |
    /// | `Modernisierung` | continues from original | Equipment replacement |
    /// | `Repowering` | **resets** to repowering date | New 20-year clock |
    /// | `Zusammenlegung` | oldest component date | §24 merger |
    /// | `Erweiterung` | new block from extension date | §24 capacity add |
    ///
    /// The engine records this in position descriptions for full audit traceability.
    /// Default: `InbetriebnahmeTyp::Erstinbetriebnahme`.
    pub inbetriebnahme_typ: crate::technology::InbetriebnahmeTyp,

    /// Installed peak power in kWp (or kW_el for KWKG).
    ///
    /// Used for:
    /// - §27 EEG guard (threshold: 100 kWp)
    /// - §51 Abs. 2 kW exemption (aggregated per §24 when `capacity_blocks` is set)
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

    /// EEG law year applicable to this plant (Gesetz-Jahr des anzuwendenden EEG).
    ///
    /// Determines which version-specific rules the engine applies:
    ///
    /// EEG law version governing this plant — the §52 Pflichtverstoß regime and
    /// the §100 Übergangsbestimmungen.
    ///
    /// **Not** the source of the §51 rules: those are keyed on the commissioning
    /// date (see [`SettleInput::negativpreis_regime`]), because the
    /// Solarspitzengesetz rewrote §51 with effect from 25.02.2025 — inside the
    /// EEG 2023 range.
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

    /// §53 Abs. 1 EEG 2023 — whether the Einspeisevergütung rate supplied in the
    /// scheme is the **gross** anzulegender Wert (as published in §48/BNetzA
    /// bulletins) rather than the net Vergütungssatz.
    ///
    /// When `true`, the engine subtracts the §53 Abs. 1 deduction
    /// (0.4 ct/kWh Solar/Wind, 0.2 ct/kWh Wasserkraft/Biomasse/Geothermie/Gas)
    /// keyed on `erzeugungsart` for [`SettlementScheme::FeedInTariff`]. Default
    /// `false`: the rate is already net (einsd's `eeg_verguetungssaetze` stores
    /// net rates), so nothing is deducted — this prevents a double deduction.
    #[cfg_attr(feature = "serde", serde(default))]
    pub aw_is_gross: bool,

    /// **§44b Abs. 1 EEG 2023** — Biogas >100 kW: annual 45% Bemessungsleistung cap.
    ///
    /// For Biogas plants (fermentation biogas, **excluding** fermentation-biomass §44 plants
    /// and Ausschreibungsanlagen §39) with installed capacity >100 kW, the EEG payment
    /// is limited to the share of annual production corresponding to 45% of installed kW:
    ///
    /// `annual_quota_kwh = leistung_kw × 0.45 × <§3 Nr. 6 hours of the year>`
    ///
    /// The hour count is **not** a flat 8 760: it is the actual hours of the
    /// calendar year (8 784 in a leap year) less the hours before the plant's
    /// first generation. See [`crate::sect44b_jahreskontingent_kwh`], which is
    /// what both the settlement and the `check_sect44b_quota` MCP tool call.
    ///
    /// When set, this field is the **eligible kWh** for the current billing period (the
    /// caller tracks cumulative annual production and passes `min(kwh, remaining_quota)`):
    /// - Eligible part: normal remuneration (this field's value)
    /// - Excess part (`einspeisemenge_kwh - eligible`):
    ///   - `MarketPremium`: AW reduces to zero, Marktprämie = 0 (§44b Abs. 1 Satz 2)
    ///   - `FeedInTariff`: paid at EPEX Marktwert (`epex_avg_ct_kwh`), requires EPEX price
    ///
    /// `None` = cap does not apply (plant ≤100 kW, fermentation biomass §44, Ausschreibung §39,
    /// or non-Biogas technology).
    ///
    /// Legal basis: §44b Abs. 1 EEG 2023 (BGBl. I Nr. 28, 10.01.2023).
    pub biogas_sect44b_eligible_kwh: Option<Decimal>,

    /// §51 Abs. 2 Nr. 1 EEG 2023 — iMSys (intelligent metering system) rolled out.
    ///
    /// The sub-100-kW exemption is transitional: §51 Abs. 2 Nr. 1 grants it only
    /// "für Zeiträume vor dem Einbau eines intelligenten Messsystems". Once the
    /// iMSys is in, a 30 kWp plant is subject to §51 like any other.
    ///
    /// It lifts **only** that exemption. The 2 kW floor of Abs. 2 Nr. 2 stands
    /// until the Bundesnetzagentur's §85 Abs. 2 Nr. 12 Festlegung, and the
    /// exemptions of the older Fassungen (400 kW, 500 kW, 3 MW) are unaffected —
    /// they have no iMSys condition.
    ///
    /// Default: `false` (conservative — retains the exemption when unknown).
    pub has_imesys: bool,

    /// Technology-specific Jahresmarktwert category (§20 Abs. 2 + Anlage 1 EEG 2023).
    ///
    /// Documents which ÜNB technology category `marktwert_ct_kwh` was sourced from.
    /// The library uses `marktwert_ct_kwh` directly — this field is informational only
    /// (validation aid and audit label).
    pub marktwert_kategorie: Option<crate::scheme::MarktpreisKategorie>,

    /// §100 EEG — the date a Bestandsanlage's opt-in into the Solarspitzengesetz
    /// regime takes effect.
    ///
    /// The operator declares in Textform to the Netzbetreiber that §§ 51 and 51a
    /// shall apply; the declaration runs at the earliest from the end of the
    /// calendar year in which the plant is fitted with an iMSys. Derive it with
    /// [`crate::negativpreis::optin_wirksam_ab`]. From that date the plant is
    /// under the Solarspitzengesetz regime and its anzulegender Wert rises by
    /// [`crate::negativpreis::SECT51_OPTIN_ZUSCHLAG_CT_KWH`].
    ///
    /// `None` — the usual case — leaves the plant on its commissioning vintage.
    pub sect51_optin_wirksam_ab: Option<Date>,

    /// §51 Abs. 3 EEG — calendar days of an unreported negative-price period,
    /// for a plant on the **Ausfallvergütung**.
    ///
    /// An operator on the Ausfallvergütung must report, with the §71 Abs. 1 Nr. 1
    /// data, the quantity it fed in while the Spotmarktpreis was continuously
    /// negative. Where it does not, the claim for that calendar month falls by
    /// **5 % per calendar day** on which such a period fell, wholly or partly.
    ///
    /// Set this to the number of those days when the figure is missing, and `0`
    /// when it was reported (or the month had no negative period). Applies only
    /// to [`SettlementScheme::TemporaryFeedInTariff`]; ignored elsewhere.
    pub sect51_abs3_unreported_days: u32,

    /// §3 Nr. 37 EEG 2023 — **Pilotwindenergieanlage an Land**.
    ///
    /// Every Fassung of §51 carves these out of the Negativpreisregel, whatever
    /// their size. The status is a BNetzA/FGW certification fact about the
    /// turbine, so it is declared rather than derived.
    ///
    /// Default: `false`.
    pub ist_pilotwindanlage: bool,
}

impl SettleInput {
    /// Effective EEG law version for §51/§52 calculation.
    ///
    /// When `tariff_source = Transitional(rule)` and the rule implies a specific
    /// `EegGesetz`, that implied version is returned instead of `self.eeg_gesetz`.
    ///
    /// This prevents silent miscalculation when a §100 Transitional rule is set
    /// without the corresponding `eeg_gesetz` being updated by the caller.
    ///
    /// | `tariff_source` | Returns |
    /// |---|---|
    /// | `Statutory` / `Auction(_)` | `self.eeg_gesetz` (caller-supplied) |
    /// | `Transitional(Pre2016Bestandsschutz)` | `EegGesetz::Eeg2012` — §51 never applies |
    /// | `Transitional(Eeg2017Negativpreis6h)` | `EegGesetz::Eeg2017` — 6h threshold |
    /// | `Transitional(OldPlantBeforeEeg2023)` | `EegGesetz::Eeg2021` — 4h threshold |
    /// | `Transitional(_)` other | `self.eeg_gesetz` |
    ///
    /// # Example
    ///
    /// ```rust
    /// use eeg_billing::{SettleInput, EegGesetz};
    /// use eeg_billing::scheme::{TariffSource, Paragraph100Rule};
    ///
    /// // Pre-2016 plant: §51 must never apply, regardless of what eeg_gesetz says.
    /// let input = SettleInput {
    ///     tariff_source: TariffSource::Transitional(Paragraph100Rule::Pre2016Bestandsschutz),
    ///     eeg_gesetz: EegGesetz::Eeg2017, // deliberately wrong — overridden
    ///     ..SettleInput::default()
    /// };
    /// assert_eq!(input.effective_eeg_gesetz(), EegGesetz::Eeg2012);
    /// ```
    #[must_use]
    pub fn effective_eeg_gesetz(&self) -> EegGesetz {
        if let crate::scheme::TariffSource::Transitional(rule) = &self.tariff_source
            && let Some(implied) = rule.implied_eeg_gesetz()
        {
            return implied;
        }
        self.eeg_gesetz
    }

    /// The §51 Negativpreisregel version governing this plant.
    ///
    /// Derived from `inbetriebnahme`, because the Solarspitzengesetz boundary
    /// (25.02.2025) falls inside a calendar year and inside the EEG 2023 range.
    /// When the commissioning date is unknown the plant's law version supplies a
    /// coarse fallback, and a §100 `Transitional` rule overrides both — a rule
    /// that pins a plant to a pre-2016 vintage must keep §51 off it.
    #[must_use]
    pub fn negativpreis_regime(&self) -> crate::negativpreis::NegativpreisRegime {
        use crate::negativpreis::NegativpreisRegime as R;
        // A §100 rule is an explicit statement about which vintage governs, so it
        // wins over the date on the record.
        if let crate::scheme::TariffSource::Transitional(rule) = &self.tariff_source
            && let Some(implied) = rule.implied_eeg_gesetz()
        {
            return match implied {
                EegGesetz::Eeg2017 => R::Eeg2017,
                EegGesetz::Eeg2021 => R::Eeg2021,
                EegGesetz::Eeg2023 => self.inbetriebnahme.map_or(R::Solarspitzen, |ibn| {
                    R::fuer_periode(ibn, self.sect51_optin_wirksam_ab, self.billing_date)
                }),
                _ => R::Keine,
            };
        }
        if let Some(ibn) = self.inbetriebnahme {
            return R::fuer_periode(ibn, self.sect51_optin_wirksam_ab, self.billing_date);
        }
        match self.eeg_gesetz {
            EegGesetz::Eeg2017 => R::Eeg2017,
            EegGesetz::Eeg2021 => R::Eeg2021,
            // No date on the record: assume current law rather than a lapsed one.
            EegGesetz::Eeg2023 => R::Solarspitzen,
            _ => R::Keine,
        }
    }
}

// ── SettleOutput ──────────────────────────────────────────────────────────────

/// Output of a settlement calculation.
///
/// [`Default`] is "nothing settled": `NoData`, no amount, no positions. Every
/// early exit in the engine builds on it with `..SettleOutput::default()`
/// rather than restating ten fields — which is how a field added to this struct
/// stays correct at the twenty-eight places that return one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SettleOutput {
    /// Total settlement amount in EUR (sum of all `positions`).
    ///
    /// `None` when `status` is [`NoData`] or [`PriceMissing`].
    /// `Some(Decimal::ZERO)` when `status` is [`Sanctioned`] or `Eigenverbrauch`.
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
    /// - `Mieterstrom`: base Vergütung + §21 Abs. 3 Zuschlag
    /// - `Direktvermarktung`/`Ausschreibung`: Gleitende Marktprämie + §§53b–54 AW-Abzüge
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
    /// This amount is NOT deducted from `settlement_eur`.
    pub pflichtzahlung_eur: Option<Decimal>,

    /// **§52 Abs. 6 Satz 1 EEG 2023** — Fälligkeitsdatum for the §52 penalty payment.
    ///
    /// The 15th calendar day of the month following the billing month — same
    /// formula as `faelligkeitsdatum` (§26 Abs. 1), but legally distinct.
    ///
    /// > „Die Zahlungen werden zum 15. Kalendertag des Kalendermonats fällig, der auf
    /// > den nach den Absätzen 2 und 4 jeweils maßgeblichen Kalendermonat folgt.“
    ///
    /// For violations with §52 Abs. 4 extra months (Nr. 5, 7: +3m; Nr. 9: +1m;
    /// Nr. 12: +6m), the Fälligkeitsdatum is the 15th after the **last relevant month**.
    /// This field computes the 15th after the billing month as the base date.
    ///
    /// `None` when `billing_date` is not set or `pflichtzahlung_eur` is `None`.
    pub pflichtzahlung_faelligkeitsdatum: Option<Date>,

    /// §51a EEG 2023 — quarter-hours by which the Vergütungszeitraum is extended.
    ///
    /// Non-zero only when `input.negative_price_quarter_hours` was provided AND
    /// §51 actually reduced the Vergütung in this period.
    /// Solar PV: `ceil(lost_qh / 2)` · Others: `lost_qh` (1:1 factor).
    pub verlaengerungsanspruch_qh: u64,

    /// **§52 Abs. 7 EEG 2023** — whether violations cause loss of dezentrale Einspeisung entgelt.
    ///
    /// When `true`, the operator loses the entitlement to the Entgelt für dezentrale Einspeisung
    /// under §18 StromNEV for the **entire calendar year** in which any §52 violation occurred.
    ///
    /// Legal basis: §52 Abs. 7 EEG 2023:
    /// *„Bei Pflichtverstößen nach Absatz 1 verlieren die Anlagenbetreiber zusätzlich
    /// für das gesamte Kalenderjahr den Anspruch auf ein Entgelt für dezentrale
    /// Einspeisung nach §18 der Stromnetzentgeltverordnung.“*
    ///
    /// `true` when `pflichtzahlung_eur.is_some_and(|p| p > 0)`.
    /// The NB should also withhold the §18 StromNEV payment for this plant for the year.
    pub dezentrale_einspeisung_anspruch_verloren: bool,

    /// **§25 Abs. 1 Satz 3 EEG** — billing_days_fraction that was actually applied.
    ///
    /// The fraction applied to `settlement_eur` and position amounts in this settlement.
    /// Either the value provided in `SettleInput.billing_days_fraction` or the
    /// auto-computed value from `billing_date`, `inbetriebnahme`, and `foerderendedatum`.
    ///
    /// `None` when the fraction is 1.0 (full month, no proration applied).
    /// `Some(f)` when partial-month proration was applied.
    ///
    /// Store in the settlement receipt for § 147 AO / GoBD audit trail.
    pub billing_days_fraction_applied: Option<Decimal>,

    /// **§26 Abs. 1 EEG 2023** — Fälligkeitsdatum for this period's advance payment.
    ///
    /// The **15th calendar day of the month following the billing month**.
    /// Per §26 Abs. 1 EEG 2023:
    /// *„Auf die zu erwartenden Zahlungen nach §19 Abs. 1 sind monatlich jeweils zum
    /// 15. Kalendertag für den Vormonat Abschläge in angemessenem Umfang zu leisten."*
    ///
    /// | Billing month | Fälligkeitsdatum |
    /// |---|---|
    /// | June 2024 | **2024-07-15** |
    /// | December 2024 | **2025-01-15** (year rolls over) |
    /// | February 2025 | **2025-03-15** |
    ///
    /// Populated when `SettleInput.billing_date` is provided. `None` otherwise.
    ///
    /// **Note**: This is the statutory *latest* due date for monthly advance payments.
    /// The final annual settlement (Endabrechnung) falls under §26 Abs. 2, whose
    /// due date depends on the operator's §71 data submission obligations.
    pub faelligkeitsdatum: Option<Date>,
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

    /// Legal basis for audit trail (e.g. `"§21 EEG 2023"`, `"§23a EEG 2023 i.V.m. Anlage 1"`).
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
        use billing::{LineItem, Quantity, RoundingStrategy, UnitPrice};

        let rate_eur = self.rate_ct_kwh / rust_decimal::Decimal::from(100);
        // Typed `Quantity`/`UnitPrice` replace the old seven-argument
        // `for_usage_rounded` — the two unit labels can no longer be transposed.
        // `.with_code("KWH")` stamps EN 16931 BT-130 (UN/ECE Rec 20), so the
        // `billing::BillingDocument` is a complete EN-16931 source rather than
        // leaving a downstream mapper to guess the unit code from "kWh".
        // `UnitPrice::rounded(6, …)` prevents silent precision drift when
        // rate_ct_kwh is derived from integer arithmetic (ct/100); BO4E Preis.wert
        // is 6 decimal places, keeping the stored unit_price consistent with the
        // rendered output.
        let mut builder = LineItem::for_usage(
            &self.description,
            Quantity::new(self.kwh, "kWh").with_code("KWH"),
            UnitPrice::new(rate_eur, "EUR/kWh").rounded(6, RoundingStrategy::MidpointAwayFromZero),
        )
        .meta("legal_basis", self.legal_basis.as_str());

        // Category tags for ERP filtering
        if self.legal_basis.contains("EEG") || self.legal_basis.contains("post-F\u{00f6}rderung") {
            builder = builder.tag("eeg");
        }
        if self.legal_basis.contains("KWKG") {
            builder = builder.tag("kwkg");
        }
        builder = match self.legal_basis.as_str() {
            b if b.starts_with("\u{00a7}23a") || b.starts_with("\u{00a7}\u{00a7}22a") => {
                builder.tag("marktpraemie")
            }
            "\u{00a7}21 Abs. 3 EEG 2023" => builder.tag("mieterstrom"),
            b if b == "\u{00a7}50b EEG 2023" || b == "\u{00a7}50 EEG 2023" => {
                builder.tag("flexibilitaet")
            }
            b if b.contains("post-F\u{00f6}rderung") => builder.tag("post-eeg-spot"),
            "\u{00a7}7 KWKG 2023" => builder.tag("kwk-zuschlag"),
            "\u{00a7}21 EEG 2023" => builder.tag("verguetung"),
            _ => builder,
        };

        builder
            .build()
            .expect("SettlePosition always has a non-empty static description")
    }
}

// ── SettlementStatus ──────────────────────────────────────────────────────────

/// Outcome of a settlement calculation.
///
/// The [`Default`] is [`NoData`](Self::NoData) — the outcome that pays nothing.
/// A default that fell into `Calculated` would make a partially-constructed
/// [`SettleOutput`] read as a settled figure of zero, which downstream is
/// indistinguishable from "correctly settled, nothing due".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SettlementStatus {
    /// Amount calculated successfully.
    Calculated,
    /// No meter data for the billing period. Try again once data arrives.
    #[default]
    NoData,
    /// Required price data (EPEX monthly average) is missing.
    PriceMissing,
    /// Förderdauer has ended (KWKG hour-limit exhausted or EEG 20-year period expired).
    FoerderungBeendet,
    /// §25 / §47 EEG: MaStR registration missing — payment suspended.
    Sanctioned,
}
