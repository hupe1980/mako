//! The four walks of the Modellwechsel — EBD 4.3 Kapitel 17.
//!
//! # The shape of the process, and why the answer is not immediate
//!
//! An Anmeldung in Modell 2 (55238, NB (LPB) → NB (VNB)) is **not** answered by
//! one decision. The VNB runs three steps in order:
//!
//! 1. [`pruefe_direkt_ablehnbar`] (`E_0513`) — is the Anmeldung refusable on
//!    its face? If yes, `A99` goes back on 55239 straight away. If no, the
//!    message is handed to `E_0514`.
//! 2. `E_0514` publishes no tree, because no answer is given there. What it
//!    stands for is the **55240 leg**: the VNB tells the MaLo's LF that its
//!    Zuordnung ends (AWH Kap. 2.1.2 Nr. 2, „unverzüglich, jedoch spätestens
//!    bis zum Ablauf des 3. WT nach Eingang der Anmeldung"). The LF answers on
//!    55241 through [`pruefe_beendigung`] (`E_0511`), within its own 3 WT
//!    (Nr. 3).
//! 3. [`pruefe_anmeldung`] (`E_0510`) — only now does the VNB answer the LPB
//!    on 55239, and its first Prüfschritt is „Ging innerhalb der Antwortfrist
//!    eine Ablehnung des Lieferanten ein?".
//!
//! The windows those three steps run in are
//! `mako_fristen::antwort::MODELL_2_ANMELDUNG_ANTWORT_WT` (7) and
//! `MODELL_2_DREI_WERKTAGE`, which is where the arithmetic behind them is kept.
//!
//! ## `A99` is never a stand-in for „unknown"
//!
//! Every tree's only refusal below the specific codes is `A99` „Sonstiges", an
//! **Ablehnung** that must carry a written `FTX+ACB` reason and whose
//! Nutzungsmöglichkeit ends 01.04.2027. A fact the caller cannot state
//! escalates to an operator; it never becomes an `A99`.

use super::codes::{
    EBD_ABMELDUNG, EBD_ANMELDUNG, EBD_BEENDIGUNG, EBD_BEENDIGUNG_ANSTOSSEN, EBD_DIREKT_ABLEHNBAR,
};
use super::types::EmobEntscheidung;

/// „Ist ein Fehler aufgetreten?" — the single Prüfschritt every Modell-2 tree
/// ends on.
///
/// The EBD states it as a question only the checking party can answer, and its
/// „ja" branch obliges a written description („Das identifizierte Problem ist
/// in der Antwort zu beschreiben/benennen"). It is therefore modelled as an
/// already-made decision carrying its own Erläuterung rather than as a bare
/// `bool`: an `A99` without text is an incomplete answer the receiver's AHB
/// layer rejects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fehler {
    /// The `FTX+ACB` text describing the problem. `None` means „no error".
    pub beschreibung: Option<String>,
}

impl Fehler {
    /// No error occurred.
    #[must_use]
    pub const fn keiner() -> Self {
        Self { beschreibung: None }
    }

    /// An error occurred, described in the caller's own words.
    #[must_use]
    pub fn mit(beschreibung: impl Into<String>) -> Self {
        Self {
            beschreibung: Some(beschreibung.into()),
        }
    }

    fn ist_fehler(&self) -> bool {
        self.beschreibung.is_some()
    }
}

/// `E_0513` — Prüfen, ob Anmeldung direkt ablehnbar. Prüfende Rolle: **NB** (VNB).
///
/// | Nr. | Prüfschritt | ja | nein |
/// |---|---|---|---|
/// | 1 | Ist ein Fehler aufgetreten? | `A99` Ablehnung | → `E_0514` |
///
/// The „nein" branch is [`EmobEntscheidung::Weiter`], **not** an agreement:
/// what follows is the 55240 leg to the LF, and the LPB's answer comes later
/// from [`pruefe_anmeldung`].
#[must_use]
pub fn pruefe_direkt_ablehnbar(fehler: &Fehler) -> EmobEntscheidung {
    if fehler.ist_fehler() {
        return EmobEntscheidung::antwort_mit(
            EBD_DIREKT_ABLEHNBAR,
            "A99",
            1,
            fehler.beschreibung.clone(),
        );
    }
    EmobEntscheidung::Weiter {
        naechster_baum: EBD_BEENDIGUNG_ANSTOSSEN,
    }
}

