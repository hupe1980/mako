//! The ESA's own invoice check — `E_0264` / `E_0266`, driven off a BO4E
//! [`Rechnung`].
//!
//! [`crate::check::InvoicCheckEngine::check_esa_rechnung`] answers „is this
//! invoice plausible" in mako's own vocabulary ([`crate::Finding`]). That is
//! the right shape for an operator queue and the wrong one for the market: the
//! answer an ESA owes its MSB is a REMADV carrying published **`E_0264`
//! Antwortcodes**, one per defect, each naming the Ebene and — on the
//! Positionsebene — the Positionsnummer.
//!
//! This module is the bridge. It maps what a `Rechnung` can answer on its own
//! onto the tree's Prüfschritte, takes the rest as [`EmpfaengerFakten`] from
//! the caller's records, and returns the
//! [`RechnungsAntwort`] — which also knows which REMADV Prüfidentifikator it
//! must ride.
//!
//! # What the invoice cannot answer about itself
//!
//! Six Prüfschritte need facts no INVOIC carries: whether the invoice
//! references an order this ESA placed (40), whether the Rechnungsnummer is a
//! repeat (50), whether the billed service was actually performed (310),
//! whether the Artikel-ID was billed before (370), whether the § 14 Abs. 4 UStG
//! content is complete (10), and — on the second round — whether the MSB's
//! COMDIS rebutted the objections (`E_0266` Prüfschritt 1). Every one of them
//! is `Option` or explicit in [`EmpfaengerFakten`], and an unknown answer
//! never refuses: a rejection is a binding statement to the market.
//!
//! # Sources
//!
//! - *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 8.27.1 / 8.27.3
//! - WiM Strom Teil 2 Kap. 4.5, INVOIC AHB 1.0b, REMADV AHB 1.0a § 3.1.2

use std::collections::HashMap;

use mako_pruefung::HolidayCalendar;
use mako_pruefung::rechnung::{
    PositionsFakten, RechnungsAntwort, RechnungsFakten, RechnungsFamilie, Steuersatzpruefung,
    StornoAntwort, UrsprungsAntwort, Zeitraum,
};
use rubo4e::convenience::{BetragExt as _, MengeExt as _, PreisExt as _};
use rubo4e::current::{Rechnung, Rechnungsposition};
use rust_decimal::Decimal;

use crate::amount::EuroAmount;
use crate::check::CheckConfig;

/// What the ESA's own records contribute — the Prüfschritte an INVOIC cannot
/// answer about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmpfaengerFakten {
    /// The day the invoice arrived — the **ÜT of the AS4-Zustellquittung**,
    /// not the local ingest timestamp (WiM Teil 1). Prüfschritte 20 and 70.
    pub eingangsdatum: Option<time::Date>,
    /// Prüfschritt 10 — does the invoice carry the § 14 Abs. 4 UStG content?
    /// `None` when nothing assessed it.
    pub ustg_konform: Option<bool>,
    /// Prüfschritt 40 — does it reference an ORDERS 17007 this ESA placed?
    /// WiM Teil 2 UC 4.5.1: „Eine Rechnung referenziert auf die zugrunde-
    /// liegende Bestellung." Defaults to `true`: refusing every invoice
    /// because nothing looked the order up would stop the whole process.
    pub bestellung_bekannt: bool,
    /// Prüfschritt 50 — has this Rechnungsnummer been seen from this MSB?
    pub rechnungsnummer_bereits_verwendet: bool,
    /// Prüfschritt 310, per Positionsnummer — was the billed service actually
    /// performed? Absent means unknown, which never refuses.
    pub leistung_erbracht: HashMap<u16, bool>,
    /// Prüfschritt 370, per Positionsnummer — the Rechnungsnummer of an
    /// earlier, not-cancelled invoice that already billed this Artikel-ID for
    /// the same period.
    pub bereits_abgerechnet_in: HashMap<u16, String>,
    /// **Second round only** — could the MSB's COMDIS 29001 rebut every
    /// objection? `A25` in `E_0266`, `AC1` in `E_0276`/`E_0277`; the family
    /// picks which.
    pub einwaende_entkraeftet: Option<bool>,
    /// **Preisblatt-B families only**, Prüfschritt 80 — does the recipient hold
    /// the Preisblatt version the invoice bills against? The sheet is
    /// „Preisblatt Technik" in the PRICAT 27002.
    ///
    /// `None` never refuses, and the ESA family ignores it: an ESA has no
    /// Preisblatt at all.
    pub preisblatt_version_gueltig: Option<bool>,
    /// **Preisblatt-B families only**, Prüfschritt 90 — the Rechnungsnummer of
    /// an earlier accepted, not-cancelled invoice that already settled this
    /// Abrechnungszeitraum. The code's Hinweis makes naming it part of the
    /// answer.
    pub zeitraum_bereits_abgerechnet_in: Option<String>,
}

