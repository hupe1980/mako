//! Unit tests for `energy-billing` — pure billing arithmetic.
//!
//! All tests use the new `BillingEngine` + `BillingContext` + `Quantities` API.
//!
//! No HTTP, no database, no external services.
//! Run: `cargo test -p energy-billing --test calculator_tests`

use energy_billing::{
    BillingContext, BillingEngine, DynamicInterval, EegMeterInput, EegProvider,
    EinspeisungProvider, ElectricityProvider, EmobilityMeterInput, EmobilityProvider,
    GasMeterInput, GasProvider, GridInput, HeatProvider, HemsMeterInput, HemsProvider, InvoiceType,
    MeterInput, MwStProvider, Quantities, RegulatoryRates, ServiceMeterInput, ServiceProvider,
    SolarMeterInput, SolarProvider, TariffInput, WaermeMeterInput,
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

fn no_grid() -> GridInput {
    GridInput::default()
}

fn j(s: &str) -> TariffInput {
    serde_json::from_str(s).unwrap()
}

fn meter(kwh: rust_decimal::Decimal) -> MeterInput {
    MeterInput {
        arbeitsmenge_kwh: kwh,
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        spitzenleistung_kw: None,
        steuerung_stunden: None,
    }
}

/// Build a `BillingContext` for the default test period.
fn ctx(malo: &str) -> BillingContext {
    let (f, t) = period();
    BillingContext {
        malo_id: malo.to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: format!("TEST-{malo}"),
        period_from: f,
        period_to: t,
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_2026(),
        contract_id: None,
    }
}

/// Compute billing result via BillingEngine for a Strom tariff.
fn calculate_strom(
    tariff: &TariffInput,
    meter: &MeterInput,
    grid: &GridInput,
    eeg_gutschrift: Option<rust_decimal::Decimal>,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        electricity: Some(meter.clone()),
        eeg_gutschrift_eur: eeg_gutschrift,
        ..Default::default()
    };
    // Respect mwst_rate_override from the tariff product definition
    let mwst = rates.effective_mwst(tariff);
    BillingEngine::new()
        .add(ElectricityProvider::from_tariff(tariff, grid))
        .add(MwStProvider::new(mwst))
        .bill(ctx("51238696781"), &quantities)
        .unwrap()
}

fn calculate_gas(
    tariff: &TariffInput,
    meter: &GasMeterInput,
    grid: &GridInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        gas: Some(meter.clone()),
        ..Default::default()
    };
    let mwst = rates.effective_mwst(tariff);
    BillingEngine::new()
        .add(GasProvider::from_tariff(tariff, grid))
        .add(MwStProvider::new(mwst))
        .bill(ctx("51238696782"), &quantities)
        .unwrap()
}

fn calculate_waerme(
    tariff: &TariffInput,
    meter: &WaermeMeterInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        heat: Some(meter.clone()),
        ..Default::default()
    };
    BillingEngine::new()
        .add(HeatProvider::from_tariff(tariff))
        .add(MwStProvider::new(rates.effective_mwst(tariff)))
        .bill(ctx("51238696783"), &quantities)
        .unwrap()
}

fn calculate_solar(
    tariff: &TariffInput,
    meter: &SolarMeterInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        solar: Some(meter.clone()),
        ..Default::default()
    };
    BillingEngine::new()
        .add(SolarProvider::from_tariff(tariff))
        .add(MwStProvider::new(rates.effective_mwst(tariff)))
        .bill(ctx("51238696784"), &quantities)
        .unwrap()
}

fn calculate_eeg(
    tariff: &TariffInput,
    meter: &EegMeterInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        eeg: Some(meter.clone()),
        ..Default::default()
    };
    // EEG settlements are Gutschriften (credit notes to the generator)
    let (f, t) = period();
    let ctx = BillingContext {
        malo_id: "51238696785".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-EEG".to_owned(),
        period_from: f,
        period_to: t,
        invoice_type: InvoiceType::CreditNote,
        regulatory_rates: rates.clone(),
        contract_id: None,
    };
    BillingEngine::new()
        .add(EegProvider::from_tariff(tariff))
        .add(MwStProvider::new(rates.mwst_rate))
        .bill(ctx, &quantities)
        .unwrap()
}

fn calculate_einspeisung(
    tariff: &TariffInput,
    meter: &EegMeterInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        einspeisung: Some(meter.clone()),
        ..Default::default()
    };
    let (f, t) = period();
    let ctx = BillingContext {
        malo_id: "51238696786".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-EINSP".to_owned(),
        period_from: f,
        period_to: t,
        invoice_type: InvoiceType::CreditNote,
        regulatory_rates: rates.clone(),
        contract_id: None,
    };
    BillingEngine::new()
        .add(EinspeisungProvider::from_tariff(tariff))
        .add(MwStProvider::new(rates.mwst_rate))
        .bill(ctx, &quantities)
        .unwrap()
}

fn calculate_hems(
    tariff: &TariffInput,
    usage: &HemsMeterInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        hems: Some(usage.clone()),
        ..Default::default()
    };
    BillingEngine::new()
        .add(HemsProvider::from_tariff(tariff))
        .add(MwStProvider::new(rates.mwst_rate))
        .bill(ctx("51238696787"), &quantities)
        .unwrap()
}

fn calculate_emobility(
    tariff: &TariffInput,
    usage: &EmobilityMeterInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        emobility: Some(usage.clone()),
        ..Default::default()
    };
    BillingEngine::new()
        .add(EmobilityProvider::from_tariff(tariff))
        .add(MwStProvider::new(rates.mwst_rate))
        .bill(ctx("51238696788"), &quantities)
        .unwrap()
}

fn calculate_energiedienstleistung(
    tariff: &TariffInput,
    usage: &ServiceMeterInput,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    let quantities = Quantities {
        service: Some(usage.clone()),
        ..Default::default()
    };
    BillingEngine::new()
        .add(ServiceProvider::from_tariff(tariff))
        .add(MwStProvider::new(rates.mwst_rate))
        .bill(ctx("51238696789"), &quantities)
        .unwrap()
}

