//! Förderdauer (subsidy duration) calculation helpers.
//!
//! All functions are pure and deterministic — no I/O, no clock access.
//!
//! ## EEG Förderdauer rules
//!
//! | Condition | Rule | Legal basis |
//! |---|---|---|
//! | Standard commissioning | 20 years + rest of the commissioning year | §25 Abs. 1 EEG 2023 |
//! | Repowering (Vollrepowering) | a fresh Inbetriebnahme restarts §25 | §3 Nr. 30 i.V.m. §25 EEG 2023 |
//! | §24 Zusammenlegung | parent's `foerderendedatum` unchanged | §24 EEG 2023 |
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

use crate::rounding::RoundMoney;
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
/// Whichever ones §22 Abs. 2–5 EEG 2023 leaves out of the Ausschreibung — that
/// is the *only* thing that decides it. §21 Abs. 1 Satz 1's Nummern are the four
/// Varianten of the Einspeisevergütung, not a per-technology threshold table.
///
/// - Solar, erstes Segment (Freifläche) ≤ 1 MW · zweites Segment (Gebäude,
///   Lärmschutzwand) ≤ 750 kW — §22 Abs. 3 Satz 2
/// - Wind an Land ≤ 1 MW — §22 Abs. 2 Satz 2 Nr. 1
/// - Biomasse ≤ 150 kW — §22 Abs. 4 Satz 2
/// - Wasserkraft, Deponie-, Klär- und Grubengas, Geothermie at **any** size —
///   §22 Abs. 5 Satz 2 keeps their AW gesetzlich bestimmt
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

/// Compute the `foerderendedatum` after a repowering event.
///
/// # Legal basis
/// A repowered generator is a fresh **Inbetriebnahme** (§3 Nr. 30 EEG 2023 — the
/// first commissioning of the *generator* after its establishment or renewal), so
/// its 20-year Förderdauer under §25 restarts. The new `foerderendedatum` =
/// `repowering_datum + 20 years` (statutory: extends to Dec 31). (§22 EEG 2023
/// governs the Ausschreibungs-Anspruchsvoraussetzungen, not the Förderdauer reset.)
///
/// **Important**: This function is only correct for **Vollrepowering**
/// (complete replacement of the turbine unit, `RepoweringScope::Full` or
/// `RepoweringScope::FullWithCapacityIncrease`).
///
/// For partial repowering (rotor-only, nacelle replacement), the original
/// commissioning date continues to govern — use `foerderendedatum_eeg`
/// with the **original** commissioning date instead.
/// See [`crate::technology::RepoweringScope`] for the legal distinctions.
///
/// The original commissioning date must be preserved in `ursprungs_inbetriebnahme`
/// for audit-trail purposes.
///
/// # Example
/// ```rust
/// use eeg_billing::foerderendedatum_repowering;
/// use time::macros::date;
/// // Full repowering in 2025 resets to 2045-12-31:
/// assert_eq!(
///     foerderendedatum_repowering(date!(2025-03-01)).unwrap(),
///     date!(2045-12-31)
/// );
/// ```
pub fn foerderendedatum_repowering(repowering_datum: Date) -> Result<Date, ComponentRange> {
    // Full repowering resets the clock; use the statutory rule (extend to Dec 31).
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
/// // Wind/biomass/other: §51a Abs. 1 Satz 2 rounds the lost QH UP to the next
/// // full calendar day (96 QH) — 40 QH → 96 QH (one day), 100 QH → 192 QH (two).
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(40, false), 96);
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(100, false), 192);
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(0, false), 0);
/// ```
pub fn verguetungszeitraum_verlaengerung_qh(lost_quarter_hours: u64, is_solar: bool) -> u64 {
    if is_solar {
        // §51a Abs. 2 EEG 2023: multiply by 0.5, round up to next full quarter-hour
        // (Volllastviertelstunden). The month-by-month draw-down is applied by the
        // caller via `volllastviertelstunden_im_monat` / `solar_verlaengerung_ende`.
        lost_quarter_hours.div_ceil(SECT51A_SOLAR_FACTOR_DENOMINATOR)
    } else {
        // §51a Abs. 1 Satz 2 EEG 2023: "Die nach Satz 1 ermittelte Anzahl an
        // Viertelstunden wird aufgerundet auf den nächsten vollen Kalendertag."
        // A calendar day is 96 quarter-hours, so the extension is a whole number
        // of days — returning the raw QH under-extended wind/biomass Förderenden.
        const QH_PER_DAY: u64 = 96;
        lost_quarter_hours.div_ceil(QH_PER_DAY) * QH_PER_DAY
    }
}

