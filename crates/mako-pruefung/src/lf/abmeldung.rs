//! **Lieferende von NB an LF** — the supplier answers an announced end of the
//! network assignment.
//!
//! | Sparte | Inbound | EBD | Answers |
//! |---|---|---|---|
//! | Strom | 55007 | `E_0609` „Abmeldung prüfen" | 55008 / 55009 |
//! | Gas | 44007 | `E_3002` (`G_0067` / `G_0068`) | 44008 / 44009 |
//!
//! One business process, two documents: Strom walks numbered Prüfschritte onto
//! `Axx` codes, Gas picks from a flat Codeliste of `E`/`Z` codes and names no
//! EBD in DE 1131.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 6.4.1
//! and 13.5.1.

use time::Date;

use mako_fristen::{HolidayCalendar, add_werktage, next_werktag};

use crate::codes::{E_0609_CODES, E_3002_CODES, EBD_ABMELDUNG, EBD_ABMELDUNG_GAS};
use crate::lf::types::{Bekannt, LfAnfrage, LfEntscheidung, LfVertragslage, Lokationsart};

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

// ── E_0609 — Abmeldung prüfen (55007 → 55008 / 55009) ─────────────────────────

/// Walk `E_0609` „Abmeldung prüfen" for an inbound **55007** Ankündigung der
/// Beendigung der Zuordnung (Lieferende von NB an LF).
///
/// The tree splits at Prüfschritt 10 on whether the Vorgang names a
/// verbrauchende or ruhende Marktlokation (`A01`–`A10`) or an erzeugende
/// Marktlokation / Tranche (`A21`–`A29`). Both halves ask the same questions
/// with different code ranges, so they share one walk and differ only in which
/// code each landing produces.
///
/// # Panics
///
/// If the tree names a code [`crate::codes::E_0609_CODES`] does not publish.
/// That is a defect in this module, not a runtime condition — the walk only
/// ever names codes the catalogue carries, and the
/// `every_landing_resolves_to_a_published_code` test walks all of them.
#[must_use]
pub fn pruefe_abmeldung(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    // Prüfschritt 10 — „verbrauchende Marktlokation **oder ruhende
    // Marktlokation**?" `E_0609` names both, unlike `E_0624`.
    let lokationsart = match anfrage.lokationsart_oder_eskalation(EBD_ABMELDUNG) {
        Ok(l) => l,
        Err(e) => return e,
    };
    let verbrauchend = lokationsart.ist_verbrauchend();
    // (verbrauchend, Tranche/erzeugend) code pairs, in Prüfschritt order.
    let c = |a: &'static str, b: &'static str| if verbrauchend { a } else { b };
    let ebd = EBD_ABMELDUNG;
    let list = E_0609_CODES;
    let termin = anfrage.termin;

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

    if let Some(e) = unroutbarer_grund(anfrage) {
        return e;
    }

    // Prüfschritt 20/25 — a Vorgang marked `ZAP` claims the MaLo is a ruhende
    // Marktlokation of a Kundenanlage. Only the supplier can say whether that is
    // true, and only for the verbrauchend branch.
    if verbrauchend && lokationsart == Lokationsart::RuhendeMalo {
        // The EBD asks whether it really *is* a ruhende MaLo under § 20 Abs. 1d
        // EnWG / § 10c EEG. mako has no field for that today, so the honest
        // outcome is an operator decision rather than a fabricated `A01`.
        return LfEntscheidung::eskalation(
            25,
            format!(
                "Abmeldung einer ruhenden Marktlokation (Transaktionsgrundergänzung ZAP) für MaLo {}: \
                 zu prüfen ist, ob es sich um eine ruhende Marktlokation einer Kundenanlage \
                 (§ 20 Abs. 1d EnWG bzw. § 10c EEG) handelt — sonst A01.",
                anfrage.malo_id
            ),
        );
    }

    // Prüfschritt 30/510 — Lieferende zum identischen Abmeldedatum bereits bestätigt?
    if let (Some(bestaetigt), Some(termin)) = (lage.bestaetigtes_zuordnungsende, anfrage.termin)
        && bestaetigt == termin
    {
        code!(c("A02", "A21"), 30);
    }

    // Prüfschritt 40/520 — Vorlauffrist (Minimum und Maximum) eingehalten?
    match lage.vorlauffrist_eingehalten {
        Bekannt::Nein => code!(c("A03", "A22"), 40),
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                40,
                "Vorlauffrist der Abmeldung nicht bewertet — die minimale und die maximale \
                 Vorlauffrist sind beide zu prüfen (E_0609 Prüfschritt 40).",
            );
        }
        Bekannt::Ja => {}
    }

    // Prüfschritt 50/530 — Transaktionsgrund „Auszug wegen Stilllegung" (Z33)?
    if anfrage.grund_ist(crate::STILLLEGUNG) {
        return match lage.keine_stilllegung {
            // 60/540 — the LF holds information that the MaLo is *not* being
            // decommissioned.
            Bekannt::Ja => antwort!(list, ebd, c("A04", "A23"), 60, termin),
            Bekannt::Nein => zustimmung(verbrauchend, termin),
            Bekannt::Unbekannt => LfEntscheidung::eskalation(
                60,
                format!(
                    "Abmeldung wegen Stilllegung für MaLo {}: zu prüfen ist, ob dem LF \
                     Informationen vorliegen, dass die Marktlokation nicht stillgelegt wird \
                     (E_0609 Prüfschritt 60 → A04).",
                    anfrage.malo_id
                ),
            ),
        };
    }

    // Prüfschritte 80–120 / 560–600 — the two Zuordnungsermächtigungs-Gründe.
    if let Some(e) = pruefe_zuordnungsermaechtigung(anfrage, lage, verbrauchend) {
        return e;
    }

    // 130/610 — kein zuvor unspezifizierter Fehler: Zustimmung.
    zustimmung(verbrauchend, termin)
}