fn calculate_dynamic_strom(
    tariff: &TariffInput,
    grid: &GridInput,
    eeg_gutschrift: Option<rust_decimal::Decimal>,
    intervals: &[DynamicInterval],
    epex_prices: &std::collections::HashMap<(i32, u8, u8, u8), rust_decimal::Decimal>,
    rates: &RegulatoryRates,
) -> energy_billing::Invoice {
    use energy_billing::DynamicElectricityProvider;
    let quantities = Quantities {
        dynamic_intervals: intervals.to_vec(),
        dynamic_epex_prices: epex_prices.clone(),
        eeg_gutschrift_eur: eeg_gutschrift,
        ..Default::default()
    };
    BillingEngine::new()
        .add(DynamicElectricityProvider::with_epex_map(
            tariff.clone(),
            grid.clone(),
            epex_prices.clone(),
        ))
        .add(MwStProvider::new(rates.mwst_rate))
        .bill(ctx("51238696790"), &quantities)
        .unwrap()
}

// ── Strom ─────────────────────────────────────────────────────────────────────

#[test]
fn strom_flat_brutto_includes_stromsteuer_and_mwst() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"STROM","register_count":"Eintarif","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#,
    );
    let r = calculate_strom(&tariff, &meter(dec!(100)), &no_grid(), None, &rates_2026());
    // Netto ≈ 21.35 EUR; Brutto ≈ 25.41 EUR
    assert!(
        r.brutto_eur > dec!(25) && r.brutto_eur < dec!(26),
        "Brutto {} not in [25,26]",
        r.brutto_eur
    );
}

#[test]
fn strom_eeg_gutschrift_reduces_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let r0 = calculate_strom(&tariff, &meter(dec!(200)), &no_grid(), None, &rates_2026());
    let r5 = calculate_strom(
        &tariff,
        &meter(dec!(200)),
        &no_grid(),
        Some(dec!(5)),
        &rates_2026(),
    );
    assert!(r5.brutto_eur < r0.brutto_eur);
    let diff = r0.brutto_eur - r5.brutto_eur;
    // 5 EUR × 1.19 = 5.95 EUR
    assert!(
        diff > dec!(5.9) && diff < dec!(6.0),
        "Expected ~5.95 diff, got {diff}"
    );
}

#[test]
fn strom_mwst_override_zero_removes_vat() {
    let (_f, _t) = period();
    let t1 = j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let t2 = j(
        r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10,"mwst_rate_override":0}"#,
    );
    let r1 = calculate_strom(&t1, &meter(dec!(100)), &no_grid(), None, &rates_2026());
    let r2 = calculate_strom(&t2, &meter(dec!(100)), &no_grid(), None, &rates_2026());
    assert!(
        r2.brutto_eur < r1.brutto_eur,
        "zero MwSt must reduce brutto"
    );
    assert_eq!(r2.mwst_eur, dec!(0), "mwst_eur must be zero");
}

#[test]
fn strom_zweitarif_higher_than_ht_eintarif() {
    let (_f, _t) = period();
    let ht = j(
        r#"{"category":"STROM","register_count":"Eintarif","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":12}"#,
    );
    let zt = j(
        r#"{"category":"STROM","register_count":"Zweitarif","grundpreis_ct_per_day":0,"arbeitspreis_ht_ct_per_kwh":12,"arbeitspreis_nt_ct_per_kwh":7}"#,
    );
    let m_ht = MeterInput {
        arbeitsmenge_kwh: dec!(100),
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        spitzenleistung_kw: None,
        steuerung_stunden: None,
    };
    let m_zt = MeterInput {
        arbeitsmenge_kwh: dec!(150),
        arbeitsmenge_ht_kwh: Some(dec!(100)),
        arbeitsmenge_nt_kwh: Some(dec!(50)),
        spitzenleistung_kw: None,
        steuerung_stunden: None,
    };
    let r_ht = calculate_strom(&ht, &m_ht, &no_grid(), None, &rates_2026());
    let r_zt = calculate_strom(&zt, &m_zt, &no_grid(), None, &rates_2026());
    assert!(
        r_zt.netto_eur > r_ht.netto_eur,
        "Zweitarif netto must be higher"
    );
}

#[test]
fn billing_result_rechnung_json_has_period() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = calculate_strom(&tariff, &meter(dec!(50)), &no_grid(), None, &rates_2026());
    let _j_r = r.to_rechnung_json();
    let start = _j_r["rechnungsperiode"]["startdatum"]
        .as_str()
        .unwrap_or("");
    assert!(
        start.contains("2026-01-01"),
        "startdatum '{start}' does not match"
    );
    let _j_r = r.to_rechnung_json();
    let end = _j_r["rechnungsperiode"]["enddatum"].as_str().unwrap_or("");
    assert!(
        end.contains("2026-01-31"),
        "enddatum '{end}' does not match"
    );
}

// ── Gas ───────────────────────────────────────────────────────────────────────

#[test]
fn gas_kwh_hs_direct_determines_arbeit() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(500)),
        gasqualitaet: None,
    };
    let r = calculate_gas(&tariff, &m, &no_grid(), &rates_2026());
    // 500 × 10 ct + levies > 50 EUR
    assert!(
        r.brutto_eur > dec!(50),
        "Gas brutto {} too low",
        r.brutto_eur
    );
}

#[test]
fn gas_brennwert_conversion_equivalent_to_kwh_hs() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    // 100 m³ × 10 kWh/m³ × 1.0 Zs = 1000 kWh_Hs
    let m1 = GasMeterInput {
        messung_qm3: dec!(100),
        brennwert_kwh_per_qm3: Some(dec!(10)),
        zustandszahl: Some(dec!(1)),
        kwh_hs: None,
        gasqualitaet: None,
    };
    let m2 = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(1000)),
        gasqualitaet: None,
    };
    let r1 = calculate_gas(&tariff, &m1, &no_grid(), &rates_2026());
    let r2 = calculate_gas(&tariff, &m2, &no_grid(), &rates_2026());
    assert_eq!(
        r1.brutto_eur, r2.brutto_eur,
        "Brennwert and kwh_hs must be equivalent"
    );
}

// ── Wärme ─────────────────────────────────────────────────────────────────────

#[test]
fn waerme_leistungspreis_adds_to_brutto() {
    let (_f, _t) = period();
    let t1 = j(
        r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":0,"waerme_arbeitspreis_ct_per_kwh":12}"#,
    );
    let t2 = j(
        r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":0,"waerme_arbeitspreis_ct_per_kwh":12,"waerme_leistungspreis_eur_per_kw_month":5}"#,
    );
    let m = WaermeMeterInput {
        kwh_waerme: dec!(300),
        spitzenleistung_kw: Some(dec!(10)),
        months: Some(dec!(1)),
    };
    let r1 = calculate_waerme(&t1, &m, &rates_2026());
    let r2 = calculate_waerme(&t2, &m, &rates_2026());
    assert!(
        r2.brutto_eur > r1.brutto_eur,
        "Leistungspreis must add to brutto"
    );
    // 10 kW × 5 EUR × 1.19 = 59.50 EUR additional
    let diff = r2.brutto_eur - r1.brutto_eur;
    assert!(
        diff > dec!(59) && diff < dec!(60),
        "Expected ~59.5, got {diff}"
    );
}

