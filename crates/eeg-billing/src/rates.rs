//! Statutory EEG feed-in tariff rate tables.
//!
//! Provides [`billing::RateLookup`] tables for the most common EEG technology types
//! and law years.  The rate is determined by the plant's **installed capacity (kWp)**
//! at commissioning.
//!
//! ## § 53 EEG — Verringerung der Einspeisevergütung (critical for correct billing)
//!
//! **All functions in this module return the statutory `anzulegender Wert` (AW — gross
//! rate).** Before storing in `einsd`'s `verguetungssatz_ct` column or using for monthly
//! settlement, you MUST subtract the §53 deduction:
//!
//! | Technology | §53 deduction | Net Vergütung |
//! |---|---|---|
//! | Solar PV, Wind | **−0.4 ct/kWh** | AW − 0.4 |
//! | Wasserkraft, Biomasse, Geothermie, Deponie-/Klär-/Grubengas | **−0.2 ct/kWh** | AW − 0.2 |
//!
//! §53 applies to all EEG versions (2017, 2021, 2023) for **Einspeisevergütung** models.
//! It does NOT apply to Direktvermarktung/Marktprämie, PostEegSpot, or KWKG.
//!
//! ## §53 Abs. 2 EEG 2023 — Exception
//!
//! No §53 deduction for plants using **unentgeltliche Abnahme** (ausgeförderte Anlagen,
//! i.e., after 20-year Förderdauer: `PostEegSpot` model).
//!
//! ## §53 Abs. 3 EEG 2023 — Ausfallvergütung
//!
//! When **Ausfallvergütung** is used (instead of regular Einspeisevergütung), the AW
//! is reduced by 20% instead of the flat ct/kWh deduction. This is covered by
//! `SanktionAlt::VerguetungReduziert20Prozent`.
//!
//! ## Usage
//!
//! ```rust
//! use eeg_billing::rates;
//! use eeg_billing::ErzeugungsArt;
//! use rust_decimal::dec;
//! use time::macros::date;
//!
//! // Gross AW for a 15 kWp rooftop plant commissioned in the 1 Feb 2025 window.
//! let gross_aw = rates::solar_pv_ueberschuss_aw_ct(dec!(15), date!(2025-04-01)).unwrap();
//! assert_eq!(gross_aw, dec!(7.28)); // §48 Abs. 2 Nr. 2: 7.50 × 0.99³
//! // Net Einspeisevergütung = AW − §53 Abs. 1 deduction (0.4 ct for solar).
//! let net = gross_aw - rates::sect53_deduction(ErzeugungsArt::SolarAufdach);
//! assert_eq!(net, dec!(6.88));
//! ```
//!
//! ## Source
//!
//! - **§48 Abs. 2 / Abs. 2a EEG 2023** in the version §101 Abs. 1 Satz 2 keeps in
//!   force pending EU state-aid approval, i.e. the one as at 15 May 2024
//! - **§49 EEG 2023** for the semi-annual degression, cross-checked against the
//!   Bundesnetzagentur "Anzulegende Werte für Solaranlagen" spreadsheets
//! - **§53 Abs. 1 EEG 2023** for the Einspeisevergütung deduction
//!
//! **Important:** the solar figures here are **gross anzulegende Werte** for the
//! §49 window of a given commissioning date. Non-solar tables are reference
//! starting rates only; for those, `einsd`'s `lookup_verguetungssatz` DB function
//! holds the full per-window series.
//!
//! ## KWKG rates
//!
//! KWKG 2023 KWK-Zuschlag rates are determined by plant size and commissioning year
//! (§7 KWKG 2023 Anlage).  §53 does NOT apply to KWKG.  See [`kwkg_zuschlag_lookup`].

use billing::{Amount, BillingError, RateLookup};
use rust_decimal::Decimal;
use rust_decimal::dec;

// ── Solar PV — §48 EEG 2023 ───────────────────────────────────────────────────

/// **§48 Abs. 2 EEG 2023** — the Gebäude/Lärmschutzwand base anzulegende Werte,
/// in ct/kWh, before any §49 degression.
///
/// | Installed capacity | Anzulegender Wert |
/// |---|---|
/// | ≤ 10 kW | 8,60 ct |
/// | ≤ 40 kW | 7,50 ct |
/// | ≤ 1 MW | 6,20 ct |
///
/// ## Which version of §48 Abs. 2 this is
///
/// The consolidated text of §48 Abs. 2 currently reads 8,51 / 7,43 / 7,64 ct —
/// the Solarpaket-I figures. Those are **not yet in force**: §101 Abs. 1 Satz 1
/// lists § 48 Absatz 2 among the provisions that "erst nach der beihilferechtlichen
/// Genehmigung durch die Europäische Kommission … angewandt werden" dürfen, and
/// Satz 2 directs that until then "§ 48 Absatz 2 … in der am 15. Mai 2024
/// geltenden Fassung anzuwenden" is — the 8,60 / 7,50 / 6,20 ladder below.
///
/// This is also what the Bundesnetzagentur actually publishes: its "Anzulegende
/// Werte für Solaranlagen" series runs the §49 degression off 8,60 / 7,50 / 6,20
/// in every window through 2026/27.
pub const SOLAR_GEBAEUDE_BASIS_CT: [(Decimal, Decimal); 3] = [
    (dec!(10), dec!(8.60)),
    (dec!(40), dec!(7.50)),
    (dec!(1_000), dec!(6.20)),
];