/// `E_0609` Prüfschritte 80–120 / 560–600 — the Zuordnungsermächtigungs-Zweig.
///
/// Reached from 50-nein, where the only remaining grounds are `ZT0` (Änderung
/// des Zeitreihentyps) and `ZQ7` (Deaktivierung durch den BKV). `None` means
/// the branch found nothing to object to and the walk continues to 130/610.
fn pruefe_zuordnungsermaechtigung(
    anfrage: &LfAnfrage,
    lage: &LfVertragslage,
    verbrauchend: bool,
) -> Option<LfEntscheidung> {
    let c = |a: &'static str, b: &'static str| if verbrauchend { a } else { b };
    let termin = anfrage.termin;
    let code = |want: &str, schritt: u16| {
        let entry = E_0609_CODES
            .iter()
            .find(|c| c.code == want)
            .unwrap_or_else(|| panic!("{EBD_ABMELDUNG} does not publish {want}"));
        Some(LfEntscheidung::antwort(entry, schritt, termin, None))
    };

    // 80/560 — Transaktionsgrund „fehlende Zuordnungsermächtigung aufgrund
    // Änderung ZRT"? The alternative edge (85/565) is the BKV-Deaktivierung.
    if anfrage.grund_ist(crate::ZRT_AENDERUNG) {
        // 90/570 — ZRT auf einen Typ geändert, für den eine
        // Zuordnungsermächtigung bestehen müsste?
        return match lage.zrt_wechsel_mit_ermaechtigung {
            Bekannt::Ja => code(c("A06", "A25"), 90),
            Bekannt::Nein => None,
            Bekannt::Unbekannt => Some(LfEntscheidung::eskalation(
                90,
                "Zeitreihentyp-Wechsel nicht bewertet — liegt keine Änderung des \
                 Zeitreihentyps zum übermittelten Lieferende vor, ist die Frage laut EBD \
                 mit ja zu beantworten (E_0609 Prüfschritt 90 → A06).",
            )),
        };
    }

    // 85/565 — Lieferende muss der nächste Monatserste 00:00 Uhr sein.
    let Some(termin) = anfrage.termin else {
        return Some(LfEntscheidung::eskalation(
            85,
            "Abmeldung wegen Deaktivierung der Zuordnungsermächtigung ohne Lieferende \
             (SG4 DTM+93) — Prüfschritt 85 vergleicht es mit dem nächsten Monatsersten.",
        ));
    };
    if termin.day() != 1 {
        return code(c("A05", "A24"), 85);
    }
    // 100/580 — hat der BKV die Deaktivierung vorgenommen?
    match lage.zuordnungsermaechtigung_deaktiviert {
        Bekannt::Nein => return code(c("A07", "A26"), 100),
        Bekannt::Unbekannt => {
            return Some(LfEntscheidung::eskalation(
                100,
                "Deaktivierung der Zuordnungsermächtigung durch den BKV nicht bekannt — \
                 der LF klärt den Sachverhalt mit dem BKV (E_0609 Prüfschritt 100 → A07).",
            ));
        }
        Bekannt::Ja => {}
    }
    // 120/600 — Eingang nach dem 5. WT des Monats, in dem die
    // Zuordnungsermächtigung endet?
    if eingang_nach_fuenftem_werktag(anfrage, termin) {
        return code(c("A09", "A28"), 120);
    }
    None
}