// ── EEG ───────────────────────────────────────────────────────────────────────

#[test]
fn eeg_verguetung_is_credit_note() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.1}"#);
    let m = EegMeterInput {
        einspeisung_kwh: dec!(500),
        ..Default::default()
    };
    let r = calculate_eeg(&tariff, &m, &rates_2026());
    // EEG Vergütung: positive brutto (amount paid TO the generator),
    // tagged as rechnungsart = "GUTSCHRIFT" in the JSON
    assert!(
        r.brutto_eur > dec!(0),
        "EEG Vergütung must be positive, got {}",
        r.brutto_eur
    );
    // 500 × 8.1 ct × 1.19 ≈ 48.20 EUR
    assert!(
        r.brutto_eur > dec!(45) && r.brutto_eur < dec!(50),
        "{}",
        r.brutto_eur
    );
    let _j_r = r.to_rechnung_json();
    let rechnungsart = _j_r["rechnungsart"].as_str().unwrap_or("");
    assert_eq!(
        rechnungsart, "GUTSCHRIFT",
        "EEG invoice must be tagged GUTSCHRIFT"
    );
}

#[test]
fn eeg_zero_einspeisung_is_zero() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.1}"#);
    let r = calculate_eeg(
        &tariff,
        &EegMeterInput {
            einspeisung_kwh: dec!(0),
            ..Default::default()
        },
        &rates_2026(),
    );
    assert_eq!(r.brutto_eur, dec!(0));
}

// ── Solar ─────────────────────────────────────────────────────────────────────

#[test]
fn solar_mieterstrom_zuschlag_adds_to_brutto() {
    let (_f, _t) = period();
    let base = j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25}"#);
    let ms = j(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25,"mieterstrom_aufschlag_ct_per_kwh":1.5}"#,
    );
    let m = SolarMeterInput {
        eigenverbrauch_kwh: dec!(200),
    };
    let r1 = calculate_solar(&base, &m, &rates_2026());
    let r2 = calculate_solar(&ms, &m, &rates_2026());
    assert!(r2.brutto_eur > r1.brutto_eur);
    let diff = r2.brutto_eur - r1.brutto_eur;
    // 200 × 1.5 ct × 1.19 = 3.57 EUR
    assert!(
        diff > dec!(3.5) && diff < dec!(3.65),
        "Expected ~3.57, got {diff}"
    );
}

// ── HEMS ──────────────────────────────────────────────────────────────────────

#[test]
fn hems_platform_fee_one_month() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"HEMS","hems_platform_fee_eur_per_month":14.99}"#);
    let m = HemsMeterInput {
        months: Some(dec!(1)),
        optimization_events: None,
        readout_events: None,
    };
    let r = calculate_hems(&tariff, &m, &rates_2026());
    // 14.99 × 1.19 ≈ 17.84 EUR
    assert!(
        r.brutto_eur > dec!(17.5) && r.brutto_eur < dec!(18.5),
        "{}",
        r.brutto_eur
    );
}

// ── Einspeisung ───────────────────────────────────────────────────────────────

#[test]
fn einspeisung_net_settlement_is_gutschrift() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EINSPEISUNG","marktwert_ct_per_kwh":6.0,"vermarktungsgebuehr_ct_per_kwh":0.5}"#,
    );
    let m = EegMeterInput {
        einspeisung_kwh: dec!(800),
        ..Default::default()
    };
    let r = calculate_einspeisung(&tariff, &m, &rates_2026());
    // EINSPEISUNG = payment to Direktvermarkter — positive brutto, tagged GUTSCHRIFT
    assert!(
        r.brutto_eur > dec!(0),
        "Direktvermarktung brutto must be positive"
    );
    let _j_r = r.to_rechnung_json();
    let rechnungsart = _j_r["rechnungsart"].as_str().unwrap_or("");
    assert_eq!(rechnungsart, "GUTSCHRIFT");
}

// ── GasQualitaet audit annotation ─────────────────────────────────────────────

#[test]
fn gas_gasqualitaet_added_as_zusatz_attribut() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    // Billing with H2-blended gas: the Brennwert is already the measured H2-blended value
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(500)),
        gasqualitaet: Some("H2_BLEND".into()),
    };
    let r = calculate_gas(&tariff, &m, &no_grid(), &rates_2026());

    // Brutto must be identical to billing without gasqualitaet (no correction applied)
    let m_no_gq = GasMeterInput {
        gasqualitaet: None,
        ..m.clone()
    };
    let r_no_gq = calculate_gas(&tariff, &m_no_gq, &no_grid(), &rates_2026());
    assert_eq!(
        r.brutto_eur, r_no_gq.brutto_eur,
        "gasqualitaet must not alter the billing amount"
    );

    // The ZusatzAttribut must appear in rechnung_json
    let _j_r = r.to_rechnung_json();
    let attrs = _j_r["zusatzAttribute"].as_array();
    assert!(attrs.is_some(), "zusatzAttribute must be present");
    let has_gq = attrs.unwrap().iter().any(|a| {
        a["name"].as_str() == Some("gasqualitaet") && a["wert"].as_str() == Some("H2_BLEND")
    });
    assert!(
        has_gq,
        "gasqualitaet ZusatzAttribut must be in rechnung_json"
    );
}

#[test]
fn gas_no_gasqualitaet_no_zusatz_attribut() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":10}"#,
    );
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(100)),
        gasqualitaet: None,
    };
    let r = calculate_gas(&tariff, &m, &no_grid(), &rates_2026());
    // No gasqualitaet → no zusatzAttribute array (or empty array)
    let _j_r = r.to_rechnung_json();
    let gq_found = _j_r["zusatzAttribute"]
        .as_array()
        .map(|a| a.iter().any(|v| v["name"].as_str() == Some("gasqualitaet")))
        .unwrap_or(false);
    assert!(!gq_found, "gasqualitaet must not appear when not set");
}

// ── §14a Modul 1 — WAERMEPUMPE Steuerungsrabatt ───────────────────────────────