/// **§48 Abs. 2a EEG 2023** — the Volleinspeisung uplift on top of Abs. 2, in ct/kWh.
///
/// Payable where the operator feeds in the entire annual generation and told the
/// Netzbetreiber so in text form before the deadline in Satz 1. The uplift is
/// itself subject to §49 (which names Absatz 2a expressly), so it degresses in
/// step with the base rather than staying nominal.
///
/// | Installed capacity | Uplift |
/// |---|---|
/// | ≤ 10 kW | +4,8 ct |
/// | ≤ 40 kW | +3,8 ct |
/// | ≤ 100 kW | +5,1 ct |
/// | ≤ 400 kW | +3,2 ct |
/// | ≤ 1 MW | +1,9 ct |
pub const SOLAR_VOLLEINSPEISUNG_ZUSCHLAG_CT: [(Decimal, Decimal); 5] = [
    (dec!(10), dec!(4.8)),
    (dec!(40), dec!(3.8)),
    (dec!(100), dec!(5.1)),
    (dec!(400), dec!(3.2)),
    (dec!(1_000), dec!(1.9)),
];

/// **§48 Abs. 1 EEG 2023** — the base anzulegender Wert for the Freiflächen and
/// other Nicht-Gebäude forms: 7,00 ct/kWh, likewise before §49 degression.
pub const SOLAR_FREIFLAECHE_BASIS_CT: Decimal = dec!(7.00);

/// The first day §48 Abs. 2/2a apply from — EEG 2023 in force.
///
/// Plants commissioned earlier settle under their own EEG version's quarterly
/// tables, which no three-row constant can express; resolve those from `einsd`'s
/// `eeg_verguetungssaetze` table instead.
const EEG2023_START: time::Date = time::macros::date!(2023 - 01 - 01);

fn tier_ct(table: &[(Decimal, Decimal)], leistung_kwp: Decimal) -> Option<Decimal> {
    table
        .iter()
        .find(|(max, _)| leistung_kwp <= *max)
        .map(|(_, ct)| *ct)
}

/// **§48 Abs. 2 EEG 2023 i.V.m. §49** — the Überschusseinspeisung **gross AW**
/// in ct/kWh for a plant of `leistung_kwp` commissioned on `inbetriebnahme`.
///
/// Gross: this is the anzulegender Wert, which is what the Marktprämie is
/// computed from directly (Anlage 1 Nr. 1). For the feste Einspeisevergütung,
/// subtract [`sect53_deduction`] — see the module docs.
///
/// Returns `None` before 1 January 2023 and above 1 MW (§22 Abs. 3 makes those
/// ausschreibungspflichtig — there is no statutory AW to look up).
///
/// ```rust
/// use eeg_billing::rates::solar_pv_ueberschuss_aw_ct;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// // §48 Abs. 2 Nr. 1, 1 Feb 2024 window: 8.60 × 0.99 = 8.51 ct gross AW.
/// assert_eq!(solar_pv_ueberschuss_aw_ct(dec!(9), date!(2024-03-01)), Some(dec!(8.51)));
/// ```
#[must_use]
pub fn solar_pv_ueberschuss_aw_ct(
    leistung_kwp: Decimal,
    inbetriebnahme: time::Date,
) -> Option<Decimal> {
    if inbetriebnahme < EEG2023_START {
        return None;
    }
    let base = tier_ct(&SOLAR_GEBAEUDE_BASIS_CT, leistung_kwp)?;
    Some(crate::degression::anzulegender_wert_bei_inbetriebnahme(
        base,
        inbetriebnahme,
    ))
}

/// **§48 Abs. 2 + Abs. 2a EEG 2023 i.V.m. §49** — the Volleinspeisung **gross AW**
/// in ct/kWh.
///
/// §49 degresses the Abs. 2 base and the Abs. 2a uplift together, so the sum is
/// formed first and the degression applied to it once — which is how the
/// Bundesnetzagentur publishes the Volleinspeisung column.
///
/// ```rust
/// use eeg_billing::rates::solar_pv_volleinspeisung_aw_ct;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// // (8.60 + 4.80) × 0.99 = 13.266 → 13.27 ct gross AW.
/// assert_eq!(solar_pv_volleinspeisung_aw_ct(dec!(9), date!(2024-03-01)), Some(dec!(13.27)));
/// ```
#[must_use]
pub fn solar_pv_volleinspeisung_aw_ct(
    leistung_kwp: Decimal,
    inbetriebnahme: time::Date,
) -> Option<Decimal> {
    if inbetriebnahme < EEG2023_START {
        return None;
    }
    let base = tier_ct(&SOLAR_GEBAEUDE_BASIS_CT, leistung_kwp)?;
    let zuschlag = tier_ct(&SOLAR_VOLLEINSPEISUNG_ZUSCHLAG_CT, leistung_kwp)?;
    Some(crate::degression::anzulegender_wert_bei_inbetriebnahme(
        base + zuschlag,
        inbetriebnahme,
    ))
}

