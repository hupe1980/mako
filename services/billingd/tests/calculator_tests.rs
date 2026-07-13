//! Unit tests for `billingd::calculator` — pure billing arithmetic.
//!
//! No HTTP, no database, no external services.
//! Run: `cargo test -p billingd --test calculator_tests`

use billingd::calculator::{
    EegMeterInput, GasMeterInput, GridInput, HemsMeterInput, MeterInput, RegulatoryRates,
    SolarMeterInput, TariffInput, WaermeMeterInput, calculate_eeg, calculate_einspeisung,
    calculate_gas, calculate_hems, calculate_solar, calculate_strom, calculate_waerme,
};
use rust_decimal_macros::dec;
use time::macros::date;

fn rates_2026() -> RegulatoryRates {
    RegulatoryRates {
        stromsteuer_ct_per_kwh: dec!(2.05),
        energiesteuer_gas_ct_per_kwh: dec!(0.55),
        behg_gas_ct_per_kwh: dec!(0.62),
        mwst_rate: dec!(0.19),
    }
}

fn period() -> (time::Date, time::Date) {
    (date!(2026 - 01 - 01), date!(2026 - 01 - 31))
}

fn no_grid() -> GridInput { GridInput::default() }

fn j(s: &str) -> TariffInput { serde_json::from_str(s).unwrap() }

fn meter(kwh: rust_decimal::Decimal) -> MeterInput {
    MeterInput { arbeitsmenge_kwh: kwh, arbeitsmenge_ht_kwh: None, arbeitsmenge_nt_kwh: None, spitzenleistung_kw: None, steuerung_stunden: None }
}

// ── Strom ─────────────────────────────────────────────────────────────────────

#[test]
fn strom_flat_brutto_includes_stromsteuer_and_mwst() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"STROM","register_count":"Eintarif","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let r = calculate_strom("malo","lf","R001",f,t,&tariff,&meter(dec!(100)),&no_grid(),None,&rates_2026()).unwrap();
    // Netto ≈ 21.35 EUR; Brutto ≈ 25.41 EUR
    assert!(r.brutto_eur > dec!(25) && r.brutto_eur < dec!(26), "Brutto {} not in [25,26]", r.brutto_eur);
}

#[test]
fn strom_eeg_gutschrift_reduces_brutto() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let r0 = calculate_strom("m","l","Ra",f,t,&tariff,&meter(dec!(200)),&no_grid(),None,          &rates_2026()).unwrap();
    let r5 = calculate_strom("m","l","Rb",f,t,&tariff,&meter(dec!(200)),&no_grid(),Some(dec!(5)), &rates_2026()).unwrap();
    assert!(r5.brutto_eur < r0.brutto_eur);
    let diff = r0.brutto_eur - r5.brutto_eur;
    // 5 EUR × 1.19 = 5.95 EUR
    assert!(diff > dec!(5.9) && diff < dec!(6.0), "Expected ~5.95 diff, got {diff}");
}

#[test]
fn strom_mwst_override_zero_removes_vat() {
    let (f, t) = period();
    let t1 = j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let t2 = j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10,"mwst_rate_override":0}"#);
    let r1 = calculate_strom("m","l","Ra",f,t,&t1,&meter(dec!(100)),&no_grid(),None,&rates_2026()).unwrap();
    let r2 = calculate_strom("m","l","Rb",f,t,&t2,&meter(dec!(100)),&no_grid(),None,&rates_2026()).unwrap();
    assert!(r2.brutto_eur < r1.brutto_eur, "zero MwSt must reduce brutto");
    assert_eq!(r2.mwst_eur, dec!(0), "mwst_eur must be zero");
}