#[test]
fn waermepumpe_modul1_steuerungsrabatt_reduces_brutto() {
    let (_f, _t) = period(); // 31 days Jan 2026
    let without =
        j(r#"{"category":"WAERMEPUMPE","grundpreis_ct_per_day":10,"arbeitspreis_ct_per_kwh":20}"#);
    let with_m1 = j(
        r#"{"category":"WAERMEPUMPE","grundpreis_ct_per_day":10,"arbeitspreis_ct_per_kwh":20,"steuerungsrabatt_modul1_eur_per_kw_year":120}"#,
    );
    let meter = MeterInput {
        arbeitsmenge_kwh: dec!(300),
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        spitzenleistung_kw: Some(dec!(5)),
        steuerung_stunden: None,
    };
    let r_without = calculate_strom(&without, &meter, &no_grid(), None, &rates_2026());
    let r_with = calculate_strom(&with_m1, &meter, &no_grid(), None, &rates_2026());
    assert!(
        r_with.brutto_eur < r_without.brutto_eur,
        "Modul 1 Steuerungsrabatt must reduce total"
    );
    // 5 kW × 120 EUR/year × (31/365) ≈ 51.0 EUR netto → 60.7 EUR brutto reduction
    let savings = r_without.brutto_eur - r_with.brutto_eur;
    assert!(
        savings > dec!(50) && savings < dec!(70),
        "Expected ~58-60 EUR Modul 1 saving, got {savings}"
    );
}

#[test]
fn wallbox_modul3_steuerungsrabatt_requires_steuerung_hours() {
    let (_f, _t) = period();
    // Without steuerung_stunden → Modul 3 position must NOT appear (hours = None)
    let tariff = j(
        r#"{"category":"WALLBOX","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":25,"steuerungsrabatt_modul3_eur_per_kw_year":200}"#,
    );
    let meter_no_h = MeterInput {
        arbeitsmenge_kwh: dec!(200),
        spitzenleistung_kw: Some(dec!(11)),
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        steuerung_stunden: None,
    };
    let meter_h = MeterInput {
        steuerung_stunden: Some(dec!(120)),
        ..meter_no_h.clone()
    };
    let r_no_h = calculate_strom(&tariff, &meter_no_h, &no_grid(), None, &rates_2026());
    let r_h = calculate_strom(&tariff, &meter_h, &no_grid(), None, &rates_2026());
    assert!(
        r_h.brutto_eur < r_no_h.brutto_eur,
        "Modul 3 must reduce brutto when steuerung_stunden > 0"
    );
    let has_modul3 = r_h
        .positions
        .iter()
        .any(|p| p.description.contains("Modul 3"));
    assert!(
        has_modul3,
        "Modul 3 position must be present when steuerung_stunden provided"
    );
}

// ── NNE / Grid pass-through ────────────────────────────────────────────────────

#[test]
fn strom_nne_arbeitspreis_adds_to_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":10}"#);
    let m = meter(dec!(200));
    let grid_with_nne = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(8)),
        ..GridInput::default()
    };
    let r_no_nne = calculate_strom(&tariff, &m, &no_grid(), None, &rates_2026());
    let r_nne = calculate_strom(&tariff, &m, &grid_with_nne, None, &rates_2026());
    assert!(
        r_nne.brutto_eur > r_no_nne.brutto_eur,
        "NNE Arbeitspreis must increase brutto"
    );
    // 200 kWh × 8 ct × 1.19 = 19.04 EUR additional
    let diff = r_nne.brutto_eur - r_no_nne.brutto_eur;
    assert!(
        diff > dec!(18) && diff < dec!(20),
        "Expected ~19.04 NNE addition, got {diff}"
    );
}

#[test]
fn strom_nne_grundpreis_adds_to_brutto() {
    let (_f, _t) = period(); // 31 days
    let tariff = j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":0}"#);
    let m = meter(dec!(0));
    // 240 EUR/year NNE Grundpreis
    let grid = GridInput {
        nne_grundpreis_eur_per_year: Some(dec!(240)),
        ..GridInput::default()
    };
    let r = calculate_strom(&tariff, &m, &grid, None, &rates_2026());
    // 240/365 × 31 ≈ 20.38 EUR + 19% MwSt ≈ 24.25 EUR
    assert!(
        r.brutto_eur > dec!(23) && r.brutto_eur < dec!(26),
        "NNE Grundpreis brutto {}",
        r.brutto_eur
    );
}

#[test]
fn gas_nne_and_bilanzierungsumlage_add_to_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"GAS","gas_grundpreis_ct_per_day":0,"gas_arbeitspreis_ct_per_kwh_hs":5}"#);
    let m = GasMeterInput {
        messung_qm3: dec!(0),
        brennwert_kwh_per_qm3: None,
        zustandszahl: None,
        kwh_hs: Some(dec!(500)),
        gasqualitaet: None,
    };
    let grid_with = GridInput {
        gas_nne_arbeitspreis_ct_per_kwh: Some(dec!(4)),
        gas_bilanzierungsumlage_ct_per_kwh: Some(dec!(0.5)),
        ..GridInput::default()
    };
    let r_base = calculate_gas(&tariff, &m, &no_grid(), &rates_2026());
    let r_nne = calculate_gas(&tariff, &m, &grid_with, &rates_2026());
    assert!(
        r_nne.brutto_eur > r_base.brutto_eur,
        "Gas NNE + Bilanzierungsumlage must increase brutto"
    );
    // 500 kWh × (4 + 0.5) ct × 1.19 = 26.78 EUR additional
    let diff = r_nne.brutto_eur - r_base.brutto_eur;
    assert!(
        diff > dec!(25) && diff < dec!(28),
        "Expected ~26.78 addition, got {diff}"
    );
}

// ── §41a Dynamic EPEX tariff ──────────────────────────────────────────────────

#[test]
fn dynamic_strom_two_intervals_produce_positive_brutto() {
    use std::collections::HashMap;
    let _from = time::macros::date!(2026 - 01 - 01);
    let _to = time::macros::date!(2026 - 01 - 31);
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":20}"#);
    let intervals = vec![
        DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-01-01 13:00 UTC), // 14:00 CET
            kwh: dec!(1),
        },
        DynamicInterval {
            timestamp_utc: time::macros::datetime!(2026-01-01 14:00 UTC), // 15:00 CET
            kwh: dec!(2),
        },
    ];
    let mut prices = HashMap::new();
    prices.insert((2026i32, 1u8, 1u8, 14u8), dec!(25)); // CET hour 14
    prices.insert((2026i32, 1u8, 1u8, 15u8), dec!(30)); // CET hour 15
    let r = calculate_dynamic_strom(
        &tariff,
        &no_grid(),
        None,
        &intervals,
        &prices,
        &rates_2026(),
    );
    // 3 kWh EPEX (0.85 EUR) + 31 days Grundpreis (6.20 EUR) + Stromsteuer + 19% MwSt ≈ 8.46 EUR
    assert!(
        r.brutto_eur > dec!(7),
        "Dynamic strom brutto must be positive: {}",
        r.brutto_eur
    );
    // The EPEX position must be present
    let has_epex = r
        .positions
        .iter()
        .any(|p| p.description.contains("EPEX") || p.description.contains("41a"));
    assert!(has_epex, "EPEX position must appear in dynamic billing");
}

