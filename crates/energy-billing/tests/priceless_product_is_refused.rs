//! A commodity product must be able to price its commodity.
//!
//! The price fields of a `Product` are populated by mapping `tarifbd`'s
//! `preistyp` strings onto struct fields. A renamed position, a typo in the
//! mapper, or a catalog row saved without its price maps to `None` — in
//! silence. Before this guard the resulting invoice was not an error: it billed
//! 1000 kWh of electricity for €20.50, which is the Stromsteuer and nothing
//! else, and looked entirely ordinary on paper.

use energy_billing::{
    BillingContext, BillingPeriod, EngineError, GridInput, MeterInput, Product, Quantities,
    RegulatoryRates, WarningSeverity,
};
use rust_decimal::dec;
use time::macros::date;

fn ctx(rates: &RegulatoryRates) -> BillingContext {
    BillingContext {
        malo_id: "51238696781".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "GUARD-1".into(),
        period: BillingPeriod::new(date!(2026 - 06 - 01), date!(2026 - 06 - 30)).unwrap(),
        regulatory_rates: rates.clone(),
        ..Default::default()
    }
}

/// The defect, exactly as it reached production shape: a STROM product whose
/// price fields are all absent.
#[test]
fn electricity_without_any_work_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(r#"{"category":"STROM"}"#).unwrap();
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            ..Default::default()
        }),
        ..Default::default()
    };
    let engine = product.build_engine(&GridInput::default(), &rates);

    let warnings = engine.validate(&ctx(&rates), &q);
    let finding = warnings
        .iter()
        .find(|w| w.code == "KEIN_ARBEITSPREIS")
        .expect("a product that cannot price its commodity must be a finding");
    assert_eq!(
        finding.severity,
        WarningSeverity::Error,
        "Warning severity would let the invoice out; only Error blocks it"
    );

    match engine.bill(ctx(&rates), &q) {
        Err(EngineError::ValidationBlocked { warnings }) => {
            assert!(warnings.iter().any(|w| w.code == "KEIN_ARBEITSPREIS"));
        }
        Err(other) => panic!("expected ValidationBlocked, got {other}"),
        Ok(inv) => panic!(
            "a priceless product billed {} EUR instead of being refused — this is the \
             Stromsteuer-only invoice the guard exists to prevent",
            inv.netto_eur
        ),
    }
}

/// Every way of pricing electricity satisfies the guard — it asks whether the
/// product can price the commodity at all, not whether it uses one nominated
/// field. A tariff priced only by HT/NT, or only dynamically, is legitimate.
#[test]
fn any_form_of_work_price_satisfies_the_guard() {
    let rates = RegulatoryRates::default();
    for json in [
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#,
        r#"{"category":"STROM","arbeitspreis_ht_ct_per_kwh":32.0,"arbeitspreis_nt_ct_per_kwh":24.0}"#,
        r#"{"category":"STROM","dynamic_epex":true}"#,
    ] {
        let product: Product = serde_json::from_str(json).unwrap();
        let engine = product.build_engine(&GridInput::default(), &rates);
        let warnings = engine.validate(&ctx(&rates), &Quantities::default());
        assert!(
            !warnings.iter().any(|w| w.code == "KEIN_ARBEITSPREIS"),
            "{json} prices its commodity and must not be flagged: {warnings:?}"
        );
    }
}

/// An operator who genuinely charges nothing per kWh says so with a zero. The
/// guard distinguishes "priced at zero" from "no price on file", which is the
/// whole point — one is a decision, the other is missing data.
#[test]
fn an_explicit_zero_price_is_a_decision_and_is_allowed() {
    let rates = RegulatoryRates::default();
    let product: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":0.0}"#).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);
    let warnings = engine.validate(&ctx(&rates), &Quantities::default());
    assert!(!warnings.iter().any(|w| w.code == "KEIN_ARBEITSPREIS"));
}

/// Gas has the same failure mode: without a work price the invoice charges the
/// Energiesteuer and the BEHG levy and nothing for the gas.
#[test]
fn gas_without_a_work_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(r#"{"category":"GAS"}"#).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);
    let warnings = engine.validate(&ctx(&rates), &Quantities::default());
    assert!(
        warnings
            .iter()
            .any(|w| w.code == "KEIN_ARBEITSPREIS" && w.severity == WarningSeverity::Error),
        "{warnings:?}"
    );
}

/// And Fernwärme.
#[test]
fn heat_without_a_work_price_is_refused() {
    let rates = RegulatoryRates::default();
    let product: Product = serde_json::from_str(r#"{"category":"WAERME"}"#).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);
    let warnings = engine.validate(&ctx(&rates), &Quantities::default());
    assert!(
        warnings
            .iter()
            .any(|w| w.code == "KEIN_ARBEITSPREIS" && w.severity == WarningSeverity::Error),
        "{warnings:?}"
    );
}
