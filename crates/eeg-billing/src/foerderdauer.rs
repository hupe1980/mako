//! Förderdauer (subsidy duration) calculation helpers.
//!
//! All functions are pure and deterministic — no I/O, no clock access.
//!
//! ## EEG Förderdauer rules
//!
//! | Condition | Rule | Legal basis |
//! |---|---|---|
//! | Standard commissioning | 20 years from `inbetriebnahme` | §21 EEG 2023 |
//! | Repowering | 20 years from `repowering_datum` | §22 EEG 2023 |
//! | §24 Zusammenlegung | Parent's `foerderendedatum` unchanged | §24 EEG 2023 |
//!
//! ## KWKG Förderdauer rules (§8 KWKG 2023)
//!
//! | Plant size | Duration type |
//! |---|---|
//! | < 50 kW_el | 20 years |
//! | 50 kW_el – 2 MW_el | 10 years |
//! | > 2 MW_el | 30 000 full-load hours (max 15 years) |
//!
//! For >2 MW plants the Förderdauer is tracked in kWh:
//! `kwk_max_kwh = leistung_kW_el × kwk_foerderdauer_h`

use rust_decimal::Decimal;
use time::Date;
use time::error::ComponentRange;

/// Compute the `foerderendedatum` for a **statutory** (nicht-Ausschreibungs) EEG plant.
///
/// ## Legal basis — §25 Abs. 1 Satz 1+2 EEG 2023 (unchanged since EEG 2017)
///
/// > "Marktprämien, Einspeisevergütungen oder Mieterstromzuschläge sind jeweils für
/// > die Dauer von 20 Jahren zu zahlen. **Bei Anlagen, deren anzulegender Wert gesetzlich
/// > bestimmt wird, verlängert sich dieser Zeitraum bis zum 31. Dezember des zwanzigsten
/// > Jahres der Zahlung.**"
///
/// For **statutory** AW plants (no BNetzA tender), the 20-year period always extends
/// to **31 December of the 20th calendar year**.
///
/// | Inbetriebnahme | foerderendedatum |
/// |---|---|
/// | 2010-05-15 | **2030-12-31** (not 2030-05-15) |
/// | 2023-01-01 | 2043-12-31 |
/// | 2023-12-31 | 2043-12-31 |
///
/// For tender plants (Ausschreibungsanlagen) the period ends at the exact 20-year
/// anniversary — use [`foerderendedatum_eeg_ausschreibung`] instead.
///
/// ## Which plants are statutory?
///
/// - Solar PV ≤ 750 kWp (§21 Abs. 1 Nr. 1 + §48 EEG 2023)
/// - Small wind ≤ 750 kW (§21 Abs. 1 Nr. 2)
/// - Biomasse ≤ 150 kW (§21 Abs. 1 Nr. 3)
/// - All other technology types below the tender threshold
///
/// Plants with a BNetzA Zuschlag (`ausschreibungs_zuschlag_id` set) use tender rules.
///
/// # Example
/// ```rust
/// use eeg_billing::foerderendedatum_eeg;
/// use time::macros::date;
/// // Statutory plant commissioned May 2010 → ends 2030-12-31, NOT 2030-05-15
/// assert_eq!(
///     foerderendedatum_eeg(date!(2010-05-15)).unwrap(),
///     date!(2030-12-31)
/// );
/// // Plant commissioned Dec 31, 2023 → still ends 2043-12-31
/// assert_eq!(
///     foerderendedatum_eeg(date!(2023-12-31)).unwrap(),
///     date!(2043-12-31)
/// );
/// ```
pub fn foerderendedatum_eeg(inbetriebnahme: Date) -> Result<Date, ComponentRange> {
    // §25 Abs. 1 Satz 2: extends to 31. Dezember des zwanzigsten Jahres der Zahlung.
    Date::from_calendar_date(inbetriebnahme.year() + 20, time::Month::December, 31)
}

