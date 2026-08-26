//! `‹Zeitreihe› prüfen` — the receiving side of a Summenzeitreihe.
//!
//! Ten trees, three shapes. The **long** form (`E_0007`, `E_0040`, `E_0041`,
//! `E_0093`) walks four Abweisungs-Prüfschritte before it looks at a single
//! energy figure; the **short** form (`E_0062`, `E_0063`, `E_0064`, `E_0098`,
//! `E_0099`) has neither a Frist nor a Versionsangabe to check; `E_0065` adds
//! the DZÜ-Liste gate.
//!
//! **The order is the rule.** Frist → Zeitraum → Dublette → Version →
//! Energiemenge, and nothing may overtake. A series that arrives after the
//! Clearingfrist *and* carries implausible figures is `A01`, not `A05`: it was
//! refused before it was assessed, and only `A01` keeps the Prüfmitteilung
//! from being forwarded (MaBiS Kap. 9.8.2 Nr. 2).
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3.

use time::OffsetDateTime;

use super::codes::EBD_DZUE;
use super::types::MabisEntscheidung;

/// The facts a „Zeitreihe prüfen" walk needs.
///
/// `None` is **unknown**, not „no": it produces an
/// [`MabisEntscheidung::Eskalation`] naming the Prüfschritt rather than a
/// guessed code.
#[derive(Debug, Clone)]
pub struct ZeitreihenPruefung<'a> {
    /// When the series arrived.
    pub eingang: OffsetDateTime,
    /// End of the Clearingfrist that applies to it, or `None` where the tree
    /// states no Frist.
    pub clearingfrist_ende: Option<OffsetDateTime>,
    /// Whether the MaBiS-ZP is active for the Bilanzierungsmonat the series
    /// names.
    pub mabis_zp_aktiv: Option<bool>,
    /// The series' Versionsangabe (`SG4 RFF+AUU`).
    pub version: Option<&'a str>,
    /// Versions of this series already held for this MaBiS-ZP and
    /// Bilanzierungsmonat, in any order.
    ///
    /// The Versionsangabe is a 17-character Erstellungszeitpunkt, so „höher
    /// als" is a lexicographic comparison and needs no parsing.
    pub bekannte_versionen: &'a [&'a str],
    /// Whether the energy figures match what the receiver expected.
    pub energiemengen_plausibel: Option<bool>,
}

/// Walk the **long** form: `E_0007`, `E_0040`, `E_0041`, `E_0093`.
///
/// # Panics
///
/// When `ebd` is not one of the four long-form trees.
#[must_use]
pub fn pruefe_zeitreihe(ebd: &'static str, p: &ZeitreihenPruefung<'_>) -> MabisEntscheidung {
    assert!(
        matches!(ebd, "E_0007" | "E_0040" | "E_0041" | "E_0093"),
        "{ebd} is not a long-form Zeitreihen-Tree"
    );

    // 1 — Eingang nach Ablauf der Clearingfrist?
    if let Some(ende) = p.clearingfrist_ende
        && p.eingang > ende
    {
        return MabisEntscheidung::antwort(ebd, "A01", 1);
    }

    // 2 — MaBiS-ZP zum Bilanzierungsmonat aktiv?
    match p.mabis_zp_aktiv {
        Some(false) => return MabisEntscheidung::antwort(ebd, "A02", 2),
        None => {
            return MabisEntscheidung::eskalation(
                "Ist der MaBiS-ZP zum betrachteten Bilanzierungsmonat aktiv?",
                2,
            );
        }
        Some(true) => {}
    }

    let Some(version) = p.version else {
        return MabisEntscheidung::eskalation(
            "Die Zeitreihe trägt keine Versionsangabe (SG4 RFF+AUU); \
             Prüfschritt 3 und 4 sind ohne sie nicht entscheidbar.",
            3,
        );
    };

    // 3 — Version bereits vorhanden?
    if p.bekannte_versionen.contains(&version) {
        return MabisEntscheidung::antwort(ebd, "A03", 3);
    }

    // 4 — Version höher als die bisher höchste verarbeitete?
    if let Some(hoechste) = p.bekannte_versionen.iter().max()
        && version <= *hoechste
    {
        return MabisEntscheidung::antwort(ebd, "A04", 4);
    }

    // 5 — Energiemengen plausibel?
    match p.energiemengen_plausibel {
        Some(true) => MabisEntscheidung::antwort(ebd, "A06", 5),
        Some(false) => MabisEntscheidung::antwort(ebd, "A05", 5),
        None => MabisEntscheidung::eskalation(
            "Entsprechen die Energiemengen den erwarteten Energiemengen?",
            5,
        ),
    }
}

/// Walk the **short** form: `E_0062`, `E_0063`, `E_0064`, `E_0098`, `E_0099`.
///
/// Three Prüfschritte. These series carry no Versionsangabe and their trees
/// state no Frist, so the Abweisung reduces to „have I already got this one".
///
/// # Panics
///
/// When `ebd` is not one of the five short-form trees.
#[must_use]
pub fn pruefe_zeitreihe_kurzform(
    ebd: &'static str,
    bereits_vorhanden: bool,
    plausibel: Option<bool>,
) -> MabisEntscheidung {
    assert!(
        matches!(ebd, "E_0062" | "E_0063" | "E_0064" | "E_0098" | "E_0099"),
        "{ebd} is not a short-form Zeitreihen-Tree"
    );
    if bereits_vorhanden {
        return MabisEntscheidung::antwort(ebd, "A01", 1);
    }
    match plausibel {
        Some(true) => MabisEntscheidung::antwort(ebd, "A03", 2),
        Some(false) => MabisEntscheidung::antwort(ebd, "A02", 2),
        None => MabisEntscheidung::eskalation(
            "Entsprechen die Energiemengen der Zeitreihe den erwarteten Energiemengen?",
            2,
        ),
    }
}

/// `E_0065` — DZÜ prüfen.
///
/// The DZÜ is only assessable once its **DZÜ-Liste** is held: without it the
/// receiver cannot know which Marktlokationen the series is supposed to cover,
/// so `A02` refuses rather than guessing at plausibility.
#[must_use]
pub fn pruefe_dzue(
    bereits_vorhanden: bool,
    dzue_liste_vorhanden: bool,
    plausibel: Option<bool>,
) -> MabisEntscheidung {
    if bereits_vorhanden {
        return MabisEntscheidung::antwort(EBD_DZUE, "A01", 1);
    }
    if !dzue_liste_vorhanden {
        return MabisEntscheidung::antwort(EBD_DZUE, "A02", 2);
    }
    match plausibel {
        Some(true) => MabisEntscheidung::antwort(EBD_DZUE, "A04", 3),
        Some(false) => MabisEntscheidung::antwort(EBD_DZUE, "A03", 3),
        None => MabisEntscheidung::eskalation(
            "Entsprechen die Energiemengen der DZÜ den erwarteten Energiemengen?",
            3,
        ),
    }
}
