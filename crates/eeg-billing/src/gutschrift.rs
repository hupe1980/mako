//! EEG Gutschrift document — the §14 UStG self-billing invoice for feed-in.
//!
//! Under the **Gutschriftverfahren (§14 Abs. 2 Satz 2 UStG)** the Netzbetreiber
//! *issues* the settlement document to the Anlagenbetreiber (the recipient of the
//! supply issues the invoice, not the supplier). For a Regelbesteuerung operator
//! that document must show 19 % USt; for a §19 Kleinunternehmer it shows none
//! (category `E`) — but the document, with its VAT breakdown (EN 16931 BG-23), is
//! required in every case. The settlement *amount* alone was never a legal document.
//!
//! [`settlement_to_gutschrift`] builds the [`billing::Document`] (the positions
//! under the VAT treatment of the operator's tax status, see
//! [`crate::bridge::settlement_document`]) and renders it, with the
//! [`GutschriftHeader`], as a BO4E [`rubo4e::current::Rechnung`]. `billing`
//! derives the money and VAT once (shared with energy-/grid-billing); the header
//! and the BO4E rendering are EEG-specific and live here — this is the crate's
//! own `bo4e` bridge, the same pattern `energy-billing::Invoice::to_rechnung`
//! and `grid_billing::bo4e::into_rechnung` already follow.

use billing::{Amount, Document, TaxCategory};
use rubo4e::current as bo;
use rust_decimal::Decimal;
use time::Date;

use crate::bridge::settlement_document;
use crate::model::SettleOutput;
use crate::ust::VatStatus;

/// The document facts of a Gutschrift that `billing` does not derive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GutschriftHeader {
    /// Gutschriftnummer (BO4E `rechnungsnummer`).
    pub nummer: String,
    /// Title, typically the settled month (BO4E `rechnungstitel`).
    pub titel: String,
    /// The settled period, first and last day inclusive.
    pub zeitraum: Option<(Date, Date)>,
    /// Rechnungsdatum — § 14 Abs. 4 Nr. 3 UStG.
    pub rechnungsdatum: Option<Date>,
    /// Fälligkeitsdatum.
    pub faelligkeitsdatum: Option<Date>,
    /// MP-ID of the Netzbetreiber, who issues the Gutschrift.
    pub aussteller: Option<String>,
    /// Identifier of the Anlagenbetreiber, who receives it.
    pub empfaenger: Option<String>,
}