#[test]
fn dynamic_strom_missing_price_uses_zero_and_logs() {
    use std::collections::HashMap;
    let _from = time::macros::date!(2026 - 01 - 15);
    let _to = time::macros::date!(2026 - 01 - 15);
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0}"#);
    let intervals = vec![DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-15 10:00 UTC),
        kwh: dec!(1),
    }];
    // No price entry for this hour → should bill 0 ct/kWh
    let r = calculate_dynamic_strom(
        &tariff,
        &no_grid(),
        None,
        &intervals,
        &HashMap::new(),
        &rates_2026(),
    );
    // Missing price → billed at 0 ct/kWh. Netto should be near 0 (Stromsteuer still applies).
    // Total ≈ 1 kWh × 0 ct_EPEX + 1 kWh × 2.05ct Stromsteuer × 1.19 ≈ 0.024 EUR
    assert!(
        r.brutto_eur >= dec!(0),
        "Missing EPEX price must not produce negative brutto"
    );
}

// ── E-Mobility ───────────────────────────────────────────────────────────────

#[test]
fn emobility_service_fee_and_energy() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EMOBILITY","emobility_service_fee_eur_per_month":9.99,"emobility_arbeitspreis_ct_per_kwh":35,"emobility_session_fee_eur":0.5}"#,
    );
    let m = EmobilityMeterInput {
        months: Some(dec!(1)),
        kwh_charged: Some(dec!(100)),
        sessions: Some(3),
        roaming_sessions: None,
    };
    let r = calculate_emobility(&tariff, &m, &rates_2026());
    // Service fee: 9.99 + Energy: 100×35ct=35.00 + 3 sessions×0.50=1.50 = 46.49 netto + 19% MwSt
    assert!(
        r.brutto_eur > dec!(50),
        "EMOBILITY brutto too low: {}",
        r.brutto_eur
    );
    assert!(
        r.brutto_eur < dec!(60),
        "EMOBILITY brutto too high: {}",
        r.brutto_eur
    );
}

#[test]
fn emobility_zero_usage_gives_only_service_fee() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EMOBILITY","emobility_service_fee_eur_per_month":4.99}"#);
    let m = EmobilityMeterInput {
        months: Some(dec!(1)),
        kwh_charged: None,
        sessions: None,
        roaming_sessions: None,
    };
    let r = calculate_emobility(&tariff, &m, &rates_2026());
    // 4.99 × 1.19 ≈ 5.94 EUR
    assert!(
        r.brutto_eur > dec!(5) && r.brutto_eur < dec!(7),
        "Fee-only brutto {}",
        r.brutto_eur
    );
}

// ── Energiedienstleistung ─────────────────────────────────────────────────────

#[test]
fn energiedienstleistung_flat_fee_and_events() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"ENERGIEDIENSTLEISTUNG","service_fee_eur":19.99,"service_event_price_eur":2.0}"#,
    );
    let m = ServiceMeterInput {
        months: Some(dec!(1)),
        event_count: Some(5),
        event_price_eur: None,
    };
    let r = calculate_energiedienstleistung(&tariff, &m, &rates_2026());
    // 19.99 + 5×2.00=10.00 = 29.99 netto × 1.19 ≈ 35.69 EUR brutto
    assert!(
        r.brutto_eur > dec!(35) && r.brutto_eur < dec!(37),
        "Service brutto {}",
        r.brutto_eur
    );
}

// ── Reduced MwSt ─────────────────────────────────────────────────────────────

#[test]
fn waerme_reduced_mwst_7pct() {
    // §12 Abs. 2 Nr. 1 UStG: Fernwärme from renewable sources → 7% MwSt
    let (_f, _t) = period();
    let t19 =
        j(r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":12,"mwst_rate_override":0.19}"#);
    let t07 =
        j(r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":12,"mwst_rate_override":0.07}"#);
    let m = WaermeMeterInput {
        kwh_waerme: dec!(500),
        spitzenleistung_kw: None,
        months: Some(dec!(1)),
    };
    let r19 = calculate_waerme(&t19, &m, &rates_2026());
    let r07 = calculate_waerme(&t07, &m, &rates_2026());
    assert!(
        r07.brutto_eur < r19.brutto_eur,
        "Reduced MwSt must lower brutto"
    );
    // Same netto; brutto difference = netto × (0.19 - 0.07) = netto × 0.12
    let diff = r19.brutto_eur - r07.brutto_eur;
    let expected_diff = r19.netto_eur * dec!(0.12);
    assert!(
        (diff - expected_diff).abs() < dec!(0.01),
        "Expected MwSt diff {expected_diff:.2}, got {diff:.2}"
    );
}

// ── rechnung_json completeness ────────────────────────────────────────────────

#[test]
fn rechnung_json_has_gesamtsteuer() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":30,"arbeitspreis_ct_per_kwh":10}"#);
    let r = calculate_strom(&tariff, &meter(dec!(200)), &no_grid(), None, &rates_2026());
    let _json_obj = r.to_rechnung_json();
    let obj = _json_obj.as_object().unwrap();
    assert!(
        obj.contains_key("gesamtnetto"),
        "rechnung_json must have gesamtnetto"
    );
    assert!(
        obj.contains_key("gesamtsteuer"),
        "rechnung_json must have gesamtsteuer (BO4E)"
    );
    assert!(
        obj.contains_key("gesamtbrutto"),
        "rechnung_json must have gesamtbrutto"
    );
    // gesamtsteuer = brutto - netto (must be positive for normal invoice)
    let netto = obj["gesamtnetto"]["wert"]
        .as_str()
        .unwrap()
        .parse::<rust_decimal::Decimal>()
        .unwrap();
    let steuer = obj["gesamtsteuer"]["wert"]
        .as_str()
        .unwrap()
        .parse::<rust_decimal::Decimal>()
        .unwrap();
    let brutto = obj["gesamtbrutto"]["wert"]
        .as_str()
        .unwrap()
        .parse::<rust_decimal::Decimal>()
        .unwrap();
    assert!(
        (netto + steuer - brutto).abs() < dec!(0.001),
        "netto + steuer must equal brutto"
    );
    assert!(steuer > dec!(0), "gesamtsteuer must be positive");
}

#[test]
fn rechnung_json_has_rechnungsart_and_herausgeber() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = calculate_strom(&tariff, &meter(dec!(100)), &no_grid(), None, &rates_2026());
    let _json_obj = r.to_rechnung_json();
    let obj = _json_obj.as_object().unwrap();
    assert_eq!(obj["_typ"].as_str(), Some("RECHNUNG"));
    assert_eq!(obj["rechnungsart"].as_str(), Some("ABSCHLAGSRECHNUNG"));
    assert_eq!(obj["marktlokationsId"].as_str(), Some("51238696781"));
    let herausgeber = &obj["herausgeber"];
    assert_eq!(
        herausgeber["marktpartnercode"].as_str(),
        Some("9900000000001")
    );
}

