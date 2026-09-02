//! BO4E bridge — renders an [`crate::InvoiceDocument`] as a
//! `rubo4e::current::Rechnung`. Feature-gated behind `bo4e` so the core engine
//! stays rubo4e-free: settlements are computed in pure domain types, and only
//! consumers that store or dispatch a grid invoice (netzbilanzd, invoicd) enable
//! the feature. The rendered Rechnung carries:
//!
//! - `rechnungsnummer`, `rechnungsdatum`, `faelligkeitsdatum` — document facts
//! - per-position `mako:calculation_trace` ZusatzAttribut
//! - settlement-level `mako:legal_references` and (when present)
//!   `mako:settlement_warnings` ZusatzAttribute

use crate::rounding::RoundMoney;
use crate::{InvoiceDocument, QuantityUnit, SettlementResult};
use rubo4e::current::{
    Betrag, Menge, Mengeneinheit, NetznutzungRechnungsart, NetznutzungRechnungstyp, Preis,
    Rechnung, Rechnungsposition, Rechnungstyp, Steuerart, Steuerbetrag, Vorauszahlung,
    Waehrungscode, Zeitraum, ZusatzAttribut,
};

/// Parse the BDEW Artikelnummer that `grid-billing` decided on.
///
/// The decision — which code applies to which position in which settlement — is
/// domain logic and lives in `grid-billing`. This is only the lookup from its
/// codelist name into the BO4E enum, which `rubo4e` derives via `strum`.
///
/// **Important:** NNE Strom positions (PID 31002, NN-Rechnung) do NOT use classic
/// Artikelnummern since BK6-20-160 — for those, `grid-billing` emits no codelist
/// name and the `artikel_id` is populated from the `PreisblattNetznutzung` by
/// the rendering service. Source: BDEW Codeliste Artikelnummern und Artikel-ID
/// v5.6 (valid 01.09.2025).
#[must_use]
pub fn kind_to_artikelnummer(
    kind: crate::BillingPositionKind,
    settlement_type: crate::SettlementType,
) -> Option<rubo4e::current::BdewArtikelnummer> {
    // `from_wire`, not `FromStr`: both parse the same strings, but `FromStr`
    // comes from `strum`, whose derive also accepts `"UNKNOWN"` — the
    // catch-all's own spelling — and would hand back `Unknown` as if it were an
    // article number. `from_wire` refuses it, and needs no feature flag.
    kind.artikelnummer(settlement_type)
        .and_then(|name| rubo4e::current::BdewArtikelnummer::from_wire(name).ok())
}

/// The BO4E `rechnungstyp` for a settlement, when it is a Netznutzungsrechnung.
///
/// Not every settlement `grid-billing` produces is one. Grid usage (NNE) and
/// Mehr-/Mindermengen are; the rest are separate commercial documents that
/// happen to share this engine:
///
/// - **`MsbRechnung`** (31009) bills Messstellenbetrieb, not network use.
/// - **`GasAwhSperrung`** (31011) is explicitly a *Rechnung sonstige Leistung*.
/// - **`RedispatchKostenblatt`** is a §13a cost sheet toward the ÜNB.
/// - **`DezentraleEinspeisung`** (§18 StromNEV) is a bilateral credit with no
///   Prüfidentifikator at all.
///
/// Typing those as Netznutzungsrechnung would assert something the AHB does not,
/// so they are left untyped rather than approximated.
#[must_use]
pub fn rechnungstyp_for(settlement_type: crate::SettlementType) -> Option<Rechnungstyp> {
    use crate::SettlementType as S;
    matches!(
        settlement_type,
        S::NneAbschlag | S::NneStrom | S::NneGas | S::MmmStrom | S::MmmGas | S::MmmSelbstausstellt
    )
    .then_some(Rechnungstyp::Netznutzungsrechnung)
}

/// The BO4E `netznutzungrechnungsart` — who issued the invoice.
///
/// PID 31006 is the Mehrmenge leg issued by the receiving party itself, which is
/// exactly what *Selbstausgestellt* denotes. Everything else in this family is a
/// conventional Handelsrechnung from the network operator.
#[must_use]
pub fn netznutzungrechnungsart_for(
    settlement_type: crate::SettlementType,
) -> Option<NetznutzungRechnungsart> {
    use crate::SettlementType as S;
    match settlement_type {
        S::MmmSelbstausstellt => Some(NetznutzungRechnungsart::Selbstausgestellt),
        S::NneAbschlag | S::NneStrom | S::NneGas | S::MmmStrom | S::MmmGas => {
            Some(NetznutzungRechnungsart::Handelsrechnung)
        }
        _ => None,
    }
}

