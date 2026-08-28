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
    Bekannt, LfAnfrage, LfEntscheidung, LfVertragslage, Lokationsart, Terminart, Vollmacht,
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
/// Prüfschritt 60 splits it a second time, and that split is the one with
/// teeth: only a Kündigung **zu einem fixen Termin** reaches the Kündbarkeits-
/// frage and its `A05`/`A06` Vertragsbindungs-Ablehnungen. A Kündigung „zum
/// nächstmöglichen Termin" — the ordinary LFW24 case, `SG4 DTM+471` instead of
/// `DTM+93` — skips 70/80 entirely: the LFA cannot disagree with a date the
/// LFN did not name, and answers `A09`/`A17` stating the date it determined.
///
/// # Panics
///
/// If the tree names a code [`crate::codes::E_0614_CODES`] does not publish.
#[must_use]
pub fn pruefe_kuendigung(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    let ebd = EBD_KUENDIGUNG;
    let list = E_0614_CODES;
    let termin = anfrage.termin;
    // Prüfschritt 10 — „verbrauchende Marktlokation?", without `E_0609`'s
    // „oder ruhende".
    let verbrauchend = match anfrage.lokationsart_oder_eskalation(ebd) {
        Ok(l) => l == Lokationsart::VerbrauchendeMalo,
        Err(e) => return *e,
    };
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
    // contract exists at all. `A18` is a *record* that there is none, never the
    // absence of one: a deployment without `vertragd`, or a Tranche `marktd`
    // does not carry, finds nothing for every object, and answering `A18` from
    // that releases customers the supplier still holds.
    if !verbrauchend {
        match lage.vertrag_vorhanden {
            Bekannt::Nein => code!("A18", 500),
            Bekannt::Unbekannt => {
                return LfEntscheidung::eskalation(
                    500,
                    format!(
                        "Kündigung für {} an MaLo {}: es ist nicht festgestellt, ob zu \
                         dem genannten Objekt überhaupt ein Vertrag vorliegt \
                         (E_0614 Prüfschritt 500 → A18).",
                        match anfrage.lokationsart {
                            Some(Lokationsart::Tranche) => "eine Tranche",
                            Some(Lokationsart::ErzeugendeMalo) => "eine erzeugende Marktlokation",
                            Some(Lokationsart::RuhendeMalo) => "eine ruhende Marktlokation",
                            _ => "dieses Objekt",
                        },
                        anfrage.malo_id
                    ),
                );
            }
            Bekannt::Ja => {}
        }
    }

    let Some(kuendigungstermin) = anfrage.termin else {
        return LfEntscheidung::eskalation(
            20,
            "Kündigung ohne Termin — die AHB macht SG4 DTM+93 (Ende zum) und DTM+471 \
             (Ende zum nächstmöglichen Termin) wechselseitig zur Muss-Angabe, und der \
             Termin ist die erste Größe, die E_0614 prüft.",
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

    // Prüfschritte 60–80 / 570–590 — Kündbarkeit, but only for a fixed date.
    if let Some(e) = pruefe_kuendbarkeit(anfrage, lage, verbrauchend, kuendigungstermin) {
        return e;
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

    // Prüfschritt 120/630 — Zustimmung. On the „nächstmöglicher Termin" branch
    // the AHB requires the answer to name the date the LFA determined
    // (`SG4 DTM+471`, Bedingung [513]); a deployment that cannot determine it
    // has nothing to put in a Muss segment, so it escalates rather than
    // confirming a Kündigung to no date at all.
    if anfrage.terminart == Terminart::Naechstmoeglich {
        let Some(naechster) = lage.naechstmoeglicher_kuendigungstermin else {
            return LfEntscheidung::eskalation(
                120,
                format!(
                    "MaLo {}: Kündigung zum nächstmöglichen Termin — der nächstmögliche \
                     Kündigungstermin ist nicht bekannt, die Bestätigung muss ihn aber \
                     nennen (UTILMD AHB SG4 DTM+471, Bedingung [513]).",
                    anfrage.malo_id
                ),
            );
        };
        let entry = list
            .iter()
            .find(|e| e.code == c("A09", "A17"))
            .unwrap_or_else(|| panic!("{ebd} publishes its Zustimmungscodes"));
        return LfEntscheidung::antwort(entry, 120, Some(naechster), None);
    }
    code!(c("A09", "A17"), 120)
}

/// `E_0614` Prüfschritte 60–80 / 570–590 — may this Kündigung be refused for
/// Vertragsbindung at all?
///
/// Prüfschritt 60 („Handelt es sich um eine Kündigung, welche zu einem fixen
/// Termin ausgesprochen wurde?") is the split with teeth. A Kündigung „zum
/// nächstmöglichen Termin" — the ordinary LFW24 case, `SG4 DTM+471` instead of
/// `DTM+93` — takes the nein-edge straight to the Vollmacht question: the LFA
/// cannot disagree with a date the LFN did not name, so `A05`/`A06` are
/// unreachable. `None` means the branch found no objection.
///
/// # Prüfschritt 70 is a date comparison
///
/// „Ist der Vertrag zum übermittelten Kündigungstermin **unter Einhaltung der
/// Kündigungsfrist unter Berücksichtigung des Eingangsdatums der Kündigung**
/// kündbar?" — not a question about whether a contract is running. Every
/// unterminated contract is running, and almost all of them are terminable to a
/// date far enough out. The step is decided from
/// [`LfVertragslage::naechstmoeglicher_kuendigungstermin`], the date the LFA's
/// own contract rules produce for this Eingangsdatum: the Kündigung is
/// admissible exactly when that date is **on or before** the requested one.
/// Deciding it from [`LfVertragslage::vertragsbindung_am_folgetag`] instead
/// would refuse `A06` to every § 20a EnWG supplier switch.
fn pruefe_kuendbarkeit(
    anfrage: &LfAnfrage,
    lage: &LfVertragslage,
    verbrauchend: bool,
    kuendigungstermin: time::Date,
) -> Option<LfEntscheidung> {
    if anfrage.terminart != Terminart::Fix {
        return None;
    }
    let c = |a: &'static str, b: &'static str| if verbrauchend { a } else { b };

    // Prüfschritt 70/580 — Kündbarkeit zum genannten Termin.
    let Some(naechster) = lage.naechstmoeglicher_kuendigungstermin else {
        return Some(LfEntscheidung::eskalation(
            70,
            format!(
                "MaLo {}: der nächstmögliche Kündigungstermin ist nicht bekannt, also ist \
                 nicht feststellbar, ob der Vertrag zum übermittelten Termin \
                 {kuendigungstermin} unter Einhaltung der Kündigungsfrist kündbar ist \
                 (E_0614 Prüfschritt 70 → A06 / A15).",
                anfrage.malo_id
            ),
        ));
    };
    if naechster <= kuendigungstermin {
        // Kündbar — the tree goes straight to the Vollmacht question.
        return None;
    }

    // 80/590 — Vertrag bereits zu einem *späteren* Zeitpunkt beendet? That is a
    // recorded termination, not merely a running contract: `A05`'s 55018
    // carries the already confirmed Kündigungsdatum in `DTM+Z05`/`Z06`, while
    // `A06` carries `DTM+157`, „der Zeitpunkt, zu welchem der Vertrag am Tag
    // des Versandes der Antwort noch kündbar ist".
    Some(match lage.vertragsende.filter(|e| *e > kuendigungstermin) {
        Some(bestaetigtes_ende) => antwort_mit_kuendbarkeit(c("A05", "A14"), 80, bestaetigtes_ende),
        None => antwort_mit_kuendbarkeit(c("A06", "A15"), 80, naechster),
    })
}

/// An `E_0614` Ablehnung whose 55018 must carry a date beside the code.
///
/// `A05`/`A14` name the already confirmed Kündigungsdatum (`SG4 DTM+Z05`/`Z06`),
/// `A06`/`A15` the date the contract is still terminable to (`SG4 DTM+157`).
/// Both are Muss, and both are already known by the time this is called:
/// Prüfschritt 70 could not have refused without the nächstmöglicher Termin,
/// and `A05` is reached only from a recorded Vertragsende.
fn antwort_mit_kuendbarkeit(
    code: &'static str,
    pruefschritt: u16,
    datum: time::Date,
) -> LfEntscheidung {
    let entry = E_0614_CODES
        .iter()
        .find(|e| e.code == code)
        .unwrap_or_else(|| panic!("E_0614 publishes {code}"));
    LfEntscheidung::antwort(entry, pruefschritt, Some(datum), None)
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

    // GeLi Gas 3.0 § 3.1: „In der Kündigung kann ein beliebiges **in der
    // Zukunft liegendes** (auch untermonatliches) Kündigungsdatum angegeben
    // werden." A date that is not is outside the process, and `E_3001` publishes
    // no code for it — the Strom `A01`/`A10` „Termin liegt vor dem
    // Nachrichteneingang" has no Gas counterpart. Falling through would confirm
    // it with `E15`, so it goes to an operator, who can send `E14` with the
    // Erläuterung the Codeliste requires.
    match termin {
        None => {
            return LfEntscheidung::eskalation(
                0,
                format!(
                    "Gas-Kündigung für MaLo {}: die Nachricht nennt keinen \
                     Kündigungstermin, und E_3001 prüft ihn als erste Größe.",
                    anfrage.malo_id
                ),
            );
        }
        Some(t) if t < anfrage.eingang.date() => {
            return LfEntscheidung::eskalation(
                0,
                format!(
                    "Gas-Kündigung für MaLo {}: der Kündigungstermin {t} liegt vor dem \
                     Nachrichteneingang. GeLi Gas 3.0 § 3.1 lässt nur ein in der Zukunft \
                     liegendes Kündigungsdatum zu, und E_3001 veröffentlicht dafür keinen \
                     Code — zu beantworten mit E14 und Erläuterung.",
                    anfrage.malo_id
                ),
            );
        }
        Some(_) => {}
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
    // `termin` is `Some` past the plausibility check above.
    if lage.vertragsende == termin {
        gas_code!(list, ebd, "Z34", 0, termin);
    }

    // Kündbarkeit — the same comparison `E_0614` Prüfschritt 70 makes on the
    // Strom side, and for the same reason: „besteht noch eine Vertragsbindung"
    // is true of every unterminated contract, while the question the answer
    // turns on is whether *this* Termin honours the Kündigungsfrist.
    let Some(naechster) = lage.naechstmoeglicher_kuendigungstermin else {
        return LfEntscheidung::eskalation(
            0,
            format!(
                "Gas-Kündigung für MaLo {}: der nächstmögliche Kündigungszeitpunkt ist \
                 nicht bekannt, also ist nicht feststellbar, ob zum angefragten Termin \
                 noch eine Vertragsbindung besteht (E_3001 → Z12 bzw. Z01).",
                anfrage.malo_id
            ),
        );
    };
    if termin.is_some_and(|t| naechster <= t) {
        gas_code!(list, ebd, "E15", 0, termin);
    }
    vertragsbindung(anfrage, naechster, list)
}

/// `E_3001` when the contract is still bound at the requested Termin.
///
/// The Codeliste gates the two answers on the **date qualifier**, exactly as
/// `E_0614` Prüfschritt 60 does on the Strom side:
///
/// - `Z12` „Ablehnung Vertragsbindung" carries Bedingung **`[43]` Wenn `SG4
///   DTM+93` (Ende zum) in der Anfrage vorhanden** — so it may answer only a
///   Kündigung to a **fixed** date. Its Anmerkung then requires the
///   nächstmöglicher Kündigungszeitpunkt in the DTM segment.
/// - `Z01` „Zustimmung mit Terminänderung" carries Bedingung **`[41]` Wenn `SG4
///   DTM+471` (Ende zum nächstmöglichen Termin) vorhanden** — the „nächstmöglich"
///   Kündigung the LFA cannot refuse, answered with the date it determined.
///
/// Answering `Z12` on a `DTM+471` Kündigung breaks Bedingung `[43]`, so it is not
/// merely the wrong business answer: the message fails AHB validation at the
/// counterparty. Both codes need the date, and a deployment that cannot
/// determine it has nothing to put in the segment.
fn vertragsbindung(
    anfrage: &LfAnfrage,
    naechster: time::Date,
    list: &'static [crate::codes::AntwortCode],
) -> LfEntscheidung {
    let code = if anfrage.terminart == Terminart::Fix {
        "Z12"
    } else {
        "Z01"
    };
    let entry = list
        .iter()
        .find(|c| c.code == code)
        .unwrap_or_else(|| panic!("E_3001 publishes {code}"));
    LfEntscheidung::antwort(entry, 0, Some(naechster), None)
}
