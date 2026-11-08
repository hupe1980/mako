//! A period straddling a USt or BEHG Stichtag is refused by the pure crate.
//!
//! `mwst_rate_for_period`, `mwst_rate_for_gas_waerme_period` and the § 10 BEHG
//! year table answer a straddling period with no single rate, because none is
//! correct for it. A provider that only warns lets the engine bill the whole
//! period at one rate — part of it wrong, on an invoice that reads like a
//! correct one. Every caller of `energy-billing` is refused, not only the
//! `billingd` paths that split such periods before they get here.

use energy_billing::{
    BillingContext, BillingPeriod, EngineError, GasMeterInput, GridInput, MeterInput, Product,
    Quantities, RegulatoryRates, WaermeMeterInput, WarningSeverity,
};
use rust_decimal::dec;
use time::macros::date;

fn ctx(from: time::Date, to: time::Date, rates: &RegulatoryRates) -> BillingContext {
    BillingContext {
        malo_id: "51238696012".into(),
        lf_mp_id: "9900000000001".into(),
        rechnungsnummer: "STICHTAG-1".into(),
        period: BillingPeriod::new(from, to).unwrap(),
        regulatory_rates: rates.clone(),
        ..Default::default()
    }
}

/// Assert that `code` is raised as an `Error` naming `stichtag`, and that
/// `bill` refuses the period with it.
fn assert_refused(
    product: &str,
    q: &Quantities,
    from: time::Date,
    to: time::Date,
    code: &str,
    stichtag: &str,
) {
    let rates = RegulatoryRates::for_year(from.year());
    let product: Product = serde_json::from_str(product).unwrap();
    let engine = product.build_engine(&GridInput::default(), &rates);

    let warnings = engine.validate(&ctx(from, to, &rates), q);
    let finding = warnings
        .iter()
        .find(|w| w.code == code)
        .unwrap_or_else(|| panic!("{code} must be raised for {from}..{to}: {warnings:?}"));
    assert_eq!(
        finding.severity,
        WarningSeverity::Error,
        "{code} as a Warning bills the whole period at one rate"
    );
    assert!(
        finding.message.contains(stichtag),
        "{code} must name the Stichtag {stichtag}: {}",
        finding.message
    );

    match engine.bill(ctx(from, to, &rates), q) {
        Err(EngineError::ValidationBlocked { warnings }) => {
            assert!(warnings.iter().any(|w| w.code == code), "{warnings:?}");
        }
        Err(other) => panic!("expected ValidationBlocked with {code}, got {other}"),
        Ok(inv) => panic!(
            "a period straddling {stichtag} was billed: {} EUR brutto",
            inv.brutto_eur
        ),
    }
}

fn strom() -> Quantities {
    Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn gas() -> Quantities {
    Quantities {
        gas: Some(GasMeterInput {
            kwh_hs: Some(dec!(5000)),
            zustandszahl: Some(dec!(0.95)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Strom carried 16 % from 01.07.2020 to 31.12.2020 (§ 28 Abs. 1 UStG a. F.).
#[test]
fn a_strom_period_across_the_2020_vat_window_is_refused() {
    assert_refused(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30"}"#,
        &strom(),
        date!(2020 - 06 - 01),
        date!(2020 - 07 - 31),
        "MWST_STICHTAG_IM_ZEITRAUM",
        "2020-07-01",
    );
}

/// Gas carried 7 % until 31.03.2024 (§ 28 Abs. 5 UStG).
#[test]
fn a_gas_period_across_the_end_of_the_7_percent_window_is_refused() {
    assert_refused(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":"0","gas_arbeitspreis_ct_per_kwh_hs":"10"}"#,
        &gas(),
        date!(2024 - 03 - 01),
        date!(2024 - 04 - 30),
        "MWST_STICHTAG_IM_ZEITRAUM",
        "2024-04-01",
    );
}

/// Fernwärme carried 7 % until 31.03.2024 (§ 28 Abs. 6 UStG).
#[test]
fn a_waerme_period_across_the_end_of_the_7_percent_window_is_refused() {
    let q = Quantities {
        heat: Some(WaermeMeterInput {
            kwh_waerme: dec!(3000),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_refused(
        r#"{"category":"WAERME","waerme_grundpreis_eur_per_month":"0","waerme_arbeitspreis_ct_per_kwh":"12"}"#,
        &q,
        date!(2024 - 03 - 01),
        date!(2024 - 04 - 30),
        "MWST_STICHTAG_IM_ZEITRAUM",
        "2024-04-01",
    );
}

/// The § 10 BEHG price stepped from 30 to 45 EUR/t at 01.01.2024; the period
/// lies inside the 7 % window, so the year boundary is the only Stichtag.
#[test]
fn a_gas_period_across_a_behg_year_boundary_is_refused() {
    assert_refused(
        r#"{"category":"GAS","gas_grundpreis_ct_per_day":"0","gas_arbeitspreis_ct_per_kwh_hs":"10"}"#,
        &gas(),
        date!(2023 - 12 - 01),
        date!(2024 - 01 - 31),
        "BEHG_JAHRESGRENZE_IM_ZEITRAUM",
        "2024-01-01",
    );
}
