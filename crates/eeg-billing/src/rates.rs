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
//! use eeg_billing::{ErzeugungsArt};
//! use rust_decimal_macros::dec;
//!
//! // Gross AW for a 15 kWp solar plant (EEG 2023 initial)
//! let lookup = rates::solar_pv_lookup(2023).expect("EEG 2023 solar PV rates known");
//! let gross_aw = lookup.rate_for(dec!(15)).expect("15 kWp is within table");
//! // Net Vergütungssatz = AW − §53 deduction (0.4 ct for solar)
//! let sect53 = rates::sect53_deduction(ErzeugungsArt::SolarAufdach);
//! // Store the NET rate in verguetungssatz_ct; the formula uses it directly.
//! ```
//!
//! ## Source
//!
//! - **EEG 2023**: BGBl I 2023 Nr. 1 (10.01.2023), §21 EEG 2023 + §48 Abs. 2
//! - **EEG 2021**: BGBl I 2021 S. 3426, §21 EEG 2021 + Anlage 1
//! - **EEG 2017**: BGBl I 2017 S. 2532, §21 EEG 2017 + Anlage 1
//!
//! **Important:** These are **reference starting rates** (gross AW).
//! Actual rates degrade quarterly (§23a EEG 2023, §49 EEG 2021) and depend on the
//! exact commissioning month.  For production billing, use `einsd`'s
//! `lookup_verguetungssatz` DB function which holds the full quarterly degression table.
//!
//! ## KWKG rates
//!
//! KWKG 2023 KWK-Zuschlag rates are determined by plant size and commissioning year
//! (§7 KWKG 2023 Anlage).  §53 does NOT apply to KWKG.  See [`kwkg_zuschlag_lookup`].

use billing::{Amount, BillingError, RateLookup};
use rust_decimal_macros::dec;

// ── Solar PV ──────────────────────────────────────────────────────────────────

