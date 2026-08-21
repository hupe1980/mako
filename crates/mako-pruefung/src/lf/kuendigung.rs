//! **Kündigung** — the incoming supplier terminates the incumbent's contract
//! directly, without the grid operator in between.
//!
//! | Sparte | Inbound | EBD | Answers |
//! |---|---|---|---|
//! | Strom | 55016 | `E_0614` „Kündigung Vertrag prüfen" | 55017 / 55018 |
//! | Gas | 44016 | `E_3001` (`G_0005` / `G_0006`) | 44017 / 44018 |
//!
//! This is the one LF-answered process where the counterparty is another
//! supplier (LFN → LFA) rather than the grid operator.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 6.2.1
//! and 13.3.1.

use crate::codes::{E_0614_CODES, E_3001_CODES, EBD_KUENDIGUNG, EBD_KUENDIGUNG_GAS};
use crate::lf::types::{
    Bekannt, LfAnfrage, LfEntscheidung, LfVertragslage, Lokationsart, Vollmacht,
};

/// Resolve a Gas code from its Codeliste. A miss is a bug in this module, not a
/// runtime condition — the walk only ever names codes the catalogue publishes.
macro_rules! gas_code {
    ($list:expr, $ebd:expr, $code:literal, $schritt:literal, $termin:expr) => {{
        let entry = $list
            .iter()
            .find(|c| c.code == $code)
            .unwrap_or_else(|| panic!("{} does not publish {}", $ebd, $code));
        return LfEntscheidung::antwort(entry, $schritt, $termin, None);
    }};
}

// ── E_0614 — Kündigung Vertrag prüfen (55016 → 55017 / 55018) ─────────────────

/// Walk `E_0614` „Kündigung Vertrag prüfen" for an inbound **55016** the new
/// supplier sends directly to the incumbent.
///
/// The tree splits at Prüfschritt 10 into a verbrauchende-Marktlokation branch
/// (`A01`–`A09`) and an „other object" branch (`A10`–`A18`) that additionally
/// asks whether a contract exists at all.
///
/// # Panics
///
/// If the tree names a code [`crate::codes::E_0614_CODES`] does not publish.
#[must_use]
pub fn pruefe_kuendigung(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    let ebd = EBD_KUENDIGUNG;
    let list = E_0614_CODES;
    let termin = anfrage.termin;
    let verbrauchend = anfrage.lokationsart == Lokationsart::VerbrauchendeMalo;
    let c = |a: &'static str, b: &'static str| if verbrauchend { a } else { b };

    macro_rules! code {
        ($code:expr, $schritt:literal) => {{
            let want = $code;
            let entry = list
                .iter()
                .find(|c| c.code == want)
                .unwrap_or_else(|| panic!("{ebd} does not publish {want}"));
            return LfEntscheidung::antwort(entry, $schritt, termin, None);
        }};
    }

    // Prüfschritt 500 — the non-verbrauchend branch first asks whether a
    // contract exists at all.
    if !verbrauchend && !lage.beliefert && lage.vertragsende.is_none() {
        code!("A18", 500);
    }

    let Some(kuendigungstermin) = anfrage.termin else {
        return LfEntscheidung::eskalation(
            20,
            "Kündigung ohne Kündigungstermin (SG4 DTM+93) — der Termin ist die erste Größe, \
             die E_0614 prüft.",
        );
    };

    // Prüfschritt 20/505 — liegt der Kündigungstermin vor dem Nachrichteneingang?
    if kuendigungstermin < anfrage.eingang.date() {
        code!(c("A01", "A10"), 20);
    }

    // Prüfschritt 40/550 — Vertrag bereits zum angefragten Termin gekündigt?
    // Cluster Zustimmung: the LFA confirms what already holds.
    if lage.vertragsende == Some(kuendigungstermin) {
        code!(c("A03", "A12"), 40);
    }

    // Prüfschritt 50/560 — Vertrag bereits zu einem *früheren* Datum gekündigt?
    if let Some(ende) = lage.vertragsende
        && ende < kuendigungstermin
    {
        code!(c("A04", "A13"), 50);
    }

    // Prüfschritte 60–80 / 570–590 — Kündbarkeit zum genannten Termin.
    match lage.vertragsbindung_am_folgetag {
        Bekannt::Ja => {
            // 80/590 — Vertrag bereits zu einem *späteren* Zeitpunkt beendet?
            if lage.vertragsende.is_some_and(|e| e > kuendigungstermin) {
                code!(c("A05", "A14"), 80);
            }
            code!(c("A06", "A15"), 80);
        }
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                70,
                format!(
                    "MaLo {}: unbekannt, ob der Vertrag zum übermittelten Kündigungstermin \
                     unter Einhaltung der Kündigungsfrist kündbar ist \
                     (E_0614 Prüfschritt 70 → A06 / A15).",
                    anfrage.malo_id
                ),
            );
        }
        Bekannt::Nein => {}
    }

    // Prüfschritte 90–110 / 600–620 — Vollmacht.
    match lage.vollmacht {
        Vollmacht::NichtAngefordert | Vollmacht::Wirksam => {}
        Vollmacht::AngefordertAusstehend => {
            // The EBD parks here: „Solange die Vollmacht beim LFA nicht
            // eingetroffen ist, wartet der Prozess an diesem Prüfschritt."
            // Parking is an operator state, not an answer.
            return LfEntscheidung::eskalation(
                100,
                format!(
                    "Kündigung an MaLo {}: die vom LFA angeforderte Vollmacht ist noch nicht \
                     eingetroffen — der Prozess wartet, die Prüfung ist regelmäßig zu \
                     wiederholen (E_0614 Prüfschritt 100).",
                    anfrage.malo_id
                ),
            );
        }
        Vollmacht::Unwirksam => code!(c("A08", "A16"), 110),
    }

    // Prüfschritt 120/630 — Zustimmung.
    code!(c("A09", "A17"), 120)
}

