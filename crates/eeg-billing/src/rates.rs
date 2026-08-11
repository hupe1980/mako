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

/// Return the EEG reference Anzulegender Wert table for **wind onshore** plants.
///
/// Wind onshore plants >750 kW must participate in BNetzA tenders since 2017
/// (§22 EEG 2017+). The values here are the statutory reference AW used as
/// seed values and for small plants (≤750 kW) that are exempt from tenders.
///
/// The parameter to `rate_for()` is the **installed capacity in kW**.
///
/// Returns `None` for unsupported years.
///
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// let table = rates::wind_onshore_lookup(2023).unwrap();
/// // ≤750 kW (small turbines, tender-exempt)
/// let rate = table.rate_for(dec!(500)).unwrap();
/// // rate = 6.28 ct/kWh for ≤750 kW (EEG 2023 §21 reference)
/// assert_eq!(rate, billing::Amount::parse("0.06280").unwrap());
/// ```
pub fn wind_onshore_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // ── EEG 2023 ──────────────────────────────────────────────────────────
        // Source: §21 EEG 2023 i.V.m. Anlage 2, Referenzwert wind onshore
        // Plants ≤750 kW: statutory AW = 6.28 ct/kWh
        // Plants >750 kW: mandatory tender (AW set by BNetzA per auction round)
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(750), amount_ct("6.28")) // ≤750 kW: tender-exempt AW
            .fallback(amount_ct("6.28")) // >750 kW: tender-based (use direktverm_aw_ct)
            .build()
            .ok(),

        // ── EEG 2021 ──────────────────────────────────────────────────────────
        2021 | 2022 => RateLookup::builder()
            .at_most(dec!(750), amount_ct("6.29")) // ≤750 kW
            .fallback(amount_ct("6.29"))
            .build()
            .ok(),

        _ => None,
    }
}

// ── Biomasse ──────────────────────────────────────────────────────────────────

/// Return the EEG rate table for **Biomasse** plants.
///
/// The parameter to `rate_for()` is the **installed capacity in kW_el**.
///
/// Returns `None` for unsupported years.
///
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// let table = rates::biomasse_lookup(2023).unwrap();
/// assert_eq!(table.rate_for(dec!(200)).unwrap(), billing::Amount::parse("0.14670").unwrap());
/// ```
pub fn biomasse_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // ── EEG 2023 ──────────────────────────────────────────────────────────
        // Source: §21 EEG 2023 i.V.m. Anlage 3 (Biomasse §21 Abs. 1)
        2023..=2026 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("14.67")) // ≤500 kW
            .at_most(dec!(5_000), amount_ct("11.90")) // ≤5 MW
            .fallback(amount_ct("7.58")) // >5 MW
            .build()
            .ok(),

        2021 | 2022 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("13.63"))
            .at_most(dec!(5_000), amount_ct("11.42"))
            .fallback(amount_ct("7.26"))
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

/// Convert a ct/kWh string to a `billing::Amount<5>` (EUR/kWh).
///
/// 8.11 ct/kWh → `Amount::parse("0.00811")`
///
/// # Panics
/// Panics if the string is malformed — only called from static table constructors.
fn amount_from_ct(ct: rust_decimal::Decimal) -> Amount<5> {
    let eur = ct / rust_decimal::Decimal::from(100u32);
    Amount::parse(&format!("{eur:.5}")).expect("5dp EUR/kWh")
}

/// Convert a ct/kWh string to a `billing::Amount<5>` (EUR/kWh).
///
/// 8.11 ct/kWh → `Amount::parse("0.00811")`
///
/// # Panics
/// Panics if the string is malformed — only called from static table constructors.
fn amount_ct(ct_str: &str) -> Amount<5> {
    // Convert ct/kWh to EUR/kWh by dividing by 100
    let ct: rust_decimal::Decimal = ct_str.parse().expect("static rate string");
    let eur = ct / rust_decimal::Decimal::from(100u32);
    let eur_str = eur.to_string();
    Amount::parse(&eur_str)
        .unwrap_or_else(|_| Amount::parse(&format!("{:.5}", eur)).expect("5dp EUR/kWh"))
}

// ── §40 Wasserkraft ───────────────────────────────────────────────────────────

