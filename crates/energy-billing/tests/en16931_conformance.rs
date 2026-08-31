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
        serde_json::from_str(r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0"}"#).unwrap();
    let heat: Product = serde_json::from_str(
        r#"{"category":"WAERME","waerme_arbeitspreis_ct_per_kwh":"10.0","mwst_rate_override":"0.07"}"#,
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

    let en = invoice
        .to_en16931(
            XRECHNUNG_SPEC_ID,
            party("Stadtwerke Musterstadt GmbH", "9900000000001"),
            party("Kunde", "51238696781"),
        )
        .expect("a single-category invoice renders");

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

/// §13b UStG reverse charge: every line is `AE`, the BG-23 breakdown is a single
/// `AE` entry with zero tax, and it carries the § 14a Abs. 5 UStG statement
/// (BR-AE-10). Deriving the category from the rate alone made these lines `Z` —
/// a zero-rated supply, which says the opposite about who owes the VAT.
#[test]
fn reverse_charge_invoice_maps_to_ae_lines_and_breakdown() {
    use energy_billing::en16931_map::{SECT13B_EXEMPTION_REASON, VATEX_REVERSE_CHARGE};

    let elec: Product = serde_json::from_str(
        r#"{"category":"STROM","grundpreis_ct_per_day":"30.0","arbeitspreis_ct_per_kwh":"30.0"}"#,
    )
    .unwrap();
    let quantities = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(100000),
            ..Default::default()
        }),
        ..Default::default()
    };
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-EN16931-13B".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: RegulatoryRates::default(),
        // §13b Abs. 2 Nr. 5 lit. b UStG — the customer is a Stromwiederverkäufer.
        reverse_charge: true,
        ..Default::default()
    };

    let invoice = BillingEngine::new()
        .add(ElectricityProvider::from_product(
            &elec,
            GridInput::default(),
        ))
        .add(MwStProvider::new(dec!(0.19)))
        .bill(ctx, &quantities)
        .unwrap();
    assert_eq!(invoice.mwst_eur, dec!(0), "the supplier invoices net");

    let en = invoice
        .to_en16931(
            XRECHNUNG_SPEC_ID,
            party("Stadtwerke Musterstadt GmbH", "9900000000001"),
            party("Reseller GmbH", "51238696781"),
        )
        .expect("a single-category invoice renders");

    let report = validate(&en);
    assert!(
        report.is_valid(),
        "en16931 findings: {:?}",
        report.fatal().map(|f| &f.rule).collect::<Vec<_>>()
    );

    assert!(!en.lines.is_empty());
    for line in &en.lines {
        assert_eq!(
            line.vat.category.as_str(),
            "AE",
            "every line is AE, never Z"
        );
    }
    assert_eq!(en.vat_breakdown.len(), 1);
    let ae = &en.vat_breakdown[0];
    assert_eq!(ae.category.as_str(), "AE");
    assert_eq!(ae.tax_amount.into_decimal(), dec!(0));
    assert_eq!(
        ae.exemption_reason.as_deref(),
        Some(SECT13B_EXEMPTION_REASON)
    );
    assert_eq!(
        ae.exemption_reason_code.as_ref().map(Code::as_str),
        Some(VATEX_REVERSE_CHARGE)
    );
}