/// Why a Gutschrift cannot be assembled.
#[derive(Debug, thiserror::Error)]
pub enum GutschriftError {
    /// `billing` refused the document (an amount outside the representable range).
    #[error("Gutschrift document: {0}")]
    Billing(#[from] billing::Error),
    /// The settled period ends before it starts.
    #[error("Gutschrift period ends {to} before it starts {from}")]
    PeriodReversed {
        /// First day.
        from: Date,
        /// Last day.
        to: Date,
    },
}

/// Render an EEG settlement as a §14 UStG Gutschrift (`rubo4e::current::Rechnung`).
///
/// `vat` selects the VAT treatment (Regelbesteuerung 19 % `S` / §19
/// Kleinunternehmer exempt `E`); `header` carries the document facts. For a
/// settlement with no billable positions (NoData / PriceMissing) the Rechnung
/// has no positions — the caller decides whether to issue it.
///
/// # Errors
///
/// [`GutschriftError`] when the document cannot be built or the period is
/// reversed.
pub fn settlement_to_gutschrift(
    output: &SettleOutput,
    vat: VatStatus,
    header: &GutschriftHeader,
) -> Result<bo::Rechnung, GutschriftError> {
    Ok(settlement_to_gutschrift_with_document(output, vat, header)?.0)
}

/// The Gutschrift **and** the [`Document`] it came from, for callers that also
/// want the typed totals (BT-109 / BT-110 / BT-112) for a ledger entry without
/// re-parsing the BO4E form.
///
/// # Errors
///
/// As [`settlement_to_gutschrift`].
pub fn settlement_to_gutschrift_with_document(
    output: &SettleOutput,
    vat: VatStatus,
    header: &GutschriftHeader,
) -> Result<(bo::Rechnung, Document), GutschriftError> {
    if let Some((from, to)) = header.zeitraum
        && to < from
    {
        return Err(GutschriftError::PeriodReversed { from, to });
    }
    let doc = settlement_document(output, vat)?;
    let rechnung = document_to_rechnung(&doc, header)?;
    Ok((rechnung, doc))
}

/// An amount the EN 16931 scale already reduced to cents, as a 2-dp `Decimal`.
fn cents(a: Amount) -> Result<Decimal, billing::Error> {
    Ok(a.exact::<2>()?.to_decimal())
}

/// Map the built [`Document`] to the BO4E Gutschrift.
///
/// EEG feed-in is electricity billed per kWh, so the mapping is small: no BDEW
/// Artikelnummer, one unit. The per-position `legal_basis` (`§21 EEG 2023`, …) and
/// the party MP-IDs ride as round-trip-preserved extension data BO4E has no field
/// for. Every amount is the document's own, derived once at two decimals
/// (EN 16931 BR-DEC); nothing is rounded here.
fn document_to_rechnung(
    doc: &Document,
    header: &GutschriftHeader,
) -> Result<bo::Rechnung, GutschriftError> {
    let rechnungspositionen: Vec<bo::Rechnungsposition> = doc
        .lines()
        .iter()
        .enumerate()
        .map(|(i, line)| position_to_bo4e(i + 1, line, doc.line_net(i)))
        .collect::<Result<_, _>>()?;

    let steuerbetraege: Vec<bo::Steuerbetrag> = doc
        .vat_groups()
        .iter()
        .map(|g| -> Result<bo::Steuerbetrag, billing::Error> {
            Ok(bo::Steuerbetrag {
                basiswert: Some(cents(g.taxable_base())?),
                steuerwert: Some(cents(g.tax())?),
                // BT-119, a percentage on both sides.
                steuersatz: Some(g.rate_percent().value()),
                steuerart: Some(match g.category() {
                    TaxCategory::ReverseCharge => bo::Steuerart::Rcv,
                    _ => bo::Steuerart::Ust,
                }),
                waehrungscode: Some(bo::Waehrungscode::Eur),
                ..Default::default()
            })
        })
        .collect::<Result<_, _>>()?;

    let rechnungsperiode = header.zeitraum.map(|(from, to)| bo::Zeitraum {
        startdatum: Some(from),
        enddatum: Some(to),
        ..Default::default()
    });

    let mut rechnung = bo::Rechnung {
        typ: Some(bo::BoTyp::Rechnung),
        sparte: Some(bo::Sparte::Strom),
        rechnungsnummer: non_empty(&header.nummer),
        rechnungstitel: non_empty(&header.titel),
        rechnungsdatum: header.rechnungsdatum.and_then(as_bo4e_timestamp),
        faelligkeitsdatum: header.faelligkeitsdatum.and_then(as_bo4e_timestamp),
        rechnungsperiode,
        // BO4E gesamtnetto/-steuer/-brutto are EN 16931 BT-109 (Summe ohne
        // USt), BT-110 (USt-Summe) and BT-112 (Summe mit USt).
        gesamtnetto: Some(betrag(cents(doc.tax_exclusive_total())?)),
        gesamtsteuer: Some(betrag(cents(doc.vat_total())?)),
        gesamtbrutto: Some(betrag(cents(doc.tax_inclusive_total())?)),
        steuerbetraege: (!steuerbetraege.is_empty()).then_some(steuerbetraege),
        rechnungspositionen: Some(rechnungspositionen),
        ..Default::default()
    };

    // The NB issues the Gutschrift (Gutschriftverfahren); the Anlagenbetreiber
    // receives it. BO4E models both as a `Geschaeftspartner`, so that is where
    // they go — carrying the MP-ID as a `mako:marktpartnercode` ZusatzAttribut,
    // the same shape `energy-billing` uses on its own invoices. Only the MP-IDs
    // are known here, and a `Geschaeftspartner` carrying nothing else satisfies
    // the field.
    fn party(mp_id: &str) -> Box<bo::Geschaeftspartner> {
        Box::new(bo::Geschaeftspartner {
            zusatz_attribute: Some(vec![bo::ZusatzAttribut {
                name: Some("mako:marktpartnercode".to_owned()),
                wert: Some(mp_id.into()),
                ..Default::default()
            }]),
            ..Default::default()
        })
    }
    rechnung.rechnungsersteller = header.aussteller.as_deref().map(party);
    rechnung.rechnungsempfaenger = header.empfaenger.as_deref().map(party);
    Ok(rechnung)
}

/// One BO4E position. `billing` carries a negative rate as a negated quantity
/// at the rate's magnitude (BR-27); BO4E states the metered kWh and the signed
/// rate, so both are turned back here and their product is unchanged.
fn position_to_bo4e(
    number: usize,
    line: &billing::Line,
    net: Amount,
) -> Result<bo::Rechnungsposition, billing::Error> {
    let q = line.quantity().value();
    let sign = if q.is_sign_negative() {
        Decimal::NEGATIVE_ONE
    } else {
        Decimal::ONE
    };
    let positions_menge = Some(bo::Menge {
        wert: Some(q.abs()),
        einheit: Some(bo::Mengeneinheit::Kwh), // EEG feed-in is always kWh
        ..Default::default()
    });
    let einzelpreis = Some(bo::Preis {
        wert: Some(sign * line.price().price().value()),
        einheit: Some(bo::Waehrungseinheit::Eur),
        ..Default::default()
    });
    let mut pos = bo::Rechnungsposition {
        positionsnummer: Some(number as i64),
        positionstext: non_empty(line.description()),
        positions_menge,
        einzelpreis,
        gesamtpreis: Some(betrag(cents(net)?)),
        ..Default::default()
    };
    // The legal basis (§21 EEG 2023, …) is the audit record of why the rate
    // applies. It rides in `zusatzAttribute` — a real BO4E field — under the
    // `mako:` namespace, not as a bare `_additional` key: an unprefixed
    // `rechtlicheGrundlage` is indistinguishable from a field BO4E might define
    // and from one the counterparty's own system writes.
    if let Some(lb) = line.label("legal_basis") {
        pos.zusatz_attribute
            .get_or_insert_with(Vec::new)
            .push(bo::ZusatzAttribut {
                name: Some("mako:rechtliche_grundlage".to_owned()),
                wert: Some(lb.into()),
                ..Default::default()
            });
    }
    Ok(pos)
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

/// A BO4E date-only market value as the `date-time` the schema declares.
///
/// BDEW INVOIC transmits `rechnungsdatum` as `DTM+137` and `faelligkeitsdatum`
/// as `SG8 DTM+265`, both with DE 2379 = `303` — `CCYYMMDDHHMMZZZ`, a *timestamp*
/// — while BO4E types both `format: date-time`. (The value is still a calendar
/// date in meaning; the wire simply carries it at 303 precision. The only INVOIC
/// DTM admitting `102` is `DTM+203`.)
///
/// **Midnight UTC.** `Rechnung::rechnungsdatum_date()` reads the date in the
/// offset the payload carries, so `+00:00` reads back as the date that went in
/// and stays that date under any later normalisation; a `+01:00` midnight
/// becomes the previous day the moment someone converts it.
///
/// `None` for a year outside RFC 3339's `0000`–`9999`, which the field
/// serialises as. The field is optional, so it is omitted rather than made
/// fatal — rejecting such a date is the caller's job, not the serializer's.
fn as_bo4e_timestamp(date: time::Date) -> Option<time::OffsetDateTime> {
    (0..=9999)
        .contains(&date.year())
        .then(|| date.midnight().assume_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SettleInput, SettlementScheme, calculate_settlement};
    use rust_decimal::dec;
    use time::macros::date;

    fn header() -> GutschriftHeader {
        GutschriftHeader {
            nummer: "GS-EEG-2026-07-000123".into(),
            titel: "Juli 2026".into(),
            zeitraum: Some((date!(2026 - 07 - 01), date!(2026 - 07 - 31))),
            rechnungsdatum: Some(date!(2026 - 08 - 05)),
            faelligkeitsdatum: Some(date!(2026 - 08 - 15)),
            aussteller: Some("9904234560001".into()), // NB MP-ID (issues the Gutschrift)
            empfaenger: Some("DE00012345678".into()), // Anlagenbetreiber
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

    /// A period that ends before it starts is refused: the Gutschrift would
    /// otherwise state a Leistungszeitraum (§ 14 Abs. 4 Nr. 6 UStG) no supply
    /// can have.
    #[test]
    fn a_reversed_period_is_refused() {
        let broken = GutschriftHeader {
            zeitraum: Some((date!(2026 - 07 - 31), date!(2026 - 07 - 01))),
            ..header()
        };
        assert!(matches!(
            settlement_to_gutschrift(&feed_in_output(), VatStatus::Regelbesteuerung, &broken),
            Err(GutschriftError::PeriodReversed { .. })
        ));
    }

    #[test]
    fn regelbesteuerung_gutschrift_shows_19pct_ust() {
        let r = settlement_to_gutschrift(&feed_in_output(), VatStatus::Regelbesteuerung, &header())
            .unwrap();
        assert_eq!(r.rechnungsnummer.as_deref(), Some("GS-EEG-2026-07-000123"));
        assert_eq!(r.sparte, Some(bo::Sparte::Strom));
        assert_eq!(r.gesamtnetto.unwrap().wert, Some(dec!(81.10)));
        // EN 16931 BR-CO-17: 81.10 × 19 % = 15.409 → 15.41, gross 96.51.
        assert_eq!(r.gesamtsteuer.unwrap().wert, Some(dec!(15.41)));
        assert_eq!(r.gesamtbrutto.unwrap().wert, Some(dec!(96.51)));
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

    /// The document's own totals are the cent figures the BO4E form states:
    /// `billing` derives BT-110 once, at the EN 16931 scale, so a ledger entry
    /// taken from the document (`einsd` stores `vat_total` and
    /// `tax_inclusive_total`) is 15.41 / 96.51 — the amounts on the Gutschrift
    /// — and never the unrounded 15.409 / 96.509.
    #[test]
    fn the_document_totals_are_the_gutschrift_cents() {
        let (_, doc) = settlement_to_gutschrift_with_document(
            &feed_in_output(),
            VatStatus::Regelbesteuerung,
            &header(),
        )
        .unwrap();
        assert_eq!(doc.tax_exclusive_total().to_decimal(), dec!(81.10));
        assert_eq!(doc.vat_total().to_decimal(), dec!(15.41));
        assert_eq!(doc.tax_inclusive_total().to_decimal(), dec!(96.51));
    }

    /// A negative rate (post-EEG at negative EPEX) is the negated quantity at
    /// the rate's magnitude inside `billing`, and the BO4E position states the
    /// metered kWh and the signed rate again.
    #[test]
    fn a_negative_rate_bills_the_signed_product() {
        let output = SettleOutput {
            status: crate::SettlementStatus::Calculated,
            positions: vec![crate::SettlePosition {
                description: "Post-EEG Spot".into(),
                legal_basis: "\u{00a7}21 Abs. 1 Nr. 3 EEG 2023 (post-F\u{00f6}rderung)".into(),
                kwh: dec!(100),
                rate_ct_kwh: dec!(-2.5),
                eur: dec!(-2.5),
            }],
            ..SettleOutput::default()
        };
        let (r, doc) =
            settlement_to_gutschrift_with_document(&output, VatStatus::Kleinunternehmer, &header())
                .unwrap();
        assert_eq!(doc.tax_exclusive_total().to_decimal(), dec!(-2.50));
        let pos = &r.rechnungspositionen.unwrap()[0];
        assert_eq!(pos.positions_menge.as_ref().unwrap().wert, Some(dec!(100)));
        assert_eq!(pos.einzelpreis.as_ref().unwrap().wert, Some(dec!(-0.025)));
        assert_eq!(pos.gesamtpreis.as_ref().unwrap().wert, Some(dec!(-2.50)));
    }

    #[test]
    fn kleinunternehmer_gutschrift_is_exempt_not_missing() {
        let r = settlement_to_gutschrift(&feed_in_output(), VatStatus::Kleinunternehmer, &header())
            .unwrap();
        assert_eq!(r.gesamtbrutto.unwrap().wert, r.gesamtnetto.unwrap().wert);
        let steuer = r
            .steuerbetraege
            .expect("§19 exempt still carries a breakdown");
        assert!(steuer[0].steuerwert.unwrap().is_zero());
    }

    // ── Outbound BO4E conformance ────────────────────────────────────────────

    /// The §14 UStG Gutschrift this module emits must be valid BO4E.
    ///
    /// It is issued *by the NB to the Anlagenbetreiber* — a document that
    /// leaves the building — and BO4E `Rechnungstyp` has no Gutschrift value,
    /// so the shape here is the one `energy-billing` uses: leave the typed
    /// field absent and label the process in a `mako:` `ZusatzAttribut`. What
    /// this test forbids is the other resolution — forcing a near-enough
    /// `Rechnungstyp` in, or letting any enum fall through to `Unknown`.
    ///
    /// Typed only: this crate carries no `serde_json` (it stays publishable and
    /// rubo4e-free by default, `bo4e` being an opt-in feature), and the JSON
    /// form is the same document — `einsd` is where that round-trip is
    /// exercised.
    #[test]
    fn every_emitted_gutschrift_is_valid_bo4e() {
        // Both VAT statuses — they set different `Steuerart` values, which is
        // the branch most likely to reach for an enum BO4E does not define.
        for vat in [VatStatus::Regelbesteuerung, VatStatus::Kleinunternehmer] {
            let g = settlement_to_gutschrift(&feed_in_output(), vat, &header())
                .unwrap_or_else(|e| panic!("{vat:?}: settlement failed: {e}"));
            // The outbound gate: out-of-schema enums *and* the BO4E-stated
            // rules — net plus tax is gross, the Steuerbetrag breakdown sums to
            // the tax total, the positions sum to the net.
            mako_markt::bo4e::ensure_conformant(&g)
                .unwrap_or_else(|e| panic!("{vat:?}: emitted a document mako would refuse: {e}"));
        }
    }
}
