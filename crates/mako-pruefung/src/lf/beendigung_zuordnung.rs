//! **Anfrage zur Beendigung der Zuordnung** — a new supplier has registered and
//! the grid operator asks the incumbent to release the Marktlokation.
//!
//! | Sparte | Inbound | EBD | Answers |
//! |---|---|---|---|
//! | Strom | 55010 | `E_0624` „Anfrage zur Beendigung der Zuordnung prüfen" | 55011 / 55012 |
//! | Gas | 44010 | `E_3020` (`G_0009` / `G_0010`) | 44011 / 44012 |
//!
//! Despite the name this belongs to the **Lieferbeginn** process, not to
//! Lieferende: it is Prozessschritt 3 of the incoming supplier's registration.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 6.6.3
//! and 13.6.3.

use time::{Date, Duration, Time};

use mako_fristen::{HolidayCalendar, berlin_at, next_werktag};

use crate::codes::{
    E_0624_CODES, E_3020_CODES, EBD_ABMELDUNGSANFRAGE_GAS, EBD_BEENDIGUNG_ZUORDNUNG,
};
use crate::lf::types::{Bekannt, LfAnfrage, LfEntscheidung, LfVertragslage};

/// Resolve a code from its Codeliste. A miss is a bug in this module, not a
/// runtime condition — the walk only ever names codes the catalogue publishes.
macro_rules! antwort {
    ($list:expr, $ebd:expr, $code:expr, $schritt:literal, $termin:expr) => {{
        let entry = $list
            .iter()
            .find(|c| c.code == $code)
            .unwrap_or_else(|| panic!("{} does not publish {}", $ebd, $code));
        LfEntscheidung::antwort(entry, $schritt, $termin, None)
    }};
}

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

// ── E_0624 — Anfrage zur Beendigung der Zuordnung (55010 → 55011 / 55012) ─────

