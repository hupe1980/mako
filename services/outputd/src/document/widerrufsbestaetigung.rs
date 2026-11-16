//! The typed view a **Widerrufsbestätigung** template renders from.
//!
//! A Widerruf sent through the operator's Widerrufsfunktion is owed an
//! Eingangsbestätigung „unverzüglich" on a durable medium, with the
//! declaration's content and the date and time of its receipt (§ 356a Abs. 4
//! BGB); a Widerruf through a website form likewise (§ 356 Abs. 1 Satz 2 BGB).
//!
//! | Requirement | Field |
//! |---|---|
//! | the declaration's content | [`WiderrufsbestaetigungView::erklaerung`] |
//! | date **and time** of receipt | [`WiderrufsbestaetigungView::eingegangen_am`] |
//!
//! The publish gate checks the page prints the date and the time separately: a
//! template printing only the date omits half of what the confirmation states.

use serde::Serialize;

use super::view::PartyView;

/// The declaration as the consumer made it — § 356a Abs. 2 BGB's data.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErklaerungView {
    /// The consumer's name.
    pub name: String,
    /// The contract the consumer named.
    pub vertrag: String,
    /// The electronic address the confirmation goes to.
    pub bestaetigung_an: String,
}

/// Everything a Widerrufsbestätigung template may render, and nothing else.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WiderrufsbestaetigungView {
    /// The date the confirmation bears, ISO 8601.
    pub datum: String,
    /// The supplier — § 126b BGB's declarant.
    pub absender: PartyView,
    /// The consumer.
    pub empfaenger: PartyView,
    /// The contract withdrawn from.
    pub vertragsnummer: String,
    /// When the Widerruf was received, ISO 8601 date-time in German local time.
    pub eingegangen_am: String,
    /// When the consumer sent it, where known.
    pub abgesendet_am: Option<String>,
    /// How it arrived — `WIDERRUFSFUNKTION`, `EMAIL`, `BRIEF`, `SONSTIG`.
    pub kanal: String,
    /// The declaration's content.
    pub erklaerung: ErklaerungView,
}

/// The Widerrufsbestätigung a template is proven against.
#[must_use]
pub fn specimen() -> WiderrufsbestaetigungView {
    WiderrufsbestaetigungView {
        datum: "2026-11-30".to_owned(),
        absender: super::vertragszusammenfassung::specimen_absender(),
        empfaenger: super::vertragszusammenfassung::specimen_empfaenger(),
        vertragsnummer: "VV-2026-00000042".to_owned(),
        // Sent before midnight and received after it, so the date and the
        // time the gate checks are the receipt's and no other date on the page.
        eingegangen_am: "2026-11-29T00:03:37+01:00".to_owned(),
        abgesendet_am: Some("2026-11-28T23:58:00+01:00".to_owned()),
        kanal: "EMAIL".to_owned(),
        erklaerung: ErklaerungView {
            // Each differs from the letter's own fields, so a template that
            // prints the address block and the Vertragsnummer but not the
            // declaration is refused.
            name: "E. Mustermann".to_owned(),
            vertrag: "Stromvertrag Beispielweg 7".to_owned(),
            bestaetigung_an: "widerruf-erika@example.org".to_owned(),
        },
    }
}