/// The BO4E `netznutzungrechnungstyp` — `IMD+7081` on the wire.
///
/// Two things decide it, and they are different in kind. The Mehr-/Mindermengen
/// family follows from the settlement itself. Everything else is the **billing
/// cadence**, which the settlement does not carry — an NNE settlement is the
/// same computation whether it is billed monthly, per Turnus, or as the
/// Abschlussrechnung that closes a year — so it comes from the document, where
/// the operator states it. An Abschlagsrechnung is the one cadence the
/// settlement does imply, because PID 31001 *is* that document.
///
/// Absent both, the field stays unset rather than guessing a rhythm nothing
/// supports.
#[must_use]
pub fn netznutzungrechnungstyp_for(
    settlement_type: crate::SettlementType,
    cadence: Option<crate::Rechnungscharakter>,
) -> Option<NetznutzungRechnungstyp> {
    use crate::Rechnungscharakter as C;
    use crate::SettlementType as S;

    if matches!(
        settlement_type,
        S::MmmStrom | S::MmmGas | S::MmmSelbstausstellt
    ) {
        return Some(NetznutzungRechnungstyp::Mehrmindermengenrechnung);
    }
    if settlement_type == S::NneAbschlag {
        return Some(NetznutzungRechnungstyp::Abschlagsrechnung);
    }
    cadence.map(|c| match c {
        C::Abschlagsrechnung => NetznutzungRechnungstyp::Abschlagsrechnung,
        C::Abschlussrechnung => NetznutzungRechnungstyp::Abschlussrechnung,
        C::Turnusrechnung => NetznutzungRechnungstyp::Turnusrechnung,
        C::Monatsrechnung => NetznutzungRechnungstyp::Monatsrechnung,
        C::Zwischenrechnung => NetznutzungRechnungstyp::Zwischenrechnung,
    })
}

/// Render a settlement, presented as an invoice, into a BO4E `Rechnung`.
///
/// Takes the document rather than the settlement: `rechnungsnummer`,
/// `rechnungsdatum` and `faelligkeitsdatum` are document facts, and the position
/// numbering is assigned here rather than carried through the calculation.
#[must_use]
pub fn into_rechnung(document: &InvoiceDocument) -> Rechnung {
    let invoice = &document.settlement;

    // Typed builders (rubo4e `builder` feature): omitted fields default to `None`,
    // and `setter(into)` accepts the value directly.
    let lz = Zeitraum::builder()
        .startdatum(invoice.period.from())
        .enddatum(invoice.period.to())
        .build();

    let positions: Vec<Rechnungsposition> = document
        .numbered_positions()
        .map(|(number, p)| {
            let einheit = match p.unit {
                QuantityUnit::Kwh => Mengeneinheit::Kwh,
                QuantityUnit::Kw => Mengeneinheit::Kw,
                // Reactive energy/power keep their own units — BO4E v202607
                // `Mengeneinheit` models them directly (KVARH/KVAR), so we no
                // longer collapse them into the kWh/kW buckets and lose fidelity.
                QuantityUnit::Kvarh => Mengeneinheit::Kvarh,
                QuantityUnit::Kvar => Mengeneinheit::Kvar,
                QuantityUnit::Monat => Mengeneinheit::Monat,
            };
            Rechnungsposition::builder()
                .positionsnummer(i64::from(number))
                .positionstext(p.text.clone())
                .artikelnummer(kind_to_artikelnummer(p.kind, invoice.settlement_type))
                // Artikel-ID (omitted) is resolved from the price sheet at
                // rendering time; the settlement states what was charged, not
                // how it is coded.
                .lieferungszeitraum(lz.clone())
                .positions_menge(Menge::builder().wert(p.quantity).einheit(einheit).build())
                .einzelpreis(Preis::builder().wert(p.unit_price_eur.round_kfm(6)).build())
                .gesamtpreis(Betrag::builder().wert(p.net_eur.round_kfm(5)).build())
                // The calculation trace travels with the position it explains.
                // grid-billing computes why each amount is what it is — the
                // inputs, the applied paragraphs, the tariff source — and that
                // is the only record of it: the engine's output is dropped once
                // this Rechnung is stored. §20 EnWG audits and LF disputes are
                // answered from here.
                .zusatz_attribute(trace_attribute(p))
                .build()
        })
        .collect();

    let settlement_type = invoice.settlement_type;
    let mut rechnung = Rechnung::builder()
        .rechnungsnummer(document.rechnungsnummer.clone())
        .rechnungsdatum(as_bo4e_timestamp(document.invoice_date))
        .faelligkeitsdatum(as_bo4e_timestamp(document.due_date))
        .rechnungsperiode(lz)
        .gesamtnetto(betrag(invoice.total_eur))
        // §14 Abs. 4 Nr. 8 UStG: the rate and the tax amount, or the note
        // saying why neither is stated. An invoice carrying only a net figure
        // gives its recipient no Vorsteuerabzug.
        .gesamtsteuer(betrag(invoice.steuer.steuer_eur))
        .gesamtbrutto(betrag(invoice.steuer.brutto_eur()))
        // What is actually owed: the gross, less the Abschläge already billed
        // and taxed. §14 Abs. 5 UStG taxes an Anzahlung when it is received, so
        // this deduction never touches `gesamtnetto` or `gesamtsteuer` — it
        // reduces the payment, not the supply. The INVOIC AHB puts it in the
        // Summenteil for the same reason (`SG50 MOA+113`).
        .zu_zahlen(betrag(zu_zahlen_eur(document)))
        .steuerbetraege(vec![steuerbetrag(invoice)])
        .rechnungspositionen(positions)
        // Every paragraph the settlement rests on, deduplicated across
        // positions, plus any warnings the engine raised.
        .zusatz_attribute(settlement_attributes(invoice))
        .build();

    // Typed after the builder rather than through it: each of these is `None`
    // for a settlement that is not a Netznutzungsrechnung, and the builder's
    // `setter(into)` would coerce an `Option` into `Some(None)`-shaped noise.
    rechnung.rechnungstyp = rechnungstyp_for(settlement_type);
    rechnung.netznutzungrechnungsart = netznutzungrechnungsart_for(settlement_type);
    rechnung.netznutzungrechnungstyp =
        netznutzungrechnungstyp_for(settlement_type, document.cadence);

    // The Sparte, on the field that carries it. NN-Rechnung Strom and Gas share
    // Prüfidentifikator 31002, so the Prüfidentifikator does not distinguish
    // them — `Rechnung.sparte` is the only place a receiver can read which
    // Sparte the document settles, and leaving it unset made a GasNEV invoice
    // indistinguishable from a StromNEV one on the wire.
    rechnung.sparte = Some(match invoice.sparte {
        crate::Sparte::Strom => rubo4e::current::Sparte::Strom,
        crate::Sparte::Gas => rubo4e::current::Sparte::Gas,
    });

    // A reversal says so in the fields `invoic-checker` stage 0 reads. A
    // Stornorechnung whose `ist_storno` is unset is not a Stornorechnung to any
    // receiver — and one that sets it without `original_rechnungsnummer` is
    // disputed on arrival (BK6-24-174 §5; Allgemeine Festlegungen §8).
    if invoice.status == crate::types::SettlementStatus::Reversal {
        rechnung.ist_storno = Some(true);
    }
    rechnung.original_rechnungsnummer = document.correction_of.clone();

    // Each Abschlag as its own Vorauszahlung, carrying the invoice number it was
    // billed under. The AHB requires the reference (`SG51 RFF+AFL`) and its date
    // (`SG51 DTM+3`) per deduction, not one lump sum: the counterparty
    // reconciles them against invoices it actually received, and a total it
    // cannot break down is a total it will dispute.
    if !document.abschlaege.is_empty() {
        rechnung.vorauszahlungen = Some(
            document
                .abschlaege
                .iter()
                .map(|a| Vorauszahlung {
                    betrag: Some(betrag(a.betrag_brutto_eur)),
                    // BO4E types this as a datetime; an invoice date carries no
                    // time of day, so it is pinned to midnight UTC.
                    datum: Some(a.rechnungsdatum.midnight().assume_utc()),
                    referenz: Some(a.rechnungsnummer.clone()),
                    ..Default::default()
                })
                .collect(),
        );
    }
    rechnung
}

