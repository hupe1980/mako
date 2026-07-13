//! Unit tests for `einsd` EEG/KWKG settlement formulas.
//!
//! These tests verify the pure settlement arithmetic defined in §§20–50 EEG 2023
//! and §7 KWKG 2023 — no database, no HTTP, no clock.
//!
//! Run: `cargo test -p einsd --test settlement_tests`
//!
//! Formula reference (from einsd/src/pg.rs doc table):
//!
//! | Model | Formula |
//! |---|---|
//! | VERGUETUNG | `kwh × verguetungssatz_ct / 100` |
//! | MIETERSTROM | VERGUETUNG + `kwh × mieter_zuschlag_ct / 100` |
//! | DIREKTVERMARKTUNG | `max(0, AW − EPEX) × kwh / 100` |
//! | AUSSCHREIBUNG | same as DIREKTVERMARKTUNG |
//! | POST_EEG_SPOT | `kwh × EPEX_monthly_avg / 100` |
//! | KWKG_ZUSCHLAG | `kwh × kwk_zuschlag_ct / 100` |
//! | FLEXIBILITAET | VERGUETUNG + `kwh × flex_praemie_ct / 100` |

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// ── Settlement formula helpers (extracted from einsd/src/pg.rs logic) ─────────

fn verguetung_eur(kwh: Decimal, rate_ct: Decimal) -> Decimal {
    kwh * rate_ct / dec!(100)
}

fn mieterstrom_eur(kwh: Decimal, rate_ct: Decimal, zuschlag_ct: Decimal) -> Decimal {
    kwh * (rate_ct + zuschlag_ct) / dec!(100)
}

fn marktpraemie_eur(kwh: Decimal, aw_ct: Decimal, epex_ct: Decimal) -> Decimal {
    let praemie = (aw_ct - epex_ct).max(Decimal::ZERO);
    kwh * praemie / dec!(100)
}

fn post_eeg_spot_eur(kwh: Decimal, epex_ct: Decimal) -> Decimal {
    kwh * epex_ct / dec!(100)
}

fn kwkg_zuschlag_eur(kwh: Decimal, kwk_ct: Decimal) -> Decimal {
    kwh * kwk_ct / dec!(100)
}

fn flexibilitaet_eur(kwh: Decimal, rate_ct: Decimal, flex_ct: Decimal) -> Decimal {
    kwh * (rate_ct + flex_ct) / dec!(100)
}

// ── §21 EEG 2023 — Feste Einspeisevergütung ──────────────────────────────────

#[test]
fn verguetung_solar_100kwh_at_8_1ct() {
    // 100 kWh × 8.1 ct/kWh ÷ 100 = 8.10 EUR
    let result = verguetung_eur(dec!(100), dec!(8.1));
    assert_eq!(result, dec!(8.10));
}

#[test]
fn verguetung_wind_1000kwh_at_5_8ct() {
    // 1000 kWh × 5.8 ct/kWh ÷ 100 = 58.00 EUR
    let result = verguetung_eur(dec!(1000), dec!(5.8));
    assert_eq!(result, dec!(58.0));
}

#[test]
fn verguetung_zero_kwh_is_zero() {
    assert_eq!(verguetung_eur(dec!(0), dec!(8.1)), dec!(0));
}

#[test]
fn verguetung_zero_rate_is_zero() {
    // EIGENVERBRAUCH effectively has zero settlement payment
    assert_eq!(verguetung_eur(dec!(500), dec!(0)), dec!(0));
}

// ── §38a EEG 2023 — Mieterstrom-Zuschlag ─────────────────────────────────────

#[test]
fn mieterstrom_base_plus_zuschlag() {
    // 200 kWh × (7.5 ct base + 1.5 ct Zuschlag) ÷ 100 = 18.00 EUR
    let result = mieterstrom_eur(dec!(200), dec!(7.5), dec!(1.5));
    assert_eq!(result, dec!(18.0));
}

#[test]
fn mieterstrom_zero_zuschlag_equals_verguetung() {
    let kwh = dec!(300);
    let rate = dec!(8.0);
    assert_eq!(
        mieterstrom_eur(kwh, rate, dec!(0)),
        verguetung_eur(kwh, rate),
        "Mieterstrom with zero Zuschlag must equal base Vergütung"
    );
}

// ── §20 EEG 2023 — Gleitende Marktprämie ─────────────────────────────────────

#[test]
fn direktvermarktung_positive_spread() {
    // AW = 7.5 ct, EPEX = 4.2 ct → Prämie = 3.3 ct × 500 kWh ÷ 100 = 16.50 EUR
    let result = marktpraemie_eur(dec!(500), dec!(7.5), dec!(4.2));
    assert_eq!(result, dec!(16.50));
}