/// §51a Abs. 2 EEG 2023 — the solar halving denominator.
///
/// For Solaranlagen the Vergütungszeitraum is extended by only **half** the lost
/// quarter-hours, rounded up; wind, biomass and the rest extend 1:1 (rounded up
/// to a full calendar day by Abs. 1 Satz 2). This is the only place the factor
/// appears.
pub const SECT51A_SOLAR_FACTOR_DENOMINATOR: u64 = 2;

/// §51a Abs. 2 Satz 3 EEG 2023 — Volllastviertelstunden per calendar month.
///
/// The statutory table. Index 0 is January.
///
/// A solar plant's extension is not a span of wall-clock time but a *contingent*
/// of Volllastviertelstunden, drawn down at a different rate in each month —
/// which is what makes a December extension take far longer than a June one.
const VOLLLASTVIERTELSTUNDEN_JE_MONAT: [u32; 12] =
    [87, 189, 340, 442, 490, 508, 498, 453, 371, 231, 118, 73];

/// The Volllastviertelstunden a whole calendar month contributes (§51a Abs. 2 Satz 3).
///
/// # Panics
///
/// Does not panic: `time::Month` is always one of the twelve.
#[must_use]
pub fn volllastviertelstunden_im_monat(month: time::Month) -> u32 {
    VOLLLASTVIERTELSTUNDEN_JE_MONAT[(month as u8 - 1) as usize]
}

/// §51a Abs. 2 Satz 4–6 EEG 2023 — the date a solar plant's extended
/// Vergütungszeitraum ends.
///
/// The contingent from [`verguetungszeitraum_verlaengerung_qh`] is drawn down
/// month by month at the statutory rate until it is exhausted; the
/// Vergütungszeitraum then runs **to the end of the month** in which the last
/// Volllastviertelstunde falls (Satz 6).
///
/// The month in which the original period ends contributes only pro rata for its
/// remaining days (Satz 4): `verbleibende Tage / Tage des Monats × VLVh des Monats`.
///
/// Returns `original_ende` unchanged when the contingent is zero — no negative
/// prices means no extension.
///
/// ## Why this is not `original_ende + n days`
///
/// A contingent of 500 Volllastviertelstunden is roughly one month if it starts
/// in June (508) and roughly seven if it starts in December (73, 87, 189, …).
/// Converting the contingent to a fixed number of days would misstate the end of
/// the Förderung by months for exactly the plants that lost the most revenue.
///
/// # Errors
///
/// Returns [`ComponentRange`] if the computed end date falls outside the
/// representable range.
pub fn solar_verlaengerung_ende(
    original_ende: Date,
    kontingent_vlvh: u64,
) -> Result<Date, ComponentRange> {
    if kontingent_vlvh == 0 {
        return Ok(original_ende);
    }

    let mut rest = Decimal::from(kontingent_vlvh);
    let mut jahr = original_ende.year();
    let mut monat = original_ende.month();

    // Satz 4 — the original period's own month counts only for the days left in it.
    let tage_im_monat = u32::from(time::util::days_in_month(monat, jahr));
    let verbleibende_tage = tage_im_monat - u32::from(original_ende.day());
    let anteilig = Decimal::from(verbleibende_tage) / Decimal::from(tage_im_monat)
        * Decimal::from(volllastviertelstunden_im_monat(monat));

    if anteilig >= rest {
        // Exhausted inside the original period's own month: Satz 6 still runs the
        // Vergütungszeitraum to the end of that month.
        return letzter_tag_des_monats(jahr, monat);
    }
    rest -= anteilig;

    // Satz 5 — draw down whole months until the contingent is used up.
    loop {
        (jahr, monat) = naechster_monat(jahr, monat);
        let monats_vlvh = Decimal::from(volllastviertelstunden_im_monat(monat));
        if monats_vlvh >= rest {
            // Satz 6 — to the end of the month carrying the last Volllastviertelstunde.
            return letzter_tag_des_monats(jahr, monat);
        }
        rest -= monats_vlvh;
    }
}