/// What the recipient actually pays: the gross, less the Abschläge deducted.
fn zu_zahlen_eur(document: &InvoiceDocument) -> rust_decimal::Decimal {
    document.settlement.steuer.brutto_eur()
        - document
            .abschlaege
            .iter()
            .map(|a| a.betrag_brutto_eur)
            .sum::<rust_decimal::Decimal>()
}

/// A EUR amount as BO4E states one.
/// A BO4E date-only market value as the `date-time` the schema declares.
///
/// BDEW INVOIC transmits `rechnungsdatum` and `faelligkeitsdatum` as DTM
/// qualifier 102 — a bare `YYYYMMDD` — while BO4E types both `format: date-time`.
///
/// **Midnight UTC.** `Rechnung::rechnungsdatum_date()` reads the date in the
/// offset the payload carries, so `+00:00` reads back as the date that went in
/// and stays that date under any later normalisation; a `+01:00` midnight
/// becomes the previous day the moment someone converts it.
///
/// `None` for a year outside RFC 3339's `0000`–`9999`, which the field
/// serialises as. The field is optional and an invoice with no billing period
/// has no issue date, so it is omitted rather than made fatal — rejecting a
/// periodless invoice is the engine's job, not the serializer's.
fn as_bo4e_timestamp(date: time::Date) -> Option<time::OffsetDateTime> {
    (0..=9999)
        .contains(&date.year())
        .then(|| date.midnight().assume_utc())
}

fn betrag(wert: rust_decimal::Decimal) -> Betrag {
    Betrag::builder()
        .wert(wert)
        .waehrung(Waehrungscode::Eur)
        .build()
}

/// The settlement's tax as a BO4E `Steuerbetrag`.
///
/// One entry, because every settlement this crate produces is taxed uniformly:
/// a Netznutzungsrechnung, a Mehr-/Mindermengensaldo, a MSB-Rechnung and an AWH
/// invoice each carry a single treatment across all their positions. A mixed-rate
/// document would need one entry per rate, and would be a model change with a
/// reason rather than a shape to leave open.
fn steuerbetrag(invoice: &SettlementResult) -> Steuerbetrag {
    Steuerbetrag::builder()
        .steuerart(match invoice.steuer.kategorie {
            // `RCV` is BO4E's reverse-charge marker; everything else this crate
            // issues is ordinary Umsatzsteuer.
            crate::TaxCategory::ReverseCharge => Steuerart::Rcv,
            _ => Steuerart::Ust,
        })
        .steuersatz(invoice.steuer.satz_prozent)
        .basiswert(invoice.steuer.bemessungsgrundlage_eur)
        .steuerwert(invoice.steuer.steuer_eur)
        .waehrungscode(Waehrungscode::Eur)
        // The note §14a Abs. 5 Satz 2 UStG requires on a reverse-charge
        // invoice, alongside the paragraph the treatment rests on. BO4E models
        // no exemption-reason field on `Steuerbetrag` — EN 16931 carries it as
        // BT-120 on the same breakdown entry — so it travels as a ZusatzAttribut
        // on the entry it belongs to, rather than loose on the document.
        .zusatz_attribute(steuer_attribute(invoice))
        .build()
}

