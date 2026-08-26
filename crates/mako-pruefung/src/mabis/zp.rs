//! MaBiS-Zählpunkt — Aktivierung, Deaktivierung und Zuordnung.
//!
//! All four trees are **ordered gates**: a sequence of yes/no Prüfschritte,
//! each with its own Ablehnungscode, ending in a single Zustimmung. Nothing
//! branches and nothing is skipped, so they share one walk — what differs is
//! the gate list, which is the published tree.
//!
//! Unlike the Summenzeitreihen-Trees these refuse with plain
//! [`Cluster::Ablehnung`]: an Aktivierung that misses its Frist has still been
//! assessed, so its Prüfmitteilung is forwarded.
//!
//! [`Cluster::Ablehnung`]: crate::codes::Cluster::Ablehnung
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3.

use super::codes::{EBD_ZP_AKTIVIERUNG, EBD_ZP_BEENDIGUNG, EBD_ZP_DEAKTIVIERUNG, EBD_ZP_ZUORDNUNG};
use super::types::MabisEntscheidung;

/// One Prüfschritt: the fact, the code it emits when the fact is `false`, the
/// published Prüfschritt number, and the tree's own wording of the question.
///
/// `None` is unknown and escalates — never „no".
type Schritt<'a> = (Option<bool>, &'static str, u16, &'a str);

