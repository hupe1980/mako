//! One invoice, one set of rounded lines.
//!
//! The BO4E `Rechnung` and the EN 16931 model are two projections of the same
//! document. Each position is rounded to the cent once, and both projections
//! and the tax are derived from those lines: `round(Σ x)` and `Σ round(x)`
//! differ by up to a cent per line, so two rounding points put two different
//! totals — and two different VAT bases — on one invoice.

#![cfg(all(feature = "bo4e", feature = "en16931"))]

use en16931::identifier::Identifier;
use en16931::invoice::{Code, Party, PostalAddress};
use energy_billing::{
    BillingContext, BillingPeriod, BillingPosition, BillingProvider as _, Invoice, InvoiceType,
    MwStProvider, PositionCategory, Quantities, RegulatoryRates, en16931_map::XRECHNUNG_SPEC_ID,
};
use proptest::prelude::*;
use rust_decimal::{Decimal, dec};
use time::macros::date;

fn party(name: &str) -> Party {
    Party {
        name: Some(name.to_owned()),
        vat_identifier: Some("DE123456789".to_owned()),
        electronic_address: Some(Identifier::schemed("buyer@example.org", "0204")),
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

fn ctx() -> BillingContext {
    BillingContext {
        malo_id: "51238696012".to_owned(),
        lf_mp_id: "9900000000001".to_owned(),
        rechnungsnummer: "R-ROUND-001".to_owned(),
        period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap(),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: RegulatoryRates::default(),
        ..Default::default()
    }
}

/// The invoice the engine would assemble: supply lines, the MwSt pass over
/// them, then `Invoice::from_positions`.
fn invoice_of(lines: &[(Decimal, Decimal)]) -> Invoice {
    let ctx = ctx();
    let mut positions: Vec<BillingPosition> = lines
        .iter()
        .enumerate()
        .map(|(i, (qty, price))| {
            BillingPosition::debit(
                format!("Position {i}"),
                *qty,
                "kWh",
                *price,
                PositionCategory::Commodity,
            )
        })
        .collect();
    let tax = MwStProvider::new(dec!(0.19))
        .bill(&ctx, &Quantities::default(), &positions)
        .expect("tax pass");
    positions.extend(tax);
    Invoice::from_positions(ctx, positions, vec![])
}

/// What the two projections state: `(BO4E gesamtnetto, gesamtsteuer,
/// gesamtbrutto, EN line_total, EN vat_total, EN gross_total)`.
fn totals(invoice: &Invoice) -> [Decimal; 6] {
    let r = invoice.to_rechnung();
    let wert = |b: Option<rubo4e::current::Betrag>| b.and_then(|b| b.wert).expect("stated");
    let en = invoice
        .to_en16931(XRECHNUNG_SPEC_ID, party("Lieferant GmbH"), party("Kunde"))
        .expect("maps");
    [
        wert(r.gesamtnetto),
        wert(r.gesamtsteuer),
        wert(r.gesamtbrutto),
        en.totals.line_total.into_decimal(),
        en.totals.vat_total.expect("VAT stated").into_decimal(),
        en.totals.gross_total.into_decimal(),
    ]
}

/// Eight lines of 0.125 EUR: `round(Σ)` is 1.00, `Σ round` is 1.04.
#[test]
fn an_eight_line_invoice_states_one_total_in_both_projections() {
    let invoice = invoice_of(&[(dec!(1), dec!(0.125)); 8]);
    let [netto, steuer, brutto, line_total, vat_total, gross_total] = totals(&invoice);
    assert_eq!(netto, line_total, "BO4E gesamtnetto vs EN 16931 BT-106");
    assert_eq!(steuer, vat_total, "BO4E gesamtsteuer vs EN 16931 BT-110");
    assert_eq!(brutto, gross_total, "BO4E gesamtbrutto vs EN 16931 BT-112");
    assert_eq!(netto, dec!(1.04), "eight lines of 0.13 each");
    assert_eq!(steuer, dec!(0.20), "19 % of the base printed beside it");
}

/// Every stated tax is its base times its rate, at the cent.
fn assert_tax_matches_base(invoice: &Invoice) {
    let r = invoice.to_rechnung();
    for s in r.steuerbetraege.unwrap_or_default() {
        let base = s.basiswert.expect("base");
        let rate = s.steuersatz.expect("rate") / dec!(100);
        let tax = s.steuerwert.expect("tax");
        let expected = (base * rate)
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
        assert_eq!(tax, expected, "tax {tax} on base {base} at {rate}");
    }
}

#[test]
fn a_one_line_invoice_is_unchanged() {
    let invoice = invoice_of(&[(dec!(250), dec!(0.32))]);
    let [netto, steuer, ..] = totals(&invoice);
    assert_eq!(netto, dec!(80.00));
    assert_eq!(steuer, dec!(15.20));
    assert_tax_matches_base(&invoice);
}

fn line() -> impl Strategy<Value = (Decimal, Decimal)> {
    // Quantities to 3 dp and unit prices to 5 dp, as meters and price sheets
    // state them.
    (1i64..=2_000_000, 1i64..=5_000_000).prop_map(|(q, p)| (Decimal::new(q, 3), Decimal::new(p, 5)))
}

proptest! {
    #[test]
    fn the_projections_agree_on_every_generated_invoice(
        lines in proptest::collection::vec(line(), 1..16)
    ) {
        let invoice = invoice_of(&lines);
        let [netto, steuer, brutto, line_total, vat_total, gross_total] = totals(&invoice);
        prop_assert_eq!(netto, line_total);
        prop_assert_eq!(steuer, vat_total);
        prop_assert_eq!(brutto, gross_total);
        assert_tax_matches_base(&invoice);
    }
}