/// The escalation a Vorgang earns when its Transaktionsgrund has no path.
///
/// `E_0609` branches on the Grund at 50 and 80, and every edge out of 80 leads
/// somewhere — but only for the three grounds the UTILMD AHB admits on a 55007.
/// A message carrying anything else, or nothing, would fall past both branches
/// to the terminal and be **confirmed** without the walk ever examining it.
fn unroutbarer_grund(anfrage: &LfAnfrage) -> Option<LfEntscheidung> {
    let Some(grund) = anfrage.transaktionsgrund.as_deref() else {
        return Some(LfEntscheidung::eskalation(
            50,
            format!(
                "Abmeldung für MaLo {}: die Nachricht nennt keinen Transaktionsgrund \
                 (SG4 STS+7 DE 9013), obwohl die UTILMD AHB ihn für 55007 als Muss führt — \
                 E_0609 verzweigt ab Prüfschritt 50 darauf.",
                anfrage.malo_id
            ),
        ));
    };
    if crate::ABMELDUNG_GRUENDE.contains(&grund) {
        return None;
    }
    Some(LfEntscheidung::eskalation(
        50,
        format!(
            "Abmeldung für MaLo {}: Transaktionsgrund `{grund}` ist für 55007 nicht \
             zugelassen (erlaubt sind {}). E_0609 kennt für ihn keinen Pfad.",
            anfrage.malo_id,
            crate::ABMELDUNG_GRUENDE.join(", ")
        ),
    ))
}

/// The `E_0609` terminal at Prüfschritt 130/610: `A10` (verbrauchend) or `A29`
/// (Tranche/erzeugend).
///
/// The step's „ja" edge is `A99` Sonstiges — „ein zuvor nicht spezifizierter
/// Fehler". A walk cannot detect an unspecified error about itself, so the
/// automated path always takes the „nein" edge and an operator who *does* see
/// one dispatches `A99` from the queue.
fn zustimmung(verbrauchend: bool, termin: Option<Date>) -> LfEntscheidung {
    let code = if verbrauchend { "A10" } else { "A29" };
    let entry = E_0609_CODES
        .iter()
        .find(|c| c.code == code)
        .expect("E_0609 publishes its Zustimmungscodes");
    LfEntscheidung::antwort(entry, 130, termin, None)
}