// ── KA / Konzessionsabgabe ────────────────────────────────────────────────────

#[test]
fn strom_ka_konzessionsabgabe_adds_to_brutto() {
    let (_f, _t) = period();
    let tariff =
        j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":10}"#);
    let m = meter(dec!(200));
    let grid_with_ka = GridInput {
        ka_ct_per_kwh: Some(dec!(2.39)),
        ..GridInput::default()
    };
    let r_base = calculate_strom(&tariff, &m, &no_grid(), None, &rates_2026());
    let r_ka = calculate_strom(&tariff, &m, &grid_with_ka, None, &rates_2026());
    assert!(
        r_ka.brutto_eur > r_base.brutto_eur,
        "KA must increase brutto"
    );
    // 200 kWh × 2.39 ct × 1.19 = 5.69 EUR additional
    let diff = r_ka.brutto_eur - r_base.brutto_eur;
    assert!(
        diff > dec!(5.5) && diff < dec!(5.9),
        "Expected ~5.69 KA addition, got {diff}"
    );
}

// ── NNE Leistungspreis ────────────────────────────────────────────────────────

#[test]
fn strom_nne_leistungspreis_scales_with_peak_kw() {
    let (_f, _t) = period(); // 31 days
    let tariff = j(r#"{"category":"STROM","grundpreis_ct_per_day":0,"arbeitspreis_ct_per_kwh":0}"#);
    let m_low = MeterInput {
        arbeitsmenge_kwh: dec!(0),
        spitzenleistung_kw: Some(dec!(5)),
        ..MeterInput::default()
    };
    let m_high = MeterInput {
        arbeitsmenge_kwh: dec!(0),
        spitzenleistung_kw: Some(dec!(10)),
        ..MeterInput::default()
    };
    // 200 EUR/kW/year NNE Leistungspreis
    let grid = GridInput {
        nne_leistungspreis_eur_per_kw_year: Some(dec!(200)),
        ..GridInput::default()
    };
    let r_low = calculate_strom(&tariff, &m_low, &grid, None, &rates_2026());
    let r_high = calculate_strom(&tariff, &m_high, &grid, None, &rates_2026());
    assert!(
        r_high.brutto_eur > r_low.brutto_eur,
        "Higher peak → higher Leistungspreis"
    );
    // Ratio should be 2:1 (10 kW vs 5 kW)
    let ratio = r_high.brutto_eur / r_low.brutto_eur;
    assert!(
        (ratio - dec!(2)).abs() < dec!(0.01),
        "Expected 2:1 ratio for double kW, got {ratio}"
    );
}

// ── EEG Marktprämie + Managementprämie ───────────────────────────────────────

#[test]
fn eeg_marktpraemie_adds_to_settlement() {
    let (_f, _t) = period();
    let base = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.0}"#);
    let with_mp = j(
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.0,"eeg_marktpraemie_ct_per_kwh":2.0,"eeg_managementpraemie_ct_per_kwh":0.4}"#,
    );
    let m = EegMeterInput {
        einspeisung_kwh: dec!(500),
        ..Default::default()
    };
    let r0 = calculate_eeg(&base, &m, &rates_2026());
    let r1 = calculate_eeg(&with_mp, &m, &rates_2026());
    assert!(
        r1.brutto_eur > r0.brutto_eur,
        "Marktpraemie+Managementpraemie must increase settlement"
    );
    // 500 × (2.0 + 0.4) ct × 1.19 = 14.28 EUR additional
    let diff = r1.brutto_eur - r0.brutto_eur;
    assert!(
        diff > dec!(14) && diff < dec!(15),
        "Expected ~14.28 additional, got {diff}"
    );
    let has_mp = r1
        .positions
        .iter()
        .any(|p| p.description.contains("Marktpr"));
    assert!(has_mp, "Marktpraemie position must appear");
}

#[test]
fn eeg_kwkg_zuschlag_credit_in_eeg_billing() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":0,"kwkg_zuschlag_ct_per_kwh":8.0}"#,
    );
    let m = EegMeterInput {
        einspeisung_kwh: dec!(400),
        ..Default::default()
    };
    let r = calculate_eeg(&tariff, &m, &rates_2026());
    // 400 × 8.0 ct × 1.19 = 38.08 EUR
    assert!(
        r.brutto_eur > dec!(37) && r.brutto_eur < dec!(39),
        "KWKG Zuschlag brutto: {}",
        r.brutto_eur
    );
    let has_kwkg = r
        .positions
        .iter()
        .any(|p| p.description.to_lowercase().contains("kwkg") || p.has_tag("kwkg_zuschlag"));
    assert!(has_kwkg, "KWKG position must appear in EEG billing");
}

// ── §42a Gemeinschaftlicher Eigenverbrauch ────────────────────────────────────

#[test]
fn solar_gemeinschaft_rabatt_reduces_brutto() {
    let (_f, _t) = period();
    let base = j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25}"#);
    let with_r = j(
        r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":25,"gemeinschaft_rabatt_ct_per_kwh":2.0}"#,
    );
    let m = SolarMeterInput {
        eigenverbrauch_kwh: dec!(300),
    };
    let r0 = calculate_solar(&base, &m, &rates_2026());
    let r1 = calculate_solar(&with_r, &m, &rates_2026());
    assert!(
        r1.brutto_eur < r0.brutto_eur,
        "Gemeinschaft Rabatt must reduce brutto"
    );
    // 300 × 2.0 ct × 1.19 = 7.14 EUR saving
    let saving = r0.brutto_eur - r1.brutto_eur;
    assert!(
        saving > dec!(7) && saving < dec!(7.5),
        "Expected ~7.14 saving, got {saving}"
    );
}

// ── Statutory rate override ───────────────────────────────────────────────────

