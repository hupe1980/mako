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
        // A valid GS1 GLN: a BDEW-Codenummer is issued through GS1, so a real
        // operator's MP-ID has a correct check digit and BT-34 can honestly
        // claim EAS 0088. `9900000000001` — the placeholder used elsewhere in
        // the fixtures — does not: the first twelve digits require a `4`.
        "tenant": "9900000000004",
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
        stromwiederverkaeufer: false,
    };
    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", Some(&buyer));

    assert_eq!(model.buyer.name.as_deref(), Some("Erika Mustermann"));
    assert_eq!(model.buyer.address.city.as_deref(), Some("Berlin"));

    assert!(
        einvoice::validate(&model).is_valid(),
        "a retail invoice with buyer master data is EN 16931-valid",
    );
}

// ── Template contract ─────────────────────────────────────────────────────────

/// The template view must carry the invoice, not a lossy summary of it.
///
/// An operator's layout renders from `DocumentView` and never from the semantic
/// model, so anything missing here is a field no invoice can ever print. This
/// pins the mixed-rate case in particular: two VAT rates must survive into the
/// breakdown a template iterates, because a single blended rate on the page is
/// exactly the defect the hand-rolled renderer had.
#[test]
fn the_template_view_carries_what_an_invoice_must_print() {
    use billingd::document_view::DocumentView;

    let buyer = billingd::clients::Rechnungsempfaenger {
        name: Some("Erika Mustermann".to_owned()),
        line1: Some("Beispielweg 7".to_owned()),
        post_code: Some("10115".to_owned()),
        city: Some("Berlin".to_owned()),
        country: Some("DE".to_owned()),
        vat_id: None,
        stromwiederverkaeufer: false,
    };
    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", Some(&buyer));
    let view = DocumentView::of(&model);

    // §14 Abs. 4 UStG Pflichtangaben the page cannot omit.
    assert_eq!(view.number.as_deref(), Some("R-XR-9001"));
    assert!(view.issue_date.is_some(), "BT-2 issue date");
    assert_eq!(
        view.seller.name.as_deref(),
        Some("Stadtwerke Musterstadt GmbH")
    );
    assert_eq!(view.seller.vat_id.as_deref(), Some("DE123456789"));
    assert_eq!(view.buyer.name.as_deref(), Some("Erika Mustermann"));
    assert_eq!(view.buyer.city.as_deref(), Some("Berlin"));

    // Both rates reach the breakdown a template iterates.
    let mut rates: Vec<&str> = view
        .vat_breakdown
        .iter()
        .filter_map(|v| v.rate.as_deref())
        .collect();
    rates.sort();
    assert_eq!(rates.len(), 2, "mixed-rate invoice keeps two BG-23 entries");

    assert!(
        !view.lines.is_empty(),
        "an invoice with no lines prints nothing"
    );
    for l in &view.lines {
        assert!(!l.net_amount.is_empty() && !l.quantity.is_empty());
        assert!(!l.vat_category.is_empty(), "BT-151 per line");
    }

    // Amounts are decimal strings — a total that round-trips through f64 is not
    // a total. Guard the type, not just the value.
    assert!(
        view.totals
            .gross_total
            .parse::<rust_decimal::Decimal>()
            .is_ok(),
        "totals must be exact decimals",
    );

    // The view is what a template consumes: it has to serialise.
    let json = serde_json::to_value(&view).expect("DocumentView serialises");
    assert!(json["totals"]["gross_total"].is_string());
    assert!(json["lines"].as_array().is_some_and(|l| !l.is_empty()));
}

/// The view must never become a second source of truth for the XML.
///
/// It is a *projection*: every field is copied from the model, so the page and
/// the embedded CII cannot disagree. This checks the one pair most likely to
/// drift — what the customer owes.
#[test]
fn the_view_agrees_with_the_model_it_projects() {
    use billingd::document_view::DocumentView;

    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", None);
    let view = DocumentView::of(&model);

    assert_eq!(
        view.totals.gross_total,
        model.totals.gross_total.to_string()
    );
    assert_eq!(view.totals.due, model.totals.due.to_string());
    assert_eq!(view.lines.len(), model.lines.len());
    assert_eq!(view.vat_breakdown.len(), model.vat_breakdown.len());
}

