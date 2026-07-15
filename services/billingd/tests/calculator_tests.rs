//! Smoke-test that the billingd re-export shim works end-to-end with the new BillingEngine API.
//!
//! The full 44-test suite lives in `crates/energy-billing/tests/calculator_tests.rs`.
//! Here we verify the public API is reachable through `billingd::calculator` using
//! the `BillingEngine` + `BillingContext` + `Quantities` pattern.

use billingd::calculator::{
    BillingContext, BillingEngine, ElectricityProvider, GasMeterInput, GasProvider, GridInput,
    InvoiceType, MeterInput, MwStProvider, Quantities, RegulatoryRates, SolarMeterInput,
    SolarProvider, TariffInput,
};
use rust_decimal_macros::dec;
use time::macros::date;

fn rates() -> RegulatoryRates {
    RegulatoryRates::default()
}
fn j(s: &str) -> TariffInput {
    serde_json::from_str(s).unwrap()
}
fn ctx(malo: &str) -> BillingContext {
    BillingContext {
        malo_id: malo.to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "TEST-001".to_owned(),
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates(),
        contract_id: None,
        ..Default::default()
    }
}

#[test]
fn strom_shim_reexport() {
    let tariff = j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#);
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_tariff(
            &tariff,
            &GridInput::default(),
        ))
        .add(MwStProvider::new(rates().mwst_rate))
        .bill(ctx("51238696780"), &quantities)
        .unwrap();
    assert!(invoice.brutto_eur > dec!(0));
    assert!(invoice.netto_eur > dec!(0));
    assert!(invoice.positions.len() >= 2); // Arbeitspreis + Stromsteuer at minimum
}

#[test]
fn gas_shim_reexport() {
    let tariff = j(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":5}"#);
    let quantities = Quantities {
        gas: Some(GasMeterInput {
            kwh_hs: Some(dec!(100)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let invoice = BillingEngine::new()
        .add(GasProvider::from_tariff(&tariff, &GridInput::default()))
        .add(MwStProvider::new(rates().mwst_rate))
        .bill(ctx("51238696781"), &quantities)
        .unwrap();
    assert!(invoice.brutto_eur > dec!(0));
}

#[test]
fn solar_shim_reexport() {
    let tariff = j(r#"{"category":"SOLAR","solar_arbeitspreis_ct_per_kwh":8.51}"#);
    let quantities = Quantities {
        solar: Some(SolarMeterInput {
            eigenverbrauch_kwh: dec!(50),
        }),
        ..Default::default()
    };
    let invoice = BillingEngine::new()
        .add(SolarProvider::from_tariff(&tariff))
        .add(MwStProvider::new(dec!(0))) // §9a StromStG: no VAT on Eigenverbrauch
        .bill(ctx("51238696782"), &quantities)
        .unwrap();
    assert!(invoice.netto_eur > dec!(0));
}
