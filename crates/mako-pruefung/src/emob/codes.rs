//! The Antwortcode catalogue of the Modell-2 trees.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten für die
//! Antwortnachrichten* 4.3 (Stand 23.06.2026), Kapitel 17 „Zum Modell 2 zur
//! ladevorgangscharfen bilanziellen Energiemengenzuordnungsmöglichkeit". The
//! chapter opens with „Die nachfolgenden EBD sind erst ab dem 1. Oktober 2023
//! anzuwenden."
//!
//! # `A01` is a Zustimmung in two of these trees and an Ablehnung in the third
//!
//! `E_0510` publishes `A01` as **Ablehnung** („Ablehnung der Abmeldung durch
//! den Lieferanten"), while `E_0511` and `E_0512` publish `A01` as
//! **Zustimmung** („Bestätigung der Beendigung" / „Bestätigung der
//! Abmeldung"). The three trees run in the *same* process, one after the
//! other, and a combined VNB+LF deployment walks all three — so a catalogue
//! keyed on the bare code would answer a confirmation with a refusal. Every
//! lookup here is therefore keyed on `(ebd, code)`, and [`lookup`] refuses a
//! code the named tree does not publish.

use crate::codes::{AntwortCode, code};

/// `E_0510` — Anmeldung prüfen. Prüfende Rolle: **NB** (Kommentar aus AD: VNB).
pub const EBD_ANMELDUNG: &str = "E_0510";
/// `E_0511` — Beendigung der Zuordnung prüfen. Prüfende Rolle: **LF**.
pub const EBD_BEENDIGUNG: &str = "E_0511";
/// `E_0512` — Abmeldung prüfen. Prüfende Rolle: **NB** (VNB).
pub const EBD_ABMELDUNG: &str = "E_0512";
/// `E_0513` — Prüfen, ob Anmeldung direkt ablehnbar. Prüfende Rolle: **NB** (VNB).
pub const EBD_DIREKT_ABLEHNBAR: &str = "E_0513";
/// `E_0514` — Beendigung der Zuordnung prüfen (VNB→LF leg).
///
/// **Publishes no tree and no codes.** The EBD document prints the heading and
/// then says: „Derzeit ist für diese Entscheidung kein Entscheidungsbaum
/// notwendig, da keine Antwort gegeben wird." It is named here because
/// [`EBD_DIREKT_ABLEHNBAR`]'s „nein" branch hands over to it, and a caller that
/// receives [`super::EmobEntscheidung::Weiter`] needs the identity of the step
/// it was handed to. Asking [`lookup`] for a code in it always answers `None`.
pub const EBD_BEENDIGUNG_ANSTOSSEN: &str = "E_0514";

/// When `A99` „Sonstiges" stops being usable, across all four trees.
///
/// Every `A99` in Kapitel 17 carries „Nutzungsmöglichkeit Ende: 01.04.2027,
/// 00:00 Uhr". After that instant the catch-all is gone and a VNB that cannot
/// place a refusal in a specific code has no code left — which is why
/// [`super::modellwechsel`] escalates rather than reaching for `A99` whenever
/// the caller cannot state the fact.
pub const A99_NUTZUNGSMOEGLICHKEIT_ENDE: &str = "2027-04-01T00:00+02:00";

const E_0510: Option<&'static str> = Some(EBD_ANMELDUNG);
const E_0511: Option<&'static str> = Some(EBD_BEENDIGUNG);
const E_0512: Option<&'static str> = Some(EBD_ABMELDUNG);
const E_0513: Option<&'static str> = Some(EBD_DIREKT_ABLEHNBAR);

/// `E_0510` — Anmeldung prüfen (Prüfschritte 1–2).
pub const E_0510_CODES: &[AntwortCode] = &[
    code!(
        "A01",
        E_0510,
        Ablehnung,
        "Ablehnung der Abmeldung durch den Lieferanten"
    ),
    code!("A99", E_0510, Ablehnung, "Sonstiges", bemerkung),
    code!("A02", E_0510, Zustimmung, "Bestätigung der Anmeldung"),
];