/// The legally required note and citation for a tax treatment.
fn steuer_attribute(invoice: &SettlementResult) -> Vec<ZusatzAttribut> {
    let mut attrs = vec![ZusatzAttribut {
        name: Some("mako:steuer_rechtsgrundlage".to_owned()),
        wert: Some(serde_json::Value::String(
            invoice.steuer.rechtsgrundlage.to_owned(),
        )),
        ..Default::default()
    }];
    if let Some(hinweis) = invoice.steuer.hinweis {
        attrs.push(ZusatzAttribut {
            name: Some("mako:umsatzsteuer_hinweis".to_owned()),
            wert: Some(serde_json::Value::String(hinweis.to_owned())),
            ..Default::default()
        });
    }
    attrs
}

/// Serialise a position's [`crate::CalculationTrace`] into a BO4E
/// `ZusatzAttribut`.
///
/// BO4E has no field for a calculation trace, and inventing one would break the
/// schema — a `ZusatzAttribut` is the sanctioned place for data a standard does
/// not model. Returns `None` when serialisation fails, so the position is still
/// emitted without its trace rather than dropped.
fn trace_attribute(p: &crate::SettlementPosition) -> Option<Vec<ZusatzAttribut>> {
    let trace = serde_json::to_value(&p.trace).ok()?;
    Some(vec![ZusatzAttribut {
        name: Some("mako:calculation_trace".to_owned()),
        wert: Some(trace),
        ..Default::default()
    }])
}

