//! The typed view a **Vertragszusammenfassung** template renders from.
//!
//! § 41 Abs. 4 EnWG: „Den Letztverbrauchern ist innerhalb einer angemessenen
//! Frist nach dem Vertragsschluss eine knappe, leicht verständliche und klar
//! gekennzeichnete Zusammenfassung der wichtigsten Vertragsbedingungen zur
//! Verfügung zu stellen." Satz 2 names what it contains at the least:
//!
//! | Satz 2 | Field |
//! |---|---|
//! | Nr. 1 die Kontaktdaten des Energielieferanten | [`VertragszusammenfassungView::absender`] |
//! | Nr. 2 die Verbrauchsstelle | [`VertragszusammenfassungView::verbrauchsstelle`] |
//! | Nr. 3 geltende Preise | [`VertragszusammenfassungView::preise`] |
//! | Nr. 4 den voraussichtlichen Belieferungsbeginn | [`VertragszusammenfassungView::belieferungsbeginn`] |
//! | Nr. 5 die Kündigungsfrist | [`VertragszusammenfassungView::kuendigungsfrist`] |
//! | Nr. 6 etwaige Bonusvereinbarungen und Mindestvertragslaufzeiten | [`VertragszusammenfassungView::bonus`], [`VertragszusammenfassungView::mindestvertragslaufzeit`] |
//!
//! It is owed to every Letztverbraucher, not only to a Haushaltskunde. The
//! publish gate renders [`specimen`] and checks the page prints each item —
//! Nr. 6 included, which the specimen carries so that a template dropping
//! it is refused rather than passing on a contract that happens to have none.
//!
//! Amounts are decimal strings, exactly as in [`super::view`]: pad, never
//! truncate.

use serde::Serialize;

use super::view::PartyView;

/// One price as the contract states it — `Arbeitspreis` in `ct/kWh`,
/// `Grundpreis` in `EUR/Jahr`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PreisZeile {
    /// What is priced, as the tariff names it.
    pub bezeichnung: String,
    /// The unit [`Self::wert`] is in.
    pub einheit: String,
    /// The price, a decimal string, gross as the Letztverbraucher pays it.
    pub wert: String,
}

/// Everything a Vertragszusammenfassung template may render, and nothing else.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VertragszusammenfassungView {
    /// The date the summary bears, ISO 8601.
    pub datum: String,
    /// The supplier — Nr. 1, its Kontaktdaten. The gate requires the name and
    /// one way to reach it: a telephone number, an e-mail address or a postal
    /// address.
    pub absender: PartyView,
    /// The Letztverbraucher.
    pub empfaenger: PartyView,
    /// The contract this summarises, as the customer knows it.
    pub vertragsnummer: String,
    /// `STROM` or `GAS`.
    pub sparte: Option<String>,
    /// Nr. 2 — the Verbrauchsstelle, as the customer recognises it: the
    /// supply address, or the Marktlokation where no address is held.
    pub verbrauchsstelle: String,
    /// The Marktlokation, printed beside the Verbrauchsstelle.
    pub malo_id: Option<String>,
    /// Nr. 3 — the prices that apply. Never empty for an issued summary.
    pub preise: Vec<PreisZeile>,
    /// Nr. 4 — the expected Belieferungsbeginn, ISO 8601.
    pub belieferungsbeginn: String,
    /// Nr. 5 — the Kündigungsfrist, in words.
    pub kuendigungsfrist: String,
    /// Nr. 6 — a Bonusvereinbarung, where the contract has one.
    pub bonus: Option<String>,
    /// Nr. 6 — a Mindestvertragslaufzeit, where the contract has one.
    pub mindestvertragslaufzeit: Option<String>,
}

/// The supplier every specimen of this kind family is issued by.
pub(super) fn specimen_absender() -> PartyView {
    PartyView {
        name: Some("Stadtwerke Musterstadt GmbH".to_owned()),
        vat_id: Some("DE123456789".to_owned()),
        tax_number: None,
        line1: Some("Musterstraße 1".to_owned()),
        post_code: Some("12345".to_owned()),
        city: Some("Musterstadt".to_owned()),
        country: Some("DE".to_owned()),
        contact_name: Some("Kundenservice".to_owned()),
        phone: Some("0800 1234567".to_owned()),
        email: Some("service@stadtwerke-musterstadt.example".to_owned()),
    }
}

/// The customer every specimen of this kind family is addressed to.
pub(super) fn specimen_empfaenger() -> PartyView {
    PartyView {
        name: Some("Erika Mustermann-Übelacker".to_owned()),
        vat_id: None,
        tax_number: None,
        line1: Some("Beispielweg 7".to_owned()),
        post_code: Some("10115".to_owned()),
        city: Some("Berlin".to_owned()),
        country: Some("DE".to_owned()),
        contact_name: None,
        phone: None,
        email: Some("erika@example.org".to_owned()),
    }
}

/// The two prices every specimen of this kind family states.
pub(super) fn specimen_preise() -> Vec<PreisZeile> {
    vec![
        PreisZeile {
            bezeichnung: "Arbeitspreis".to_owned(),
            einheit: "ct/kWh".to_owned(),
            wert: "34.90".to_owned(),
        },
        PreisZeile {
            bezeichnung: "Grundpreis".to_owned(),
            einheit: "EUR/Jahr".to_owned(),
            wert: "143.88".to_owned(),
        },
    ]
}

/// The Vertragszusammenfassung a template is proven against: every Satz 2
/// item present, Nr. 6 included.
#[must_use]
pub fn specimen() -> VertragszusammenfassungView {
    VertragszusammenfassungView {
        datum: "2026-11-16".to_owned(),
        absender: specimen_absender(),
        empfaenger: specimen_empfaenger(),
        vertragsnummer: "VV-2026-00000042".to_owned(),
        sparte: Some("STROM".to_owned()),
        verbrauchsstelle: "Lieferstraße 12, 10117 Berlin".to_owned(),
        malo_id: Some("51238696012".to_owned()),
        preise: specimen_preise(),
        belieferungsbeginn: "2027-02-01".to_owned(),
        kuendigungsfrist: "1 Monat zum Ende der Mindestvertragslaufzeit".to_owned(),
        bonus: Some("Neukundenbonus 60,00 EUR mit der ersten Jahresrechnung".to_owned()),
        mindestvertragslaufzeit: Some("12 Monate ab Belieferungsbeginn".to_owned()),
    }
}