/// Compute the `foerderendedatum` for a **tender** (Ausschreibungs) EEG plant.
///
/// ## Legal basis — §25 Abs. 1 Satz 1 EEG 2023
///
/// For plants whose AW was determined by BNetzA tender (`ausschreibungs_zuschlag_id` set),
/// Satz 2 does NOT apply — the period is exactly **20 years from `inbetriebnahme`**.
///
/// # Example
/// ```rust
/// use eeg_billing::foerderendedatum_eeg_ausschreibung;
/// use time::macros::date;
/// // Tender plant commissioned 2020-04-01 → exactly 20 years
/// assert_eq!(
///     foerderendedatum_eeg_ausschreibung(date!(2020-04-01)).unwrap(),
///     date!(2040-04-01)
/// );
/// ```
pub fn foerderendedatum_eeg_ausschreibung(inbetriebnahme: Date) -> Result<Date, ComponentRange> {
    inbetriebnahme.replace_year(inbetriebnahme.year() + 20)
}

/// Compute the `foerderendedatum` after a §22 EEG 2023 repowering event.
///
/// # Legal basis
/// §22 EEG 2023: replacing components with higher-capacity ones resets the
/// 20-year Förderdauer clock.  The new `foerderendedatum` =
/// `repowering_datum + 20 years` (statutory: extends to Dec 31).
///
/// The original commissioning date must be preserved in `ursprungs_inbetriebnahme`
/// for audit-trail purposes.
///
/// # Example
/// ```rust
/// use eeg_billing::foerderendedatum_repowering;
/// use time::macros::date;
/// // Repowering in 2025 resets to 2045-12-31:
/// assert_eq!(
///     foerderendedatum_repowering(date!(2025-03-01)).unwrap(),
///     date!(2045-12-31)
/// );
/// ```
pub fn foerderendedatum_repowering(repowering_datum: Date) -> Result<Date, ComponentRange> {
    // Repowering resets the clock; use the statutory rule (extend to Dec 31).
    Date::from_calendar_date(repowering_datum.year() + 20, time::Month::December, 31)
}

/// Compute the `foerderendedatum` for a year-limited KWKG plant (≤2 MW).
///
/// # Legal basis
/// §8 KWKG 2023: plants ≤2 MW have a year-based Förderdauer
/// (typically 10 or 20 years depending on capacity).
pub fn foerderendedatum_kwkg_years(
    inbetriebnahme: Date,
    foerderdauer_years: i16,
) -> Result<Date, ComponentRange> {
    inbetriebnahme.replace_year(inbetriebnahme.year() + i32::from(foerderdauer_years))
}

/// Compute the maximum total eligible kWh for a KWKG plant >2 MW.
///
/// # Legal basis
/// §8 KWKG 2023: plants >2 MW have a Förderdauer expressed in full-load hours
/// (typically 30 000 h).  The corresponding kWh limit is:
///
/// `kwk_max_kwh = leistung_kW_el × kwk_foerderdauer_h`
///
/// # Example
/// ```rust
/// use eeg_billing::kwk_max_kwh;
/// use rust_decimal::Decimal;
/// use std::str::FromStr;
/// // 2.5 MW plant, 30 000 full-load hours → 75 000 000 kWh cap
/// let limit = kwk_max_kwh(Decimal::from_str("2500").unwrap(), 30_000);
/// assert_eq!(limit, Decimal::from_str("75000000").unwrap());
/// ```
pub fn kwk_max_kwh(leistung_kw_el: Decimal, kwk_foerderdauer_h: i32) -> Decimal {
    leistung_kw_el * Decimal::from(kwk_foerderdauer_h)
}

/// Determine how many kWh are eligible for KWK-Zuschlag this period,
/// enforcing the §8 KWKG 2023 full-load-hour limit.
///
/// Returns `(eligible_kwh, limit_reached)`.
///
/// - `eligible_kwh` ≤ `produced_kwh`; may be 0 when limit is already exhausted.
/// - `limit_reached` is `true` when this period exhausts or exceeds the cap,
///   triggering a status transition to `FoerderungBeendet`.
///
/// # Example
/// ```rust
/// use eeg_billing::kwk_eligible_kwh;
/// use rust_decimal::Decimal;
/// use std::str::FromStr;
///
/// let d = |s| Decimal::from_str(s).unwrap();
/// // 29 900 kWh paid, 30 000 kWh cap, 400 kWh this period → prorated to 100 kWh
/// let (eligible, done) = kwk_eligible_kwh(d("400"), d("29900"), d("30000"));
/// assert_eq!(eligible, d("100"));
/// assert!(done, "limit reached in this period");
/// ```
pub fn kwk_eligible_kwh(
    produced_kwh: Decimal,
    already_paid_kwh: Decimal,
    max_kwh: Decimal,
) -> (Decimal, bool) {
    let remaining = (max_kwh - already_paid_kwh).max(Decimal::ZERO);
    if produced_kwh >= remaining {
        // Last period or already exhausted
        (remaining, true)
    } else {
        (produced_kwh, false)
    }
}

