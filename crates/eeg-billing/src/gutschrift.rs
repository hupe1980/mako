//! EEG Gutschrift document — the §14 UStG self-billing invoice for feed-in.
//!
//! Under the **Gutschriftverfahren (§14 Abs. 2 Satz 2 UStG)** the Netzbetreiber
//! *issues* the settlement document to the Anlagenbetreiber (the recipient of the
//! supply issues the invoice, not the supplier). For a Regelbesteuerung operator
//! that document must show 19 % USt; for §12 Abs. 3 (0 %) and §19 Kleinunternehmer
//! it shows none — but the document, with its VAT breakdown (EN 16931 BG-23), is
//! required in every case. The settlement *amount* alone was never a legal document.
//!
//! The [`settlement_to_gutschrift`] function assembles a [`billing::BillingDocument`]
//! (positions + the VAT layers for the operator's tax status) and renders it as a
//! BO4E [`rubo4e::current::Rechnung`]. The `billing` crate does the money and VAT
//! (shared with energy-/grid-billing); the BO4E rendering below is EEG-specific and
//! lives here — this is the crate's own `bo4e` bridge, the same pattern
//! `energy-billing::Invoice::to_rechnung` and `grid_billing::bo4e::into_rechnung`
//! already follow.

use billing::{BillingDocument, BillingError, DocumentMeta, LineItem, ScalarTariff, TaxCategory};
use rubo4e::current as bo;
use rust_decimal::Decimal;

use crate::model::SettleOutput;
use crate::tariff::EegSettleTariff;
use crate::ust::{VatStatus, ust_tax_layers};

/// Render an EEG settlement as a §14 UStG Gutschrift (`rubo4e::current::Rechnung`).
///
/// `vat` selects the tax layers (Regelbesteuerung 19 % / §12 Abs. 3 zero-rated /
/// §19 exempt); `meta` carries the document facts (Gutschrift number, period, dates,
/// NB = `issuer_id`, Anlagenbetreiber = `recipient_id`). For a settlement with no
/// billable positions (NoData / PriceMissing) the Rechnung has no positions — the
/// caller decides whether to issue it.
///
/// # Errors
///
/// Propagates any [`BillingError`] from document assembly (e.g. a position amount
/// outside the representable range).
pub fn settlement_to_gutschrift(
    output: &SettleOutput,
    vat: VatStatus,
    meta: DocumentMeta,
) -> Result<bo::Rechnung, BillingError> {
    Ok(settlement_to_gutschrift_with_document(output, vat, meta)?.0)
}

/// The Gutschrift **and** the [`BillingDocument`] it came from, for callers that
/// also want the typed totals (net / tax / gross) for a ledger entry without
/// re-parsing the BO4E form.
///
/// # Errors
///
/// Propagates any [`BillingError`] from document assembly.
pub fn settlement_to_gutschrift_with_document(
    output: &SettleOutput,
    vat: VatStatus,
    meta: DocumentMeta,
) -> Result<(bo::Rechnung, BillingDocument), BillingError> {
    let positions = EegSettleTariff::new(output).positions()?.into_inner();
    let doc = BillingDocument::from_positions(meta, positions, ust_tax_layers(vat), vec![])?;
    let rechnung = document_to_rechnung(&doc);
    Ok((rechnung, doc))
}

/// Map the assembled [`BillingDocument`] to the BO4E Gutschrift.
///
/// EEG feed-in is electricity billed per kWh, so the mapping is small: no BDEW
/// Artikelnummer, one unit. The per-position `legal_basis` (`§21 EEG 2023`, …) and
/// the party MP-IDs ride as round-trip-preserved extension data BO4E has no field
/// for.
fn document_to_rechnung(doc: &BillingDocument) -> bo::Rechnung {
    let meta = &doc.meta;

    let rechnungspositionen: Vec<bo::Rechnungsposition> = doc
        .net_positions()
        .iter()
        .enumerate()
        .map(|(i, p)| position_to_bo4e(i + 1, p))
        .collect();

    let steuerbetraege: Vec<bo::Steuerbetrag> = doc
        .tax_breakdown()
        .iter()
        .map(|e| bo::Steuerbetrag {
            basiswert: Some(e.taxable_base.into_decimal()),
            steuerwert: Some(e.tax_amount.into_decimal()),
            // BO4E carries the rate as a percentage (BT-119); billing stores a fraction.
            steuersatz: Some(e.rate * Decimal::ONE_HUNDRED),
            steuerart: Some(match e.category {
                TaxCategory::ReverseCharge => bo::Steuerart::Rcv,
                _ => bo::Steuerart::Ust,
            }),
            waehrungscode: Some(bo::Waehrungscode::Eur),
            ..Default::default()
        })
        .collect();

    let mut rechnung = bo::Rechnung {
        typ: Some(bo::BoTyp::Rechnung),
        sparte: Some(bo::Sparte::Strom),
        rechnungsnummer: non_empty(&meta.invoice_number),
        rechnungstitel: non_empty(&meta.period_label),
        rechnungsdatum: meta.issue_date.as_deref().and_then(parse_iso_date),
        faelligkeitsdatum: meta.due_date.as_deref().and_then(parse_iso_date),
        rechnungsperiode: meta.period.as_ref().map(|per| bo::Zeitraum {
            startdatum: parse_iso_date(&per.from),
            enddatum: parse_iso_date(&per.to),
            ..Default::default()
        }),
        gesamtnetto: Some(betrag(doc.net_total().into_decimal())),
        gesamtsteuer: Some(betrag(doc.tax_total().into_decimal())),
        gesamtbrutto: Some(betrag(doc.gross_total().into_decimal())),
        steuerbetraege: (!steuerbetraege.is_empty()).then_some(steuerbetraege),
        rechnungspositionen: Some(rechnungspositionen),
        ..Default::default()
    };

    // The NB issues the Gutschrift (Gutschriftverfahren); the Anlagenbetreiber
    // receives it. BO4E models these as full Geschaeftspartner objects — we carry
    // only the MP-IDs, as round-trip-preserved keys.
    if let Some(id) = &meta.issuer_id {
        let _ = rechnung
            ._additional
            .try_insert("rechnungserstellerId".into(), id.as_str().into());
    }
    if let Some(id) = &meta.recipient_id {
        let _ = rechnung
            ._additional
            .try_insert("rechnungsempfaengerId".into(), id.as_str().into());
    }
    rechnung
}