fn naechster_monat(jahr: i32, monat: time::Month) -> (i32, time::Month) {
    if monat == time::Month::December {
        (jahr + 1, time::Month::January)
    } else {
        (jahr, monat.next())
    }
}

fn letzter_tag_des_monats(jahr: i32, monat: time::Month) -> Result<Date, ComponentRange> {
    Date::from_calendar_date(jahr, monat, time::util::days_in_month(monat, jahr))
}

/// §51a EEG 2023 — the plant's **effective** Förderende after the accrued
/// negative-price extension is applied.
///
/// `negative_qh_gesamt` is the TOTAL raw quarter-hours in which the anzulegender
/// Wert was reduced to null under §51 across the commissioning year and the
/// following 19 calendar years (§51a Abs. 1 Satz 1) — summed over every monthly
/// settlement. Rounding is applied **once over the total**, so accruing raw QH
/// (not the per-month rounded extension) is essential: `ceil(40/96)+ceil(40/96)`
/// would extend by two days where the single 80-QH total extends by one.
///
/// - **Solar** (Abs. 2): the contingent `ceil(qh/2)` Volllastviertelstunden is
///   drawn down month by month via [`solar_verlaengerung_ende`].
/// - **Others** (Abs. 1 Satz 2): the total rounds up to whole calendar days
///   (96 QH per day) added to the original Förderende.
///
/// Returns `original_ende` unchanged when nothing accrued.
///
/// # Errors
/// [`ComponentRange`] if the extended date leaves the representable range.
pub fn effektives_foerderende(
    original_ende: Date,
    negative_qh_gesamt: u64,
    is_solar: bool,
) -> Result<Date, ComponentRange> {
    if negative_qh_gesamt == 0 {
        return Ok(original_ende);
    }
    if is_solar {
        // ×0.5 → Volllastviertelstunden contingent, drawn down per the monthly table.
        let kontingent = verguetungszeitraum_verlaengerung_qh(negative_qh_gesamt, true);
        solar_verlaengerung_ende(original_ende, kontingent)
    } else {
        // Abs. 1 Satz 2: whole calendar days (96 QH/day), rounded up once.
        let full_days = i32::try_from(negative_qh_gesamt.div_ceil(96)).unwrap_or(i32::MAX);
        Date::from_julian_day(original_ende.to_julian_day() + full_days)
    }
}

// ── §52 EEG 2023 Pflichtzahlung ───────────────────────────────────────────────