/// `E_0609` Prüfschritt 120/600 — „Liegt das Eingangsdatum der Abmeldung nach
/// dem 5. WT des Monats, in dem die Zuordnungsermächtigung endet?"
fn eingang_nach_fuenftem_werktag(anfrage: &LfAnfrage, lieferende: Date) -> bool {
    let Ok(monatserster) = Date::from_calendar_date(lieferende.year(), lieferende.month(), 1)
    else {
        return false;
    };
    // The 1st is itself the first Werktag when it falls on one, so the count
    // starts there rather than a day later.
    let erster_wt = next_werktag(monatserster, HolidayCalendar::BdewMaKo);
    let fuenfter_wt = add_werktage(erster_wt, 4, HolidayCalendar::BdewMaKo);
    anfrage.eingang.date() > fuenfter_wt
}

// ── Gas ───────────────────────────────────────────────────────────────────────

/// **44007 → 44008 / 44009** — Abmeldung NN vom NB (`E_3002`).
///
/// The Gas twin of the Strom `E_0609`: the GNB announces that the network
/// assignment ends and the supplier answers. `G_0067` offers exactly one
/// Zustimmungscode (`E15`), so every path that is not a stated Ablehnungsgrund
/// ends there.
///
/// # Panics
///
/// If the walk names a code [`crate::codes::E_3002_CODES`] does not publish —
/// a defect in this module, covered by `every_landing_resolves_to_a_published_code`.
#[must_use]
pub fn pruefe_abmeldung_gas(anfrage: &LfAnfrage, lage: &LfVertragslage) -> LfEntscheidung {
    let list = E_3002_CODES;
    let ebd = EBD_ABMELDUNG_GAS;
    let termin = anfrage.termin;

    // `Z08` — der angefragte Geschäftsvorfall wurde bereits zum gleichen
    // Zeitpunkt bestätigt.
    if let (Some(bestaetigt), Some(t)) = (lage.bestaetigtes_zuordnungsende, termin)
        && bestaetigt == t
    {
        gas_code!(list, ebd, "Z08", 0, termin);
    }

    // `E17` — Fristüberschreitung. The Gas Vorlauffrist is per-Netzbetreiber
    // under GeLi Gas Kap. 2.6, so the caller evaluates it and passes the verdict.
    match lage.vorlauffrist_eingehalten {
        Bekannt::Nein => gas_code!(list, ebd, "E17", 0, termin),
        Bekannt::Unbekannt => {
            return LfEntscheidung::eskalation(
                0,
                "Gas-Abmeldung vom NB: die Vorlauffrist wurde nicht bewertet — sie ist \
                 netzbetreiberindividuell (GeLi Gas Kap. 2.6) und muss vom Aufrufer \
                 beigebracht werden (E_3002 → E17).",
            );
        }
        Bekannt::Ja => {}
    }

    // `Z09` — Transaktionsgrund und mitgelieferte Daten passen nicht zusammen:
    // a Stilllegung the supplier knows is not happening.
    if anfrage.grund_ist(crate::STILLLEGUNG) {
        match lage.keine_stilllegung {
            Bekannt::Ja => gas_code!(list, ebd, "Z09", 0, termin),
            Bekannt::Unbekannt => {
                return LfEntscheidung::eskalation(
                    0,
                    format!(
                        "Gas-Abmeldung wegen Stilllegung für MaLo {}: unbekannt, ob dem LF \
                         Informationen vorliegen, dass die Marktlokation nicht stillgelegt \
                         wird (E_3002 → Z09).",
                        anfrage.malo_id
                    ),
                );
            }
            Bekannt::Nein => {}
        }
    }

    // The supplier must actually hold the assignment it is being asked to end.
    if !lage.beliefert {
        return LfEntscheidung::eskalation(
            0,
            format!(
                "Gas-Abmeldung vom NB für MaLo {}: diese Marktlokation wird von uns nicht \
                 beliefert — vor einer Zustimmung ist der Datenschiefstand zu klären.",
                anfrage.malo_id
            ),
        );
    }

    gas_code!(list, ebd, "E15", 0, termin)
}
