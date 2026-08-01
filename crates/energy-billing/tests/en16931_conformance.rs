//! `Invoice::to_en16931` produces a semantic model that passes the real EN 16931
//! business rules — including per-line VAT (BT-151/152) that reconciles with the
//! BG-23 breakdown on a **mixed-rate** invoice (the defect the hand-rolled
//! renderer had).

#![cfg(feature = "en16931")]

use en16931::identifier::Identifier;
use en16931::invoice::{Code, Party, PostalAddress};
use en16931::validation::validate;
use energy_billing::{
    BillingContext, BillingEngine, BillingPeriod, ElectricityProvider, GridInput, HeatProvider,
    InvoiceType, MeterInput, MwStProvider, Product, Quantities, RegulatoryRates, WaermeMeterInput,
    en16931_map::XRECHNUNG_SPEC_ID,
};
use rust_decimal::dec;
use time::macros::date;

fn party(name: &str, eas: &str) -> Party {
    Party {
        name: Some(name.to_owned()),
        vat_identifier: Some("DE123456789".to_owned()),
        electronic_address: Some(Identifier::schemed(eas, "0204")),
        address: PostalAddress {
            line1: Some("Musterstraße 1".to_owned()),
            city: Some("Berlin".to_owned()),
            post_code: Some("10115".to_owned()),
            country: Some(Code::from("DE")),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A single engine billing electricity (19 %) and Fernwärme (7 % via override)
/// maps to an EN 16931 invoice that is valid, with a distinct BT-152 per line and
/// a two-entry BG-23 breakdown that reconciles.
#[test]
fn mixed_rate_invoice_maps_to_conformant_en16931() {
    let elec: Product =
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"#).unwrap();
    let heat: Product = serde_json::from_str(
        r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":10.0,"mwst_rate_override":0.07}"#,
    )
    .unwrap();

    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100),
            ..Default::default()
        }),
        heat: Some(WaermeMeterInput {
            kwh_waerme: dec!(200),
            months: Some(dec!(1)),
            ..Default::default()
        }),
        ..Default::default()
    };

    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-EN16931-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: RegulatoryRates::default(),
        ..Default::default()
    };

    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &elec,
            GridInput::default(),
        ))
        .add(HeatProvider::from_product(&heat))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap();

    let en = invoice.to_en16931(
        XRECHNUNG_SPEC_ID,
        party("Stadtwerke Musterstadt GmbH", "9900000000001"),
        party("Kunde", "51238696781"),
    );

    // The real EN 16931 rule engine — arithmetic (BR-CO-*), VAT categories
    // (BR-S-*, BR-Z-*) and totals must all hold.
    let report = validate(&en);
    assert!(
        report.is_valid(),
        "en16931 findings: {:?}",
        report.fatal().map(|f| &f.rule).collect::<Vec<_>>()
    );

    // Per-line VAT: at least one 19 % line and one 7 % line (the fix — a single
    // blended rate is exactly what this proves is gone).
    let rates: Vec<_> = en
        .lines
        .iter()
        .filter_map(|l| l.vat.rate.map(|r| r.as_fraction()))
        .collect();
    assert!(rates.contains(&dec!(0.19)), "a 19 % line: {rates:?}");
    assert!(rates.contains(&dec!(0.07)), "a 7 % line: {rates:?}");

    // BG-23: two buckets (19 % and 7 %), taxable amounts summing to the line total.
    assert_eq!(en.vat_breakdown.len(), 2, "one BG-23 entry per rate");
}