/// **§48 Abs. 1 EEG 2023 i.V.m. §49** — the Freiflächen **gross AW** in ct/kWh.
#[must_use]
pub fn solar_pv_freiflaeche_aw_ct(inbetriebnahme: time::Date) -> Option<Decimal> {
    (inbetriebnahme >= EEG2023_START).then(|| {
        crate::degression::anzulegender_wert_bei_inbetriebnahme(
            SOLAR_FREIFLAECHE_BASIS_CT,
            inbetriebnahme,
        )
    })
}

/// The §48 Abs. 2 Überschusseinspeisung table as a [`RateLookup`], for the §49
/// window `inbetriebnahme` falls into. Gross AW, in EUR/kWh.
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// let table = rates::solar_pv_ueberschuss_lookup(date!(2024-03-01)).unwrap();
/// assert_eq!(table.rate_for(dec!(9)).unwrap(), billing::Amount::parse("0.08510").unwrap());
/// ```
#[must_use]
pub fn solar_pv_ueberschuss_lookup(inbetriebnahme: time::Date) -> Option<RateLookup> {
    if inbetriebnahme < EEG2023_START {
        return None;
    }
    let at = |kwp| solar_pv_ueberschuss_aw_ct(kwp, inbetriebnahme).unwrap_or(Decimal::ZERO);
    RateLookup::builder()
        .at_most(dec!(10), amount_from_ct(at(dec!(10))))
        .at_most(dec!(40), amount_from_ct(at(dec!(40))))
        .at_most(dec!(1_000), amount_from_ct(at(dec!(1_000))))
        .build()
        .ok()
}

/// The §48 Abs. 2 + Abs. 2a Volleinspeisung table as a [`RateLookup`], for the
/// §49 window `inbetriebnahme` falls into. Gross AW, in EUR/kWh.
#[must_use]
pub fn solar_pv_volleinspeisung_lookup(inbetriebnahme: time::Date) -> Option<RateLookup> {
    if inbetriebnahme < EEG2023_START {
        return None;
    }
    let at = |kwp| solar_pv_volleinspeisung_aw_ct(kwp, inbetriebnahme).unwrap_or(Decimal::ZERO);
    RateLookup::builder()
        .at_most(dec!(10), amount_from_ct(at(dec!(10))))
        .at_most(dec!(40), amount_from_ct(at(dec!(40))))
        .at_most(dec!(100), amount_from_ct(at(dec!(100))))
        .at_most(dec!(400), amount_from_ct(at(dec!(400))))
        .at_most(dec!(1_000), amount_from_ct(at(dec!(1_000))))
        .build()
        .ok()
}

// ── Wind Onshore ──────────────────────────────────────────────────────────────

/// **There is no statutory anzulegender Wert for Windenergie an Land.**
///
/// § 22 Abs. 2 Satz 1 EEG 2023: the claim under § 19 Abs. 1 exists "nur, solange
/// und soweit ein von der Bundesnetzagentur erteilter Zuschlag für die Anlage
/// wirksam ist". The value itself is then derived per § 36h Abs. 1 — the
/// Zuschlagswert for the Referenzstandort, multiplied by the Korrekturfaktor of
/// the plant's Gütefaktor (Anlage 2 Nr. 2 and 7) — which is a property of the
/// individual award and site, not of a capacity band.
///
/// So this function always answers `None`, and the caller supplies the awarded
/// value as [`crate::TariffSource::Auction`] with
/// [`crate::wind::korrekturfaktor_fuer_guetefaktor`] applied.
#[must_use]
pub fn wind_onshore_lookup(_eeg_year: i16) -> Option<RateLookup> {
    None
}

// ── Biomasse — § 42 EEG 2023 ─────────────────────────────────────────────────

/// § 42 EEG 2023 — the statutory anzulegender Wert for **Biomasse**.
///
/// The statute gives **one** tier: 12,67 ct/kWh up to a Bemessungsleistung of
/// 150 kW, and Satz 2 excludes Biomethan. Above 150 kW the anzulegender Wert is
/// set by tender (§ 22 Abs. 4), so there is no statutory value to return and the
/// table's open tier is deliberately absent — `rate_for` then answers `Err`
/// rather than a made-up rate.
///
/// § 43 (Vergärung von Bioabfällen) and § 44 (Vergärung von Gülle) set their own
/// higher values for plants that qualify — those are separate claims, not tiers
/// of this one.
///
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// let table = rates::biomasse_lookup(2023).unwrap();
/// assert_eq!(table.rate_for(dec!(120)).unwrap(), billing::Amount::parse("0.12670").unwrap());
/// // Above 150 kW the value comes from a tender, not from the statute.
/// assert!(table.rate_for(dec!(600)).is_err());
/// ```
pub fn biomasse_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(150), amount_ct("12.67"))
            .build()
            .ok(),
        _ => None,
    }
}