/// Statutory §20 Abs. 3 EEG 2023 Managementprämie in ct/kWh.
///
/// Paid by the NB to the Direktvermarkter for market-integration administration.
///
/// | Plant capacity | Rate |
/// |---|---|
/// | ≤ 100 MW | 0.4 ct/kWh |
/// | > 100 MW | 0.2 ct/kWh (§20 Abs. 3 Nr. 1 EEG 2023) |
///
/// Applies to `Direktvermarktung` and `Ausschreibung` models only.
///
/// # Example
/// ```rust
/// use eeg_billing::managementpraemie_ct;
/// use rust_decimal::Decimal;
/// use std::str::FromStr;
///
/// let standard = managementpraemie_ct(Decimal::from_str("2500").unwrap());    // 2.5 MW
/// let large    = managementpraemie_ct(Decimal::from_str("110000").unwrap()); // 110 MW
/// assert_eq!(standard, Decimal::from_str("0.4").unwrap());
/// assert_eq!(large,    Decimal::from_str("0.2").unwrap());
/// ```
pub fn managementpraemie_ct(leistung_kwp: Decimal) -> Decimal {
    // §20 Abs. 3 Nr. 1 EEG 2023: reduced to 0.2 ct/kWh for plants >100 MW
    // 100 MW = 100 000 kWp
    if leistung_kwp > Decimal::from(100_000u32) {
        Decimal::new(2, 1) // 0.2
    } else {
        Decimal::new(4, 1) // 0.4
    }
}

/// Compute the §8 Abs. 4 KWKG 2023 **calendar-year** Förderungsende for large CHP plants.
///
/// # Legal basis
/// §8 Abs. 4 KWKG 2023: even if the full-load-hour limit (e.g. 30 000 h) has not
/// been reached, the KWK-Zuschlag ends after **15 calendar years** from
/// commissioning.  The effective `foerderendedatum` is therefore:
///
/// `min(kwk_foerderend_hours, inbetriebnahme + 15 years)`
///
/// This function computes the calendar-year component.  Combine with
/// [`foerderendedatum_kwkg_years`] (with `foerderdauer_years = 15`) or use the
/// helper directly when the plant size class is ≤2 MW.
///
/// # Example
/// ```rust
/// use eeg_billing::kwk_foerderend_calendar;
/// use time::macros::date;
/// // Plant commissioned 2020-01-15: calendar Förderungsende = 2035-01-15
/// assert_eq!(
///     kwk_foerderend_calendar(date!(2020-01-15)).unwrap(),
///     date!(2035-01-15)
/// );
/// ```
pub fn kwk_foerderend_calendar(inbetriebnahme: Date) -> Result<Date, ComponentRange> {
    // §8 Abs. 4 KWKG: maximum 15 calendar years for large plants
    inbetriebnahme.replace_year(inbetriebnahme.year() + 15)
}

/// §51 EEG — Return `true` when the Negativpreisregel threshold is met for the given
/// EEG version and number of consecutive negative-price hours.
///
/// Delegates to [`EegGesetz::negativpreis_stunden_erreicht`].
///
/// # Example
///
/// ```rust
/// use eeg_billing::foerderdauer::negativpreis_rule_applies_for_version;
/// use eeg_billing::EegGesetz;
///
/// assert!(!negativpreis_rule_applies_for_version(5, EegGesetz::Eeg2017)); // 5h < 6h
/// assert!( negativpreis_rule_applies_for_version(6, EegGesetz::Eeg2017)); // 6h = threshold
/// assert!(!negativpreis_rule_applies_for_version(3, EegGesetz::Eeg2021)); // 3h < 4h
/// assert!( negativpreis_rule_applies_for_version(4, EegGesetz::Eeg2021)); // 4h = threshold
/// assert!( negativpreis_rule_applies_for_version(1, EegGesetz::Eeg2023)); // any period
/// assert!(!negativpreis_rule_applies_for_version(100, EegGesetz::Eeg2012)); // pre-2017: never
/// ```
pub fn negativpreis_rule_applies_for_version(
    consecutive_negative_hours: u32,
    eeg_gesetz: crate::version::EegGesetz,
) -> bool {
    eeg_gesetz.negativpreis_stunden_erreicht(consecutive_negative_hours)
}