// ── Gas ───────────────────────────────────────────────────────────────────────

/// **44016 → 44017 / 44018** — Kündigung Gasliefervertrag (`E_3001`).
///
/// The Gas twin of the Strom `E_0614`, sent LFN → LFA. The Codeliste order
/// matters: „Die Prüfungen, die zu den Codes `A03` und `A04` führen, sind
/// zuerst durchzuführen" — identifying the Marktlokation comes before anything
/// about the contract.
///
/// # Panics
///
/// If the walk names a code [`crate::codes::E_3001_CODES`] does not publish —
/// a defect in this module, covered by `every_landing_resolves_to_a_published_code`.
#[must_use]
pub fn pruefe_kuendigung_gas(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    let list = E_3001_CODES;
    let ebd = EBD_KUENDIGUNG_GAS;
    let termin = anfrage.termin;

    // `A03` — keine Identifizierung einer Marktlokation. Checked first, per the
    // Codeliste's own Hinweis.
    if anfrage.malo_id.is_empty() {
        gas_code!(list, ebd, "A03", 0, termin);
    }

    // `Z29` — das Vertragsverhältnis wurde bereits zu einem früheren Zeitpunkt
    // beendet.
    if let (Some(ende), Some(t)) = (lage.vertragsende, termin)
        && ende < t
    {
        gas_code!(list, ebd, "Z29", 0, termin);
    }

    // `Z34` — Mehrfachkündigung: der Vertrag wurde bereits zum angefragten
    // Termin durch einen anderen Marktpartner oder den Kunden gekündigt.
    if lage.vertragsende == termin && termin.is_some() {
        gas_code!(list, ebd, "Z34", 0, termin);
    }

    match lage.vertragsbindung_am_folgetag {
        Bekannt::Ja => {
            // The Codeliste's Anmerkung on `Z12`: „Im DTM Segment … muss dann
            // der nächstmögliche Kündigungszeitpunkt mitgegeben werden."
            let entry = list
                .iter()
                .find(|c| c.code == "Z12")
                .expect("E_3001 publishes Z12");
            LfEntscheidung::antwort(entry, 0, lage.vertragsende.or(termin), None)
        }
        Bekannt::Unbekannt => LfEntscheidung::eskalation(
            0,
            format!(
                "Gas-Kündigung für MaLo {}: unbekannt, ob zum Kündigungstermin noch eine \
                 Vertragsbindung besteht (E_3001 → Z12).",
                anfrage.malo_id
            ),
        ),
        Bekannt::Nein => gas_code!(list, ebd, "E15", 0, termin),
    }
}