/// § 43 Abs. 1 EEG 2023 — **Vergärung von Bioabfällen** (≥ 90 Masseprozent
/// separately collected Bioabfälle), by Bemessungsleistung.
///
/// Abs. 2 additionally requires the anaerobic digestion to be directly coupled
/// to a Nachrotte stage whose residue is materially recovered — a plant
/// condition the caller checks.
pub fn bioabfall_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("14.16"))
            .at_most(dec!(20_000), amount_ct("12.41"))
            .build()
            .ok(),
        _ => None,
    }
}

/// § 44 Abs. 1 EEG 2023 — **Vergärung von Gülle** (Güllekleinanlage), by
/// Bemessungsleistung.
///
/// Abs. 2 conditions the claim on generation at the Biogaserzeugungsanlage's
/// site, ≤ 150 kW installed there, and an average manure share the statute
/// names — see [`crate::biomasse`].
pub fn guelle_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(75), amount_ct("22.00"))
            .at_most(dec!(150), amount_ct("19.00"))
            .build()
            .ok(),
        _ => None,
    }
}

// ── KWKG ──────────────────────────────────────────────────────────────────────

/// Return the KWKG 2023 KWK-Zuschlag rate table.
///
/// The parameter to `rate_for()` is the **electric capacity in kW_el**.
///
/// Source: §7 KWKG 2023, Anlage (Vergütungssätze).
///
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// let table = rates::kwkg_zuschlag_lookup().unwrap();
/// // 50 kW_el CHP plant
/// assert_eq!(table.rate_for(dec!(50)).unwrap(), billing::Amount::parse("0.08000").unwrap());
/// // 2,000 kW_el large plant
/// assert_eq!(table.rate_for(dec!(2000)).unwrap(), billing::Amount::parse("0.04000").unwrap());
/// ```
pub fn kwkg_zuschlag_lookup() -> Option<RateLookup> {
    // §7 KWKG 2023, Anlage: Vergütungssätze nach Leistungsklasse
    RateLookup::builder()
        .at_most(dec!(50), amount_ct("8.00")) // ≤50 kW_el:   8.00 ct/kWh
        .at_most(dec!(100), amount_ct("6.00")) // ≤100 kW_el:  6.00 ct/kWh
        .at_most(dec!(250), amount_ct("5.00")) // ≤250 kW_el:  5.00 ct/kWh
        .at_most(dec!(2_000), amount_ct("4.00")) // ≤2 MW_el:    4.00 ct/kWh
        .fallback(amount_ct("3.00")) // >2 MW_el:    3.00 ct/kWh
        .build()
        .ok()
}

// ── Convenience helper ────────────────────────────────────────────────────────

/// A rate in ct/kWh as a `billing::Amount<5>` in EUR/kWh.
///
/// 8,11 ct/kWh → `0.00811 EUR/kWh`.
///
/// Both forms go through [`Amount::from_decimal_rounded`] with the workspace's
/// kaufmännisches Runden.
///
/// # Panics
/// Panics on a malformed literal or an out-of-range rate. Only ever called from
/// the static table constructors below, where both are authorship errors.
fn amount_from_ct(ct: rust_decimal::Decimal) -> Amount<5> {
    Amount::from_decimal_rounded(
        ct / rust_decimal::Decimal::from(100u32),
        billing::RoundingStrategy::MidpointAwayFromZero,
    )
    .expect("statutory rate is in range")
}

/// [`amount_from_ct`] for a literal, e.g. `amount_ct("12.03")`.
///
/// # Panics
/// Panics if the literal is not a decimal, or the rate is out of range.
fn amount_ct(ct_str: &str) -> Amount<5> {
    amount_from_ct(ct_str.parse().expect("static rate string"))
}

// ── Nicht-solare gesetzliche anzulegende Werte (§§ 40–45 EEG 2023) ───────────
//
// Every table below is the **Startwert** as enacted for its EEG version — the
// value before the statutory annual Absenkung, which each Erzeugungsart carries
// at its own rate and cadence:
//
// | Erzeugungsart | Absenkung | ab | § |
// |---|---|---|---|
// | Wasserkraft | 0,5 %/Jahr | 01.01.2024 | § 40 Abs. 5 |
// | Deponie-/Klär-/Grubengas | 1,5 %/Jahr | 01.01.2024 | § 41 Abs. 4 |
// | Biomasse (§§ 42–44) | 0,5 %/Jahr | **01.07.**2024 | § 44a |
// | Geothermie | 0,5 %/Jahr | 01.01.2024 | § 45 Abs. 2 |
//
// Apply it with [`crate::degression::JaehrlicheAbsenkung`]. Every value here is
// asserted against the statute by `statutory_rate_tests`.

