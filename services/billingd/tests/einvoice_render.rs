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
    let model = einvoice::build(&invoice, &cfg(), "51238696781", None);

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

/// What `einvoice::build` produces, checked against the profile it declares.
///
/// `energy-billing`'s own conformance test validates a **hand-crafted** pair of
/// parties — full name, VAT ID, postal address. Production assembles its parties
/// in `einvoice::seller_party`/`buyer_party` from config and a MaLo-ID, and
/// nothing validated *those*. The B2G path is proven before writing
/// (`render_xrechnung_cii`); the ordinary retail path stamped XRechnung into
/// BT-24 and never checked it.
///
/// The document declares XRechnung in BT-24, so XRechnung is what it is held to.
/// The remaining findings are pinned by rule id rather than asserted away — they
/// are all one missing input (buyer master data), and pinning them means a
/// *new* violation fails this test instead of joining a growing pile.
#[test]
fn the_model_production_builds_is_checked_against_the_profile_it_declares() {
    let invoice = mixed_rate_invoice();
    let model = einvoice::build(&invoice, &cfg(), "51238696781", None);

    let report = einvoice::validate(&model);
    let mut fatal: Vec<String> = report.fatal().map(|f| f.rule.clone()).collect();
    fatal.sort();
    fatal.dedup();

    // A retail invoice declares plain EN 16931 and satisfies it. It used to
    // declare XRechnung — a B2G CIUS needing a Leitweg-ID and a Peppol endpoint
    // that a household does not have — and failed four of its rules.
    assert!(
        fatal.is_empty(),
        "a retail invoice must satisfy the profile it declares: {fatal:?}",
    );
}

/// Retail declares core EN 16931; only a B2G document may claim XRechnung.
///
/// XRechnung is the German B2G CIUS. §14 UStG requires conformance to EN 16931,
/// not to XRechnung, so core is both sufficient and truthful for retail. The
/// upgrade happens exactly where the missing terms arrive.
///
/// When BT-24 *does* name XRechnung it must name a published one: the namespace
/// moved from XÖV to XStandards Einkauf at 3.0, and a `xoev-de` URN with a
/// `_3.0` version — which shipped until 2026-08 — matches no version and fails
/// BR-DE-21.
#[test]
fn only_a_b2g_document_declares_xrechnung() {
    let retail = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", None);
    assert_eq!(
        retail.specification_id.as_deref(),
        Some("urn:cen.eu:en16931:2017"),
        "a retail invoice declares plain EN 16931",
    );

    let b2g = einvoice::with_buyer_reference(retail, "991-33333TEST-33");
    assert_eq!(
        b2g.specification_id.as_deref(),
        Some("urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0"),
        "supplying BT-10 is what lets the document claim the CIUS",
    );
    assert!(
        !einvoice::validate(&b2g)
            .fatal()
            .any(|f| f.rule == "BR-DE-21"),
        "BT-24 must be one of the identifiers XRechnung 3.0 publishes",
    );
}

/// A MaLo-ID must not be dressed up as a GS1 GLN.
///
/// `Identifier::eas` validates the *scheme code*, never the value, so
/// `eas(malo, "0088")` passes every rule while asserting that an 11-digit BDEW
/// Marktlokations-ID is a 13-digit GS1 Global Location Number. No rule can catch
/// that; only this test can.
#[test]
fn the_buyer_electronic_address_does_not_fabricate_a_gln() {
    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", None);
    assert!(
        model.buyer.electronic_address.is_none(),
        "a MaLo-ID has no EAS scheme — omit BT-49 rather than claim GLN",
    );
}

/// With the buyer vertragd supplies, the address findings are gone.
///
/// `billingd` holds no customer master; `vertragd.kunden` does. Feeding that
/// through closes BR-DE-8 (city) and BR-DE-9 (post code) — the two findings that
/// were purely a missing input. What remains is genuinely absent for a retail
/// customer: a household has no Leitweg-ID (BT-10) and no Peppol endpoint
/// (BT-49), so those stay open rather than being fabricated.
#[test]
fn a_buyer_from_vertragd_closes_the_address_findings() {
    let buyer = billingd::clients::Rechnungsempfaenger {
        name: Some("Erika Mustermann".to_owned()),
        line1: Some("Beispielweg 7".to_owned()),
        post_code: Some("10115".to_owned()),
        city: Some("Berlin".to_owned()),
        country: Some("DE".to_owned()),
        vat_id: None,
    };
    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", Some(&buyer));

    assert_eq!(model.buyer.name.as_deref(), Some("Erika Mustermann"));
    assert_eq!(model.buyer.address.city.as_deref(), Some("Berlin"));

    assert!(
        einvoice::validate(&model).is_valid(),
        "a retail invoice with buyer master data is EN 16931-valid",
    );
}
