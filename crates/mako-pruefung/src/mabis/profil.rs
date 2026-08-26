//! `E_0100` — Profile bzw. Profilscharen prüfen. Prüfende Rolle: **LF**.
//!
//! The only MaBiS tree that answers **nothing** on success: an acceptable
//! profile is acknowledged by using it. Every published code is a
//! Reklamationsgrund, and a Reklamation does not invalidate the profile it
//! complains about — the LF keeps bilanzierend with it until a corrected
//! version arrives.
//!
//! Prüfschritt 2 splits the tree and the halves do not rejoin: a **Profil** is
//! checked only for its Abonnement and Version (`A01`, `A02`), while `A03`–`A06`
//! — the Profilschar Version, the Maßeinheit and the two Temperaturmaßzahl
//! steps — are reachable only from the **Profilschar** half.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 Kap. 7.11.1.

use super::codes::EBD_PROFILE;
use super::types::MabisEntscheidung;

/// What arrived — the two halves of the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profilart {
    /// A single Profil: Prüfschritte 1–3 only.
    Profil,
    /// A Profilschar: Prüfschritte 1, 2, 4–7.
    Profilschar,
}

/// The facts `E_0100` walks. `None` is unknown and escalates.
#[derive(Debug, Clone, Copy)]
pub struct ProfilPruefung {
    /// Whether a Profil or a Profilschar arrived.
    pub art: Profilart,
    /// Prüfschritt 1 — it belongs to a previously subscribed Profilgruppe.
    pub abonniert: Option<bool>,
    /// Prüfschritt 3 resp. 4 — the version is higher than the highest already
    /// processed for the same period.
    pub version_hoeher: Option<bool>,
    /// Prüfschritt 5 — the OBIS-Kennzahl's Maßeinheit matches the
    /// Normierungsfaktor from the Liste der Profildefinitionen.
    /// Read on the Profilschar branch only.
    pub masseinheit_passt: Option<bool>,
    /// Prüfschritt 6 — the lowest Temperaturmaßzahl matches the
    /// Begrenzungskonstante. Profilschar branch only.
    pub niedrigste_temperaturmasszahl_passt: Option<bool>,
    /// Prüfschritt 7 — the number of Temperaturmaßzahlen matches.
    /// Profilschar branch only.
    pub anzahl_temperaturmasszahlen_passt: Option<bool>,
}

/// `E_0100` — decide whether a Reklamation is owed.
///
/// Returns [`MabisEntscheidung::Schweigen`] when the profile is acceptable.
#[must_use]
pub fn pruefe_profil(p: &ProfilPruefung) -> MabisEntscheidung {
    // 1 — Gehört das Profil bzw. die Profilschar zu einer abonnierten Profilgruppe?
    match p.abonniert {
        Some(false) => return MabisEntscheidung::antwort(EBD_PROFILE, "A01", 1),
        None => {
            return MabisEntscheidung::eskalation(
                "Gehört das empfangene Profil bzw. die Profilschar zu einer zuvor \
                 abonnierten Profilgruppe aus der Liste der Profildefinitionen?",
                1,
            );
        }
        Some(true) => {}
    }

    // 2 — Wurde eine Profilschar empfangen? The branch does not rejoin.
    let (version_code, version_schritt) = match p.art {
        Profilart::Profil => ("A02", 3),
        Profilart::Profilschar => ("A03", 4),
    };
    match p.version_hoeher {
        Some(false) => {
            return MabisEntscheidung::antwort(EBD_PROFILE, version_code, version_schritt);
        }
        None => {
            return MabisEntscheidung::eskalation(
                "Ist die übermittelte Version höher als die bisher höchste verarbeitete \
                 Version des gleichen Zeitraums?",
                version_schritt,
            );
        }
        Some(true) => {}
    }

    if p.art == Profilart::Profil {
        // Prüfschritt 3 „ja → Ende": a Profil has no further steps.
        return MabisEntscheidung::Schweigen;
    }

    for (fakt, code, nr, frage) in [
        (
            p.masseinheit_passt,
            "A04",
            5u16,
            "Stimmt die Maßeinheit der verwendeten OBIS-Kennzahl mit der Maßeinheit des \
             Normierungsfaktors aus der Liste der Profildefinitionen überein?",
        ),
        (
            p.niedrigste_temperaturmasszahl_passt,
            "A05",
            6,
            "Entspricht die niedrigste Temperaturmaßzahl der Profilschar der \
             Begrenzungskonstante aus der Liste der Profildefinitionen?",
        ),
        (
            p.anzahl_temperaturmasszahlen_passt,
            "A06",
            7,
            "Entspricht die Anzahl der Temperaturmaßzahlen der Liste der Profildefinitionen?",
        ),
    ] {
        match fakt {
            Some(true) => {}
            Some(false) => return MabisEntscheidung::antwort(EBD_PROFILE, code, nr),
            None => return MabisEntscheidung::eskalation(frage, nr),
        }
    }

    MabisEntscheidung::Schweigen
}