/// Attach the settlement's deduplicated legal citations and any warnings.
///
/// A warning records what the engine could not do — a levy omitted for want of a
/// published rate, a Konzessionsabgabe above the KAV ceiling. Dropping it leaves
/// an invoice that looks complete and is not.
fn settlement_attributes(invoice: &SettlementResult) -> Vec<ZusatzAttribut> {
    let mut attrs = vec![ZusatzAttribut {
        name: Some("mako:legal_references".to_owned()),
        wert: Some(serde_json::json!(invoice.all_legal_refs())),
        ..Default::default()
    }];
    if !invoice.warnings.is_empty()
        && let Ok(warnings) = serde_json::to_value(&invoice.warnings)
    {
        attrs.push(ZusatzAttribut {
            name: Some("mako:settlement_warnings".to_owned()),
            wert: Some(warnings),
            ..Default::default()
        });
    }
    attrs
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Present a settlement as a document, so the adapter can render it.
    fn as_document(settlement: crate::SettlementResult) -> crate::InvoiceDocument {
        crate::InvoiceDocument {
            settlement,
            pid: 31002,
            rechnungsnummer: "NNE-2026-001".to_owned(),
            correction_of: None,
            invoice_date: time::macros::date!(2026 - 02 - 15),
            due_date: time::macros::date!(2026 - 03 - 15),
            cadence: None,
            abschlaege: Vec::new(),
        }
    }

    fn sample_nne() -> crate::NneInput {
        crate::NneInput {
            blindarbeit: None,
            malo_id: "51238696012".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: crate::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: crate::ArbeitspreisModell::Einheitlich(crate::MengePreis {
                menge_kwh: rust_decimal::Decimal::from(1000),
                preis_ct_per_kwh: rust_decimal::Decimal::new(35, 1),
            }),
            leistungspreis: None,
            letztverbrauchergruppe: Default::default(),
            enfg_jahresvorverbrauch_kwh: None,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: crate::Sparte::Strom,
        }
    }

    /// §14 Abs. 4 Nr. 8 UStG: the rate, the tax and the gross reach the document.
    ///
    /// An invoice carrying only `gesamtnetto` is not a Rechnung — its recipient
    /// has no Vorsteuerabzug from it.
    #[test]
    fn the_tax_block_reaches_the_rechnung() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let netto = settlement.total_eur;
        let rechnung = into_rechnung(&as_document(settlement));

        let wert = |b: &Option<Betrag>| b.as_ref().and_then(|b| b.wert).expect("amount");
        let steuer = wert(&rechnung.gesamtsteuer);
        assert_eq!(wert(&rechnung.gesamtnetto), netto);
        assert_eq!(
            steuer,
            (netto * rust_decimal::Decimal::from(19) / rust_decimal::Decimal::from(100))
                .round_kfm(2)
        );
        assert_eq!(wert(&rechnung.gesamtbrutto), netto + steuer);
        assert_eq!(
            wert(&rechnung.zu_zahlen),
            netto + steuer,
            "what is actually owed is the gross"
        );

        let breakdown = rechnung.steuerbetraege.expect("a VAT breakdown");
        let entry = breakdown.first().expect("one entry");
        assert_eq!(entry.steuerart, Some(Steuerart::Ust));
        assert_eq!(entry.steuersatz, Some(rust_decimal::Decimal::from(19)));
        assert_eq!(entry.basiswert, Some(netto));
        assert_eq!(entry.steuerwert, Some(steuer));
    }

    /// A self-issued Mehrmenge leg renders as *Selbstausgestellt* under PID
    /// 31006.
    ///
    /// The Prüfidentifikator and the Rechnungsart are separate fields on the
    /// wire, and nothing downstream cross-checks them: an invoice stamped with
    /// PID 31006 while stating `Handelsrechnung` is a contradiction the AHB
    /// rejects and no local test caught, because the self-issued arm had no
    /// producer at all — the one caller reached for `settle_nne` and relabelled
    /// the document, which also made it a Netznutzungs- rather than a
    /// Mehrmindermengenrechnung.
    #[test]
    fn a_self_issued_mehrmenge_renders_as_selbstausgestellt_under_31006() {
        let settlement = crate::settle_mmm(&crate::MmmInput {
            malo_id: "51238696012".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: crate::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            sparte: crate::Sparte::Strom,
            actual_kwh: rust_decimal::Decimal::from(1000),
            profil_kwh: rust_decimal::Decimal::from(1200),
            mehr_preis_ct_per_kwh: rust_decimal::Decimal::from(5),
            minder_preis_ct_per_kwh: rust_decimal::Decimal::from(4),
            wiederverkaeufer: crate::Wiederverkaeuferstatus::KEINER,
            selbstausgestellt: true,
        })
        .expect("settle");

        assert_eq!(
            settlement.settlement_type,
            crate::SettlementType::MmmSelbstausstellt
        );
        assert_eq!(settlement.settlement_type.default_pid(), 31006);
        assert_eq!(
            netznutzungrechnungsart_for(settlement.settlement_type),
            Some(NetznutzungRechnungsart::Selbstausgestellt),
            "PID 31006 is the invoice the receiving party writes"
        );
        assert_eq!(
            netznutzungrechnungstyp_for(settlement.settlement_type, None),
            Some(NetznutzungRechnungstyp::Mehrmindermengenrechnung),
            "the type follows from the settlement, not from a billing cadence"
        );
    }

    /// A reverse-charged MMM states no tax and carries the §14a Abs. 5 wording.
    #[test]
    fn a_reverse_charged_supply_states_no_tax_and_says_why() {
        let settlement = crate::settle_mmm(&crate::MmmInput {
            malo_id: "51238696012".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: crate::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            sparte: crate::Sparte::Strom,
            actual_kwh: rust_decimal::Decimal::from(1200),
            profil_kwh: rust_decimal::Decimal::from(1000),
            mehr_preis_ct_per_kwh: rust_decimal::Decimal::from(5),
            minder_preis_ct_per_kwh: rust_decimal::Decimal::from(4),
            // Both parties hold §3g status — what electricity requires.
            wiederverkaeufer: crate::Wiederverkaeuferstatus::BEIDE,
            selbstausgestellt: false,
        })
        .expect("settle");
        let netto = settlement.total_eur;
        let rechnung = into_rechnung(&as_document(settlement));

        let wert = |b: &Option<Betrag>| b.as_ref().and_then(|b| b.wert).expect("amount");
        assert_eq!(wert(&rechnung.gesamtsteuer), rust_decimal::Decimal::ZERO);
        assert_eq!(
            wert(&rechnung.gesamtbrutto),
            netto,
            "under a reverse charge the gross is the net"
        );

        let breakdown = rechnung.steuerbetraege.expect("a VAT breakdown");
        let entry = breakdown.first().expect("one entry");
        assert_eq!(entry.steuerart, Some(Steuerart::Rcv));
        assert_eq!(entry.steuerwert, Some(rust_decimal::Decimal::ZERO));

        let note = entry
            .zusatz_attribute
            .as_ref()
            .expect("the entry carries its note")
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:umsatzsteuer_hinweis"))
            .and_then(|a| a.wert.as_ref())
            .and_then(serde_json::Value::as_str)
            .expect("the §14a Abs. 5 wording");
        assert_eq!(note, crate::umsatzsteuer::HINWEIS_REVERSE_CHARGE);
    }

    /// A reversal mirrors the tax it cancels, sign and all.
    #[test]
    fn a_reversal_mirrors_the_tax_it_cancels() {
        let original = crate::settle_nne(&sample_nne()).expect("settle");
        let reversal = crate::reverse(&original, crate::KorrekturGrund::Messwertkorrektur);
        assert_eq!(reversal.steuer.steuer_eur, -original.steuer.steuer_eur);
        assert_eq!(reversal.steuer.brutto_eur(), -original.steuer.brutto_eur());
        assert_eq!(reversal.steuer.kategorie, original.steuer.kategorie);

        // Net of the pair: nothing owed, and nothing owed in tax either.
        assert_eq!(
            original.total_eur + reversal.total_eur,
            rust_decimal::Decimal::ZERO
        );
        assert_eq!(
            original.steuer.steuer_eur + reversal.steuer.steuer_eur,
            rust_decimal::Decimal::ZERO
        );
    }

    /// An Abschlagsrechnung is one line, an amount, and its tax.
    ///
    /// INVOIC AHB 1.0b Änd-ID 26817: "Eine Abschlagsrechnung kann und muss genau
    /// eine Positionszeile enthalten", with `LIN DE1082` fixed at 1.
    #[test]
    fn an_abschlagsrechnung_is_exactly_one_position() {
        let settlement = crate::settle_abschlag(&crate::AbschlagInput {
            malo_id: "51238696012".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: crate::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            sparte: crate::Sparte::Strom,
            betrag_netto_eur: rust_decimal::Decimal::from(1000),
            grundlage: crate::AbschlagGrundlage::Vorjahresverbrauch,
        })
        .expect("settle");

        assert_eq!(settlement.settlement_type.default_pid(), 31001);
        assert_eq!(settlement.positions.len(), 1);
        assert_eq!(settlement.total_eur, rust_decimal::Decimal::from(1000));

        let rechnung = into_rechnung(&as_document(settlement));
        let positions = rechnung.rechnungspositionen.expect("positions");
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].positionsnummer, Some(1));
        assert_eq!(
            rechnung.netznutzungrechnungstyp,
            Some(NetznutzungRechnungstyp::Abschlagsrechnung)
        );
        // The tax is stated: an Anzahlung is taxed on receipt (§14 Abs. 5 UStG),
        // so the Abschlagsrechnung is a Rechnung like any other.
        assert_eq!(
            rechnung.gesamtsteuer.as_ref().and_then(|b| b.wert),
            Some(rust_decimal::Decimal::from(190))
        );
    }

    /// Abschläge reduce what is owed, never what was supplied.
    ///
    /// §14 Abs. 5 UStG taxes an Anzahlung when it is received, so the invoice
    /// that settles the period must not tax the same money again: the deduction
    /// belongs on `zuZahlen`, and `gesamtnetto` / `gesamtsteuer` stand.
    #[test]
    fn abschlaege_reduce_what_is_owed_not_what_was_supplied() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let netto = settlement.total_eur;
        let steuer = settlement.steuer.steuer_eur;
        let brutto = settlement.steuer.brutto_eur();

        let mut document = as_document(settlement);
        document.cadence = Some(crate::Rechnungscharakter::Abschlussrechnung);
        document.abschlaege = vec![
            crate::Abschlagsverrechnung {
                rechnungsnummer: "ABS-2026-000001".to_owned(),
                rechnungsdatum: time::macros::date!(2026 - 01 - 05),
                betrag_brutto_eur: rust_decimal::Decimal::from(100),
            },
            crate::Abschlagsverrechnung {
                rechnungsnummer: "ABS-2026-000002".to_owned(),
                rechnungsdatum: time::macros::date!(2026 - 02 - 05),
                betrag_brutto_eur: rust_decimal::Decimal::from(50),
            },
        ];

        let rechnung = into_rechnung(&document);
        let wert = |b: &Option<Betrag>| b.as_ref().and_then(|b| b.wert).expect("amount");

        assert_eq!(
            wert(&rechnung.gesamtnetto),
            netto,
            "the supply is unchanged"
        );
        assert_eq!(wert(&rechnung.gesamtsteuer), steuer, "and so is its tax");
        assert_eq!(wert(&rechnung.gesamtbrutto), brutto);
        assert_eq!(
            wert(&rechnung.zu_zahlen),
            brutto - rust_decimal::Decimal::from(150),
            "only what is owed moves"
        );
        assert_eq!(
            rechnung.netznutzungrechnungstyp,
            Some(NetznutzungRechnungstyp::Abschlussrechnung)
        );

        // Each deduction names the invoice it reconciles against — the AHB wants
        // `RFF+AFL` per Abschlag, not one lump sum the counterparty cannot break down.
        let vz = rechnung.vorauszahlungen.expect("prepayments");
        assert_eq!(vz.len(), 2);
        assert_eq!(vz[0].referenz.as_deref(), Some("ABS-2026-000001"));
        assert_eq!(vz[1].referenz.as_deref(), Some("ABS-2026-000002"));
    }

    /// Without Abschläge, what is owed is simply the gross.
    #[test]
    fn without_abschlaege_what_is_owed_is_the_gross() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let brutto = settlement.steuer.brutto_eur();
        let rechnung = into_rechnung(&as_document(settlement));
        assert_eq!(
            rechnung.zu_zahlen.as_ref().and_then(|b| b.wert),
            Some(brutto)
        );
        assert!(rechnung.vorauszahlungen.is_none());
    }

    /// A Gas settlement says so on `Rechnung.sparte`.
    ///
    /// NN-Rechnung Strom and Gas share Prüfidentifikator 31002, so the PID
    /// cannot tell them apart. This field is the only thing that can.
    #[test]
    fn the_sparte_reaches_the_rechnung() {
        for (sparte, want) in [
            (crate::Sparte::Strom, rubo4e::current::Sparte::Strom),
            (crate::Sparte::Gas, rubo4e::current::Sparte::Gas),
        ] {
            let mut input = sample_nne();
            input.sparte = sparte;
            let settlement = crate::settle_nne(&input).expect("settle");
            let rechnung = into_rechnung(&as_document(settlement));
            assert_eq!(rechnung.sparte, Some(want));
        }
    }

    /// A reversal renders as a Stornorechnung: `ist_storno` set, and the
    /// original invoice number in the field `invoic-checker` stage 0 reads.
    #[test]
    fn a_reversal_renders_as_a_stornorechnung() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let reversal = crate::reverse(&settlement, crate::KorrekturGrund::Messwertkorrektur);
        let mut document = as_document(reversal);
        document.rechnungsnummer = "NNE-2026-002".to_owned();
        document.correction_of = Some("NNE-2026-001".to_owned());

        let rechnung = into_rechnung(&document);
        assert_eq!(rechnung.ist_storno, Some(true));
        assert_eq!(
            rechnung.original_rechnungsnummer.as_deref(),
            Some("NNE-2026-001")
        );
        // The engine negated the amounts; the renderer must not undo that.
        let total = rechnung
            .gesamtnetto
            .as_ref()
            .and_then(|b| b.wert)
            .expect("total");
        assert!(total < rust_decimal::Decimal::ZERO, "storno total {total}");
    }

    /// An ordinary invoice is not marked as a Storno.
    #[test]
    fn an_initial_settlement_is_not_a_storno() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let rechnung = into_rechnung(&as_document(settlement));
        assert_eq!(rechnung.ist_storno, None);
        assert_eq!(rechnung.original_rechnungsnummer, None);
    }

    /// The calculation trace must survive into the rendered Rechnung.
    ///
    /// grid-billing computes, per position, the inputs it used, the paragraphs
    /// it applied and where the rate came from. That is the only record of it —
    /// a §20 EnWG audit or an LF dispute is answered from here.
    #[test]
    fn the_calculation_trace_reaches_the_rechnung() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let rechnung = into_rechnung(&as_document(settlement));

        let positions = rechnung.rechnungspositionen.expect("positions");
        let first = positions.first().expect("at least one position");
        let attrs = first
            .zusatz_attribute
            .as_ref()
            .expect("position carries its trace");
        let trace = attrs
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:calculation_trace"))
            .and_then(|a| a.wert.as_ref())
            .expect("mako:calculation_trace present");

        assert!(trace.get("explanation").is_some(), "{trace}");
        assert!(trace.get("legal_refs").is_some(), "{trace}");
        assert!(trace.get("input_quantity").is_some(), "{trace}");
        assert!(trace.get("gross_eur").is_some(), "{trace}");
    }

    /// The settlement's citations survive too, deduplicated.
    #[test]
    fn the_legal_references_reach_the_rechnung() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let rechnung = into_rechnung(&as_document(settlement));

        let refs = rechnung
            .zusatz_attribute
            .as_ref()
            .expect("settlement attributes")
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:legal_references"))
            .and_then(|a| a.wert.as_ref())
            .expect("mako:legal_references present");

        let list = refs.as_array().expect("an array of citations");
        assert!(!list.is_empty(), "a settlement always rests on something");
    }

    /// The two behaviours that drifted apart in the per-service copies must
    /// both hold: the document's `rechnungsnummer` is carried (invoicd had it,
    /// netzbilanzd dropped it) AND the settlement warnings are emitted
    /// (netzbilanzd had them, invoicd dropped them).
    #[test]
    fn rechnungsnummer_and_warnings_are_both_present() {
        let mut settlement = crate::settle_nne(&sample_nne()).expect("settle");
        settlement.warnings.push(crate::SettlementWarning {
            severity: crate::WarningSeverity::Warning,
            code: "TEST_WARNING",
            message: "levy omitted for want of a published rate".to_owned(),
        });
        let rechnung = into_rechnung(&as_document(settlement));

        assert_eq!(
            rechnung.rechnungsnummer.as_deref(),
            Some("NNE-2026-001"),
            "the document's rechnungsnummer must reach the Rechnung"
        );

        let warnings = rechnung
            .zusatz_attribute
            .as_ref()
            .expect("settlement attributes")
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:settlement_warnings"))
            .and_then(|a| a.wert.as_ref())
            .expect("mako:settlement_warnings present");
        let list = warnings.as_array().expect("an array of warnings");
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].get("code").and_then(|c| c.as_str()),
            Some("TEST_WARNING")
        );
    }

    /// A settlement without warnings emits no empty warnings attribute.
    #[test]
    fn no_warnings_attribute_when_clean() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        assert!(settlement.warnings.is_empty(), "fixture must be clean");
        let rechnung = into_rechnung(&as_document(settlement));
        let has_warnings = rechnung
            .zusatz_attribute
            .as_ref()
            .expect("settlement attributes")
            .iter()
            .any(|a| a.name.as_deref() == Some("mako:settlement_warnings"));
        assert!(!has_warnings);
    }

    // ── Outbound BO4E conformance ────────────────────────────────────────────

    /// Every `Rechnung` this adapter emits must round-trip with no `Unknown`.
    ///
    /// `into_rechnung` is what reaches the LF over AS4. An out-of-schema enum
    /// in it costs the recipient differently depending on what they run:
    /// `rubo4e` decodes it to its `Unknown` catch-all and says nothing, while
    /// go-bo4e (`invalid <Enum> %q`, no catch-all) and BO4E-python (a pydantic
    /// `ValidationError`) reject the **whole document**. So the failure is
    /// either a settlement position the recipient cannot classify or an invoice
    /// they cannot parse — and this test is what stops either.
    ///
    /// The cases below are the branches that differ in which BO4E enums get
    /// set: the Sparte, the tax treatment (`Steuerart::Ust` vs `Rcv`), the
    /// reversal (`Rechnungstyp::Stornorechnung`), and the settlement types this
    /// adapter deliberately leaves untyped.
    #[test]
    fn every_emitted_rechnung_is_valid_bo4e() {
        let mut cases: Vec<(&str, crate::InvoiceDocument)> = Vec::new();

        let strom = crate::settle_nne(&sample_nne()).expect("settle strom");
        cases.push(("nne strom", as_document(strom.clone())));

        let mut gas_input = sample_nne();
        gas_input.sparte = crate::Sparte::Gas;
        let gas = crate::settle_nne(&gas_input).expect("settle gas");
        cases.push(("nne gas", as_document(gas)));

        // A reversal sets `Rechnungstyp::Stornorechnung` and negates the tax.
        let mut storno = as_document(strom);
        storno.correction_of = Some("NNE-2026-000".to_owned());
        cases.push(("storno", storno));

        for (label, doc) in cases {
            // The outbound gate: out-of-schema enums *and* the BO4E-stated
            // rules. mako refuses a received document that breaks these, so a
            // Netznutzungsrechnung it issues must not break them either — the
            // counterparty runs the same arithmetic (`invoic-checker` stage 3)
            // and disputes what does not reconcile.
            let rechnung = into_rechnung(&doc);
            mako_markt::bo4e::ensure_conformant(&rechnung)
                .unwrap_or_else(|e| panic!("{label}: emitted a Rechnung mako would refuse: {e}"));

            let json = serde_json::to_value(&rechnung)
                .unwrap_or_else(|e| panic!("{label}: not serialisable: {e}"));
            let back: Rechnung = serde_json::from_value(json)
                .unwrap_or_else(|e| panic!("{label}: does not round-trip: {e}"));
            mako_markt::bo4e::ensure_conformant(&back)
                .unwrap_or_else(|e| panic!("{label}: the JSON form would be refused: {e}"));
        }
    }
}