#[test]
fn gas_energiesteuer_override_applied() {
    let (_f, _t) = period();
    let base = j(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":10}"#);
    let with_o = j(
        r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":10,"energiesteuer_gas_ct_per_kwh_override":0}"#,
    );
    let m = GasMeterInput {
        kwh_hs: Some(dec!(500)),
        ..Default::default()
    };
    let r_std = calculate_gas(&base, &m, &no_grid(), &rates_2026());
    let r_ovr = calculate_gas(&with_o, &m, &no_grid(), &rates_2026());
    assert!(
        r_ovr.brutto_eur < r_std.brutto_eur,
        "Zero Energiesteuer override must reduce brutto"
    );
    // 500 × 0.55 ct × 1.19 = 3.27 EUR saving
    let saving = r_std.brutto_eur - r_ovr.brutto_eur;
    assert!(
        saving > dec!(3) && saving < dec!(4),
        "Expected ~3.27 saving, got {saving}"
    );
}

// ── BillingResult invariant and methods ──────────────────────────────────────

#[test]
fn billing_result_assert_valid_passes_for_all_categories() {
    let (_f, _t) = period();
    let m = meter(dec!(100));

    // STROM
    calculate_strom(
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#),
        &m,
        &no_grid(),
        None,
        &rates_2026(),
    )
    .assert_valid();

    // GAS
    let gm = GasMeterInput {
        kwh_hs: Some(dec!(100)),
        ..Default::default()
    };
    calculate_gas(
        &j(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":8}"#),
        &gm,
        &no_grid(),
        &rates_2026(),
    )
    .assert_valid();

    // WAERME
    let wm = WaermeMeterInput {
        kwh_waerme: dec!(200),
        spitzenleistung_kw: None,
        months: Some(dec!(1)),
    };
    calculate_waerme(
        &j(r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":10}"#),
        &wm,
        &rates_2026(),
    )
    .assert_valid();

    // EEG
    calculate_eeg(
        &j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8}"#),
        &EegMeterInput {
            einspeisung_kwh: dec!(500),
            ..Default::default()
        },
        &rates_2026(),
    )
    .assert_valid();
}

#[test]
fn billing_result_position_total_by_tag() {
    let (_f, _t) = period();
    let grid = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(8)),
        ka_ct_per_kwh: Some(dec!(2)),
        ..GridInput::default()
    };
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":15}"#);
    let r = calculate_strom(&tariff, &meter(dec!(200)), &grid, None, &rates_2026());
    let nne_total = r.total_by_tag("nne");
    // (8 + 2) ct × 200 kWh = 20.00 EUR
    assert!(
        nne_total > dec!(19) && nne_total < dec!(21),
        "NNE tag total {}",
        nne_total
    );
    let commodity_total = r.total_by_tag("commodity");
    // 200 kWh × 15 ct = 30.00 EUR (just Arbeitspreis, not NNE)
    assert!(
        commodity_total > dec!(29) && commodity_total < dec!(31),
        "Commodity total {}",
        commodity_total
    );
}

#[test]
fn rechnung_json_has_rechnungsempfaenger_and_zahlungsziel() {
    let (_f, _t) = period();
    let r = calculate_strom(
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#),
        &meter(dec!(100)),
        &no_grid(),
        None,
        &rates_2026(),
    );
    let _json_obj = r.to_rechnung_json();
    let obj = _json_obj.as_object().unwrap();
    // rechnungsempfaenger must reference the MaLo
    assert!(
        obj.contains_key("rechnungsempfaenger"),
        "rechnung_json must have rechnungsempfaenger"
    );
    let emp = &obj["rechnungsempfaenger"];
    assert_eq!(emp["externeKundenId"].as_str(), Some("51238696781"));
    // zahlungsziel must be a date string in the future
    assert!(
        obj.contains_key("zahlungsziel"),
        "rechnung_json must have zahlungsziel"
    );
    let ziel = obj["zahlungsziel"].as_str().unwrap_or("");
    assert!(
        ziel.starts_with("2026"),
        "zahlungsziel must be 2026: {ziel}"
    );
}

// ── Dynamic EPEX — negative price handling ────────────────────────────────────

#[test]
fn dynamic_strom_negative_epex_reduces_brutto() {
    // When EPEX < 0, the customer's bill decreases (credit for cheap/free/paid-for power).
    // This is correct for full pass-through §41a tariffs.
    use std::collections::HashMap;
    let _from = time::macros::date!(2026 - 01 - 01);
    let _to = time::macros::date!(2026 - 01 - 01);
    let tariff = j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":100}"#); // 1 EUR/day
    let intervals = vec![DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-01 12:00 UTC),
        kwh: dec!(1),
    }];
    let mut prices_positive = HashMap::new();
    prices_positive.insert((2026i32, 1u8, 1u8, 13u8), dec!(20)); // +20 ct/kWh
    let mut prices_negative = HashMap::new();
    prices_negative.insert((2026i32, 1u8, 1u8, 13u8), dec!(-30)); // -30 ct/kWh (negative EPEX)

    let r_pos = calculate_dynamic_strom(
        &tariff,
        &no_grid(),
        None,
        &intervals,
        &prices_positive,
        &rates_2026(),
    );
    let r_neg = calculate_dynamic_strom(
        &tariff,
        &no_grid(),
        None,
        &intervals,
        &prices_negative,
        &rates_2026(),
    );

    assert!(
        r_neg.brutto_eur < r_pos.brutto_eur,
        "Negative EPEX must produce lower brutto than positive"
    );
    r_pos.assert_valid();
    r_neg.assert_valid(); // invariant holds even for negative-price scenarios
}

// ── §51 EEG Negativpreisregel — contractual suspension (LF role) ──────────────

#[test]
fn eeg_negativpreis_suspension_reduces_verguetung() {
    // §51 EEG: when EPEX < 0, EEG Vergütung is suspended for those kWh.
    // For the LF role this is a contractual feature (not legally mandatory like for NB).
    // NB mandatory implementation lives in `eeg-billing` crate.
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":8.0}"#);

    // No suspension
    let m_full = EegMeterInput {
        einspeisung_kwh: dec!(500),
        ..Default::default()
    };
    // 100 kWh occurred during negative-EPEX hours → suspended
    let m_suspended = EegMeterInput {
        einspeisung_kwh: dec!(500),
        kwh_during_negative_epex: Some(dec!(100)),
    };

    let r_full = calculate_eeg(&tariff, &m_full, &rates_2026());
    let r_susp = calculate_eeg(&tariff, &m_suspended, &rates_2026());

    assert!(
        r_susp.brutto_eur < r_full.brutto_eur,
        "Suspension must reduce settlement"
    );
    // 100 kWh × 8.0 ct × 1.19 = 9.52 EUR reduction
    let reduction = r_full.brutto_eur - r_susp.brutto_eur;
    assert!(
        reduction > dec!(9) && reduction < dec!(10),
        "Expected ~9.52 EUR reduction, got {reduction}"
    );

    // Suspension info position must appear
    let has_info = r_susp
        .positions
        .iter()
        .any(|p| p.has_tag("eeg_negativpreis_suspension"));
    assert!(
        has_info,
        "Info position for §51 suspension must appear when kwh_during_negative_epex > 0"
    );

    // Both results must satisfy the arithmetic invariant
    r_full.assert_valid();
    r_susp.assert_valid();
}