impl EmpfaengerFakten {
    /// The neutral starting point: nothing known, nothing refused on that
    /// account.
    #[must_use]
    pub fn neu(eingangsdatum: time::Date) -> Self {
        Self {
            eingangsdatum: Some(eingangsdatum),
            bestellung_bekannt: true,
            ..Self::default()
        }
    }
}

/// Run `E_0264` over an inbound INVOIC 31009.
///
/// `agreed` is the accepted QUOTES 15003 as `(Artikel-ID, Preis)` pairs — the
/// ESA's whole price basis, since § 35 MsbG leaves the Entgelt for a
/// Zusatzleistung to be agreed per request and no `PreisblattMessung` covers a
/// Kapitel-4.6 Messprodukt.
///
/// An **empty** `agreed` is not an empty offer: it means mako holds no record
/// of one. Prüfschritte 300, 320 and 500 then answer „not comparable" rather
/// than „wrong", because disputing on a gap in mako's own books sends a REMADV
/// refusing a correct invoice.
#[must_use]
pub fn antwort_auf_rechnung(
    familie: RechnungsFamilie,
    rechnung: &Rechnung,
    agreed: &[(String, EuroAmount)],
    fakten: &EmpfaengerFakten,
    config: &CheckConfig,
    cal: HolidayCalendar,
) -> RechnungsAntwort {
    let input = build(rechnung, agreed, fakten, config);
    mako_pruefung::rechnung::pruefe_rechnung(familie, &input, cal)
}

/// Run `E_0266` — the second round, after the MSB answered a Nicht-Zahlungsavis
/// with a COMDIS 29001 claiming its invoice was correct.
///
/// Identical to [`antwort_auf_rechnung`] except for Prüfschritt 1, which reads
/// [`EmpfaengerFakten::einwaende_entkraeftet`]. That question is the ESA's
/// judgement of the MSB's prose, so it has no automatic answer: `None` passes
/// it and the remaining Prüfschritte decide.
#[must_use]
pub fn antwort_auf_erneute_rechnung(
    familie: RechnungsFamilie,
    rechnung: &Rechnung,
    agreed: &[(String, EuroAmount)],
    fakten: &EmpfaengerFakten,
    config: &CheckConfig,
    cal: HolidayCalendar,
) -> RechnungsAntwort {
    let input = build(rechnung, agreed, fakten, config);
    mako_pruefung::rechnung::pruefe_rechnung_erneut(familie, &input, cal)
}