/// `E_0511` — Beendigung der Zuordnung prüfen (Prüfschritt 1).
pub const E_0511_CODES: &[AntwortCode] = &[
    code!("A99", E_0511, Ablehnung, "Sonstiges", bemerkung),
    code!("A01", E_0511, Zustimmung, "Bestätigung der Beendigung"),
];

/// `E_0512` — Abmeldung prüfen (Prüfschritt 1).
pub const E_0512_CODES: &[AntwortCode] = &[
    code!("A99", E_0512, Ablehnung, "Sonstiges", bemerkung),
    code!("A01", E_0512, Zustimmung, "Bestätigung der Abmeldung"),
];

/// `E_0513` — Prüfen, ob Anmeldung direkt ablehnbar (Prüfschritt 1).
///
/// The tree publishes a single code. Its „nein" branch is not a code at all —
/// it hands the message to `E_0514`.
pub const E_0513_CODES: &[AntwortCode] = &[code!("A99", E_0513, Ablehnung, "Sonstiges", bemerkung)];

/// Every Modell-2 tree, keyed by its EBD id.
pub const EMOB_TREES: &[(&str, &[AntwortCode])] = &[
    (EBD_ANMELDUNG, E_0510_CODES),
    (EBD_BEENDIGUNG, E_0511_CODES),
    (EBD_ABMELDUNG, E_0512_CODES),
    (EBD_DIREKT_ABLEHNBAR, E_0513_CODES),
];

/// Resolve `code` **within** `ebd`.
///
/// Returns `None` when the tree does not publish the code. That is the check
/// that keeps `E_0510`'s refusing `A01` off an `E_0511` confirmation.
#[must_use]
pub fn lookup(ebd: &str, code: &str) -> Option<&'static AntwortCode> {
    let (_, codes) = EMOB_TREES.iter().find(|(id, _)| *id == ebd)?;
    codes.iter().find(|c| c.code == code)
}

/// The tree's own Zustimmungscode, or `None` where it publishes none.
///
/// `E_0513` publishes none: it either refuses or hands over to `E_0514`.
#[must_use]
pub fn zustimmung(ebd: &str) -> Option<&'static AntwortCode> {
    let (_, codes) = EMOB_TREES.iter().find(|(id, _)| *id == ebd)?;
    codes.iter().find(|c| c.ist_zustimmung() == Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a01_is_a_refusal_in_e0510_and_an_agreement_in_the_other_two() {
        assert_eq!(
            lookup(EBD_ANMELDUNG, "A01").unwrap().ist_zustimmung(),
            Some(false)
        );
        assert_eq!(
            lookup(EBD_BEENDIGUNG, "A01").unwrap().ist_zustimmung(),
            Some(true)
        );
        assert_eq!(
            lookup(EBD_ABMELDUNG, "A01").unwrap().ist_zustimmung(),
            Some(true)
        );
    }

    #[test]
    fn a02_exists_only_in_e0510() {
        assert!(lookup(EBD_ANMELDUNG, "A02").is_some());
        for ebd in [EBD_BEENDIGUNG, EBD_ABMELDUNG, EBD_DIREKT_ABLEHNBAR] {
            assert!(lookup(ebd, "A02").is_none(), "{ebd} must not publish A02");
        }
    }

    #[test]
    fn e0514_publishes_nothing() {
        assert!(lookup(EBD_BEENDIGUNG_ANSTOSSEN, "A01").is_none());
        assert!(zustimmung(EBD_BEENDIGUNG_ANSTOSSEN).is_none());
    }

    #[test]
    fn e0513_publishes_no_zustimmung() {
        assert!(zustimmung(EBD_DIREKT_ABLEHNBAR).is_none());
    }

    #[test]
    fn every_a99_demands_a_written_reason() {
        for (ebd, _) in EMOB_TREES {
            let a99 = lookup(ebd, "A99").unwrap();
            assert!(a99.braucht_bemerkung, "{ebd} A99 must carry FTX+ACB");
        }
    }
}