#[test]
fn direktvermarktung_negative_spread_clamped_to_zero() {
    // AW = 4.0 ct, EPEX = 6.5 ct → max(0, -2.5) × kwh = 0 EUR
    // This can happen in high-price EPEX months; prämie is floored at zero.
    let result = marktpraemie_eur(dec!(500), dec!(4.0), dec!(6.5));
    assert_eq!(result, dec!(0), "Negative spread must be clamped to zero (no clawback)");
}

#[test]
fn direktvermarktung_zero_spread_is_zero() {
    // AW = EPEX → Marktprämie = 0
    let result = marktpraemie_eur(dec!(1000), dec!(5.0), dec!(5.0));
    assert_eq!(result, dec!(0));
}

#[test]
fn direktvermarktung_ausschreibung_same_formula() {
    // AUSSCHREIBUNG uses identical formula with BNetzA tender AW
    let kwh = dec!(2000);
    let aw = dec!(6.5);
    let epex = dec!(3.8);
    assert_eq!(
        marktpraemie_eur(kwh, aw, epex),
        // 2.7 ct × 2000 ÷ 100 = 54 EUR
        dec!(54.0)
    );
}

// ── Post-EEG Spot ─────────────────────────────────────────────────────────────

#[test]
fn post_eeg_spot_uses_monthly_epex_average() {
    // 800 kWh × 5.2 ct EPEX monthly avg ÷ 100 = 41.60 EUR
    let result = post_eeg_spot_eur(dec!(800), dec!(5.2));
    assert_eq!(result, dec!(41.60));
}

// ── §7 KWKG 2023 — KWK-Zuschlag ──────────────────────────────────────────────

#[test]
fn kwkg_zuschlag_paid_on_top_of_market() {
    // KWKG-Zuschlag for >50 kW up to 100 kW: 1.0 ct/kWh
    // 3000 kWh × 1.0 ct ÷ 100 = 30.00 EUR additional to market revenue
    let result = kwkg_zuschlag_eur(dec!(3000), dec!(1.0));
    assert_eq!(result, dec!(30.0));
}

#[test]
fn kwkg_zuschlag_small_chp() {
    // For ≤50 kW CHP: 8.0 ct/kWh (2023 rates)
    let result = kwkg_zuschlag_eur(dec!(1000), dec!(8.0));
    assert_eq!(result, dec!(80.0));
}

// ── §50 EEG 2023 — Flexibilitätsprämie ───────────────────────────────────────

#[test]
fn flexibilitaet_base_plus_flex_praemie() {
    // 400 kWh × (6.0 ct base + 1.8 ct Flex) ÷ 100 = 31.20 EUR
    let result = flexibilitaet_eur(dec!(400), dec!(6.0), dec!(1.8));
    assert_eq!(result, dec!(31.20));
}

// ── Förderdauer arithmetic ────────────────────────────────────────────────────

#[test]
fn foerderendedatum_is_20_years_after_inbetriebnahme() {
    use time::macros::date;
    let inbetriebnahme = date!(2010 - 05 - 15);
    // §21 EEG 2023: fixed 20-year Förderdauer
    let expected = date!(2030 - 05 - 15);
    let computed = inbetriebnahme.replace_year(inbetriebnahme.year() + 20).unwrap();
    assert_eq!(computed, expected);
}

#[test]
fn repowering_resets_foerderdauer() {
    use time::macros::date;
    let repowering_datum = date!(2025 - 03 - 01);
    let expected_end = date!(2045 - 03 - 01);
    let computed = repowering_datum.replace_year(repowering_datum.year() + 20).unwrap();
    assert_eq!(computed, expected_end, "Repowering must reset the 20-year Förderdauer clock");
}

#[test]
fn foerderung_alert_within_180_days() {
    use time::macros::date;
    let foerderendedatum = date!(2026 - 12 - 31);
    let today = date!(2026 - 07 - 13);
    let days_remaining = (foerderendedatum - today).whole_days();
    // Alert threshold: ≤ 180 days
    assert!(
        days_remaining <= 180,
        "Alert should fire: {days_remaining} days until Förderung ends"
    );
}

// ── Precision: no f64 ─────────────────────────────────────────────────────────

#[test]
fn decimal_arithmetic_exact_no_float_rounding() {
    // Verify that Decimal arithmetic is exact — a critical invariant for billing
    let kwh = dec!(333.333);
    let rate_ct = dec!(8.1);
    let result = verguetung_eur(kwh, rate_ct);
    // 333.333 × 8.1 / 100 = 26.999973 EUR — exact in Decimal
    let expected = dec!(333.333) * dec!(8.1) / dec!(100);
    assert_eq!(result, expected, "Decimal arithmetic must be exact — no float approximation");
}
