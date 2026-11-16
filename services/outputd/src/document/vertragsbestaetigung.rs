//! The typed view a **Vertragsbestätigung** template renders from.
//!
//! § 312f Abs. 2 BGB owes a consumer in Fernabsatz a Bestätigung des Vertrags
//! „innerhalb einer angemessenen Frist nach Vertragsschluss, spätestens jedoch
//! … bevor mit der Ausführung der Dienstleistung begonnen wird, auf einem
//! dauerhaften Datenträger", carrying the Art. 246a EGBGB information. Two of
//! those decide money and are what the page is checked for:
//!
//! | Information | Field | Norm |
//! |---|---|---|
//! | the contract's content | [`VertragsbestaetigungView::produkt`], [`VertragsbestaetigungView::preise`], [`VertragsbestaetigungView::belieferungsbeginn`], [`VertragsbestaetigungView::laufzeit`], [`VertragsbestaetigungView::kuendigungsfrist`] | § 41 Abs. 1 Satz 2 EnWG |
//! | Bedingungen, Fristen und Verfahren des Widerrufs, the Muster-Widerrufsformular | [`WiderrufsbelehrungView`] | Art. 246a § 1 Abs. 2 Satz 1 Nr. 1 EGBGB |
//! | the Wertersatz for energy supplied after an express request | [`WiderrufsbelehrungView::wertersatz_hinweis`] | Art. 246a § 1 Abs. 2 Satz 1 Nr. 3 EGBGB |
//!
//! The Belehrung is printed **always**: mako cannot prove the order page was a
//! durable medium, and § 356 Abs. 3 BGB lets the Widerrufsfrist begin only once
//! the consumer was informed. A delivered Vertragsbestätigung carrying the
//! Belehrung is what starts it when the order page did not, so a template that
//! drops the Belehrung leaves the Frist unstarted for twelve months and
//! fourteen days.

use serde::Serialize;

use super::vertragszusammenfassung::PreisZeile;
use super::view::PartyView;

/// The Widerrufsbelehrung as the page prints it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WiderrufsbelehrungView {
    /// The Frist in words — `"14 Tage ab Vertragsschluss"`.
    pub frist: String,
    /// The operator's Widerrufsbelehrung, printed as registered.
    pub belehrung_text: String,
    /// The Muster-Widerrufsformular, printed as registered.
    pub muster_widerrufsformular: String,
    /// Where the „Vertrag widerrufen" function sits (§ 356a BGB), where the
    /// operator offers one.
    pub widerrufsfunktion: Option<String>,
    /// Art. 246a § 1 Abs. 2 Satz 1 Nr. 3 EGBGB — the Wertersatz owed for
    /// energy supplied before the Widerruf after an express request.
    pub wertersatz_hinweis: String,
}

/// Everything a Vertragsbestätigung template may render, and nothing else.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VertragsbestaetigungView {
    /// The date the confirmation bears, ISO 8601.
    pub datum: String,
    /// The supplier — § 126b BGB's declarant.
    pub absender: PartyView,
    /// The consumer.
    pub empfaenger: PartyView,
    /// The contract, as the customer knows it.
    pub vertragsnummer: String,
    /// When the contract came about, ISO 8601 date-time.
    pub vertragsschluss_am: String,
    /// `STROM` or `GAS`.
    pub sparte: Option<String>,
    /// The Verbrauchsstelle — supply address or Marktlokation.
    pub verbrauchsstelle: String,
    /// The Marktlokation.
    pub malo_id: Option<String>,
    /// The product the consumer ordered, as the offer named it.
    pub produkt: String,
    /// The prices agreed.
    pub preise: Vec<PreisZeile>,
    /// The expected Belieferungsbeginn, ISO 8601.
    pub belieferungsbeginn: String,
    /// The term, in words.
    pub laufzeit: String,
    /// The Kündigungsfrist, in words.
    pub kuendigungsfrist: String,
    /// The Widerrufsbelehrung with the Muster-Widerrufsformular.
    pub widerruf: WiderrufsbelehrungView,
    /// Whether the consumer expressly asked for supply to begin within the
    /// Widerrufsfrist (§ 357a Abs. 2 Satz 1 Nr. 1 BGB).
    pub lieferung_in_widerrufsfrist_verlangt: bool,
}

/// The Vertragsbestätigung a template is proven against.
#[must_use]
pub fn specimen() -> VertragsbestaetigungView {
    VertragsbestaetigungView {
        datum: "2026-11-16".to_owned(),
        absender: super::vertragszusammenfassung::specimen_absender(),
        empfaenger: super::vertragszusammenfassung::specimen_empfaenger(),
        vertragsnummer: "VV-2026-00000042".to_owned(),
        vertragsschluss_am: "2026-11-15T19:02:11+01:00".to_owned(),
        sparte: Some("STROM".to_owned()),
        verbrauchsstelle: "Lieferstraße 12, 10117 Berlin".to_owned(),
        malo_id: Some("51238696012".to_owned()),
        produkt: "Musterstrom Öko 24".to_owned(),
        preise: super::vertragszusammenfassung::specimen_preise(),
        belieferungsbeginn: "2027-02-01".to_owned(),
        laufzeit: "12 Monate, danach unbefristet".to_owned(),
        kuendigungsfrist: "1 Monat zum Ende der Laufzeit".to_owned(),
        widerruf: WiderrufsbelehrungView {
            frist: "14 Tage ab Vertragsschluss".to_owned(),
            belehrung_text: "Sie haben das Recht, binnen vierzehn Tagen ohne Angabe von \
                             Gründen diesen Vertrag zu widerrufen."
                .to_owned(),
            muster_widerrufsformular: "Hiermit widerrufe ich den von mir abgeschlossenen \
                                       Vertrag über die Belieferung mit Strom."
                .to_owned(),
            widerrufsfunktion: Some(
                "Schaltfläche „Vertrag widerrufen“ in Ihrem Kundenportal".to_owned(),
            ),
            wertersatz_hinweis: "Haben Sie verlangt, dass die Belieferung während der \
                                 Widerrufsfrist beginnen soll, so haben Sie uns einen \
                                 angemessenen Betrag zu zahlen."
                .to_owned(),
        },
        lieferung_in_widerrufsfrist_verlangt: true,
    }
}