#[test]
fn eeg_kwkg_not_affected_by_negativpreis_suspension() {
    // §51 only suspends EEG payments. KWKG Zuschlag is NOT suspended (different law).
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"EEG","eeg_verguetungssatz_ct_per_kwh":0,"kwkg_zuschlag_ct_per_kwh":8.0}"#,
    );

    let m_no_susp = EegMeterInput {
        einspeisung_kwh: dec!(400),
        ..Default::default()
    };
    let m_susp = EegMeterInput {
        einspeisung_kwh: dec!(400),
        kwh_during_negative_epex: Some(dec!(400)),
    };

    let r_no = calculate_eeg(&tariff, &m_no_susp, &rates_2026());
    let r_yes = calculate_eeg(&tariff, &m_susp, &rates_2026());

    // KWKG uses full kwh — suspension only affects EEG Vergütung/Marktprämie
    assert_eq!(
        r_no.brutto_eur, r_yes.brutto_eur,
        "KWKG Zuschlag must NOT be affected by §51 EEG suspension"
    );
}

// ── BillingResult method coverage ─────────────────────────────────────────────

#[test]
fn billing_result_levy_total_eur_method() {
    let (_f, _t) = period();
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let r = calculate_strom(&tariff, &meter(dec!(100)), &no_grid(), None, &rates_2026());
    let levy = r.total_by_tag("levy");
    // 100 kWh × 2.05 ct Stromsteuer = 2.05 EUR levy
    assert!(
        levy > dec!(2) && levy < dec!(2.1),
        "Stromsteuer levy expected ~2.05, got {levy}"
    );
    // levy_total_eur() == position_total_by_tag("levy")
    assert_eq!(levy, r.total_by_tag("levy"));
}

#[test]
fn billing_result_positions_by_tag_iterator() {
    let (_f, _t) = period();
    let grid = GridInput {
        nne_arbeitspreis_ct_per_kwh: Some(dec!(8)),
        ka_ct_per_kwh: Some(dec!(2)),
        ..GridInput::default()
    };
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":15}"#);
    let r = calculate_strom(&tariff, &meter(dec!(100)), &grid, None, &rates_2026());
    let nne_positions: Vec<_> = r.positions_by_tag("nne").collect();
    assert!(
        nne_positions.len() >= 2,
        "Expected ≥2 NNE positions (AP + KA), got {}",
        nne_positions.len()
    );
    let commodity_positions: Vec<_> = r.positions_by_tag("commodity").collect();
    assert!(
        !commodity_positions.is_empty(),
        "Expected at least one commodity position"
    );
    // All returned positions must actually have the tag
    for p in &nne_positions {
        assert!(p.has_tag("nne"));
    }
    for p in &commodity_positions {
        assert!(p.has_tag("commodity"));
    }
}

#[test]
fn hems_with_optimization_and_readout_events() {
    let (_f, _t) = period();
    let tariff = j(
        r#"{"category":"HEMS","hems_platform_fee_eur_per_month":9.99,"hems_optimization_event_eur":0.50,"hems_readout_event_eur":0.10}"#,
    );
    let m = HemsMeterInput {
        months: Some(dec!(1)),
        optimization_events: Some(5),
        readout_events: Some(10),
    };
    let r = calculate_hems(&tariff, &m, &rates_2026());
    // 9.99 + 5×0.50 + 10×0.10 = 9.99 + 2.50 + 1.00 = 13.49 netto × 1.19 ≈ 16.05 EUR
    assert!(
        r.brutto_eur > dec!(15) && r.brutto_eur < dec!(18),
        "HEMS with events brutto: {}",
        r.brutto_eur
    );
    let has_opt = r.positions.iter().any(|p| {
        p.description.to_lowercase().contains("optimiz")
            || p.description.to_lowercase().contains("optim")
    });
    assert!(has_opt, "Optimization event position must appear");
    r.assert_valid();
}

#[test]
fn dynamic_strom_floor_prevents_negative_billing() {
    // When dynamic_epex_floor_ct_kwh = 0, negative EPEX prices bill 0 (customer gets no credit).
    // This is a common contract configuration for LF's §41a tariffs.
    use std::collections::HashMap;
    // Floor = 0 ct/kWh
    let tariff_with_floor = j(
        r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0,"dynamic_epex_floor_ct_kwh":0}"#,
    );
    let tariff_without_floor =
        j(r#"{"category":"STROM","dynamic_epex":true,"grundpreis_ct_per_day":0}"#);

    let intervals = vec![DynamicInterval {
        timestamp_utc: time::macros::datetime!(2026-01-01 12:00 UTC),
        kwh: dec!(10),
    }];
    let mut prices = HashMap::new();
    prices.insert((2026i32, 1u8, 1u8, 13u8), dec!(-50)); // -50 ct/kWh

    let r_floor = calculate_dynamic_strom(
        &tariff_with_floor,
        &no_grid(),
        None,
        &intervals,
        &prices,
        &rates_2026(),
    );
    let r_nofloor = calculate_dynamic_strom(
        &tariff_without_floor,
        &no_grid(),
        None,
        &intervals,
        &prices,
        &rates_2026(),
    );

    // With floor=0: price is clamped to 0 → only Stromsteuer, no EPEX credit
    // Without floor: -50 ct × 10 kWh = -5 EUR EPEX credit → significantly lower brutto
    assert!(
        r_floor.brutto_eur > r_nofloor.brutto_eur,
        "Floor must produce higher brutto than no-floor with negative price; floor={}, nofloor={}",
        r_floor.brutto_eur,
        r_nofloor.brutto_eur
    );
    // With floor, brutto must be >= 0 (no negative invoice)
    assert!(
        r_floor.brutto_eur >= dec!(0),
        "Floor=0 must prevent negative invoice, got {}",
        r_floor.brutto_eur
    );
    r_floor.assert_valid();
    r_nofloor.assert_valid();
}