/// Walk `E_0624` „Anfrage zur Beendigung der Zuordnung prüfen" for an inbound
/// **55010**.
///
/// This is the tree the LFA runs when a *new* supplier has registered and the
/// NB asks the incumbent to release the Marktlokation. Its codes overlap with
/// `E_0609` in name only.
///
/// # Panics
///
/// If the tree names a code [`crate::codes::E_0624_CODES`] does not publish.
#[must_use]
pub fn pruefe_beendigung_zuordnung(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    let ebd = EBD_BEENDIGUNG_ZUORDNUNG;
    let list = E_0624_CODES;
    let termin = anfrage.termin;

    macro_rules! code {
        ($code:literal, $schritt:literal) => {
            return antwort!(list, ebd, $code, $schritt, termin)
        };
        ($code:literal, $schritt:literal, $termin:expr) => {
            return antwort!(list, ebd, $code, $schritt, $termin)
        };
    }

    // Prüfschritt 5 — „Ist die Anfrage ausgehend vom ÜT der Lieferanmeldung bis
    // 07:00 Uhr des nächsten Werktages eingegangen?" This is the LFA's own
    // Frist check on the *incoming* message, and it is the first thing the tree
    // asks — before the Vorgang is even classified.
    if let Some(uet) = anfrage.uet_lieferanmeldung
        && !anfrage_rechtzeitig(uet, anfrage)
    {
        code!("A43", 5);
    }

    let verbrauchend = anfrage.lokationsart.ist_verbrauchend();

    // Prüfschritt 20/200 — besteht zum Folgetag des genannten Termins noch eine
    // Zuordnung? Ersatz-/Grundversorgung zählt laut EBD-Hinweis als ja.
    let zuordnung = match lage.zuordnung_am_folgetag {
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                20,
                format!(
                    "Für MaLo {} ist unbekannt, ob zum Folgetag des angefragten Termins noch \
                     eine Zuordnung besteht (E_0624 Prüfschritt 20).",
                    anfrage.malo_id
                ),
            );
        }
        other => other == Bekannt::Ja,
    };

    if !zuordnung {
        // Prüfschritt 30/210 — liegt bereits ein bestätigtes Zuordnungsende vor?
        return if lage.bestaetigtes_zuordnungsende.is_some() {
            antwort!(
                list,
                ebd,
                if verbrauchend { "A30" } else { "A41" },
                30,
                termin
            )
        } else {
            // A31/A42 confirm the date of the LFA's own, still unanswered
            // Abmeldung — so the answer states *that* date, not the NB's.
            antwort!(
                list,
                ebd,
                if verbrauchend { "A31" } else { "A42" },
                30,
                lage.vertragsende.or(termin)
            )
        };
    }

    if !verbrauchend {
        // Prüfschritt 220 — Tranche: only the Vertragsbindung question remains.
        return match lage.vertragsbindung_am_folgetag {
            Bekannt::Ja => antwort!(list, ebd, "A39", 220, termin),
            Bekannt::Nein => antwort!(list, ebd, "A40", 220, termin),
            Bekannt::Unbekannt => LfEntscheidung::eskalation(
                220,
                format!(
                    "Für Tranche an MaLo {} ist unbekannt, ob das Vertragsverhältnis zum Tag \
                     nach dem Enddatum fortbesteht (E_0624 Prüfschritt 220).",
                    anfrage.malo_id
                ),
            ),
        };
    }

    // Prüfschritt 40 — Transaktionsgrund Ein-/Auszug (Umzug)?
    if anfrage.grund_ist(crate::EIN_AUSZUG) {
        return umzug_zweig(anfrage, lage);
    }

    // Prüfschritt 70/80 — ist der LFA auch Grundversorger, und ist die MaLo zum
    // Folgetag in der Ersatzversorgung? Dann ist die Ersatzversorgung beendet.
    if lage.ist_grundversorger {
        match lage.in_ersatzversorgung_am_folgetag {
            Bekannt::Ja => code!("A38", 80),
            Bekannt::Unbekannt => {
                return LfEntscheidung::eskalation(
                    80,
                    format!(
                        "MaLo {}: der LFA ist Grundversorger, aber es ist unbekannt, ob die \
                         Marktlokation zum Folgetag in der Ersatzversorgung ist \
                         (E_0624 Prüfschritt 80 → A38).",
                        anfrage.malo_id
                    ),
                );
            }
            Bekannt::Nein => {}
        }
    }

    // Prüfschritt 90 — bleibt das Vertragsverhältnis zum Tag nach dem Enddatum
    // bestehen?
    match lage.vertragsbindung_am_folgetag {
        Bekannt::Ja => antwort!(list, ebd, "A35", 90, termin),
        Bekannt::Nein => antwort!(list, ebd, "A36", 90, lage.vertragsende.or(termin)),
        Bekannt::Unbekannt => LfEntscheidung::eskalation(
            90,
            format!(
                "MaLo {}: unbekannt, ob das Vertragsverhältnis zum Tag nach dem Enddatum \
                 fortbesteht (E_0624 Prüfschritt 90 → A35 / A36).",
                anfrage.malo_id
            ),
        ),
    }
}

/// `E_0624` Prüfschritte 50–60 — the Ein-/Auszug (Umzug) arm.
///
/// Two questions, both about the *person*: is the customer named in the
/// request the one the LFA has on file (then it is not an Einzug), and does
/// the LFA know they did not move out.
fn umzug_zweig(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    let list = E_0624_CODES;
    let ebd = EBD_BEENDIGUNG_ZUORDNUNG;
    let termin = anfrage.termin;

    // Prüfschritt 50 — ist der Kunde aus der Anfrage mit dem Kunden beim LFA
    // identisch? Wenn ja, ist es kein Einzug.
    match lage.kunde_identisch {
        Bekannt::Ja => return antwort!(list, ebd, "A32", 50, termin),
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                50,
                format!(
                    "Einzug an MaLo {}: der Kunde aus der Anfrage konnte nicht mit dem \
                     Kunden beim LFA abgeglichen werden (E_0624 Prüfschritt 50 → A32).",
                    anfrage.malo_id
                ),
            );
        }
        Bekannt::Nein => {}
    }

    // Prüfschritt 60 — hat der LFA Informationen, dass sein Kunde *nicht*
    // ausgezogen ist?
    match lage.kunde_nicht_ausgezogen {
        Bekannt::Ja => antwort!(list, ebd, "A33", 60, termin),
        // A34: „Der LFA beendet die Belieferung und teilt sein Lieferendedatum
        // in der Antwort mit."
        Bekannt::Nein => antwort!(list, ebd, "A34", 60, lage.vertragsende.or(termin)),
        Bekannt::Unbekannt => LfEntscheidung::eskalation(
            60,
            format!(
                "Einzug an MaLo {}: unbekannt, ob der Kunde ausgezogen ist \
                 (E_0624 Prüfschritt 60 → A33 / A34).",
                anfrage.malo_id
            ),
        ),
    }
}