#[test]
fn strom_zweitarif_higher_than_ht_eintarif() {
    let (f, t) = period();
    let ht = j(r#"{"category":"STROM","register_count":"Eintarif","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":12}"#);
    let zt = j(r#"{"category":"STROM","register_count":"Zweitarif","grundpreis_ct_per_day":0,"arbeitspreis_ht_ct_per_kwh":12,"arbeitspreis_nt_ct_per_kwh":7}"#);
    let m_ht = MeterInput { arbeitsmenge_kwh: dec!(100), arbeitsmenge_ht_kwh: None, arbeitsmenge_nt_kwh: None, spitzenleistung_kw: None, steuerung_stunden: None };
    let m_zt = MeterInput { arbeitsmenge_kwh: dec!(150), arbeitsmenge_ht_kwh: Some(dec!(100)), arbeitsmenge_nt_kwh: Some(dec!(50)), spitzenleistung_kw: None, steuerung_stunden: None };
    let r_ht = calculate_strom("m","l","Ra",f,t,&ht,&m_ht,&no_grid(),None,&rates_2026()).unwrap();
    let r_zt = calculate_strom("m","l","Rb",f,t,&zt,&m_zt,&no_grid(),None,&rates_2026()).unwrap();
    assert!(r_zt.netto_eur > r_ht.netto_eur, "Zweitarif netto must be higher");
}

#[test]
fn billing_result_rechnung_json_has_period() {
    let from = date!(2026 - 03 - 01);
    let to   = date!(2026 - 03 - 31);
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = calculate_strom("m","l","R",from,to,&tariff,&meter(dec!(50)),&no_grid(),None,&rates_2026()).unwrap();
    let start = r.rechnung_json["rechnungsperiode"]["startdatum"].as_str().unwrap_or("");
    assert!(start.contains("2026-03-01"), "startdatum '{start}' does not match");
    let end = r.rechnung_json["rechnungsperiode"]["enddatum"].as_str().unwrap_or("");
    assert!(end.contains("2026-03-31"), "enddatum '{end}' does not match");
}

// ── Gas ───────────────────────────────────────────────────────────────────────

#[test]
fn gas_kwh_hs_direct_determines_arbeit() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#);
    let m = GasMeterInput { messung_qm3: dec!(0), brennwert_kwh_per_qm3: None, zustandszahl: None, kwh_hs: Some(dec!(500)), gasqualitaet: None };
    let r = calculate_gas("m","l","G01",f,t,&tariff,&m,&no_grid(),&rates_2026()).unwrap();
    // 500 × 10 ct + levies > 50 EUR
    assert!(r.brutto_eur > dec!(50), "Gas brutto {} too low", r.brutto_eur);
}

#[test]
fn gas_brennwert_conversion_equivalent_to_kwh_hs() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#);
    // 100 m³ × 10 kWh/m³ × 1.0 Zs = 1000 kWh_Hs
    let m1 = GasMeterInput { messung_qm3: dec!(100), brennwert_kwh_per_qm3: Some(dec!(10)), zustandszahl: Some(dec!(1)), kwh_hs: None, gasqualitaet: None };
    let m2 = GasMeterInput { messung_qm3: dec!(0), brennwert_kwh_per_qm3: None, zustandszahl: None, kwh_hs: Some(dec!(1000)), gasqualitaet: None };
    let r1 = calculate_gas("m","l","G02",f,t,&tariff,&m1,&no_grid(),&rates_2026()).unwrap();
    let r2 = calculate_gas("m","l","G03",f,t,&tariff,&m2,&no_grid(),&rates_2026()).unwrap();
    assert_eq!(r1.brutto_eur, r2.brutto_eur, "Brennwert and kwh_hs must be equivalent");
}

// ── Wärme ─────────────────────────────────────────────────────────────────────

#[test]
fn waerme_leistungspreis_adds_to_brutto() {
    let (f, t) = period();
    let t1 = j(r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":0,"waerme_arbeitspreis_ct_per_kwh":12}"#);
    let t2 = j(r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":0,"waerme_arbeitspreis_ct_per_kwh":12,"waerme_leistungspreis_eur_per_kw_month":5}"#);
    let m = WaermeMeterInput { kwh_waerme: dec!(300), spitzenleistung_kw: Some(dec!(10)), months: Some(dec!(1)) };
    let r1 = calculate_waerme("m","l","W01",f,t,&t1,&m,&rates_2026()).unwrap();
    let r2 = calculate_waerme("m","l","W02",f,t,&t2,&m,&rates_2026()).unwrap();
    assert!(r2.brutto_eur > r1.brutto_eur, "Leistungspreis must add to brutto");
    // 10 kW × 5 EUR × 1.19 = 59.50 EUR additional
    let diff = r2.brutto_eur - r1.brutto_eur;
    assert!(diff > dec!(59) && diff < dec!(60), "Expected ~59.5, got {diff}");
}

// ── EEG ───────────────────────────────────────────────────────────────────────

#[test]
fn eeg_verguetung_is_credit_note() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.1}"#);
    let m = EegMeterInput { einspeisung_kwh: dec!(500) };
    let r = calculate_eeg("m","l","E01",f,t,&tariff,&m,&rates_2026()).unwrap();
    // EEG Vergütung: positive brutto (amount paid TO the generator),
    // tagged as rechnungsart = "GUTSCHRIFT" in the JSON
    assert!(r.brutto_eur > dec!(0), "EEG Vergütung must be positive, got {}", r.brutto_eur);
    // 500 × 8.1 ct × 1.19 ≈ 48.20 EUR
    assert!(r.brutto_eur > dec!(45) && r.brutto_eur < dec!(50), "{}", r.brutto_eur);
    let rechnungsart = r.rechnung_json["rechnungsart"].as_str().unwrap_or("");
    assert_eq!(rechnungsart, "GUTSCHRIFT", "EEG invoice must be tagged GUTSCHRIFT");
}