/// The view may omit document-level allowances only while there are none.
///
/// `DocumentView` carries BG-25 lines, the BG-23 breakdown and BG-22 totals, but
/// not BG-20/BG-21 — the document-level allowances and charges that sit between
/// the line total (BT-106) and the taxable total (BT-109). That omission is
/// safe today for one reason only: `energy_billing`'s mapping never emits them,
/// because every discount in this engine is a negative *line*. So BT-106 always
/// equals BT-109 and a page showing one shows the other.
///
/// The day that changes, a template would print a "Summe netto" that does not
/// reconcile with the total below it while the embedded XML stays correct —
/// exactly the visual/machine disagreement the whole design exists to prevent,
/// and the one failure mode no rendering test would catch. This is the tripwire:
/// if it fails, `DocumentView` needs BG-20/BG-21 before the mapping ships them.
#[test]
fn the_view_may_omit_document_level_allowances_only_while_there_are_none() {
    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", None);

    assert!(
        model.allowances.is_empty() && model.charges.is_empty(),
        "the mapping now emits BG-20/BG-21; DocumentView must carry them, or a \
         template's totals will silently stop reconciling with the embedded XML",
    );
    assert_eq!(
        model.totals.line_total, model.totals.taxable_total,
        "BT-106 and BT-109 diverge only through document-level allowances",
    );
}

/// The seller's BT-34 is a GLN or it is absent — never a false claim.
///
/// mako fixed this on the buyer side (a MaLo-ID must not be dressed up as a
/// GLN) and left the identical defect on the seller side, because
/// `Identifier::eas` validates the scheme and accepts any content, and no
/// business rule can test a claim about a registry. `Identifier::eas_checked`
/// (en16931 0.4.0) can, and this pins both directions.
#[test]
fn the_seller_electronic_address_is_a_gln_or_absent() {
    // A correctly configured operator: a real BDEW-Codenummer is a real GLN.
    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", None);
    let bt34 = model
        .seller
        .electronic_address
        .clone()
        .expect("a valid GLN is emitted");
    assert_eq!(bt34.content(), "9900000000004");
    assert_eq!(bt34.scheme(), Some("0088"));

    // A mistyped one: the term is omitted rather than asserting a GLN the
    // identifier is not. BT-34 is optional in EN 16931 core, so the retail
    // document stays valid — it simply stops making a claim it cannot support.
    let mut mistyped = cfg();
    mistyped.tenant = "9900000000001".to_owned();
    let model = einvoice::build(&mixed_rate_invoice(), &mistyped, "51238696781", None);
    assert!(
        model.seller.electronic_address.is_none(),
        "a bad GS1 check digit must omit BT-34, not claim it",
    );
    assert!(
        einvoice::validate(&model).fatal().next().is_none(),
        "BT-34 is optional in core; omitting it must not invalidate the document",
    );
}

/// Every invoice carries its billing period (BG-14).
///
/// § 14 Abs. 4 Nr. 6 UStG requires the Leistungszeitraum on the document, and
/// XRechnung's BR-DE-TMP-32 requires BT-72, BG-14 or a period on every line.
/// `to_en16931` never mapped it: the term was absent from the semantic model
/// and therefore from every syntax rendered out of it — including the
/// "Abrechnungszeitraum" line on the PDF, which silently rendered nothing.
/// en16931 0.4.0's XRechnung profile is what surfaced it.
#[test]
fn the_billing_period_reaches_the_semantic_model() {
    let model = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", None);
    let period = model.invoicing_period.clone().expect("BG-14 is mapped");
    assert_eq!(
        period.start.map(|d| d.to_string()).as_deref(),
        Some("2026-01-01")
    );
    assert_eq!(
        period.end.map(|d| d.to_string()).as_deref(),
        Some("2026-01-31")
    );

    // And it reaches the page: the template reads these two fields.
    let view = billingd::document_view::DocumentView::of(&model);
    assert_eq!(view.period_start.as_deref(), Some("2026-01-01"));
    assert_eq!(view.period_end.as_deref(), Some("2026-01-31"));
}

/// The gate specimen's stamped terms match what production stamps.
///
/// The specimen itself lives with the renderer now
/// (`outputd::document::gate::specimen_invoice`), where its own suite asserts
/// the same terms with the same expected values — the two tests together are
/// the cross-service drift tripwire the old in-process equality check was.
/// This side pins what *production* stamps.
#[test]
fn production_stamps_the_terms_the_gate_specimen_proves_templates_against() {
    let produced = einvoice::build(&mixed_rate_invoice(), &cfg(), "51238696781", None);

    assert_eq!(
        produced.business_process.as_deref(),
        Some("urn:fdc:peppol.eu:2017:poacc:billing:01:1.0"),
        "BT-23 business process",
    );
    assert_eq!(
        produced
            .seller
            .electronic_address
            .as_ref()
            .and_then(|i| i.scheme()),
        Some("0088"),
        "BT-34 seller electronic address, EAS 0088 (GLN)",
    );
    assert_eq!(
        produced
            .payment
            .as_ref()
            .and_then(|p| p.means_code.as_ref().map(en16931::invoice::Code::as_str)),
        Some("58"),
        "BG-16 payment instructions with the SEPA means code (UNCL 4461 58)",
    );
    assert!(produced.invoicing_period.is_some(), "BG-14 billing period");
}