/// § 40 Abs. 1 EEG 2023 — **Wasserkraft**, by Bemessungsleistung.
///
/// Gezeiten-, Wellen-, Salzgradienten- und Strömungsenergie are Wasserkraft
/// (§ 3 Nr. 21 lit. a) and settle from this table. There is no § 41a EEG.
///
/// The parameter to `rate_for()` is the **Bemessungsleistung in kW** (§ 3 Nr. 6
/// — the annual energy divided by the hours of the year), not the installed
/// capacity: § 40 Abs. 1 says "Bemessungsleistung" in every Nummer.
///
/// ## §53 deduction
///
/// Subtract 0.2 ct/kWh for an Einspeisevergütung plant:
/// `net = lookup.rate_for(kw)? - sect53_deduction(ErzeugungsArt::Wasserkraft)`
///
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// let table = rates::wasserkraft_lookup(2023).unwrap();
/// // 200 kW: ≤ 500 kW tier
/// assert_eq!(table.rate_for(dec!(200)).unwrap(), billing::Amount::parse("0.12030").unwrap());
/// // 3 MW: ≤ 5 MW tier
/// assert_eq!(table.rate_for(dec!(3000)).unwrap(), billing::Amount::parse("0.06070").unwrap());
/// // 60 MW: the open top tier
/// assert_eq!(table.rate_for(dec!(60000)).unwrap(), billing::Amount::parse("0.03370").unwrap());
/// ```
pub fn wasserkraft_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // § 40 Abs. 1 EEG 2023.
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("12.03"))
            .at_most(dec!(2_000), amount_ct("7.93"))
            .at_most(dec!(5_000), amount_ct("6.07"))
            .at_most(dec!(10_000), amount_ct("5.32"))
            .at_most(dec!(20_000), amount_ct("5.13"))
            .at_most(dec!(50_000), amount_ct("4.12"))
            .fallback(amount_ct("3.37"))
            .build()
            .ok(),
        // § 40 Abs. 1 EEG 2021.
        2017..=2022 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("12.15"))
            .at_most(dec!(2_000), amount_ct("8.01"))
            .at_most(dec!(5_000), amount_ct("6.13"))
            .at_most(dec!(10_000), amount_ct("5.37"))
            .at_most(dec!(20_000), amount_ct("5.18"))
            .at_most(dec!(50_000), amount_ct("4.16"))
            .fallback(amount_ct("3.40"))
            .build()
            .ok(),
        _ => None,
    }
}

/// § 41 Abs. 1 EEG 2023 — **Deponiegas**, by Bemessungsleistung.
///
/// **The ladder ends at 5 MW.** Abs. 1 has two Nummern, both „bis einschließlich",
/// and no „ab einer Bemessungsleistung von mehr als" row — unlike § 40 Nr. 7 and
/// § 41 Abs. 3 Nr. 3, where the drafters wrote one when they wanted an open top.
/// A plant above 5 MW therefore has **no** gesetzlich bestimmter Wert here and the
/// lookup answers `None`, which the caller has to decide rather than being paid
/// the 5-MW rate.
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
/// let t = rates::deponiegas_lookup(2023).unwrap();
/// assert_eq!(t.rate_for(dec!(400)).unwrap(), billing::Amount::parse("0.07460").unwrap());
/// assert_eq!(t.rate_for(dec!(3000)).unwrap(), billing::Amount::parse("0.05170").unwrap());
/// assert!(t.rate_for(dec!(6000)).is_err());
/// ```
pub fn deponiegas_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("7.46"))
            .at_most(dec!(5_000), amount_ct("5.17"))
            .build()
            .ok(),
        _ => None,
    }
}

/// § 41 Abs. 2 EEG 2023 — **Klärgas**, by Bemessungsleistung.
///
/// Same shape as Abs. 1, including the closed top at 5 MW.
pub fn klaergas_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("5.93"))
            .at_most(dec!(5_000), amount_ct("5.17"))
            .build()
            .ok(),
        _ => None,
    }
}

/// § 41 Abs. 3 EEG 2023 — **Grubengas**, by Bemessungsleistung.
///
/// Satz 2: the claim exists only where the gas comes from active or
/// decommissioned mining operations — a condition on the plant, checked by the
/// caller, not expressible in a rate table.
pub fn grubengas_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(1_000), amount_ct("5.98"))
            .at_most(dec!(5_000), amount_ct("3.81"))
            .fallback(amount_ct("3.37"))
            .build()
            .ok(),
        _ => None,
    }
}

/// § 45 Abs. 1 EEG 2023 — **Geothermie**, a flat 25,20 ct/kWh.
///
/// Not § 41: that is Deponie-, Klär- und Grubengas. And there is **no Geothermie
/// Ausschreibung at any size**: § 22 Abs. 5 Satz 2 names Geothermie among the
/// technologies whose anzulegender Wert „durch die §§ 40 bis 49 gesetzlich
/// bestimmt" is. § 22 Abs. 4 is Biomasse.
pub fn geothermie_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        2017..=2026 => RateLookup::builder()
            .fallback(amount_ct("25.20"))
            .build()
            .ok(),
        _ => None,
    }
}