fn build(
    rechnung: &Rechnung,
    agreed: &[(String, EuroAmount)],
    fakten: &EmpfaengerFakten,
    config: &CheckConfig,
) -> RechnungsFakten {
    let rechnungsdatum = rechnung.rechnungsdatum_date();
    // The ÜT is the Frist anchor everywhere in WiM, so a caller that has one
    // passes it. Falling back to the Rechnungsdatum makes Prüfschritt 20
    // vacuous rather than wrong — it never refuses an invoice for arriving
    // before it was written.
    let eingangsdatum = fakten
        .eingangsdatum
        .or(rechnungsdatum)
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

    let leistungszeitraum = rechnung.billing_period().map(|p| Zeitraum {
        von: *p.start(),
        bis: *p.end(),
    });

    let preis_je_artikel: HashMap<&str, EuroAmount> = agreed
        .iter()
        .map(|(id, preis)| (id.as_str(), *preis))
        .collect();

    let positionen: Vec<PositionsFakten> = rechnung
        .rechnungspositionen
        .iter()
        .flatten()
        .map(|pos| position(pos, &preis_je_artikel, fakten, config))
        .collect();

    // Prüfschritt 500 — Artikel-IDs the offer priced that this invoice omits.
    // Only meaningful when an offer is on record at all.
    let billed: Vec<&str> = positionen
        .iter()
        .filter_map(|p| p.artikel_id.as_deref())
        .collect();
    let fehlende_artikel_ids = agreed
        .iter()
        .map(|(id, _)| id)
        .filter(|id| !billed.contains(&id.as_str()))
        .cloned()
        .collect();

    RechnungsFakten {
        einwaende_entkraeftet: fakten.einwaende_entkraeftet,
        ustg_konform: fakten.ustg_konform,
        rechnungsdatum: rechnungsdatum.unwrap_or(eingangsdatum),
        eingangsdatum,
        leistungszeitraum,
        bestellung_bekannt: fakten.bestellung_bekannt,
        rechnungsnummer_bereits_verwendet: fakten.rechnungsnummer_bereits_verwendet,
        faelliger_betrag_nicht_negativ: faelliger_betrag_nicht_negativ(rechnung),
        zahlungsziel: rechnung.faelligkeitsdatum_date(),
        // Read straight through: the walk itself ignores both for the ESA
        // family, whose trees publish no such Prüfschritt.
        preisblatt_version_gueltig: fakten.preisblatt_version_gueltig,
        zeitraum_bereits_abgerechnet_in: fakten.zeitraum_bereits_abgerechnet_in.clone(),
        sonstiger_kopffehler: None,
        positionen,
        fehlende_artikel_ids,
        steuersaetze: steuersaetze(rechnung, config),
        rechnungsbetrag_stimmt: rechnungsbetrag_stimmt(rechnung, config),
        sonstiger_summenfehler: None,
    }
}

fn position(
    pos: &Rechnungsposition,
    preis_je_artikel: &HashMap<&str, EuroAmount>,
    fakten: &EmpfaengerFakten,
    config: &CheckConfig,
) -> PositionsFakten {
    let nr = u16::try_from(pos.positionsnummer.unwrap_or(0)).unwrap_or(0);
    let artikel_id = pos.artikel_id.clone().filter(|a| !a.is_empty());
    // Prüfschritt 300 — with no offer on record nothing can be compared
    // against it, so the position is not refused for naming an Artikel-ID mako
    // never saw priced.
    let artikel_id_aus_bestellung = preis_je_artikel.is_empty()
        || artikel_id
            .as_deref()
            .is_some_and(|id| preis_je_artikel.contains_key(id));

    let invoiced = pos.einzelpreis.wert_decimal().and_then(euro);
    // Prüfschritt 320 — the price against the offer valid for this position.
    let preis_wie_angebot = match (invoiced, artikel_id.as_deref()) {
        (Some(actual), Some(id)) => preis_je_artikel
            .get(id)
            .map(|expected| actual.within_tolerance_ppm(*expected, config.tariff_tolerance_ppm)),
        _ => None,
    };

    PositionsFakten {
        positionsnummer: nr,
        artikel_id,
        artikel_id_aus_bestellung,
        leistung_erbracht: fakten.leistung_erbracht.get(&nr).copied(),
        preis_wie_angebot,
        // Prüfschritt 330 needs the rate valid for the period, which is a
        // legal fact about the Leistungszeitraum rather than a property of the
        // document. `crate::check` runs the arithmetic on it; the *validity*
        // of the rate stays unassessed here rather than guessed.
        steuersatz_korrekt: None,
        zeitraum: match (pos.lieferung_von_date(), pos.lieferung_bis_date()) {
            (Some(von), Some(bis)) => Some(Zeitraum { von, bis }),
            (Some(tag), None) | (None, Some(tag)) => Some(Zeitraum::tag(tag)),
            (None, None) => None,
        },
        bereits_abgerechnet_in: fakten.bereits_abgerechnet_in.get(&nr).cloned(),
        rechenfehler: rechenfehler(pos, config),
        sonstiger_fehler: None,
    }
}

/// Prüfschritt 420 — `menge × einzelpreis` against the stated `gesamtpreis`.
///
/// A position that states no quantity or no unit price has nothing to
/// recompute, which is not a Rechenfehler: the ESA Betriebspreis is billed per
/// Tag and the Einrichtungspreis once per Stück, and either may arrive as a
/// bare Gesamtpreis.
fn rechenfehler(pos: &Rechnungsposition, config: &CheckConfig) -> bool {
    let (Some(menge), Some(preis), Some(gesamt)) = (
        pos.positions_menge.wert_decimal(),
        pos.einzelpreis.wert_decimal(),
        pos.gesamtpreis.wert_decimal(),
    ) else {
        return false;
    };
    let (Some(computed), Some(stated)) = (euro(menge * preis), euro(gesamt)) else {
        return false;
    };
    !computed.within_tolerance_ppm(stated, config.arithmetic_tolerance_ppm)
}