/// Walk an ordered gate list and land on `zustimmung` when every gate passes.
fn walk(
    ebd: &'static str,
    schritte: &[Schritt<'_>],
    zustimmung: (&'static str, u16),
    sonstiges: Option<(&'static str, u16, Option<String>)>,
) -> MabisEntscheidung {
    for &(fakt, code, nr, frage) in schritte {
        match fakt {
            Some(true) => {}
            Some(false) => return MabisEntscheidung::antwort(ebd, code, nr),
            None => return MabisEntscheidung::eskalation(frage, nr),
        }
    }
    // „Ist ein nicht spezifizierter Fehler aufgetreten?" — the last gate of the
    // Zuordnungs-Trees. It cannot be expressed as a fact because only the
    // caller knows it, so it arrives already decided, with its Erläuterung.
    if let Some((code, nr, detail)) = sonstiges
        && detail.is_some()
    {
        return MabisEntscheidung::antwort_mit(ebd, code, nr, detail);
    }
    MabisEntscheidung::antwort(ebd, zustimmung.0, zustimmung.1)
}

/// The facts `E_0020` („MaBiS-ZP Aktivierung prüfen") walks.
///
/// Every field states whether the check **passed**. `None` escalates.
#[derive(Debug, Clone, Copy, Default)]
pub struct Aktivierung {
    /// Prüfschritt 1 — the Frist was met.
    pub frist_gewahrt: Option<bool>,
    /// Prüfschritt 2 — the requested Zeitpunkt is permitted.
    pub zeitpunkt_zulaessig: Option<bool>,
    /// Prüfschritt 3 — the ID is not already in use outside MaBiS.
    pub id_frei: Option<bool>,
    /// Prüfschritt 4 — the neighbouring NB's Bilanzierungsgebiet is valid.
    pub bilanzierungsgebiet_gueltig: Option<bool>,
    /// Prüfschritt 5 — the Regelzone is right.
    pub regelzone_korrekt: Option<bool>,
    /// Prüfschritt 6 — the sender is entitled to activate.
    pub berechtigt: Option<bool>,
    /// Prüfschritt 7 — no deviating MaBiS-ZP is already held.
    pub kein_abweichender_zp: Option<bool>,
    /// Prüfschritt 8 — no deviating ID is already held for this MaBiS-ZP.
    pub keine_abweichende_id: Option<bool>,
    /// Prüfschritt 9 — the Zuordnungsermächtigung permits the activation.
    pub zrt_berechtigt: Option<bool>,
    /// Prüfschritt 10 — the OBIS-Kennzahl fits.
    pub obis_passend: Option<bool>,
    /// Prüfschritt 11 — the MaBiS-ZP is not already active.
    pub nicht_bereits_aktiv: Option<bool>,
}

/// `E_0020` — MaBiS-ZP Aktivierung prüfen. Prüfende Rolle: **NB**.
#[must_use]
pub fn pruefe_aktivierung(a: &Aktivierung) -> MabisEntscheidung {
    walk(
        EBD_ZP_AKTIVIERUNG,
        &[
            (a.frist_gewahrt, "A01", 1, "Wurde die Frist eingehalten?"),
            (
                a.zeitpunkt_zulaessig,
                "A02",
                2,
                "Ist der gewählte Zeitpunkt zulässig?",
            ),
            (
                a.id_frei,
                "A03",
                3,
                "Wird die ID bereits außerhalb MaBiS verwendet?",
            ),
            (
                a.bilanzierungsgebiet_gueltig,
                "A04",
                4,
                "Ist das Bilanzierungsgebiet des benachbarten NB gültig?",
            ),
            (a.regelzone_korrekt, "A05", 5, "Ist die Regelzone korrekt?"),
            (a.berechtigt, "A06", 6, "Besteht eine Berechtigung?"),
            (
                a.kein_abweichender_zp,
                "A07",
                7,
                "Ist ein abweichender MaBiS-ZP bereits vorhanden?",
            ),
            (
                a.keine_abweichende_id,
                "A08",
                8,
                "Ist eine abweichende ID zum MaBiS-ZP bereits vorhanden?",
            ),
            (
                a.zrt_berechtigt,
                "A09",
                9,
                "Ist die ZRT-Aktivierung berechtigt?",
            ),
            (a.obis_passend, "A10", 10, "Passt die OBIS-Kennzahl?"),
            (
                a.nicht_bereits_aktiv,
                "A11",
                11,
                "Ist der MaBiS-ZP bereits aktiviert?",
            ),
        ],
        ("A12", 11),
        None,
    )
}

/// The facts `E_0010` („MaBiS-ZP Deaktivierung prüfen") walks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Deaktivierung {
    /// Prüfschritt 1 — the Frist was met.
    pub frist_gewahrt: Option<bool>,
    /// Prüfschritt 2 — the requested Zeitpunkt is permitted.
    pub zeitpunkt_zulaessig: Option<bool>,
    /// Prüfschritt 3 — the ID is not already in use outside MaBiS.
    pub id_frei: Option<bool>,
    /// Prüfschritt 4 — the MaBiS-ZP is not already deactivated.
    pub nicht_bereits_deaktiviert: Option<bool>,
    /// Prüfschritt 5 — no Zeitreihen are held that would block the deactivation.
    pub keine_zeitreihen_vorhanden: Option<bool>,
}

/// `E_0010` — MaBiS-ZP Deaktivierung prüfen. Prüfende Rolle: **NB**.
#[must_use]
pub fn pruefe_deaktivierung(d: &Deaktivierung) -> MabisEntscheidung {
    walk(
        EBD_ZP_DEAKTIVIERUNG,
        &[
            (d.frist_gewahrt, "A01", 1, "Wurde die Frist eingehalten?"),
            (
                d.zeitpunkt_zulaessig,
                "A02",
                2,
                "Ist der gewählte Zeitpunkt zulässig?",
            ),
            (
                d.id_frei,
                "A03",
                3,
                "Wird die ID bereits außerhalb MaBiS verwendet?",
            ),
            (
                d.nicht_bereits_deaktiviert,
                "A04",
                4,
                "Ist der MaBiS-ZP bereits deaktiviert?",
            ),
            (
                d.keine_zeitreihen_vorhanden,
                "A05",
                5,
                "Sind zum MaBiS-ZP noch Zeitreihen vorhanden?",
            ),
        ],
        ("A06", 5),
        None,
    )
}

/// The facts the Zuordnungs-Trees (`E_0102`, `E_0103`) walk.
#[derive(Debug, Clone, Default)]
pub struct Zuordnung {
    /// `E_0102` Prüfschritt 1 — the ID is not already in use outside MaBiS.
    /// Not asked by `E_0103`, which ends an existing Zuordnung.
    pub id_frei: Option<bool>,
    /// The Zuordnung resp. its Beendigung matches the Vereinbarung zur
    /// messtechnischen Abgrenzung of the two neighbouring NB.
    pub passt_zur_vereinbarung: Option<bool>,
    /// The sender is entitled to the Netzzeitreihe.
    pub berechtigt: Option<bool>,
    /// The receiver participates in the Netzzeitreihe.
    pub beteiligt: Option<bool>,
    /// `E_0102`: the Zuordnung is not already held.
    /// `E_0103`: the Zuordnung to be ended **does** exist.
    pub zuordnungslage_ok: Option<bool>,
    /// An unspecified error, with the Erläuterung the code requires. `Some`
    /// produces `A99`; `None` lets the walk reach its Zustimmung.
    pub sonstiges: Option<String>,
}

/// `E_0102` — Zuordnung prüfen. Prüfende Rolle: **NB**.
///
/// `A05` („Zuordnung bereits vorhanden") requires the already-assigned
/// Netzzeitreihe to be named in the answer — [`crate::AntwortCode::braucht_bemerkung`]
/// is set on it.
#[must_use]
pub fn pruefe_zuordnung(z: &Zuordnung) -> MabisEntscheidung {
    walk(
        EBD_ZP_ZUORDNUNG,
        &[
            (
                z.id_frei,
                "A01",
                1,
                "Wird die ID bereits außerhalb MaBiS verwendet?",
            ),
            (
                z.passt_zur_vereinbarung,
                "A02",
                2,
                "Passt die Zuordnung zur Vereinbarung zur messtechnischen Abgrenzung?",
            ),
            (
                z.berechtigt,
                "A03",
                3,
                "Besteht eine Berechtigung für die Netzzeitreihe?",
            ),
            (
                z.beteiligt,
                "A04",
                4,
                "Ist der Empfänger an der Netzzeitreihe beteiligt?",
            ),
            (
                z.zuordnungslage_ok,
                "A05",
                5,
                "Ist die Zuordnung bereits vorhanden?",
            ),
        ],
        ("A06", 6),
        Some(("A99", 6, z.sonstiges.clone())),
    )
}

/// `E_0103` — Beendigung der Zuordnung prüfen. Prüfende Rolle: **NB**.
///
/// [`Zuordnung::id_frei`] is not read: ending a Zuordnung asserts nothing about
/// the ID.
#[must_use]
pub fn pruefe_beendigung_zuordnung(z: &Zuordnung) -> MabisEntscheidung {
    walk(
        EBD_ZP_BEENDIGUNG,
        &[
            (
                z.passt_zur_vereinbarung,
                "A01",
                1,
                "Passt die Beendigung der Zuordnung zur Vereinbarung zur messtechnischen Abgrenzung?",
            ),
            (
                z.berechtigt,
                "A02",
                2,
                "Besteht eine Berechtigung für die Netzzeitreihe?",
            ),
            (
                z.beteiligt,
                "A03",
                3,
                "Ist der Empfänger an der Netzzeitreihe beteiligt?",
            ),
            (
                z.zuordnungslage_ok,
                "A04",
                4,
                "Existiert zum Zuordnungsende eine Zuordnung?",
            ),
        ],
        ("A05", 5),
        Some(("A99", 5, z.sonstiges.clone())),
    )
}