/// Look up the EEG feed-in tariff for a plant given its technology type,
/// installed capacity, and EEG year.
///
/// This is the unified entry point that dispatches to the per-technology tables.
///
/// Returns `Err` when the EEG year or technology is not in the static tables
/// (use `einsd`'s DB lookup instead).
///
/// ## Parameters
///
/// - `erzeugungsart`: technology type string (same values as `eeg_anlagen.erzeugungsart`)
/// - `leistung_kwp`: installed capacity in kWp (or kW_el for KWKG)
/// - `eeg_year`: EEG version year from the plant's `eeg_gesetz` column
///
/// For solar this returns the **§48 Abs. 2 Startwerte** — a calendar year cannot
/// select a half-yearly §49 window. Use [`solar_pv_ueberschuss_aw_ct`] when the
/// commissioning date is known.
///
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// let rate = rates::lookup_rate("SOLAR_AUFDACH", dec!(9), 2023).unwrap();
/// // 9 kWp ≤ 10 kWp bracket → §48 Abs. 2 Nr. 1 Startwert 8.60 ct/kWh gross AW.
/// assert_eq!(rate, billing::Amount::parse("0.08600").unwrap());
/// ```
pub fn lookup_rate(
    erzeugungsart: &str,
    leistung_kwp: rust_decimal::Decimal,
    eeg_year: i16,
) -> Result<Amount<5>, BillingError> {
    let art = crate::ErzeugungsArt::from_db_str(erzeugungsart).map_err(|_| {
        BillingError::InvalidInput {
            reason: format!("unknown erzeugungsart {erzeugungsart:?}"),
        }
    })?;
    lookup_rate_for(art, leistung_kwp, eeg_year)
}

/// Statutory rate lookup keyed on the typed [`crate::ErzeugungsArt`].
///
/// The routing is an **exhaustive** match on the enum, so a new technology
/// variant forces a routing decision at compile time — the string-keyed
/// [`lookup_rate`] cannot silently misroute a plant whose DB spelling drifts
/// from the rate table's. Variants without a static statutory table (offshore
/// wind, tendered plants) return [`BillingError::InvalidInput`] — the caller
/// resolves those from the `eeg_verguetungssaetze` DB table instead.
///
/// # Errors
/// [`BillingError::InvalidInput`] when no static table covers the technology or
/// EEG year.
pub fn lookup_rate_for(
    art: crate::ErzeugungsArt,
    leistung_kwp: rust_decimal::Decimal,
    eeg_year: i16,
) -> Result<Amount<5>, BillingError> {
    use crate::ErzeugungsArt as E;
    let table = match art {
        // Solar has half-yearly §49 windows, which a calendar year cannot
        // select. This routes to the §48 Abs. 2 Startwerte; for the window that
        // actually applies to a plant use `solar_pv_ueberschuss_aw_ct`.
        E::SolarAufdach
        | E::SolarFreiflaeche
        | E::SolarAgriPv
        | E::SolarMieterstrom
        | E::SolarStecker => (eeg_year >= 2023)
            .then(|| solar_pv_ueberschuss_lookup(EEG2023_START))
            .flatten(),
        E::WindOnshore => wind_onshore_lookup(eeg_year),
        // Offshore wind (§§70 ff.) is tender-only — no static statutory table.
        E::WindOffshore => None,
        // § 42 Satz 2 excludes Biomethan from the statutory value, and
        // Holzbiomasse has none of its own — both resolve from the DB series.
        E::Biomasse | E::Biogas => biomasse_lookup(eeg_year),
        E::BiomassHolz | E::Biomethan => None,
        E::Kwk => kwkg_zuschlag_lookup(),
        // Gezeitenenergie is Wasserkraft (§ 3 Nr. 21 lit. a), settled from § 40.
        E::Wasserkraft | E::Gezeiten => wasserkraft_lookup(eeg_year),
        E::Geothermie => geothermie_lookup(eeg_year),
        // § 41 gives each gas its own ladder; they are not interchangeable.
        E::Deponiegas => deponiegas_lookup(eeg_year),
        E::Klaegas => klaergas_lookup(eeg_year),
        E::Grubengas => grubengas_lookup(eeg_year),
    }
    .ok_or(BillingError::InvalidInput {
        reason:
            "no static rate table for this erzeugungsart/eeg_year combination — use einsd DB lookup"
                .to_owned(),
    })?;

    table.rate_for(leistung_kwp)
}

// ── §53 EEG — Vergütungsabzug ─────────────────────────────────────────────────