fn position_to_bo4e(number: usize, p: &LineItem) -> bo::Rechnungsposition {
    let positions_menge = p.quantity.as_ref().map(|q| bo::Menge {
        wert: Some(q.value),
        einheit: Some(bo::Mengeneinheit::Kwh), // EEG feed-in is always kWh
        ..Default::default()
    });
    let einzelpreis = p.unit_price.as_ref().map(|up| bo::Preis {
        wert: Some(up.value),
        einheit: Some(bo::Waehrungseinheit::Eur),
        ..Default::default()
    });
    let mut pos = bo::Rechnungsposition {
        positionsnummer: Some(number as i64),
        positionstext: non_empty(&p.description),
        positions_menge,
        einzelpreis,
        gesamtpreis: Some(betrag(p.net_amount.into_decimal())),
        ..Default::default()
    };
    // The legal basis (§21 EEG 2023, …) is the audit record of why the rate applies.
    if let Some(lb) = p.get_meta("legal_basis") {
        let _ = pos
            ._additional
            .try_insert("rechtlicheGrundlage".into(), lb.into());
    }
    pos
}

fn betrag(wert: Decimal) -> bo::Betrag {
    bo::Betrag {
        wert: Some(wert),
        waehrung: Some(bo::Waehrungscode::Eur),
        ..Default::default()
    }
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

/// Parse an ISO `YYYY-MM-DD` date (tolerating a trailing time); `None` otherwise.
fn parse_iso_date(s: &str) -> Option<time::Date> {
    let date_part = s.get(..10)?;
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    time::Date::parse(date_part, fmt).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SettleInput, SettlementScheme, calculate_settlement};
    use billing::{Currency, Period};
    use rust_decimal::dec;

    fn meta() -> DocumentMeta {
        DocumentMeta {
            invoice_number: "GS-EEG-2026-07-000123".into(),
            currency: Currency::EUR,
            period_label: "Juli 2026".into(),
            period: Some(Period::from_display("2026-07-01", "2026-07-31")),
            issue_date: Some("2026-08-05".into()),
            due_date: Some("2026-08-15".into()),
            issuer_id: Some("9904234560001".into()), // NB MP-ID (issues the Gutschrift)
            recipient_id: Some("DE00012345678".into()), // Anlagenbetreiber
            ..Default::default()
        }
    }

    fn feed_in_output() -> SettleOutput {
        calculate_settlement(&SettleInput {
            scheme: SettlementScheme::FeedInTariff {
                verguetungssatz_ct: dec!(8.11),
            },
            einspeisemenge_kwh: Some(dec!(1000)),
            ..SettleInput::default()
        })
    }

    #[test]
    fn regelbesteuerung_gutschrift_shows_19pct_ust() {
        let r = settlement_to_gutschrift(&feed_in_output(), VatStatus::Regelbesteuerung, meta())
            .unwrap();
        assert_eq!(r.rechnungsnummer.as_deref(), Some("GS-EEG-2026-07-000123"));
        assert_eq!(r.sparte, Some(bo::Sparte::Strom));
        assert_eq!(r.gesamtnetto.unwrap().wert, Some(dec!(81.10000)));
        // 81.10 × 19 % = 15.409 → gross 96.509
        assert_eq!(r.gesamtbrutto.unwrap().wert, Some(dec!(96.50900)));
        let steuer = r
            .steuerbetraege
            .expect("a Gutschrift must carry the VAT breakdown");
        assert_eq!(steuer.len(), 1);
        assert_eq!(steuer[0].steuersatz, Some(dec!(19)));
        let pos = r.rechnungspositionen.unwrap();
        assert_eq!(
            pos[0].positions_menge.as_ref().unwrap().einheit,
            Some(bo::Mengeneinheit::Kwh)
        );
    }

    #[test]
    fn par12abs3_gutschrift_is_zero_rated_not_missing() {
        let r = settlement_to_gutschrift(&feed_in_output(), VatStatus::BefreitNach12Abs3, meta())
            .unwrap();
        assert_eq!(r.gesamtbrutto.unwrap().wert, r.gesamtnetto.unwrap().wert);
        let steuer = r
            .steuerbetraege
            .expect("zero-rated still carries a breakdown");
        assert!(steuer[0].steuerwert.unwrap().is_zero());
    }
}
