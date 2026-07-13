//! Smoke-test that the billingd re-export shim works end-to-end.
//!
//! The full 28-test suite lives in `crates/energy-billing/tests/calculator_tests.rs`.
//! Here we just verify the public API is reachable through `billingd::calculator`.

use billingd::calculator::{
    GasMeterInput, GridInput, MeterInput, RegulatoryRates, TariffInput, calculate_gas,
    calculate_strom,
};
use rust_decimal_macros::dec;
use time::macros::date;

fn rates() -> RegulatoryRates {
    RegulatoryRates::default()
}
fn j(s: &str) -> TariffInput {
    serde_json::from_str(s).unwrap()
}
fn m(kwh: rust_decimal::Decimal) -> MeterInput {
    MeterInput {
        arbeitsmenge_kwh: kwh,
        arbeitsmenge_ht_kwh: None,
        arbeitsmenge_nt_kwh: None,
        spitzenleistung_kw: None,
        steuerung_stunden: None,
    }
}

#[test]
fn strom_shim_reexport() {
    let f = date!(2026 - 01 - 01);
    let t = date!(2026 - 01 - 31);
    let r = calculate_strom(
        "m",
        "l",
        "R",
        f,
        t,
        &j(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":10}"#),
        &m(dec!(100)),
        &GridInput::default(),
        None,
        &rates(),
    )
    .unwrap();
    assert!(r.brutto_eur > dec!(0));
}

#[test]
fn gas_shim_reexport() {
    let f = date!(2026 - 01 - 01);
    let t = date!(2026 - 01 - 31);
    let r = calculate_gas(
        "m",
        "l",
        "G",
        f,
        t,
        &j(r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":5}"#),
        &GasMeterInput {
            kwh_hs: Some(dec!(100)),
            ..Default::default()
        },
        &GridInput::default(),
        &rates(),
    )
    .unwrap();
    assert!(r.brutto_eur > dec!(0));
}