/// The facts `E_0510` („Anmeldung prüfen") walks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Anmeldung {
    /// Prüfschritt 1 — „Ging innerhalb der Antwortfrist eine Ablehnung des
    /// Lieferanten ein?"
    ///
    /// `Some(true)` means the LF refused the Beendigung **and** did so inside
    /// its 3-Werktage window. A refusal that arrived late is not this fact:
    /// the Prüfschritt is explicit about „innerhalb der Antwortfrist", so a
    /// late `A99` on 55241 leaves this `Some(false)`.
    ///
    /// `None` escalates. It is the honest value while the LF's window is still
    /// running and nothing has arrived — the answer is simply not yet
    /// determined, and guessing `false` would confirm an Anmeldung the LF may
    /// still refuse.
    pub lf_ablehnung_fristgerecht: Option<bool>,
    /// Prüfschritt 2 — „Ist ein Fehler aufgetreten?"
    pub fehler: Fehler,
}

/// `E_0510` — Anmeldung prüfen. Prüfende Rolle: **NB** (VNB).
///
/// | Nr. | Prüfschritt | ja | nein |
/// |---|---|---|---|
/// | 1 | Ging innerhalb der Antwortfrist eine Ablehnung des Lieferanten ein? | `A01` Ablehnung | → 2 |
/// | 2 | Ist ein Fehler aufgetreten? | `A99` Ablehnung | `A02` Zustimmung |
#[must_use]
pub fn pruefe_anmeldung(a: &Anmeldung) -> EmobEntscheidung {
    match a.lf_ablehnung_fristgerecht {
        Some(true) => return EmobEntscheidung::antwort(EBD_ANMELDUNG, "A01", 1),
        None => {
            return EmobEntscheidung::eskalation(
                "Ging innerhalb der Antwortfrist eine Ablehnung des Lieferanten ein?",
                1,
            );
        }
        Some(false) => {}
    }
    if a.fehler.ist_fehler() {
        return EmobEntscheidung::antwort_mit(
            EBD_ANMELDUNG,
            "A99",
            2,
            a.fehler.beschreibung.clone(),
        );
    }
    EmobEntscheidung::antwort(EBD_ANMELDUNG, "A02", 2)
}

/// `E_0511` — Beendigung der Zuordnung prüfen. Prüfende Rolle: **LF**.
///
/// | Nr. | Prüfschritt | ja | nein |
/// |---|---|---|---|
/// | 1 | Ist ein Fehler aufgetreten? | `A99` Ablehnung | `A01` **Zustimmung** |
///
/// The LF is not being asked whether it consents to losing the Marktlokation —
/// the Anmeldung in Modell 2 is the LPB's right, and this leg only tells the LF
/// its Zuordnung ends. `A01` here confirms the Beendigung; the same code in
/// [`pruefe_anmeldung`] refuses.
#[must_use]
pub fn pruefe_beendigung(fehler: &Fehler) -> EmobEntscheidung {
    if fehler.ist_fehler() {
        return EmobEntscheidung::antwort_mit(
            EBD_BEENDIGUNG,
            "A99",
            1,
            fehler.beschreibung.clone(),
        );
    }
    EmobEntscheidung::antwort(EBD_BEENDIGUNG, "A01", 1)
}