#[cfg(test)]
mod artikelnummer_bridge_tests {
    use crate::{BillingPositionKind as K, SettlementType as ST};

    /// Every codelist name grid-billing emits must parse into the BO4E enum.
    ///
    /// The two are joined by a string, so a typo on either side degrades
    /// silently: `from_str` returns `Err`, the article number becomes `None`,
    /// and the INVOIC ships without it. This is the test that makes the seam
    /// safe.
    #[test]
    fn every_emitted_codelist_name_parses() {
        let kinds = [
            K::NneArbeit,
            K::NneArbeitHt,
            K::NneArbeitNt,
            K::NneArbeitModul1,
            K::NneArbeitModul3,
            K::NneLeistung,
            K::NneGasGrundpreis,
            K::Konzessionsabgabe,
            K::Mehrmenge,
            K::Mindermenge,
            K::MsbGrundgebuehr,
            K::Messdienstleistung,
            K::GasAwhSperrung,
            K::GasAwhEntsprrung,
            K::GasAwhSonstige,
            K::Blindmehrarbeit,
            K::Sect19StromNevUmlage,
            K::OffshoreNetzumlage,
            K::KwkgUmlage,
            K::DezentraleEinspeisung,
            K::Sect19IndividuellesEntgelt,
            K::GasKapazitaetsentgelt,
        ];
        let types = [
            ST::NneStrom,
            ST::NneGas,
            ST::MmmStrom,
            ST::MmmGas,
            ST::MsbRechnung,
            ST::GasAwhSperrung,
            ST::DezentraleEinspeisung,
        ];

        for kind in kinds {
            for st in types {
                let Some(name) = kind.artikelnummer(st) else {
                    continue; // carries an Artikel-ID instead
                };
                assert!(
                    super::kind_to_artikelnummer(kind, st).is_some(),
                    "grid-billing emits {name:?} for {kind:?}/{st:?}, \
                     but rubo4e cannot parse it"
                );
            }
        }
    }