/// §51 EEG — Return the minimum installed capacity in kW below which
/// the Negativpreisregel exemption applies.
///
/// Delegates to [`EegGesetz::negativpreis_kw_grenze`].
/// Pass `None` for `erzeugungsart` to use conservative defaults (non-wind).
///
/// # Example
///
/// ```rust
/// use eeg_billing::foerderdauer::negativpreis_kw_exemption;
/// use eeg_billing::{EegGesetz, ErzeugungsArt};
///
/// assert_eq!(negativpreis_kw_exemption(EegGesetz::Eeg2017, Some(ErzeugungsArt::WindOnshore)), Some(3000));
/// assert_eq!(negativpreis_kw_exemption(EegGesetz::Eeg2017, Some(ErzeugungsArt::Solar)),       Some(500));
/// assert_eq!(negativpreis_kw_exemption(EegGesetz::Eeg2021, Some(ErzeugungsArt::WindOnshore)), Some(500));
/// assert_eq!(negativpreis_kw_exemption(EegGesetz::Eeg2023, None),                             Some(100));
/// assert_eq!(negativpreis_kw_exemption(EegGesetz::Eeg2012, None),                             None);
/// ```
pub fn negativpreis_kw_exemption(
    eeg_gesetz: crate::version::EegGesetz,
    erzeugungsart: Option<crate::technology::ErzeugungsArt>,
) -> Option<u32> {
    let art = erzeugungsart.unwrap_or(crate::technology::ErzeugungsArt::Solar);
    eeg_gesetz.negativpreis_kw_grenze(&art)
}

/// Return `true` when the §51 EEG 2023 Negativpreisregel applies.
///
/// **Deprecated**: Use [`negativpreis_rule_applies_for_version`] with a typed
/// [`EegGesetz`][crate::EegGesetz] for version-specific behavior.
/// This function always assumes EEG 2023 rules (any negative period).
///
/// # Example
///
/// ```rust
/// use eeg_billing::foerderdauer::negativpreis_rule_applies;
/// assert!(!negativpreis_rule_applies(0));
/// assert!( negativpreis_rule_applies(1));
/// ```
pub fn negativpreis_rule_applies(consecutive_negative_hours: u32) -> bool {
    consecutive_negative_hours >= 1
}

/// §51a EEG 2021/2023 — Vergütungszeitraum-Verlängerung (extension for lost periods).
///
/// When §51 reduces the Vergütung to zero, the Förderdauer is extended to compensate.
///
/// ## EEG version differences
///
/// | EEG version | Unit tracked | Plants covered | Solar factor |
/// |---|---|---|---|
/// | EEG 2021 §51a | **Stunden** (hours) | Ausschreibungsanlagen only | none |
/// | EEG 2023 §51a | **Viertelstunden** (QH) | ALL plants (§51a Abs. 1) | ×0.5 (§51a Abs. 2) |
///
/// This function works in **quarter-hours** (EEG 2023 unit).
/// For EEG 2021 Ausschreibungsanlagen (which track hours), multiply `lost_hours × 4`
/// before calling, and use `is_solar = false` (EEG 2021 §51a had no solar factor).
///
/// ## §51a EEG 2023 Abs. 2 — Solar PV Volllastviertelstunden
///
/// For solar PV, the extension QH are multiplied by **0.5** (halved), rounded up to
/// the next full QH. This accounts for the lower effective utilization of solar plants.
/// The result is called "Volllastviertelstunden". The extension months are then
/// determined from a statutory monthly QH table (January = 87 QH, etc.).
///
/// # Returns
/// Number of additional **quarter-hours** to add to the Förderdauer.
///
/// # Example
/// ```rust
/// use eeg_billing::verguetungszeitraum_verlaengerung_qh;
/// // 40 quarter-hours lost; solar PV → 20 quarter-hours added (×0.5, rounded up)
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(40, true), 20);
/// // 41 quarter-hours; solar → ceiling(41×0.5) = 21
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(41, true), 21);
/// // Wind or non-solar: 1:1 extension
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(40, false), 40);
/// ```
pub fn verguetungszeitraum_verlaengerung_qh(lost_quarter_hours: u64, is_solar: bool) -> u64 {
    if is_solar {
        // §51a Abs. 2 EEG 2023: multiply by 0.5, round up to next full quarter-hour
        lost_quarter_hours.div_ceil(2)
    } else {
        // §51a Abs. 1 EEG 2023: 1:1 extension
        lost_quarter_hours
    }
}