/// Compute the §52 EEG 2023 penalty payment owed by the plant operator to the NB.
///
/// Returns the EUR amount for one violation over `monate_des_verstosses`.
///
/// ## The rate — §52 Abs. 2 and Abs. 3
///
/// The base rate is **€10/kW/month** for every violation in §52 Abs. 1, for each
/// calendar month the violation is wholly or partly in effect. Abs. 3 reduces it:
///
/// | Rule | Violation types | Rate |
/// |---|---|---|
/// | Abs. 3 Nr. 1 — retroactive on fulfilment | Nr. 1 (Fernsteuerbarkeit), Nr. 3 (iMSys), Nr. 4 (Direktverm.), Nr. 11 (MaStR) | €10 → **€2** retroactively |
/// | Abs. 3 Nr. 2 — always | Nr. 9a (§37 Abs. 1a / §48), Nr. 10 (Volleinspeisung) | **€2** |
/// | otherwise | Nr. 2, 5, 6, 7, 8, 9, 12 | **€10**, not reducible |
///
/// ## What the caller must supply
///
/// Three rules act on the *months*, not the rate, so they are the caller's to
/// fold into `monate_des_verstosses`:
///
/// - **Abs. 3 Satz 2 (from 01.01.2024)** — for Nr. 1, 3, 4, 8 caused by a
///   technical defect, the month of the defect and the following one are waived.
///   The operator carries the Darlegungs- und Beweislast, so exclude those months.
/// - **Abs. 4** — some violations run past the violation itself: Nr. 7
///   (§21b Abs. 2) +3 months, Nr. 9 (§21b/§21c notification) +1, Nr. 12 (§80) +6,
///   and Nr. 10 (Volleinspeisung) covers every calendar month of the year.
/// - **Abs. 5** — concurrent violations are capped at **€10/kW/month in total**.
///   This function prices one violation, so cap the sum outside it.
///
/// ## Bestandsschutz (§100 Abs. 3 EEG 2023)
///
/// A plant commissioned before 01.01.2023 keeps Bestandsschutz for the
/// Fernsteuerbarkeit obligation: legacy equipment (full shutdown only, no
/// modulation) counts as compliant until an iMSys is installed and tested, so
/// `FernsteuerbarkeitFehlend` does not apply to it.
///
/// # Example
///
/// ```rust
/// use eeg_billing::{Pflichtverstoss, SanktionsTyp};
/// use eeg_billing::foerderdauer::calculate_pflichtzahlung;
/// use rust_decimal::dec;
///
/// // 500 kW plant, MaStR not registered for 3 months, obligation NOT yet fulfilled
/// let violation = Pflichtverstoss {
///     typ: SanktionsTyp::MastrNichtRegistriert,
///     leistung_kw: dec!(500),
///     monate_des_verstosses: 3,
///     nachtraeglich_erfuellt: false,
///     technischer_defekt: false,
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
///     nachtraeglich_erfuellt: false,
///     technischer_defekt: false, // has no effect for this type
/// };
/// assert_eq!(calculate_pflichtzahlung(&nr10), dec!(12000)); // 500 × €2 × 12
/// ```
pub fn calculate_pflichtzahlung(violation: &crate::model::Pflichtverstoss) -> Decimal {
    use crate::model::SanktionsTyp;
    use rust_decimal::dec;

    // §52 Abs. 3 Nr. 2 — these types are ALWAYS €2/kW/month, not €10
    // (§37 Abs. 1a / §48 Abs. 6 post-commissioning; Volleinspeisung not maintained)
    let always_two_eur = matches!(
        violation.typ,
        SanktionsTyp::InbetriebnahmeVorgabeVerletzt | SanktionsTyp::VolleinspeisungspflichtVerletzt
    );
    if always_two_eur {
        return dec!(2) * violation.leistung_kw * Decimal::from(violation.monate_des_verstosses);
    }

    // §52 Abs. 3 Satz 2 — technical defect grace: first 2 months waived.
    // Applies to Nr. 1 (Fernsteuerbarkeit), Nr. 3 (iMSys), Nr. 4 (§10b), Nr. 8 (§21b Abs. 3).
    // Only for violations occurring after 31 December 2023.
    let defect_grace_eligible = matches!(
        violation.typ,
        SanktionsTyp::FernsteuerbarkeitFehlend
            | SanktionsTyp::IMssAnforderungNichtErfuellt
            | SanktionsTyp::Sect10bVorgabenVerletzt
            | SanktionsTyp::VeraeusserungsformNachweispflichtVerletzt
    );
    let effective_months = if violation.technischer_defekt && defect_grace_eligible {
        // §52 Abs. 3 Satz 2: waive the defect month + following month
        violation.monate_des_verstosses.saturating_sub(2)
    } else {
        violation.monate_des_verstosses
    };
    if effective_months == 0 {
        return Decimal::ZERO;
    }

    // §52 Abs. 3 Nr. 1 — retroactively reduces to €2/kW when obligation is fulfilled
    let reduction_eligible = matches!(
        violation.typ,
        SanktionsTyp::FernsteuerbarkeitFehlend
            | SanktionsTyp::IMssAnforderungNichtErfuellt
            | SanktionsTyp::Sect10bVorgabenVerletzt
            | SanktionsTyp::MastrNichtRegistriert
    );

    let rate = if violation.nachtraeglich_erfuellt && reduction_eligible {
        dec!(2) // §52 Abs. 3 Nr. 1: reduced retroactively on fulfillment
    } else {
        dec!(10) // §52 Abs. 2: base rate for all other violations
    };

    rate * violation.leistung_kw * Decimal::from(effective_months)
}