    /// Gas NNE keeps the classic code; Strom NNE carries an Artikel-ID instead.
    ///
    /// BK6-20-160 changed Strom only, and getting this backwards puts the wrong
    /// identifier on every grid invoice.
    #[test]
    fn strom_and_gas_nne_are_coded_differently() {
        assert_eq!(K::NneArbeit.artikelnummer(ST::NneGas), Some("WIRKARBEIT"));
        assert_eq!(K::NneArbeit.artikelnummer(ST::NneStrom), None);
    }
}

#[cfg(test)]
mod rechnungstyp_tests {
    use super::*;
    use crate::SettlementType as S;

    #[test]
    fn grid_usage_and_mmm_are_netznutzungsrechnungen() {
        for st in [
            S::NneStrom,
            S::NneGas,
            S::MmmStrom,
            S::MmmGas,
            S::MmmSelbstausstellt,
        ] {
            assert_eq!(
                rechnungstyp_for(st),
                Some(Rechnungstyp::Netznutzungsrechnung),
                "{st:?} bills network use and must be typed as such"
            );
        }
    }

    #[test]
    fn non_grid_settlements_are_left_untyped() {
        // Typing these as Netznutzungsrechnung would assert something the AHB
        // does not. `None` is the honest answer, not a gap to fill later.
        for st in [
            S::MsbRechnung,
            S::GasAwhSperrung,
            S::RedispatchKostenblatt,
            S::DezentraleEinspeisung,
        ] {
            assert_eq!(
                rechnungstyp_for(st),
                None,
                "{st:?} is not a Netznutzungsrechnung"
            );
            assert_eq!(netznutzungrechnungsart_for(st), None);
            assert_eq!(netznutzungrechnungstyp_for(st, None), None);
        }
    }