/// §24 Abs. 1 Nr. 4 EEG 2023 — Check whether two plants fall within the
/// 12-consecutive-calendar-months commissioning window for Zusammenlegung.
///
/// Under §24 Abs. 1 EEG 2023, multiple plants at the same location that are
/// commissioned **within 12 consecutive calendar months** are treated as a single
/// plant for tariff-threshold purposes (`§21`, `§22`).
///
/// # Returns
/// `true` when the time condition is met (both plants in the same 12-month window).
/// The caller must additionally verify the location and energy-type conditions.
///
/// # Legal basis
/// §24 Abs. 1 Satz 1 Nr. 4 EEG 2023: "innerhalb von zwölf aufeinanderfolgenden
/// Kalendermonaten in Betrieb genommen worden sind."
///
/// # Example
/// ```rust
/// use eeg_billing::zusammenlegung_within_12_months;
/// use time::macros::date;
/// // Jan 2024 and Dec 2024: same 12-month window → true
/// assert!(zusammenlegung_within_12_months(date!(2024-01-15), date!(2024-12-15)));
/// // Jan 2024 and Jan 2025: exactly 12 months apart → true (boundary)
/// assert!(zusammenlegung_within_12_months(date!(2024-01-01), date!(2025-01-01)));
/// // Jan 2024 and Feb 2025: >12 months → false
/// assert!(!zusammenlegung_within_12_months(date!(2024-01-01), date!(2025-02-01)));
/// ```
pub fn zusammenlegung_within_12_months(ibn_a: Date, ibn_b: Date) -> bool {
    use time::Duration;
    let diff = if ibn_a > ibn_b {
        ibn_a - ibn_b
    } else {
        ibn_b - ibn_a
    };
    // 12 calendar months ≈ 366 days (conservative; covers leap-year February edge)
    diff <= Duration::days(366)
}

// ── §52 EEG 2023 Pflichtzahlung ───────────────────────────────────────────────