/// Prüfschritt 60 — „Ist der fällige Betrag ≥ Null?"
///
/// A negative total is a credit, and an MSB Abrechnung has no credits: a
/// correction is a Stornorechnung plus a new invoice, which is `E_0267`'s
/// business. Read from `zuZahlen` where the invoice states one, else from
/// `gesamtbrutto`; an invoice stating neither is a § 14 UStG defect that
/// Prüfschritt 10 owns, so this one passes it.
fn faelliger_betrag_nicht_negativ(rechnung: &Rechnung) -> bool {
    rechnung
        .zu_zahlen
        .wert_decimal()
        .or_else(|| rechnung.gesamtbrutto.wert_decimal())
        .is_none_or(|w| w >= Decimal::ZERO)
}

/// Prüfschritte 510/520, per (Steuersatz, Steuerkategorie) of `SG52 TAX`.
///
/// The `steuerbetraege` breakdown is what the recipient computes its
/// Vorsteuerabzug from (§ 14 Abs. 4 Nr. 8, § 15 Abs. 1 UStG), so both halves
/// are checked against the positions carrying that rate — not against the
/// document total, which is Prüfschritt 540.
fn steuersaetze(rechnung: &Rechnung, config: &CheckConfig) -> Vec<Steuersatzpruefung> {
    // `total_tolerance_ppm`, not `arithmetic_`: these are Summen-level checks,
    // and `InvoicCheckEngine` asks the same questions under that knob. Reading
    // a different one lets the plausibility report and the REMADV disagree
    // about one invoice the moment an operator tunes either — the engine would
    // record a `SteuerMismatch` Dispute while the walk sent a Zahlungsavis.
    let tol = config.total_tolerance_ppm;
    rechnung
        .steuerbetraege
        .iter()
        .flatten()
        .map(|tax| {
            let satz = tax.steuersatz.unwrap_or_default();
            // Σ der Positionen mit genau diesem Steuersatz.
            let basis: Decimal = rechnung
                .rechnungspositionen
                .iter()
                .flatten()
                .filter(|p| {
                    p.steuerbetrag
                        .as_ref()
                        .and_then(|t| t.steuersatz)
                        .is_some_and(|s| s == satz)
                })
                .filter_map(|p| p.gesamtpreis.wert_decimal())
                .sum();

            let stated_basis = tax.basiswert;
            let stated_tax = tax.steuerwert;
            // With no position carrying the rate there is nothing to compare
            // against; the breakdown stands rather than being refused on a sum
            // of zero positions.
            let vergleichbar = rechnung
                .rechnungspositionen
                .iter()
                .flatten()
                .any(|p| p.steuerbetrag.as_ref().and_then(|t| t.steuersatz).is_some());

            Steuersatzpruefung {
                steuersatz: satz.to_string(),
                steuerkategorie: tax
                    .steuerart
                    .map_or_else(|| "-".to_owned(), |a| format!("{a:?}")),
                besteuerungsgrundlage_stimmt: !vergleichbar
                    || stated_basis.is_none_or(|s| decimals_match(s, basis, tol)),
                steuerbetrag_stimmt: !vergleichbar
                    || stated_tax.is_none_or(|s| {
                        decimals_match(s, basis * satz / Decimal::ONE_HUNDRED, tol)
                    }),
            }
        })
        .collect()
}

/// Prüfschritt 540 — Rechnungsbetrag = Besteuerungsgrundlage + Steuerbetrag.
///
/// Reads `total_tolerance_ppm`, the same knob `InvoicCheckEngine` uses for the
/// identical cross-check — see [`steuersaetze`].
fn rechnungsbetrag_stimmt(rechnung: &Rechnung, config: &CheckConfig) -> bool {
    let (Some(netto), Some(steuer), Some(brutto)) = (
        rechnung.gesamtnetto.wert_decimal(),
        rechnung.gesamtsteuer.wert_decimal(),
        rechnung.gesamtbrutto.wert_decimal(),
    ) else {
        // An invoice that states no totals has nothing to reconcile; that is a
        // § 14 UStG defect (Prüfschritt 10), not a Summenfehler.
        return true;
    };
    decimals_match(brutto, netto + steuer, config.total_tolerance_ppm)
}