/// §36h EEG 2023 — Corrected Anzulegender Wert for wind onshore plants.
///
/// Multiplies the statutory base AW by the certified Korrekturfaktor to obtain
/// the effective AW for the current settlement period.
///
/// ## Legal basis
/// §36h EEG 2023: the AW for wind onshore is adjusted by a location-specific
/// Korrekturfaktor that reflects the ratio of local to reference yield.
/// Factors are certified by a BNetzA-accredited Gutachter.
///
/// ## Korrekturfaktor interpretation
///
/// | Gütegrad (local/reference yield) | Korrekturfaktor | Effect |
/// |---|---|---|
/// | ≥ 150 % | 0.70–0.84 | Lower AW (excellent wind site) |
/// | 100 % | 1.00 | No change (reference site) |
/// | 80 % | 1.10–1.15 | Higher AW (poor wind site) |
///
/// ## When to use
///
/// Supply the certified Korrekturfaktor for `SettleInput.wind_korrekturfaktor`
/// and the uncorrected statutory AW for `direktverm_aw_ct`. The engine applies
/// this function automatically.
///
/// OR: apply this function when storing the initial plant record in `einsd`
/// to pre-compute the corrected AW for the `direktverm_aw_ct` column.
///
/// ## Pre-2017 Bestandsschutz
/// §36h does not apply to EEG ≤2012 plants (§100 Abs. 1 EEG 2023). Do not
/// supply a Korrekturfaktor for these plants.
///
/// # Example
/// ```rust
/// use eeg_billing::foerderdauer::wind_onshore_korrekturfaktor_corrected_aw;
/// use rust_decimal::dec;
///
/// // Statutory base AW = 7.35 ct/kWh, Korrekturfaktor = 1.08 (low-wind site)
/// let corrected = wind_onshore_korrekturfaktor_corrected_aw(dec!(7.35), dec!(1.08));
/// // 7.35 × 1.08 = 7.938 (rounded to 5 decimal places)
/// assert_eq!(corrected, dec!(7.938));
/// ```
pub fn wind_onshore_korrekturfaktor_corrected_aw(
    base_aw_ct_kwh: Decimal,
    korrekturfaktor: Decimal,
) -> Decimal {
    (base_aw_ct_kwh * korrekturfaktor).round_kfm(5)
}

// ── §52a Netztrennung ─────────────────────────────────────────────────────────

/// §52a EEG 2023 — Check whether mandatory grid disconnection warning must be issued.
///
/// Under §52a Abs. 1 EEG 2023, the NB **must** disconnect the plant (or issue a
/// one-month warning first per §52a Abs. 2) when the operator has violated §9 Abs. 1/2
/// or §10b in **at least 6 out of the last 12 calendar months**.
///
/// ## Parameters
///
/// `violation_months_in_12_month_window` — number of distinct calendar months in the
/// last 12 months in which the plant was in violation of §9 Abs. 1/2 (Fernsteuerbarkeit)
/// or §10b (Direktvermarktungspflicht). The caller tracks this from the violation start
/// dates and the billing history.
///
/// ## Return value
///
/// `true` when `violation_months_in_12_month_window >= 6`.
///
/// ## Legal basis
///
/// §52a Abs. 1 EEG 2023:
/// *„Der Netzbetreiber... muss die Anlage... vom Netz trennen... wenn der Anlagenbetreiber
/// hinsichtlich dieser Anlage in einem Zeitraum von zwölf Monaten in insgesamt mindestens
/// sechs Monaten jeweils mindestens einmal gegen §9 Absatz 1 oder Absatz 2 oder gegen
/// §10b Absatz 1 verstoßen hat..."*
///
/// # Example
///
/// ```rust
/// use eeg_billing::foerderdauer::sect52a_netztrennung_erforderlich;
/// assert!(sect52a_netztrennung_erforderlich(6));
/// assert!(!sect52a_netztrennung_erforderlich(5));
/// ```
#[must_use]
pub fn sect52a_netztrennung_erforderlich(violation_months_in_12_month_window: u32) -> bool {
    violation_months_in_12_month_window >= 6
}

// ── §52 Abs. 6 Satz 3 Verjährung ─────────────────────────────────────────────