/// §53 EEG 2017/2021/2023 — flat deduction from the anzulegender Wert (AW)
/// for **Einspeisevergütung** plants.
///
/// The functions in this module return **gross AW values** (as published in §48 EEG 2023,
/// BNetzA bulletins). Before storing in `verguetungssatz_ct`, subtract this deduction
/// to get the actual net Vergütungssatz the operator receives.
///
/// ## Does NOT apply to
///
/// - `Direktvermarktung`/`Ausschreibung` (Marktprämie, not Einspeisevergütung)
/// - `PostEegSpot` (ausgeförderte Anlagen using unentgeltliche Abnahme — §53 Abs. 2 EEG 2023)
/// - `KwkgZuschlag` (KWKG, separate law)
/// - Plants with `SanktionAlt::VerguetungReduziert20Prozent` (§53 Abs. 3: 20% reduction instead)
///
/// # Example
///
/// ```rust
/// use eeg_billing::rates::sect53_deduction;
/// use eeg_billing::ErzeugungsArt;
/// use rust_decimal::dec;
///
/// // Solar PV and Wind: -0.4 ct/kWh
/// assert_eq!(sect53_deduction(ErzeugungsArt::SolarAufdach),       dec!(0.4));
/// assert_eq!(sect53_deduction(ErzeugungsArt::WindOnshore), dec!(0.4));
///
/// // Biomasse, Wasserkraft, etc.: -0.2 ct/kWh
/// assert_eq!(sect53_deduction(ErzeugungsArt::Biomasse),    dec!(0.2));
/// assert_eq!(sect53_deduction(ErzeugungsArt::Wasserkraft), dec!(0.2));
///
/// // KWKG: no deduction
/// assert_eq!(sect53_deduction(ErzeugungsArt::Kwk), dec!(0));
/// ```
pub fn sect53_deduction(art: crate::technology::ErzeugungsArt) -> rust_decimal::Decimal {
    use crate::technology::ErzeugungsArt as A;
    match art {
        // §53 Nr. 2: Solar PV and Wind → -0.4 ct/kWh
        A::SolarAufdach
        | A::SolarFreiflaeche
        | A::SolarAgriPv
        | A::SolarMieterstrom
        | A::SolarStecker
        | A::WindOnshore
        | A::WindOffshore => dec!(0.4),

        // §53 Nr. 1: Wasserkraft, Biomasse, Geothermie, Deponie-/Klär-/Grubengas → -0.2 ct/kWh
        A::Biomasse
        | A::BiomassHolz
        | A::Biogas
        | A::Biomethan
        | A::Klaegas
        | A::Grubengas
        | A::Deponiegas
        | A::Wasserkraft
        | A::Geothermie
        | A::Gezeiten => dec!(0.2),

        // KWKG: §53 EEG does not apply to KWKG plants
        A::Kwk => dec!(0),
    }
}

// ── §44 Güllekleinanlage ──────────────────────────────────────────────────────

/// Return the **gross AW** for **§44 Güllekleinanlage** (manure-fed small biogas).
///
/// ## Eligibility criteria (§44 EEG 2023)
///
/// - Installed capacity **≤ 75 kW_el**
/// - ≥ 80 % of energy input from liquid or solid manure (Gülle / Festmist)
/// - Use [`crate::biomasse::BiomassSettlementData::new`] to determine eligibility.
///
/// When both criteria are met, the plant receives the Güllekleinanlage Anzulegender
/// Wert instead of the standard Biomasse rate from [`biomasse_lookup`].
///
/// ## Net Vergütungssatz
///
/// Subtract the §53 deduction (0.2 ct/kWh for Biomasse) before storing:
/// `net = gross_aw − sect53_deduction(ErzeugungsArt::Biogas)` = 16.90 − 0.20 = **16.70 ct/kWh**
///
/// ## Sources
///
/// - §44 Abs. 1 EEG 2023 (BGBl. I 2023 Nr. 1, 10.01.2023)
/// - BNetzA Ausschreibungsergebnisse Biomasse (reference)
///
/// # Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// // 50 kW Güllekleinanlage — eligible under §44 EEG 2023
/// let table = rates::guellekleinanlage_rate(2023).expect("known year");
/// assert_eq!(table.rate_for(dec!(50)).unwrap(), billing::Amount::parse("0.16900").unwrap());
///
/// // Plant above 75 kW — not returned; use biomasse_lookup instead
/// assert!(table.rate_for(dec!(80)).is_err());
/// ```
pub fn guellekleinanlage_rate(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // Source: §44 Abs. 1 EEG 2023 (BGBl I 2023 Nr. 1)
        // Gross AW = 16.90 ct/kWh for ≤75 kW_el.
        // Net (after §53 -0.2 ct) = 16.70 ct/kWh.
        // Solarpaket I (BGBl I 2024 Nr. 107) did not change §44 rates.
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(75), amount_ct("16.90")) // ≤75 kW_el (hard capacity ceiling per §44)
            // No fallback: plants > 75 kW are NOT eligible for Güllekleinanlage rate.
            .build()
            .ok(),
        _ => None,
    }
}

#[cfg(test)]
mod statutory_rate_tests {
    use super::*;

    /// Assert a whole ladder at once: `(Bemessungsleistung, ct/kWh)`.
    fn assert_ladder(table: &RateLookup, tiers: &[(&str, &str)], what: &str) {
        for (kw, ct) in tiers {
            let kw: Decimal = kw.parse().expect("test literal");
            let expected = amount_ct(ct);
            assert_eq!(
                table
                    .rate_for(kw)
                    .unwrap_or_else(|e| panic!("{what} at {kw} kW: {e}")),
                expected,
                "{what}: {kw} kW must pay {ct} ct/kWh",
            );
        }
    }