fn decimals_match(a: Decimal, b: Decimal, tol_ppm: u32) -> bool {
    match (euro(a), euro(b)) {
        (Some(x), Some(y)) => x.within_tolerance_ppm(y, tol_ppm),
        _ => a == b,
    }
}

fn euro(d: Decimal) -> Option<EuroAmount> {
    crate::amount::euro_from_decimal(d)
}

// ── E_0267 — the Stornorechnung, and whether it needs an answer ─────────────

/// What the ESA's own records say about a Stornorechnung and the invoice it
/// cancels.
///
/// Every field is a Prüfschritt `E_0267` asks, and most of them compare the
/// Storno against the **original** — which only the recipient's own books hold,
/// since a 31004 names its predecessor by Rechnungsnummer and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StornoEmpfaengerFakten {
    /// Prüfschritt 10 — is the invoice being cancelled on the ESA's books?
    pub ursprungsrechnung_bekannt: bool,
    /// Prüfschritt 15 — is the Storno's own Rechnungsnummer already on file
    /// from this Rechnungssteller?
    pub rechnungsnummer_bereits_verwendet: bool,
    /// Prüfschritt 17 — § 14 Abs. 4 UStG. `None` is „not assessed".
    pub ustg_konform: Option<bool>,
    /// Prüfschritt 20 — was the original already cancelled?
    pub bereits_storniert: bool,
    /// Prüfschritt 30 — same Rechnungstyp as the original?
    pub rechnungstyp_identisch: bool,
    /// Prüfschritt 40 — same Abrechnungszeitraum / Ausführungsdatum?
    pub zeitraum_identisch: bool,
    /// Prüfschritt 50 — does every `MOA` amount equal the original's × (−1)?
    pub betraege_negiert_identisch: bool,
    /// Prüfschritte 70/80 — **how the original was answered**, which is what
    /// decides whether an answer is owed at all.
    pub ursprungsantwort: UrsprungsAntwort,
}

impl StornoEmpfaengerFakten {
    /// The neutral reading of a Storno whose original the ESA paid: everything
    /// matches, so the Storno is confirmed.
    ///
    /// Deliberately not [`Default`]: `ursprungsantwort` has no safe default —
    /// guessing `Zugestimmt` sends a REMADV the MSB may not be waiting on, and
    /// guessing `Abgelehnt` withholds one it is.
    #[must_use]
    pub fn neu(ursprungsantwort: UrsprungsAntwort) -> Self {
        Self {
            ursprungsrechnung_bekannt: true,
            rechnungsnummer_bereits_verwendet: false,
            ustg_konform: None,
            bereits_storniert: false,
            rechnungstyp_identisch: true,
            zeitraum_identisch: true,
            betraege_negiert_identisch: true,
            ursprungsantwort,
        }
    }
}

/// Run `E_0267` over an inbound Stornorechnung (INVOIC 31004).
///
/// # Three outcomes, and the quiet one is the common one
///
/// The tree's name is „Prüfen, **ob** Antwort auf Stornierung erforderlich".
/// Prüfschritt 70 confirms a Storno whose original the ESA had paid (REMADV
/// 33001); Prüfschritte 10–60 refuse it (33002); and Prüfschritt 80 sends
/// **nothing** — the original was itself refused with a Nicht-Zahlungsavis, or
/// was never answered, so the Storno needs no answer either.
///
/// Collapsing that into accept/reject answers a message the MSB is not waiting
/// on, which is why [`StornoAntwort`] has three variants rather than two.
#[must_use]
pub fn antwort_auf_stornorechnung(
    familie: RechnungsFamilie,
    rechnung: &Rechnung,
    fakten: &StornoEmpfaengerFakten,
) -> StornoAntwort {
    mako_pruefung::rechnung::pruefe_stornorechnung(
        familie,
        &mako_pruefung::rechnung::StornoFakten {
            ursprungsrechnung_bekannt: fakten.ursprungsrechnung_bekannt
            // A Storno that names no original cannot be matched to one, which
            // is Prüfschritt 10 by itself.
            && rechnung
                .original_rechnungsnummer
                .as_deref()
                .is_some_and(|n| !n.is_empty()),
            rechnungsnummer_bereits_verwendet: fakten.rechnungsnummer_bereits_verwendet,
            ustg_konform: fakten.ustg_konform,
            bereits_storniert: fakten.bereits_storniert,
            rechnungstyp_identisch: fakten.rechnungstyp_identisch,
            zeitraum_identisch: fakten.zeitraum_identisch,
            betraege_negiert_identisch: fakten.betraege_negiert_identisch,
            sonstiger_fehler: None,
            ursprungsantwort: fakten.ursprungsantwort,
        },
    )
}