/// §52 Abs. 6 Satz 3 EEG 2023 — Compute the Verjährungsdatum for a §52 penalty claim.
///
/// The NB's claim for §52 Pflichtzahlung expires at the end of the **second calendar
/// year following the year of the violation**.
///
/// Legal basis: §52 Abs. 6 Satz 3 EEG 2023:
/// *„Der Anspruch auf die Zahlung verjährt mit Ablauf des zweiten Kalenderjahres,
/// das auf den Pflichtverstoß nach Absatz 1 folgt."*
///
/// | Violation year | Verjährungsdatum |
/// |---|---|
/// | 2023 | **2025-12-31** |
/// | 2024 | **2026-12-31** |
/// | 2025 | **2027-12-31** |
///
/// # Example
///
/// ```rust
/// use eeg_billing::foerderdauer::pflichtzahlung_verjaehrt_am;
/// use time::macros::date;
///
/// assert_eq!(pflichtzahlung_verjaehrt_am(2024).unwrap(), date!(2026-12-31));
/// assert_eq!(pflichtzahlung_verjaehrt_am(2023).unwrap(), date!(2025-12-31));
/// ```
pub fn pflichtzahlung_verjaehrt_am(
    violation_year: i32,
) -> Result<time::Date, time::error::ComponentRange> {
    time::Date::from_calendar_date(violation_year + 2, time::Month::December, 31)
}

// ── §25 billing_days_fraction ─────────────────────────────────────────────────

/// §25 Abs. 1 Satz 3 / §26 Abs. 1 EEG 2023 — the partial-month billing fraction
/// for the **Förderende** month.
///
/// Returns `Some(day / days_in_month)` when the Förderdauer ends part-way through
/// the billing month, `None` for full months (the common case).
///
/// ## Why the commissioning month is *not* prorated
///
/// Proration exists to cut a month's *entitlement* down to the eligible days. In
/// the Förderende month the meter keeps running past the entitlement end, so the
/// reading covers more than the plant may be paid for and must be narrowed. The
/// commissioning month is the opposite case: §25 Abs. 1 Satz 3 starts the period
/// at the Inbetriebnahme, and the meter only starts recording then — a plant
/// commissioned on 15 June reports the 500 kWh it fed in from 15 June, already
/// the partial month. Scaling that by 16/30 would bill a fraction of a fraction
/// and underpay the operator by roughly half of their first month.
///
/// ## Legal basis
///
/// §25 Abs. 1 Satz 3 EEG 2023: "Beginn der Frist… ist der Zeitpunkt der Inbetriebnahme."
/// §26 Abs. 1 EEG 2023: monthly advance payments for the billing month.
///
/// # Example
///
/// ```rust
/// use eeg_billing::foerderdauer::compute_billing_days_fraction;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// // Förderende on 20 June → 20/30 of the month is still entitled.
/// let fraction = compute_billing_days_fraction(Some(date!(2024-06-20)), Some(date!(2024-06-01)));
/// assert_eq!(fraction, Some(dec!(20) / dec!(30)));
///
/// // Full month → None.
/// assert!(compute_billing_days_fraction(None, Some(date!(2024-06-01))).is_none());
/// ```
pub fn compute_billing_days_fraction(
    foerderendedatum: Option<Date>,
    billing_date: Option<Date>,
) -> Option<rust_decimal::Decimal> {
    let bd = billing_date?;
    let by = bd.year();
    let bm = bd.month();
    let days_in_month = bm.length(by) as i64;

    // Check Förderdauer expiry in current billing month
    if let Some(fed) = foerderendedatum
        && fed.year() == by
        && fed.month() == bm
        && fed.day() < days_in_month as u8
    {
        let days_active = fed.day() as i64;
        return Some(
            rust_decimal::Decimal::from(days_active) / rust_decimal::Decimal::from(days_in_month),
        );
    }

    None
}

#[cfg(test)]
mod effektives_foerderende_tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn zero_qh_leaves_foerderende_unchanged() {
        let ende = date!(2043 - 12 - 31);
        assert_eq!(effektives_foerderende(ende, 0, false).unwrap(), ende);
        assert_eq!(effektives_foerderende(ende, 0, true).unwrap(), ende);
    }

    #[test]
    fn non_solar_rounds_total_to_full_days_once() {
        let ende = date!(2043 - 12 - 31);
        // 96 QH = exactly 1 day.
        assert_eq!(
            effektives_foerderende(ende, 96, false).unwrap(),
            date!(2044 - 01 - 01)
        );
        // 100 QH (1.04 d) rounds up to 2 days — proving the round-once semantics:
        // two 50-QH months accrue 100 raw QH → 2 days, not 2×1 day from per-month rounding.
        assert_eq!(
            effektives_foerderende(ende, 100, false).unwrap(),
            date!(2044 - 01 - 02)
        );
    }

    #[test]
    fn solar_draws_down_the_volllastviertelstunden_contingent() {
        // 100 raw QH → ceil(100/2)=50 Volllastviertelstunden contingent. Ending in
        // December (73 VLVh/month) it spills across months, unlike the day model.
        let ende = date!(2043 - 12 - 31);
        let solar = effektives_foerderende(ende, 100, true).unwrap();
        let non_solar = effektives_foerderende(ende, 100, false).unwrap();
        assert!(solar > ende);
        // Solar's monthly draw-down extends further than the flat day rounding.
        assert!(solar >= non_solar);
    }
}

