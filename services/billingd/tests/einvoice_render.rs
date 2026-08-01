//! End-to-end e-invoice rendering: a mixed-rate `energy_billing::Invoice` maps to
//! the EN 16931 model and renders to conformant XRechnung/CII and PEPPOL UBL, with
//! the per-line VAT reaching the wire — the defect the hand-rolled renderer had.

use billingd::config::BillingdConfig;
use billingd::einvoice;
use energy_billing::{
    BillingContext, BillingEngine, BillingPeriod, ElectricityProvider, GridInput, HeatProvider,
    InvoiceType, MeterInput, MwStProvider, Product, Quantities, RegulatoryRates, WaermeMeterInput,
};
use rust_decimal::dec;
use time::macros::date;

fn mixed_rate_invoice() -> energy_billing::Invoice {
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
        rechnungsnummer: "R-XR-9001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: RegulatoryRates::default(),
        ..Default::default()
    };
    BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &elec,
            GridInput::default(),
        ))
        .add(HeatProvider::from_product(&heat))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap()
}

fn cfg() -> BillingdConfig {
    serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://localhost/x" },
        "tenant": "9900000000001",
        "tarifbd_url": "http://tarifbd",
        "edmd_url": "http://edmd",
        "marktd_url": "http://marktd",
        "seller_name": "Stadtwerke Musterstadt GmbH",
        "seller_vat_id": "DE123456789",
        "seller_address": "Musterstraße 1, 12345 Musterstadt",
        "seller_contact": "Tel. 0800 1234567, service@stadtwerke-musterstadt.de",
        "seller_iban": "DE89370400440532013000",
        "seller_bic": "COBADEFFXXX",
    }))
    .expect("minimal billingd config")
}

#[test]
fn mixed_rate_invoice_renders_conformant_cii_and_ubl() {
    let invoice = mixed_rate_invoice();
    let model = einvoice::build(&invoice, &cfg(), "51238696781");

    let cii = einvoice::render_cii(&model);
    assert!(cii.contains("R-XR-9001"), "invoice number in CII");
    // Both VAT rates reach the wire — a single blended rate is exactly what the
    // hand-rolled renderer produced and this proves is gone.
    assert!(
        cii.contains("19") && cii.contains('7'),
        "per-rate VAT in CII"
    );

    let ubl = einvoice::render_ubl(&model);
    assert!(ubl.contains("R-XR-9001"), "invoice number in UBL");
    assert!(!ubl.is_empty());

    // The enriched seller party (split address + contact) reaches the wire.
    assert!(
        cii.contains("12345"),
        "seller post code (split address) in CII"
    );
    assert!(
        cii.contains("service@stadtwerke-musterstadt.de"),
        "seller contact e-mail in CII"
    );

    // Strict XRechnung fails on the placeholder buyer, and `buyer_gaps` names the
    // exact BG-7 terms the customer master must supply.
    let before = einvoice::render_xrechnung_cii(&model);
    assert!(
        before.is_err(),
        "placeholder buyer is not XRechnung-complete"
    );
    let gaps = einvoice::buyer_gaps(&model);
    assert!(!gaps.is_empty() && gaps.iter().all(|g| g.contains("BG-7")));

    // Complete the buyer (as a B2G submission does) → the document is now
    // XRechnung 3.0-valid and renders through `to_string_for`.
    let recipient = einvoice::B2gBuyer {
        name: "Bundesamt für Musterverwaltung".to_owned(),
        line1: Some("Behördenstraße 2".to_owned()),
        post_code: Some("53113".to_owned()),
        city: Some("Bonn".to_owned()),
        country: Some("DE".to_owned()),
        contact_name: Some("Rechnungseingang".to_owned()),
        phone: Some("+49 228 000".to_owned()),
        email: Some("re@bund.example".to_owned()),
        vat_id: None,
        electronic_address: Some("991-33333TEST-33".to_owned()),
    };
    let b2g = einvoice::with_buyer_reference(
        einvoice::apply_b2g_buyer(model, &recipient),
        "991-33333TEST-33",
    );
    let xml = einvoice::render_xrechnung_cii(&b2g).expect("XRechnung-valid after buyer completion");
    assert!(xml.contains("Bundesamt für Musterverwaltung"));
    assert!(
        einvoice::buyer_gaps(&b2g).is_empty(),
        "no buyer gaps remain"
    );
}