/// A hoheitliche Abwassergebühr is **not subject to VAT** (EN 16931 `O`), not
/// zero-rated (`Z`). BR-O-11 … BR-O-14 make `O` exclusive to its document, so a
/// combined Trinkwasser-plus-Gebühr invoice has no valid rendering — it must be
/// refused here rather than handed to a recipient whose schematron rejects it
/// days later. Over 90 % of German municipalities levy the public-law form, so
/// this was every combined water invoice the platform produced.
#[test]
fn a_public_law_fee_cannot_share_a_document_with_a_taxable_supply() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"WASSER","wasser_grundpreis_eur_per_month":"8.0",
             "wasser_mengenpreis_eur_per_m3":"2.10","schmutzwasser_eur_per_m3":"2.60",
             "abwasser_regime":"PUBLIC_LAW_FEE"}"#,
    )
    .unwrap();
    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "W-2026-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            ctx,
            &Quantities {
                wasser: Some(WasserMeterInput {
                    frischwasser_m3: dec!(120),
                    months: Some(dec!(1)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .expect("a combined paper statement is lawful and still bills");

    // The engine warns; the paper document is fine.
    assert!(
        invoice
            .warnings
            .iter()
            .any(|w| w.code == "GEBUEHR_UND_ENTGELT_AUF_EINEM_BELEG")
    );

    // The Gebühr is `O`, the Trinkwasser is `S` at the reduced rate.
    let subs = invoice.tax_subtotals(rates.mwst_rate);
    assert!(
        subs.iter().any(|s| s.category == VatCategory::OutOfScope),
        "the public-law fee must be O, not Z: {subs:?}"
    );
    assert!(subs.iter().any(|s| s.rate_percent == dec!(7)));

    // …and the e-invoice is refused, with the reason.
    let err = invoice
        .to_en16931(
            XRECHNUNG_SPEC_ID,
            party("Stadtwerke Musterstadt GmbH", "9900000000001"),
            party("Kunde", "51238696781"),
        )
        .expect_err("BR-O-11 ff. forbids mixing O with any other category");
    assert!(format!("{err:?}").contains("EN16931_KATEGORIE_O_NICHT_KOMBINIERBAR"));
}

/// The privatised form is an ordinary taxable supply, so it renders.
#[test]
fn a_private_law_wastewater_charge_renders_normally() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"WASSER","wasser_mengenpreis_eur_per_m3":"2.10",
             "schmutzwasser_eur_per_m3":"2.60","abwasser_regime":"PRIVATE_LAW_CHARGE"}"#,
    )
    .unwrap();
    let rates = RegulatoryRates::default();
    let ctx = BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "W-2026-002".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates.clone(),
        ..Default::default()
    };
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            ctx,
            &Quantities {
                wasser: Some(WasserMeterInput {
                    frischwasser_m3: dec!(120),
                    months: Some(dec!(1)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    let en = invoice
        .to_en16931(
            XRECHNUNG_SPEC_ID,
            party("Stadtwerke Musterstadt GmbH", "9900000000001"),
            party("Kunde", "51238696781"),
        )
        .expect("7 % and 19 % coexist happily");
    let report = validate(&en);
    assert!(report.is_valid(), "en16931 findings: {:?}", report);
}

/// `BillingContext::settlement_form` selects how a settling invoice presents
/// the advances. It selected nothing: the field was declared, documented and
/// read by no code path, so every e-invoice went out as an Endrechnung with a
/// flat BT-113 — including the ones the BMF recommends the other form for
/// (Schreiben v. 15.10.2024, Rn. 48), and the ones whose advances were invoiced
/// at a different rate from the settlement.
#[test]
fn both_settlement_forms_render_and_agree_on_what_is_due() {
    use energy_billing::*;
    use rust_decimal::dec;
    use time::macros::date;

    let tariff: Product = serde_json::from_str(
        r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0","grundpreis_ct_per_day":"30.0"}"#,
    )
    .unwrap();
    let rates = RegulatoryRates::default();
    let abschlaege = vec![
        AbschlagDeduction {
            datum: date!(2026 - 03 - 01),
            betrag_eur: dec!(119.00),
            ust_satz: dec!(0.19),
            beschreibung: Some("Abschlag März".to_owned()),
        },
        AbschlagDeduction {
            datum: date!(2026 - 06 - 01),
            betrag_eur: dec!(238.00),
            ust_satz: dec!(0.19),
            beschreibung: Some("Abschlag Juni".to_owned()),
        },
    ];
    let ctx = |form| BillingContext {
        malo_id: "51238696781".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "JAHR-2026-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 12 - 31)).unwrap(),
        invoice_type: InvoiceType::Final,
        settlement_form: form,
        regulatory_rates: rates.clone(),
        abschlage: abschlaege.clone(),
        ..Default::default()
    };
    let q = Quantities {
        electricity: Some(MeterInput {
            arbeitsmenge_kwh: dec!(3500),
            ..Default::default()
        }),
        ..Default::default()
    };
    let bill = |form| {
        tariff
            .build_engine(&GridInput::default(), &rates)
            .bill(ctx(form), &q)
            .unwrap()
    };
    let end = bill(SettlementForm::Endrechnung);
    let rest = bill(SettlementForm::Restrechnung);

    // The engine's own totals are the same either way — the form changes the
    // document, not the money.
    assert_eq!(end.zahlbetrag_eur, rest.zahlbetrag_eur);
    assert_eq!(end.abschlag_total_eur, dec!(357.00));

    let render = |inv: &Invoice| {
        inv.to_en16931(
            XRECHNUNG_SPEC_ID,
            party("Stadtwerke Musterstadt GmbH", "9900000000001"),
            party("Kunde", "51238696781"),
        )
        .expect("renders")
    };
    let en_end = render(&end);
    let en_rest = render(&rest);

    // Both are valid EN 16931 documents.
    for (name, en) in [("Endrechnung", &en_end), ("Restrechnung", &en_rest)] {
        let report = validate(en);
        assert!(report.is_valid(), "{name}: {report:?}");
    }

    // Endrechnung: the full supply is taxed, the advances are the paid amount.
    assert!(en_end.totals.paid.is_some());
    assert!(en_end.allowances.is_empty());

    // Restrechnung: the advances ride as document-level allowances carrying
    // their own VAT rate, nothing is stated as paid, and the taxable base is
    // the residual — 357,00 gross of advance is 300,00 net off the base.
    assert!(en_rest.totals.paid.is_none());
    assert_eq!(
        en_rest.allowances.len(),
        1,
        "one group per (category, rate)"
    );
    assert_eq!(
        en_rest.allowances[0].amount.into_decimal(),
        dec!(300.00),
        "the allowance is the advances' net, not their gross"
    );

    // …and both documents ask the customer for the same money.
    assert_eq!(
        en_end.totals.due.into_decimal(),
        en_rest.totals.due.into_decimal(),
    );
}