/// `E_0512` — Abmeldung prüfen. Prüfende Rolle: **NB** (VNB).
///
/// | Nr. | Prüfschritt | ja | nein |
/// |---|---|---|---|
/// | 1 | Ist ein Fehler aufgetreten? | `A99` Ablehnung | `A01` **Zustimmung** |
#[must_use]
pub fn pruefe_abmeldung(fehler: &Fehler) -> EmobEntscheidung {
    if fehler.ist_fehler() {
        return EmobEntscheidung::antwort_mit(EBD_ABMELDUNG, "A99", 1, fehler.beschreibung.clone());
    }
    EmobEntscheidung::antwort(EBD_ABMELDUNG, "A01", 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direkt_ablehnbar_hands_over_when_nothing_is_wrong() {
        assert_eq!(
            pruefe_direkt_ablehnbar(&Fehler::keiner()),
            EmobEntscheidung::Weiter {
                naechster_baum: EBD_BEENDIGUNG_ANSTOSSEN
            }
        );
    }

    #[test]
    fn direkt_ablehnbar_carries_the_written_reason() {
        let d =
            pruefe_direkt_ablehnbar(&Fehler::mit("BG des LPB ist am Anmeldedatum nicht gültig"));
        let a = d.antwort_ref().unwrap();
        assert_eq!(a.code, "A99");
        assert_eq!(a.pruefschritt, 1);
        assert!(!a.ist_zustimmung());
        assert_eq!(
            a.bemerkung.as_deref(),
            Some("BG des LPB ist am Anmeldedatum nicht gültig")
        );
    }

    #[test]
    fn anmeldung_confirms_when_the_lf_did_not_refuse_in_time() {
        let d = pruefe_anmeldung(&Anmeldung {
            lf_ablehnung_fristgerecht: Some(false),
            fehler: Fehler::keiner(),
        });
        let a = d.antwort_ref().unwrap();
        assert_eq!(a.code, "A02");
        assert!(a.ist_zustimmung());
    }

    #[test]
    fn anmeldung_refuses_on_a_timely_lf_rejection() {
        let d = pruefe_anmeldung(&Anmeldung {
            lf_ablehnung_fristgerecht: Some(true),
            fehler: Fehler::keiner(),
        });
        let a = d.antwort_ref().unwrap();
        assert_eq!(a.code, "A01");
        assert!(!a.ist_zustimmung());
    }

    /// An unfinished LF window must not be read as „the LF did not refuse".
    #[test]
    fn anmeldung_escalates_while_the_lf_window_is_open() {
        let d = pruefe_anmeldung(&Anmeldung::default());
        assert!(matches!(
            d,
            EmobEntscheidung::Eskalation {
                pruefschritt: 1,
                ..
            }
        ));
    }

    #[test]
    fn a_timely_lf_rejection_wins_over_an_error() {
        // Prüfschritt 1 precedes Prüfschritt 2: the tree is ordered.
        let d = pruefe_anmeldung(&Anmeldung {
            lf_ablehnung_fristgerecht: Some(true),
            fehler: Fehler::mit("irgendein Fehler"),
        });
        assert_eq!(d.antwort_ref().unwrap().code, "A01");
    }

    #[test]
    fn beendigung_and_abmeldung_confirm_with_a01() {
        for d in [
            pruefe_beendigung(&Fehler::keiner()),
            pruefe_abmeldung(&Fehler::keiner()),
        ] {
            let a = d.antwort_ref().unwrap();
            assert_eq!(a.code, "A01");
            assert!(a.ist_zustimmung(), "{} A01 is a Zustimmung", a.tree);
        }
    }

    #[test]
    fn every_a99_is_a_refusal_that_carries_text() {
        for d in [
            pruefe_direkt_ablehnbar(&Fehler::mit("x")),
            pruefe_beendigung(&Fehler::mit("x")),
            pruefe_abmeldung(&Fehler::mit("x")),
            pruefe_anmeldung(&Anmeldung {
                lf_ablehnung_fristgerecht: Some(false),
                fehler: Fehler::mit("x"),
            }),
        ] {
            let a = d.antwort_ref().unwrap();
            assert_eq!(a.code, "A99");
            assert!(!a.ist_zustimmung());
            assert!(a.braucht_bemerkung);
            assert_eq!(a.bemerkung.as_deref(), Some("x"));
        }
    }
}