#[cfg(test)]
mod tests {
    /// These cases were written for the ESA relationship; the shared walk is
    /// exercised across every family in `mako_pruefung::rechnung`.
    use mako_pruefung::rechnung::ESA as ESA_F;

    use super::*;
    use rubo4e::current::{Betrag, Menge, Mengeneinheit, Preis, Steuerbetrag, Waehrungscode};
    use time::macros::date;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    fn dec(s: &str) -> Decimal {
        s.parse().expect("decimal")
    }

    fn betrag(s: &str) -> Betrag {
        Betrag {
            wert: Some(dec(s)),
            waehrung: Some(Waehrungscode::Eur),
            ..Betrag::default()
        }
    }

    fn pos(nr: i64, artikel: &str, einzel: &str, menge: &str, gesamt: &str) -> Rechnungsposition {
        Rechnungsposition {
            positionsnummer: Some(nr),
            artikel_id: Some(artikel.to_owned()),
            positions_menge: Some(Menge {
                wert: Some(dec(menge)),
                einheit: Some(Mengeneinheit::Tag),
                ..Menge::default()
            }),
            einzelpreis: Some(Preis {
                wert: Some(dec(einzel)),
                ..Preis::default()
            }),
            gesamtpreis: Some(betrag(gesamt)),
            lieferungszeitraum: Some(rubo4e::current::Zeitraum {
                startdatum: Some(date!(2026 - 03 - 01)),
                enddatum: Some(date!(2026 - 03 - 31)),
                ..rubo4e::current::Zeitraum::default()
            }),
            steuerbetrag: Some(Steuerbetrag {
                steuersatz: Some(dec("19")),
                ..Steuerbetrag::default()
            }),
            ..Rechnungsposition::default()
        }
    }

    /// 31 Tage Betriebspreis à 0,45 EUR = 13,95 netto, 19 % = 2,6505.
    fn rechnung() -> Rechnung {
        Rechnung {
            rechnungsdatum: Some(date!(2026 - 04 - 01).midnight().assume_utc()),
            faelligkeitsdatum: Some(date!(2026 - 05 - 15).midnight().assume_utc()),
            rechnungsperiode: Some(rubo4e::current::Zeitraum {
                startdatum: Some(date!(2026 - 03 - 01)),
                enddatum: Some(date!(2026 - 03 - 31)),
                ..rubo4e::current::Zeitraum::default()
            }),
            rechnungspositionen: Some(vec![pos(1, "9990001100002", "0.45", "31", "13.95")]),
            steuerbetraege: Some(vec![Steuerbetrag {
                steuersatz: Some(dec("19")),
                basiswert: Some(dec("13.95")),
                steuerwert: Some(dec("2.6505")),
                ..Steuerbetrag::default()
            }]),
            gesamtnetto: Some(betrag("13.95")),
            gesamtsteuer: Some(betrag("2.6505")),
            gesamtbrutto: Some(betrag("16.6005")),
            ..Rechnung::default()
        }
    }

    fn agreed() -> Vec<(String, EuroAmount)> {
        vec![(
            "9990001100002".to_owned(),
            crate::amount::euro_from_decimal(dec("0.45")).expect("euro"),
        )]
    }

    fn fakten() -> EmpfaengerFakten {
        EmpfaengerFakten::neu(date!(2026 - 04 - 01))
    }