#[cfg(test)]
mod sect51a_solar_tests {
    use super::*;
    use time::macros::date;

    /// §51a Abs. 2 Satz 3 — the statutory table, verbatim.
    #[test]
    fn statutory_month_table() {
        use time::Month::*;
        for (m, expected) in [
            (January, 87),
            (February, 189),
            (March, 340),
            (April, 442),
            (May, 490),
            (June, 508),
            (July, 498),
            (August, 453),
            (September, 371),
            (October, 231),
            (November, 118),
            (December, 73),
        ] {
            assert_eq!(volllastviertelstunden_im_monat(m), expected, "{m:?}");
        }
    }

    /// No negative prices → no extension.
    #[test]
    fn zero_contingent_does_not_extend() {
        let ende = date!(2045 - 06 - 15);
        assert_eq!(solar_verlaengerung_ende(ende, 0).unwrap(), ende);
    }

    /// Satz 6 — the period runs to the end of the month carrying the last
    /// Volllastviertelstunde, even when the contingent is exhausted on day one.
    #[test]
    fn small_contingent_still_runs_to_month_end() {
        // 15 June: 15 days left of 30 → 15/30 × 508 = 254 VLVh available.
        // A contingent of 10 is exhausted inside June, so the period ends 30 June.
        assert_eq!(
            solar_verlaengerung_ende(date!(2045 - 06 - 15), 10).unwrap(),
            date!(2045 - 06 - 30)
        );
    }

    /// Satz 4 + 5 — the partial month is consumed first, then whole months.
    #[test]
    fn contingent_spans_into_following_months() {
        // 15 June leaves 254. Contingent 700 → 700-254 = 446 into July (498).
        // 498 >= 446, so the last VLVh falls in July → end of July.
        assert_eq!(
            solar_verlaengerung_ende(date!(2045 - 06 - 15), 700).unwrap(),
            date!(2045 - 07 - 31)
        );
    }

    /// The same contingent lasts far longer starting in winter — which is the
    /// whole reason the extension is a contingent and not a fixed span of days.
    #[test]
    fn winter_start_extends_much_further_than_summer_start() {
        let contingent = 700;
        let summer = solar_verlaengerung_ende(date!(2045 - 06 - 15), contingent).unwrap();
        let winter = solar_verlaengerung_ende(date!(2045 - 12 - 15), contingent).unwrap();

        // 15 Dec leaves 16/31 × 73 = 37.7. Then Jan 87, Feb 189, Mar 340 → 653.7
        // still short of 700; April (442) carries the last one → end of April.
        assert_eq!(summer, date!(2045 - 07 - 31));
        assert_eq!(winter, date!(2046 - 04 - 30));
    }

    /// The draw-down crosses the year boundary correctly.
    #[test]
    fn contingent_crosses_the_year_boundary() {
        // 31 Dec leaves 0 days of December, so the whole contingent falls in the
        // new year: Jan 87 >= 50 → end of January.
        assert_eq!(
            solar_verlaengerung_ende(date!(2045 - 12 - 31), 50).unwrap(),
            date!(2046 - 01 - 31)
        );
    }

    /// February's length is taken from the actual year, not assumed.
    #[test]
    fn february_is_leap_year_aware() {
        // 2048 is a leap year: 15 Feb leaves 14 of 29 days → 14/29 × 189 = 91.2
        assert_eq!(
            solar_verlaengerung_ende(date!(2048 - 02 - 15), 90).unwrap(),
            date!(2048 - 02 - 29)
        );
    }
}
