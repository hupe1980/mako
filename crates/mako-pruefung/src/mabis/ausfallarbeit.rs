//! Redispatch-Ausfallarbeit — `E_0902` und `E_0901`. Prüfende Rolle: **NB**.
//!
//! **`E_0902` runs twice.** It is published once but applies „sowohl für die
//! Ausfallarbeitszeitreihe als auch für die Fahrplananteilzeitreihe", and BDEW
//! states the two runs can reach different results — so it is decided per
//! series, not once per message.
//!
//! **Its two Ablehnungen are two obligations.** `A02` and `A03` state the same
//! reason and differ in what the NB owes next: `A02` carries a
//! **Gegenvorschlag**, `A03` a **Korrekturanforderung**. Collapsing them drops
//! the obligation.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 Kap. 16.

use super::codes::{EBD_AUSFALLARBEIT, EBD_GEGENVORSCHLAG};
use super::types::MabisEntscheidung;

/// Which series an `E_0902` run is deciding.
///
/// Carried so the caller cannot silently reuse one verdict for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AusfallarbeitsZeitreihe {
    /// Die Ausfallarbeitszeitreihe.
    Ausfallarbeit,
    /// Die Fahrplananteilzeitreihe.
    Fahrplananteil,
}

/// `E_0902` — Ausfallarbeit unter Einbeziehung Fahrplananteil plausibilisieren.
///
/// `gegenvorschlag_moeglich` is read **only** when the figures are implausible.
#[must_use]
pub fn pruefe_ausfallarbeit(
    _zeitreihe: AusfallarbeitsZeitreihe,
    energiemengen_plausibel: Option<bool>,
    gegenvorschlag_moeglich: Option<bool>,
    erlaeuterung: Option<String>,
) -> MabisEntscheidung {
    match energiemengen_plausibel {
        Some(true) => return MabisEntscheidung::antwort(EBD_AUSFALLARBEIT, "A01", 1),
        None => {
            return MabisEntscheidung::eskalation(
                "Entsprechen die Energiemengen der Ausfallarbeitszeitreihe bzw. der \
                 Fahrplananteilzeitreihe den erwarteten Energiemengen?",
                1,
            );
        }
        Some(false) => {}
    }
    match gegenvorschlag_moeglich {
        Some(true) => MabisEntscheidung::antwort_mit(EBD_AUSFALLARBEIT, "A02", 2, erlaeuterung),
        Some(false) => MabisEntscheidung::antwort_mit(EBD_AUSFALLARBEIT, "A03", 2, erlaeuterung),
        None => MabisEntscheidung::eskalation("Kann ein Gegenvorschlag erstellt werden?", 2),
    }
}

/// The facts `E_0901` („Gegenvorschlag prüfen") walks.
#[derive(Debug, Clone, Copy, Default)]
pub struct GegenvorschlagPruefung {
    /// Prüfschritt 1 — no Zustimmung to the Ausfallarbeitszeitreihe is on file
    /// yet. Once one is, the series is settled and no Gegenvorschlag is
    /// admissible.
    pub noch_keine_zustimmung: Option<bool>,
    /// Prüfschritt 2 — it arrived inside the Frist.
    pub frist_gewahrt: Option<bool>,
    /// Prüfschritt 3 — no earlier Gegenvorschlag exists. Exactly one is
    /// admissible per Ausfallarbeitszeitreihe.
    pub kein_frueherer_gegenvorschlag: Option<bool>,
    /// Prüfschritt 4 — its energy figures are plausible.
    pub energiemengen_plausibel: Option<bool>,
}

/// `E_0901` — Gegenvorschlag prüfen. Prüfende Rolle: **NB**.
#[must_use]
pub fn pruefe_gegenvorschlag(g: &GegenvorschlagPruefung) -> MabisEntscheidung {
    for (fakt, code, nr, frage) in [
        (
            g.noch_keine_zustimmung,
            "A01",
            1u16,
            "Liegt für die Ausfallarbeitszeitreihe bereits eine Zustimmung vor?",
        ),
        (
            g.frist_gewahrt,
            "A02",
            2,
            "Ist der Gegenvorschlag innerhalb der vorgegebenen Frist eingegangen?",
        ),
        (
            g.kein_frueherer_gegenvorschlag,
            "A03",
            3,
            "Liegt bereits ein Gegenvorschlag zur Ausfallarbeitszeitreihe vor?",
        ),
        (
            g.energiemengen_plausibel,
            "A04",
            4,
            "Können die Energiemengen des Gegenvorschlages nachvollzogen werden?",
        ),
    ] {
        match fakt {
            Some(true) => {}
            Some(false) => return MabisEntscheidung::antwort(EBD_GEGENVORSCHLAG, code, nr),
            None => return MabisEntscheidung::eskalation(frage, nr),
        }
    }
    MabisEntscheidung::antwort(EBD_GEGENVORSCHLAG, "A05", 4)
}