    #[test]
    fn a_conformant_invoice_is_a_zahlungsavis() {
        let a = antwort_auf_rechnung(
            ESA_F,
            &rechnung(),
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert!(a.ist_zustimmung(), "{:?}", a.befunde);
        assert_eq!(a.remadv_pid(), 33_001);
    }

    /// The whole point of the ESA branch: the price basis is the accepted
    /// QUOTES 15003, and a deviation from it is `E_0264` Prüfschritt 320 —
    /// `A11`, on the Positionsebene, so the answer rides REMADV 33004.
    #[test]
    fn a_price_away_from_the_offer_is_a11_on_the_positionsebene() {
        let mut r = rechnung();
        r.rechnungspositionen = Some(vec![pos(1, "9990001100002", "0.90", "31", "27.90")]);
        r.gesamtnetto = Some(betrag("27.90"));
        r.steuerbetraege = Some(vec![Steuerbetrag {
            steuersatz: Some(dec("19")),
            basiswert: Some(dec("27.90")),
            steuerwert: Some(dec("5.301")),
            ..Steuerbetrag::default()
        }]);
        r.gesamtsteuer = Some(betrag("5.301"));
        r.gesamtbrutto = Some(betrag("33.201"));

        let a = antwort_auf_rechnung(
            ESA_F,
            &r,
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A11"]);
        assert_eq!(a.remadv_pid(), 33_004);
    }

    /// An Artikel-ID the offer never priced is a charge the ESA did not agree
    /// to — Prüfschritt 300, and the position's later Prüfschritte are skipped.
    #[test]
    fn an_unagreed_artikel_id_is_a09() {
        let mut r = rechnung();
        r.rechnungspositionen = Some(vec![pos(1, "9990009999999", "0.45", "31", "13.95")]);
        let a = antwort_auf_rechnung(
            ESA_F,
            &r,
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A09"]);
    }

    /// **Not holding the offer is a gap in mako's books, not a defect in the
    /// invoice.** With no accepted Angebot on record the price and Artikel-ID
    /// Prüfschritte are not comparable, and refusing on them would send a
    /// REMADV rejecting a correct invoice.
    #[test]
    fn without_an_offer_on_record_nothing_is_refused_on_price() {
        let a = antwort_auf_rechnung(
            ESA_F,
            &rechnung(),
            &[],
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert!(a.ist_zustimmung(), "{:?}", a.befunde);
    }

    /// Prüfschritt 500 — the offer priced two Artikel-IDs and the invoice bills
    /// one. The answer names the missing one, as the code's Hinweis requires.
    #[test]
    fn an_agreed_position_the_invoice_omits_is_a21() {
        let mut agreed = agreed();
        agreed.push((
            "9990001100001".to_owned(),
            crate::amount::euro_from_decimal(dec("120.00")).expect("euro"),
        ));
        let a = antwort_auf_rechnung(
            ESA_F,
            &rechnung(),
            &agreed,
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A21"]);
        assert!(a.befunde[0].detail.contains("9990001100001"));
    }

    /// Prüfschritt 70 — WiM Teil 2 UC 4.5.2 Nr. 1 gives the ESA ten Werktage,
    /// and the MSB knew it when it wrote the date.
    #[test]
    fn a_zahlungsziel_under_ten_werktage_is_a07() {
        let mut r = rechnung();
        r.faelligkeitsdatum = Some(date!(2026 - 04 - 08).midnight().assume_utc());
        let a = antwort_auf_rechnung(
            ESA_F,
            &r,
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A07"]);
        assert_eq!(a.remadv_pid(), 33_003);
    }

    /// Prüfschritt 60 — an MSB Abrechnung has no Rückerstattung; a credit
    /// travels as a Stornorechnung.
    #[test]
    fn a_negative_total_is_a06() {
        let mut r = rechnung();
        r.gesamtbrutto = Some(betrag("-16.6005"));
        let a = antwort_auf_rechnung(
            ESA_F,
            &r,
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A06"]);
    }

    /// Prüfschritt 540 — the stated total against netto + steuer.
    #[test]
    fn a_wrong_document_total_is_a24() {
        let mut r = rechnung();
        r.gesamtbrutto = Some(betrag("99.99"));
        let a = antwort_auf_rechnung(
            ESA_F,
            &r,
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A24"]);
    }

    /// Prüfschritt 420 — menge × einzelpreis against gesamtpreis.
    #[test]
    fn a_line_arithmetic_error_is_a20() {
        let mut r = rechnung();
        r.rechnungspositionen = Some(vec![pos(1, "9990001100002", "0.45", "31", "99.00")]);
        let a = antwort_auf_rechnung(
            ESA_F,
            &r,
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert!(a.codes().contains(&"A20"), "{:?}", a.codes());
    }

    /// **Prüfschritt 40 is answerable.** WiM Teil 2 UC 4.5.1: „Eine Rechnung
    /// referenziert auf die zugrundeliegende Bestellung", and INVOIC AHB 1.0b
    /// makes `SG1 RFF+ACE` Muss on the 31009 carrying the ORDERS
    /// Dokumentennummer. An invoice billing against an order this ESA never
    /// placed is `A04` — „der Rechnungsempfänger lehnt die Zahlung ab, da die
    /// Rechnung auf keiner Bestellung basiert".
    #[test]
    fn an_invoice_against_an_unknown_order_is_a04() {
        let mut f = fakten();
        f.bestellung_bekannt = false;
        let a = antwort_auf_rechnung(
            ESA_F,
            &rechnung(),
            &agreed(),
            &f,
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A04"]);
        // Kopf-level, so the walk stops there and the answer rides 33003.
        assert_eq!(a.remadv_pid(), 33_003);
    }

    /// Facts the invoice cannot state about itself never refuse while unknown,
    /// and do refuse once the ESA's records answer them.
    #[test]
    fn caller_supplied_facts_only_refuse_once_they_are_known() {
        let mut f = fakten();
        f.leistung_erbracht.insert(1, false);
        let a = antwort_auf_rechnung(
            ESA_F,
            &rechnung(),
            &agreed(),
            &f,
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.codes(), vec!["A10"]);

        // …and the default (nothing known) leaves the invoice payable.
        let quiet = antwort_auf_rechnung(
            ESA_F,
            &rechnung(),
            &agreed(),
            &fakten(),
            &CheckConfig::default(),
            CAL,
        );
        assert!(quiet.ist_zustimmung());
    }

    /// The second round is a different tree with a code the first does not
    /// publish, so the two cannot be answered from one walk.
    #[test]
    fn the_second_round_runs_e_0266() {
        let mut f = fakten();
        f.einwaende_entkraeftet = Some(false);
        let a = antwort_auf_erneute_rechnung(
            ESA_F,
            &rechnung(),
            &agreed(),
            &f,
            &CheckConfig::default(),
            CAL,
        );
        assert_eq!(a.tree, "E_0266");
        assert_eq!(a.codes(), vec!["A25"]);
    }

    /// **The two paths must answer one invoice the same way.**
    ///
    /// `InvoicCheckEngine` produces mako's own `Finding`s for the operator queue
    /// and the § 147 AO receipt; the tree walk produces the published
    /// Antwortcodes for the REMADV. They ask several of the same questions, and
    /// where they do they must read the **same** tolerance — otherwise tuning
    /// one knob makes mako record a Dispute while sending a Zahlungsavis.
    ///
    /// This is a regression guard: the Summen-level Prüfschritte used to read
    /// `arithmetic_tolerance_ppm` here and `total_tolerance_ppm` in the engine.
    #[test]
    fn the_summen_checks_read_the_same_tolerance_as_the_plausibility_engine() {
        // A brutto off by ~1.5 % against netto + steuer — outside the 1 %
        // default, inside a 5 % one.
        let mut r = rechnung();
        r.gesamtnetto = Some(betrag("100.00"));
        r.gesamtsteuer = Some(betrag("19.00"));
        r.gesamtbrutto = Some(betrag("120.80"));

        // Widen only the Summen knob. Both paths must follow it.
        let config = CheckConfig {
            total_tolerance_ppm: 50_000, // 5 %
            ..CheckConfig::default()
        };
        let antwort = antwort_auf_rechnung(
            ESA_F,
            &r,
            &[],
            &EmpfaengerFakten::neu(date!(2026 - 04 - 02)),
            &config,
            CAL,
        );
        assert!(
            !antwort.antwortcodes().contains(&"A24"),
            "the walk must follow total_tolerance_ppm, got {:?}",
            antwort.antwortcodes()
        );

        // Narrow it: both must now refuse.
        let config = CheckConfig {
            total_tolerance_ppm: 1,
            ..CheckConfig::default()
        };
        let antwort = antwort_auf_rechnung(
            ESA_F,
            &r,
            &[],
            &EmpfaengerFakten::neu(date!(2026 - 04 - 02)),
            &config,
            CAL,
        );
        assert!(
            antwort.antwortcodes().contains(&"A24"),
            "a 1 ppm tolerance must reach Prüfschritt 540, got {:?}",
            antwort.antwortcodes()
        );
    }
}
