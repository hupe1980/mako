//! The trees an **Energieserviceanbieter** runs on the MSB's Rechnung —
//! WiM Strom Teil 2 Kap. 4.5, „Abrechnung einer für den ESA erbrachten
//! Leistung".
//!
//! | Function | Inbound | Prüfende Rolle | EBD | Answer |
//! |---|---|---|---|---|
//! | [`pruefe_rechnung`] | INVOIC 31009 | **ESA** | `E_0264` | REMADV 33001 / 33003 / 33004 |
//! | [`pruefe_rechnung_erneut`] | INVOIC 31009 nach COMDIS | **ESA** | `E_0266` | REMADV 33001 / 33003 / 33004 |
//! | [`pruefe_nicht_zahlungsavis`] | REMADV 33003/33004 | MSB | `E_0265` | COMDIS 29001 / Storno 31004 |
//! | [`pruefe_stornorechnung`] | INVOIC 31004 | **ESA** | `E_0267` | REMADV 33001 / 33002 / — |
//!
//! # This module is a binding, not an engine
//!
//! The walk itself lives in [`crate::rechnung`], because the BDEW publishes it
//! three times: once for the ESA here and twice more for the Abrechnung der
//! Leistungen des Preisblatts B (AWH Kap. 9.3/9.4, `E_0270`–`E_0277`). These
//! four functions are [`crate::rechnung::ESA`] applied to that engine.
//!
//! Two differences make a second copy a bad idea rather than a tedious one:
//! `E_0266`'s Prüfschritt 1 is **`A25`** where `E_0276`/`E_0277` use **`AC1`**
//! for the identical question, and `A25` in the Preisblatt-B trees means
//! something else entirely (doppelter Abrechnungszeitraum, Prüfschritt 90 — a
//! step the ESA trees do not publish at all, because an ESA has no Preisblatt).
//!
//! # Why this is not [`super::wertebestellung`]
//!
//! The order handshake of Kap. 4.1–4.4 is checked by the **MSB**: it decides
//! whether to serve the ESA. Kap. 4.5 runs the other way — the ESA is the
//! payer, and three of these four trees are its own. They also answer in a
//! different shape: an ORDRSP carries exactly one `SG2 AJT`, while `E_0264`
//! answers with a **set** of (Ebene, Positionsnummer, code) triples.
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2b, WiM Strom Teil 2 Kap. 4.5
//! - *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 8.27
//! - REMADV AHB 1.0a § 3.1.1 / § 3.1.2, COMDIS AHB 1.0h, INVOIC AHB 1.0b

use crate::rechnung::{
    self, ESA, NichtZahlungsavisAntwort, RechnungsAntwort, RechnungsFakten, StornoAntwort,
    StornoFakten,
};

/// Walk `E_0264` — the ESA's check of an inbound INVOIC 31009.
///
/// # Panics
///
/// Only if the `E_0264` Codeliste is missing a code the walk names, which a
/// test in [`crate::rechnung`] rules out.
#[must_use]
pub fn pruefe_rechnung(r: &RechnungsFakten, cal: crate::HolidayCalendar) -> RechnungsAntwort {
    rechnung::pruefe_rechnung(ESA, r, cal)
}

/// Walk `E_0266` — the ESA's second look, after the MSB's COMDIS 29001 claimed
/// the invoice was correct.
///
/// Identical to [`pruefe_rechnung`] except for Prüfschritt 1: if the COMDIS did
/// not rebut every objection, the answer is **`A25`** and the walk stops. That
/// is a Kopf-level refusal, so it rides REMADV 33003.
///
/// # Panics
///
/// As [`pruefe_rechnung`].
#[must_use]
pub fn pruefe_rechnung_erneut(
    r: &RechnungsFakten,
    cal: crate::HolidayCalendar,
) -> RechnungsAntwort {
    rechnung::pruefe_rechnung_erneut(ESA, r, cal)
}

/// Walk `E_0265` — the MSB's single Prüfschritt on an inbound Nicht-Zahlungsavis.
///
/// `begruendung` is only read on the [`NichtZahlungsavisAntwort::Widersprechen`]
/// branch, where the code's own Hinweis makes it mandatory.
///
/// # Panics
///
/// Only if the `E_0265` Codeliste is missing `A99`, which a test rules out.
#[must_use]
pub fn pruefe_nicht_zahlungsavis(
    ablehnung_gerechtfertigt: bool,
    begruendung: impl Into<String>,
) -> NichtZahlungsavisAntwort {
    rechnung::pruefe_nicht_zahlungsavis(ESA, ablehnung_gerechtfertigt, begruendung)
}

/// Walk `E_0267` — the ESA's check of an inbound Stornorechnung.
///
/// # Panics
///
/// Only if the `E_0267` Codeliste is missing a code the walk names.
#[must_use]
pub fn pruefe_stornorechnung(s: &StornoFakten) -> StornoAntwort {
    rechnung::pruefe_stornorechnung(ESA, s)
}