    #[test]
    fn only_pid_31006_is_self_issued() {
        assert_eq!(
            netznutzungrechnungsart_for(S::MmmSelbstausstellt),
            Some(NetznutzungRechnungsart::Selbstausgestellt)
        );
        for st in [S::NneStrom, S::NneGas, S::MmmStrom, S::MmmGas] {
            assert_eq!(
                netznutzungrechnungsart_for(st),
                Some(NetznutzungRechnungsart::Handelsrechnung),
                "{st:?} is issued by the network operator"
            );
        }
    }

    #[test]
    fn the_cadence_field_is_only_set_where_it_is_known() {
        // Mehr-/Mindermengen has a dedicated code, so it can be stated.
        for st in [S::MmmStrom, S::MmmGas, S::MmmSelbstausstellt] {
            assert_eq!(
                netznutzungrechnungstyp_for(st, None),
                Some(NetznutzungRechnungstyp::Mehrmindermengenrechnung)
            );
        }
        // NNE has none: Turnus/Monats/Abschlag describe billing rhythm, which a
        // settlement type does not carry. Emitting a guess would put an
        // unsupported claim about billing cadence on the wire.
        for st in [S::NneStrom, S::NneGas] {
            assert_eq!(
                netznutzungrechnungstyp_for(st, None),
                None,
                "{st:?} has no cadence to state"
            );
        }
    }
}