/// Return the EEG statutory rate table for **Wasserkraft** (run-of-river hydro).
///
/// The parameter to `rate_for()` is the installed capacity in **kW_el**.
/// Rates are defined in §40 EEG 2023 / §40 EEG 2021 / §29 EEG 2017.
///
/// Hydro rates are unchanged across EEG 2017–2023.
///
/// **Note**: Plants > 500 kW require Ausschreibung per §22 Abs. 3 Nr. 3 EEG 2023.
/// For tendered plants, use `TariffSource::Auction` with the BNetzA awarded value.
///
/// ## §53 deduction
///
/// Subtract 0.2 ct/kWh from the returned gross rate:
/// `net_verguetung = lookup.rate_for(kw) - sect53_deduction(ErzeugungsArt::Wasserkraft)`
///
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal::dec;
///
/// let table = rates::wasserkraft_lookup(2023).unwrap();
/// // 200 kW run-of-river plant: ≤500 kW tier
/// assert_eq!(table.rate_for(dec!(200)).unwrap(), billing::Amount::parse("0.12370").unwrap());
/// // 3,000 kW plant: ≤5,000 kW tier
/// assert_eq!(table.rate_for(dec!(3000)).unwrap(), billing::Amount::parse("0.07560").unwrap());
/// ```
pub fn wasserkraft_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // EEG 2017–2023: §40 EEG 2023 / §40 EEG 2021 / §29 EEG 2017.
        // Rates are identical across EEG versions for Wasserkraft.
        // Source: §40 Abs. 1 EEG 2023 (BGBl. I 2023 Nr. 1, S. 2476).
        2017..=2026 => RateLookup::builder()
            .at_most(dec!(500), amount_ct("12.37")) // ≤ 500 kW
            .at_most(dec!(2_000), amount_ct("9.79")) // ≤ 2 MW
            .at_most(dec!(5_000), amount_ct("7.56")) // ≤ 5 MW
            .at_most(dec!(10_000), amount_ct("6.47")) // ≤ 10 MW
            .at_most(dec!(20_000), amount_ct("5.59")) // ≤ 20 MW
            .fallback(amount_ct("3.88")) //  > 20 MW
            .build()
            .ok(),
        _ => None,
    }
}

// ── §41 Geothermie / §41a Gezeiten ───────────────────────────────────────────

/// Return the EEG statutory rate table for **Geothermie** and **Gezeiten**.
///
/// The parameter to `rate_for()` is the installed capacity in **kW_el**.
/// Rates are defined in §41 EEG 2023 (Geothermie) / §41a EEG 2023 (Gezeiten).
///
/// Geothermie is a flat rate — all capacity classes receive the same AW.
/// Plants > 150 kW require Ausschreibung per §22 Abs. 3 Nr. 3 EEG 2023.
///
/// ## §53 deduction
///
/// Subtract 0.2 ct/kWh: `net = rate − sect53_deduction(ErzeugungsArt::Geothermie)`
pub fn geothermie_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // Source: §41 Abs. 1 EEG 2023. Flat rate, no capacity tiers.
        // For plants > 150 kW: AW is set by BNetzA tender — use TariffSource::Auction.
        2023..=2026 => RateLookup::builder()
            .fallback(amount_ct("25.20")) // flat for ≤ 150 kW; > 150 kW uses auction
            .build()
            .ok(),
        2017..=2022 => RateLookup::builder()
            .fallback(amount_ct("25.20"))
            .build()
            .ok(),
        _ => None,
    }
}

// ── §42 Klärgas / Deponiegas / Grubengas ──────────────────────────────────────

/// Return the EEG statutory rate table for **Klärgas**, **Deponiegas**, and **Grubengas**.
///
/// The parameter to `rate_for()` is the installed capacity in **kW_el**.
/// Rates are defined in §42 EEG 2023.
///
/// These are flat rates — all capacity classes receive the same AW.
/// Plants > 500 kW are uncommon for these fuel types; they use Ausschreibung.
///
/// ## §53 deduction
///
/// Subtract 0.2 ct/kWh from the returned rate.
pub fn gasart_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // Source: §42 Abs. 1 EEG 2023. Flat rate regardless of capacity.
        2023..=2026 => RateLookup::builder()
            .fallback(amount_ct("7.74"))
            .build()
            .ok(),
        2017..=2022 => RateLookup::builder()
            .fallback(amount_ct("7.74"))
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
        E::Solar
        | E::SolarAufdach
        | E::SolarFreiflaeche
        | E::SolarAgriPv
        | E::SolarMieterstrom
        | E::SolarStecker => (eeg_year >= 2023)
            .then(|| solar_pv_ueberschuss_lookup(EEG2023_START))
            .flatten(),
        E::WindOnshore => wind_onshore_lookup(eeg_year),
        // Offshore wind (§§70 ff.) is tender-only — no static statutory table.
        E::WindOffshore => None,
        E::Biomasse | E::BiomassHolz | E::Biogas | E::Biomethan => biomasse_lookup(eeg_year),
        E::Kwk => kwkg_zuschlag_lookup(),
        E::Wasserkraft => wasserkraft_lookup(eeg_year),
        E::Geothermie | E::Gezeiten => geothermie_lookup(eeg_year),
        E::Klaegas | E::Grubengas | E::Deponiegas => gasart_lookup(eeg_year),
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
/// assert_eq!(sect53_deduction(ErzeugungsArt::Solar),       dec!(0.4));
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
        A::Solar
        | A::SolarAufdach
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