#[test]
fn eeg_zero_einspeisung_is_zero() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.1}"#);
    let r = calculate_eeg("m","l","E02",f,t,&tariff,&EegMeterInput { einspeisung_kwh: dec!(0) },&rates_2026()).unwrap();
    assert_eq!(r.brutto_eur, dec!(0));
}

// ── Solar ─────────────────────────────────────────────────────────────────────

#[test]
fn solar_mieterstrom_zuschlag_adds_to_brutto() {
    let (f, t) = period();
    let base = j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25}"#);
    let ms   = j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25,"mieterstrom_aufschlag_ct_per_kwh":1.5}"#);
    let m = SolarMeterInput { eigenverbrauch_kwh: dec!(200) };
    let r1 = calculate_solar("m","l","S01",f,t,&base,&m,&rates_2026()).unwrap();
    let r2 = calculate_solar("m","l","S02",f,t,&ms  ,&m,&rates_2026()).unwrap();
    assert!(r2.brutto_eur > r1.brutto_eur);
    let diff = r2.brutto_eur - r1.brutto_eur;
    // 200 × 1.5 ct × 1.19 = 3.57 EUR
    assert!(diff > dec!(3.5) && diff < dec!(3.65), "Expected ~3.57, got {diff}");
}

// ── HEMS ──────────────────────────────────────────────────────────────────────

#[test]
fn hems_platform_fee_one_month() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"HEMS","hems_platform_fee_eur_per_month":14.99}"#);
    let m = HemsMeterInput { months: Some(dec!(1)), optimization_events: None, readout_events: None };
    let r = calculate_hems("m","l","H01",f,t,&tariff,&m,&rates_2026()).unwrap();
    // 14.99 × 1.19 ≈ 17.84 EUR
    assert!(r.brutto_eur > dec!(17.5) && r.brutto_eur < dec!(18.5), "{}", r.brutto_eur);
}

// ── Einspeisung ───────────────────────────────────────────────────────────────

#[test]
fn einspeisung_net_settlement_is_gutschrift() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"EINSPEISUNG","marktwert_ct_per_kwh":6.0,"vermarktungsgebuehr_ct_per_kwh":0.5}"#);
    let m = EegMeterInput { einspeisung_kwh: dec!(800) };
    let r = calculate_einspeisung("m","l","I01",f,t,&tariff,&m,&rates_2026()).unwrap();
    // EINSPEISUNG = payment to Direktvermarkter — positive brutto, tagged GUTSCHRIFT
    assert!(r.brutto_eur > dec!(0), "Direktvermarktung brutto must be positive");
    let rechnungsart = r.rechnung_json["rechnungsart"].as_str().unwrap_or("");
    assert_eq!(rechnungsart, "GUTSCHRIFT");
}

// ── GasQualitaet audit annotation ─────────────────────────────────────────────

#[test]
fn gas_gasqualitaet_added_as_zusatz_attribut() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#);
    // Billing with H2-blended gas: the Brennwert is already the measured H2-blended value
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(500)),
        gasqualitaet: Some("H2_BLEND".into()),
    };
    let r = calculate_gas("m","l","G-GQ01",f,t,&tariff,&m,&no_grid(),&rates_2026()).unwrap();

    // Brutto must be identical to billing without gasqualitaet (no correction applied)
    let m_no_gq = GasMeterInput { gasqualitaet: None, ..m.clone() };
    let r_no_gq = calculate_gas("m","l","G-GQ02",f,t,&tariff,&m_no_gq,&no_grid(),&rates_2026()).unwrap();
    assert_eq!(r.brutto_eur, r_no_gq.brutto_eur, "gasqualitaet must not alter the billing amount");

    // The ZusatzAttribut must appear in rechnung_json
    let attrs = r.rechnung_json["zusatzAttribute"].as_array();
    assert!(attrs.is_some(), "zusatzAttribute must be present");
    let has_gq = attrs.unwrap().iter().any(|a| {
        a["name"].as_str() == Some("gasqualitaet") && a["wert"].as_str() == Some("H2_BLEND")
    });
    assert!(has_gq, "gasqualitaet ZusatzAttribut must be in rechnung_json");
}

#[test]
fn gas_no_gasqualitaet_no_zusatz_attribut() {
    let (f, t) = period();
    let tariff = j(r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#);
    let m = GasMeterInput {
        messung_qm3: dec!(0), brennwert_kwh_per_qm3: None, zustandszahl: None,
        kwh_hs: Some(dec!(100)), gasqualitaet: None,
    };
    let r = calculate_gas("m","l","G-GQ03",f,t,&tariff,&m,&no_grid(),&rates_2026()).unwrap();
    // No gasqualitaet → no zusatzAttribute array (or empty array)
    let gq_found = r.rechnung_json["zusatzAttribute"]
        .as_array()
        .map(|a| a.iter().any(|v| v["name"].as_str() == Some("gasqualitaet")))
        .unwrap_or(false);
    assert!(!gq_found, "gasqualitaet must not appear when not set");
}