/// Return the EEG rate table for **solar PV** plants (Gebäudeanlagen, Überschusseinspeisung).
///
/// These are the **§48 Abs. 2** rates — for plants that do NOT declare full feed-in
/// (Überschusseinspeisung: surplus after self-consumption). For Volleinspeisung
/// (100% feed-in) rates, use [`solar_pv_volleinspeisung_lookup`].
///
/// The parameter passed to `rate_for()` is the **installed capacity in kWp**.
///
/// ## Rate history (§48 Abs. 2 EEG 2023, Gebäudeanlagen)
///
/// | Period | ≤10 kWp | ≤40 kWp | ≤1 MWp | Source |
/// |---|---|---|---|---|
/// | 2023-02 – 2024-04 | 8.11 ct | 6.79 ct | 5.56 ct | EEG 2023 initial (BGBl 2023 Nr.1) |
/// | from 2024-05 | **8.51 ct** | **7.43 ct** | **7.64 ct** | Solarpaket I (BGBl 2024 Nr.107) |
///
/// **Important:** These are **reference starting rates** for the respective EEG
/// version. Actual rates degrade quarterly (§23a EEG 2023) from commissioning month.
/// For production billing, use `einsd`'s `lookup_verguetungssatz` DB function.
///
/// Use `eeg_year = 2023` for plants commissioned from 01.02.2023 (initial rates)
/// or `eeg_year = 2024` for plants commissioned from 01.05.2024 (Solarpaket I).
///
/// ## Supported EEG years
///
/// | Year | Source | Rate track |
/// |---|---|---|
/// | 2023 | BGBl I 2023 Nr. 1, §48 Abs. 2 | Initial EEG 2023 rates (before Solarpaket I) |
/// | 2024+ | BGBl I 2024 Nr. 107, Solarpaket I | Increased rates from 01.05.2024 |
/// | 2021 | BGBl I 2021 S. 3426, §21 + Anlage 1 | Q3 2021 reference rate |
/// | 2017 | BGBl I 2017 S. 2532, §21 + Anlage 1 | Q2 2017 reference rate |
///
/// # Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal_macros::dec;
///
/// // EEG 2024 (Solarpaket I): 9 kWp building-mounted PV → 8.51 ct/kWh
/// let table = rates::solar_pv_ueberschuss_lookup(2024).expect("known year");
/// assert_eq!(table.rate_for(dec!(9)).unwrap(), billing::Amount::parse("0.08510").unwrap());
/// ```
pub fn solar_pv_ueberschuss_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // ── EEG 2024 / Solarpaket I — valid from 01.05.2024 ──────────────────
        // Source: §48 Abs. 2 EEG 2023 n.F. (Solarpaket I, BGBl I 2024 Nr. 107)
        2024..=2026 => RateLookup::builder()
            .at_most(dec!(10), amount_ct("8.51")) // ≤10 kWp:  8.51 ct/kWh
            .at_most(dec!(40), amount_ct("7.43")) // ≤40 kWp:  7.43 ct/kWh
            .fallback(amount_ct("7.64")) // >40 kWp:  7.64 ct/kWh (≤1 MWp)
            .build()
            .ok(),

        // ── EEG 2023 (initial) — valid from 01.02.2023 to 30.04.2024 ─────────
        // Source: §48 Abs. 2 EEG 2023 a.F. (BGBl I 2023 Nr. 1)
        2023 => RateLookup::builder()
            .at_most(dec!(10), amount_ct("8.11")) // ≤10 kWp:  8.11 ct/kWh
            .at_most(dec!(40), amount_ct("6.79")) // ≤40 kWp:  6.79 ct/kWh
            .fallback(amount_ct("5.56")) // >40 kWp:  5.56 ct/kWh (≤1 MWp)
            .build()
            .ok(),

        // ── EEG 2021 — Q3 2021 starting rate ─────────────────────────────────
        2021 | 2022 => RateLookup::builder()
            .at_most(dec!(10), amount_ct("9.03")) // ≤10 kWp:  9.03 ct/kWh
            .at_most(dec!(40), amount_ct("8.75")) // ≤40 kWp:  8.75 ct/kWh
            .fallback(amount_ct("7.29")) // >40 kWp:  7.29 ct/kWh (≤750 kWp)
            .build()
            .ok(),

        // ── EEG 2017 — Q2 2017 starting rate ─────────────────────────────────
        2017..=2020 => RateLookup::builder()
            .at_most(dec!(10), amount_ct("12.35")) // ≤10 kWp: 12.35 ct/kWh
            .at_most(dec!(40), amount_ct("12.00")) // ≤40 kWp: 12.00 ct/kWh
            .fallback(amount_ct("10.96")) // >40 kWp: 10.96 ct/kWh (≤750 kWp)
            .build()
            .ok(),

        // ── Earlier EEG versions ──────────────────────────────────────────────
        _ => None,
    }
}

/// Backward-compatible alias for [`solar_pv_ueberschuss_lookup`].
///
/// Returns Überschusseinspeisung rates. Prefer the explicit function name in new code.
pub fn solar_pv_lookup(eeg_year: i16) -> Option<RateLookup> {
    solar_pv_ueberschuss_lookup(eeg_year)
}