/// `E_0624` Prüfschritt 5 — the Anfrage must arrive between the ÜT of the LFN's
/// Lieferanmeldung and **07:00 Uhr des nächsten Werktages**.
///
/// The instant is German local time; `mako_fristen` owns the DST and holiday
/// semantics so this does not recompute them.
fn anfrage_rechtzeitig(uet: Date, anfrage: &LfAnfrage) -> bool {
    let Some(morgen) = uet.checked_add(Duration::days(1)) else {
        return true;
    };
    // `next_werktag` leaves a Werktag unchanged, so the walk starts on the day
    // *after* the ÜT — „bis 07:00 Uhr des nächsten Werktages".
    let naechster_wt = next_werktag(morgen, HolidayCalendar::BdewMaKo);
    let Ok(sieben_uhr) = Time::from_hms(7, 0, 0) else {
        return true;
    };
    anfrage.eingang >= berlin_at(uet, Time::MIDNIGHT)
        && anfrage.eingang <= berlin_at(naechster_wt, sieben_uhr)
}

// ── Gas ───────────────────────────────────────────────────────────────────────

/// **44010 → 44011 / 44012** — Abmeldungsanfrage des NB (`E_3020`).
///
/// The Gas twin of the Strom `E_0624`: a new supplier registered and the GNB
/// asks the incumbent to release the Marktlokation. `G_0010` allows `Z01`
/// „Zustimmung mit Terminänderung" **only** when the Transaktionsgrund is `E01`
/// Ein-/Auszug — a Bedingung the code table states explicitly.
///
/// # Panics
///
/// If the walk names a code [`crate::codes::E_3020_CODES`] does not publish —
/// a defect in this module, covered by `every_landing_resolves_to_a_published_code`.
#[must_use]
pub fn pruefe_abmeldungsanfrage_gas(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    let list = E_3020_CODES;
    let ebd = EBD_ABMELDUNGSANFRAGE_GAS;
    let termin = anfrage.termin;

    // `Z08` — bereits zum gleichen Zeitpunkt bestätigt.
    if let (Some(bestaetigt), Some(t)) = (lage.bestaetigtes_zuordnungsende, termin)
        && bestaetigt == t
    {
        gas_code!(list, ebd, "Z08", 0, termin);
    }

    match lage.vorlauffrist_eingehalten {
        Bekannt::Nein => gas_code!(list, ebd, "E17", 0, termin),
        Bekannt::Unbekannt | Bekannt::Ja => {}
    }

    // `Z12` — Vertragsbindung. The question is the same one `E_0624`
    // Prüfschritt 90 asks: does the contract survive the requested date?
    match lage.vertragsbindung_am_folgetag {
        Bekannt::Ja => {
            // `Z01` states the next possible date instead of a flat refusal —
            // but only where the code table permits it (Transaktionsgrund E01).
            if anfrage.grund_ist(crate::EIN_AUSZUG) && lage.vertragsende.is_some() {
                let entry = list
                    .iter()
                    .find(|c| c.code == "Z01")
                    .expect("E_3020 publishes Z01");
                return LfEntscheidung::antwort(entry, 0, lage.vertragsende, None);
            }
            gas_code!(list, ebd, "Z12", 0, termin);
        }
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                0,
                format!(
                    "Gas-Abmeldungsanfrage für MaLo {}: unbekannt, ob das Vertragsverhältnis \
                     zum angefragten Termin fortbesteht (E_3020 → Z12).",
                    anfrage.malo_id
                ),
            );
        }
        Bekannt::Nein => {}
    }

    gas_code!(list, ebd, "E15", 0, lage.vertragsende.or(termin))
}