    /// **§ 40 Abs. 1 EEG 2023** — seven tiers, and the top one is open.
    ///
    /// The table ran `12,37 / 9,79 / 7,56 / 6,47 / 5,59 / 3,88` — six tiers,
    /// none of them a statutory figure, missing the ≤ 50 MW band, and above the
    /// law at every step. Nothing caught it because nothing asserted it.
    #[test]
    fn wasserkraft_pays_the_seven_tiers_of_sect40() {
        let t = wasserkraft_lookup(2023).expect("EEG 2023 table");
        assert_ladder(
            &t,
            &[
                ("500", "12.03"),
                ("2000", "7.93"),
                ("5000", "6.07"),
                ("10000", "5.32"),
                ("20000", "5.13"),
                ("50000", "4.12"),
                ("60000", "3.37"),
            ],
            "§ 40 Abs. 1 EEG 2023",
        );
        // EEG 2021 is its own ladder, one cent-fraction above throughout.
        let t = wasserkraft_lookup(2021).expect("EEG 2021 table");
        assert_ladder(
            &t,
            &[("500", "12.15"), ("2000", "8.01"), ("60000", "3.40")],
            "§ 40 Abs. 1 EEG 2021",
        );
    }

    /// **§ 41 EEG 2023** — Deponie-, Klär- und Grubengas each have their own
    /// ladder. They shared one flat 7,74 ct, which paid Klärgas 1,81 ct/kWh
    /// above its statutory rate and large Grubengas more than twice.
    #[test]
    fn each_gas_pays_its_own_sect41_ladder() {
        assert_ladder(
            &deponiegas_lookup(2023).expect("table"),
            &[("500", "7.46"), ("3000", "5.17")],
            "§ 41 Abs. 1 EEG 2023 (Deponiegas)",
        );
        assert_ladder(
            &klaergas_lookup(2023).expect("table"),
            &[("500", "5.93"), ("3000", "5.17")],
            "§ 41 Abs. 2 EEG 2023 (Klärgas)",
        );
        assert_ladder(
            &grubengas_lookup(2023).expect("table"),
            &[("1000", "5.98"), ("5000", "3.81"), ("9000", "3.37")],
            "§ 41 Abs. 3 EEG 2023 (Grubengas)",
        );
    }

    /// **§ 42 Satz 1 EEG 2023** — one tier, and nothing above it.
    ///
    /// Above 150 kW the anzulegender Wert is set by tender (§ 22 Abs. 4), so the
    /// table has no open tier: `rate_for` answers `Err` rather than inventing a
    /// rate. The old table ran three tiers to 5 MW and beyond.
    #[test]
    fn biomasse_pays_one_statutory_tier_and_refuses_above_it() {
        let t = biomasse_lookup(2023).expect("table");
        assert_ladder(&t, &[("150", "12.67")], "§ 42 Satz 1 EEG 2023");
        assert!(
            t.rate_for(dec!(600)).is_err(),
            "above 150 kW the value comes from a tender, not from this table",
        );
    }

    /// **§§ 43 and 44 EEG 2023** — Bioabfall- and Güllevergärung are separate,
    /// higher claims, not tiers of § 42.
    #[test]
    fn bioabfall_and_guelle_are_their_own_claims() {
        assert_ladder(
            &bioabfall_lookup(2023).expect("table"),
            &[("500", "14.16"), ("20000", "12.41")],
            "§ 43 Abs. 1 EEG 2023",
        );
        assert_ladder(
            &guelle_lookup(2023).expect("table"),
            &[("75", "22.00"), ("150", "19.00")],
            "§ 44 Abs. 1 EEG 2023",
        );
    }

    /// **§ 45 Abs. 1 EEG 2023** — Geothermie is a flat 25,20 ct/kWh.
    #[test]
    fn geothermie_is_flat() {
        let t = geothermie_lookup(2023).expect("table");
        assert_ladder(
            &t,
            &[("100", "25.20"), ("5000", "25.20")],
            "§ 45 Abs. 1 EEG 2023",
        );
    }

    /// **Wind an Land has no statutory anzulegender Wert.**
    ///
    /// § 22 Abs. 2 Satz 1: the claim exists "nur, solange und soweit ein von der
    /// Bundesnetzagentur erteilter Zuschlag für die Anlage wirksam ist", and
    /// § 36h Abs. 1 then derives the value from that Zuschlagswert. A flat
    /// 6,28 ct "statutory AW for ≤ 750 kW" was a rate no statute contains.
    #[test]
    fn wind_onshore_has_no_statutory_rate() {
        for year in [2021, 2023, 2026] {
            assert!(wind_onshore_lookup(year).is_none(), "year {year}");
        }
        assert!(
            lookup_rate_for(crate::ErzeugungsArt::WindOnshore, dec!(700), 2023).is_err(),
            "a wind plant is settled from its award, not from a table",
        );
    }

    /// Gezeitenenergie is Wasserkraft (§ 3 Nr. 21 lit. a), so it settles from
    /// § 40 — not from a table of its own, and not at the geothermal rate.
    #[test]
    fn tidal_settles_as_wasserkraft() {
        use crate::ErzeugungsArt as E;
        let tidal = lookup_rate_for(E::Gezeiten, dec!(3000), 2023).expect("§ 40 covers it");
        let hydro = lookup_rate_for(E::Wasserkraft, dec!(3000), 2023).expect("§ 40");
        assert_eq!(tidal, hydro);
        assert_eq!(tidal, amount_ct("6.07"));
    }
}