/// Compute the §52 EEG 2023 penalty payment owed by the plant operator to the NB.
///
/// ## §52 Abs. 2 — Base rate: **€10/kW/month**
///
/// For each calendar month in which a compliance violation (§52 Abs. 1) is wholly
/// or partially in effect.
///
/// ## §52 Abs. 3 — Reduced rate: **€2/kW/month** (retroactive)
///
/// When the obligation is subsequently fulfilled (`nachtraeglich_erfuellt = true`),
/// the penalty is retroactively reduced to €2/kW/month for violation types
/// Nr. 1 (Fernsteuerbarkeit), Nr. 3 (iMSys), Nr. 4 (Direktvermarktung), Nr. 11 (MaStR).
///
/// ## §52 Abs. 5 — Cap: €10/kW/month total
///
/// When multiple simultaneous violations exist, the total penalty is capped at
/// €10/kW/month. Callers with multiple violations should call this function
/// once per violation and cap the sum externally.
///
/// ## Bestandsschutz (§100 Abs. 3 EEG 2023)
///
/// Old plants commissioned before 01.01.2023 retain Bestandsschutz for the
/// Fernsteuerbarkeit obligation: legacy equipment (full-shutdown only, no modulation)
/// is treated as compliant until iMSys is installed and tested. For these plants,
/// `FernsteuerbarkeitmFehlend` should NOT be flagged if legacy equipment is present.
///
/// ## Return
///
/// EUR amount the operator owes the NB for the specified violation period.
///
/// ## §52 Abs. 2 EEG 2023 — Base rate
///
/// The base rate is **€10/kW/month** for all violations in §52 Abs. 1.
///
/// ## §52 Abs. 3 EEG 2023 — Rate reductions
///
/// | Rule | Violation types | Rate |
/// |---|---|---|
/// | Abs. 3 Nr. 1: retroactive on fulfillment | Nr. 1 (Fernsteuerbarkeit), Nr. 3 (iMSys), Nr. 4 (Direktverm.), Nr. 11 (MaStR) | €10 → **€2** retroactively |
/// | Abs. 3 Nr. 2: always €2 | Nr. 9a (§37a/§48 post-commissioning), Nr. 10 (Volleinspeisung) | **€2** always |
/// | All other types | Nr. 2, 5, 6, 7, 8, 9, 12 | **€10** (not reducible) |
///
/// ## §52 Abs. 3 Satz 2 — Defect grace (from 01.01.2024)
///
/// For violations Nr. 1, 3, 4, 8 caused by a **technical defect**, the payment is
/// waived for the month the defect occurs and the following month. The operator
/// must demonstrate the defect (Darlegungs- und Beweislast).
/// Track this by excluding those months from `monate_des_verstosses`.
///
/// ## §52 Abs. 4 — Additional months beyond the violation period
///
/// Some violations extend the payment obligation beyond when the violation ends:
/// - Nr. 7 (§21b Abs. 2): +3 additional months after violation ends
/// - Nr. 9 (§21b/§21c notification): +1 additional month
/// - Nr. 10 (Volleinspeisung): payment for ALL calendar months of the year
/// - Nr. 12 (§80 violation): +6 additional months
///
/// Include these extra months in `monate_des_verstosses` when calling this function.
///
/// ## §52 Abs. 5 — Monthly cap
///
/// When multiple violations occur in the same calendar month, the **total is capped
/// at €10/kW/month**. Apply this cap in the billing system when aggregating violations.
///
/// # Example
///
/// ```rust
/// use eeg_billing::{Pflichtverstoss, SanktionsTyp};
/// use eeg_billing::foerderdauer::calculate_pflichtzahlung;
/// use rust_decimal_macros::dec;
///
/// // 500 kW plant, MaStR not registered for 3 months, obligation NOT yet fulfilled
/// let violation = Pflichtverstoss {
///     typ: SanktionsTyp::MastrNichtRegistriert,
///     leistung_kw: dec!(500),
///     monate_des_verstosses: 3,
///     nachtraeglich_erfuellt: false,
/// };
/// let penalty = calculate_pflichtzahlung(&violation);
/// assert_eq!(penalty, dec!(15000)); // 500 kW × €10 × 3 months
///
/// // Same violation, but obligation was later fulfilled → retroactively €2/kW
/// let fulfilled = Pflichtverstoss { nachtraeglich_erfuellt: true, ..violation };
/// let reduced = calculate_pflichtzahlung(&fulfilled);
/// assert_eq!(reduced, dec!(3000)); // 500 kW × €2 × 3 months
///
/// // Nr. 9a / Nr. 10: ALWAYS €2/kW regardless of fulfillment
/// let nr10 = Pflichtverstoss {
///     typ: SanktionsTyp::VolleinspeisungspflichtVerletzt,
///     leistung_kw: dec!(500),
///     monate_des_verstosses: 12,
///     nachtraeglich_erfuellt: false, // has no effect for this type
/// };
/// assert_eq!(calculate_pflichtzahlung(&nr10), dec!(12000)); // 500 × €2 × 12
/// ```
pub fn calculate_pflichtzahlung(violation: &crate::model::Pflichtverstoss) -> Decimal {
    use crate::model::SanktionsTyp;
    use rust_decimal_macros::dec;

    // §52 Abs. 3 Nr. 2 — these types are ALWAYS €2/kW/month, not €10
    // (§37a Abs. 1a / §48 Abs. 6 post-commissioning; Volleinspeisung not maintained)
    let always_two_eur = matches!(
        violation.typ,
        SanktionsTyp::InbetriebnahmeVorgabeVerletzt | SanktionsTyp::VolleinspeisungspflichtVerletzt
    );
    if always_two_eur {
        return dec!(2) * violation.leistung_kw * Decimal::from(violation.monate_des_verstosses);
    }

    // §52 Abs. 3 Nr. 1 — retroactively reduces to €2/kW when obligation is fulfilled
    let reduction_eligible = matches!(
        violation.typ,
        SanktionsTyp::FernsteuerbarkeitmFehlend
            | SanktionsTyp::IMssAnforderungNichtErfuellt
            | SanktionsTyp::DirektvermarktungspflichtVerletzt
            | SanktionsTyp::MastrNichtRegistriert
    );

    let rate = if violation.nachtraeglich_erfuellt && reduction_eligible {
        dec!(2) // §52 Abs. 3 Nr. 1: reduced retroactively on fulfillment
    } else {
        dec!(10) // §52 Abs. 2: base rate for all other violations
    };

    rate * violation.leistung_kw * Decimal::from(violation.monate_des_verstosses)
}