/// Return the EEG **Volleinspeisung** rate table for solar PV plants.
///
/// These are the **§48 Abs. 2 + Abs. 2a** rates — for plants that declare 100%
/// grid feed-in (Volleinspeisung: all generated electricity goes to the grid).
/// The bonus (§48 Abs. 2a) is added to the §48 Abs. 2 base rate.
///
/// ## EEG 2024 (Solarpaket I) Volleinspeisung rates (§48 Abs. 2 + Abs. 2a):
///
/// | Capacity | Base (Abs. 2) | Bonus (Abs. 2a) | Total |
/// |---|---|---|---|
/// | ≤10 kWp | 8.51 ct | +4.80 ct | **13.31 ct/kWh** |
/// | ≤40 kWp | 7.43 ct | +3.80 ct | **11.23 ct/kWh** |
/// | ≤100 kWp | 7.64 ct | +5.10 ct | **12.74 ct/kWh** |
/// | ≤400 kWp | 7.64 ct | +3.20 ct | **10.84 ct/kWh** |
/// | ≤1 MWp | 7.64 ct | +1.90 ct | **9.54 ct/kWh** |
///
/// # Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal_macros::dec;
///
/// // EEG 2024: 9 kWp full feed-in → 13.31 ct/kWh
/// let table = rates::solar_pv_volleinspeisung_lookup(2024).expect("known year");
/// assert_eq!(table.rate_for(dec!(9)).unwrap(), billing::Amount::parse("0.13310").unwrap());
/// ```
pub fn solar_pv_volleinspeisung_lookup(eeg_year: i16) -> Option<RateLookup> {
    match eeg_year {
        // ── EEG 2024 (Solarpaket I) Volleinspeisung ────────────────────────────
        2024..=2026 => RateLookup::builder()
            .at_most(dec!(10), amount_ct("13.31")) // 8.51 + 4.80
            .at_most(dec!(40), amount_ct("11.23")) // 7.43 + 3.80
            .at_most(dec!(100), amount_ct("12.74")) // 7.64 + 5.10
            .at_most(dec!(400), amount_ct("10.84")) // 7.64 + 3.20
            .fallback(amount_ct("9.54")) // 7.64 + 1.90  (≤1 MWp)
            .build()
            .ok(),

        // ── EEG 2023 initial Volleinspeisung ──────────────────────────────────
        // §48 Abs. 2 (8.11/6.79/5.56) + Abs. 2a bonus (4.89/3.79/…)
        2023 => RateLookup::builder()
            .at_most(dec!(10), amount_ct("13.00")) // 8.11 + 4.89
            .at_most(dec!(40), amount_ct("10.58")) // 6.79 + 3.79
            .at_most(dec!(100), amount_ct("11.36")) // 5.56 + 5.80
            .at_most(dec!(400), amount_ct("8.60")) // 5.56 + 3.04
            .fallback(amount_ct("7.31")) // 5.56 + 1.75
            .build()
            .ok(),

        // ── Earlier versions ── use einsd DB lookup for quarterly degression
        _ => None,
    }
}
// | 2017 | BGBl I 2017 S. 2532, §21 + Anlage 1 | Q2 2017 starting rate |
//
// Returns `None` for unsupported years (use `einsd`'s DB table instead).
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
/// use rust_decimal_macros::dec;
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
/// use rust_decimal_macros::dec;
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
/// use rust_decimal_macros::dec;
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
/// Hydro rates have not changed between EEG 2017–2023 because new hydro capacity
/// is minimal and political pressure to adjust these rates is low.
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
/// use rust_decimal_macros::dec;
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
/// ## Example
///
/// ```rust
/// use eeg_billing::rates;
/// use rust_decimal_macros::dec;
///
/// let rate = rates::lookup_rate("SOLAR_AUFDACH", dec!(9), 2023).unwrap();
/// // 9 kWp ≤10 kWp bracket → 8.11 ct/kWh (EEG 2023)
/// assert_eq!(rate, billing::Amount::parse("0.08110").unwrap());
/// ```
pub fn lookup_rate(
    erzeugungsart: &str,
    leistung_kwp: rust_decimal::Decimal,
    eeg_year: i16,
) -> Result<Amount<5>, BillingError> {
    let table = match erzeugungsart {
        "SOLAR_AUFDACH" | "SOLAR_FREIFLAECHE" | "SOLAR_BALKON" | "SOLAR" => {
            solar_pv_lookup(eeg_year)
        }
        "WIND_ONSHORE" => wind_onshore_lookup(eeg_year),
        "BIOMASSE" | "BIOGAS" | "BIOMETHANE" => biomasse_lookup(eeg_year),
        "KWKG" => kwkg_zuschlag_lookup(),
        "WASSERKRAFT" => wasserkraft_lookup(eeg_year),
        "GEOTHERMIE" | "GEZEITEN" => geothermie_lookup(eeg_year),
        "KLAEGAS" | "GRUBENGAS" | "DEPONIEGAS" => gasart_lookup(eeg_year),
        _ => None,
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
/// use rust_decimal_macros::dec;
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
/// use rust_decimal_macros::dec;
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
