//! **NZR-EMob / Modell 2** — the answers a VNB and an LF owe when a
//! Ladepunktbetreiber moves a Marktlokation into or out of the virtual
//! Bilanzierungsgebiet.
//!
//! Four Prüfidentifikatoren carry an obligation here, and the prüfende Rolle is
//! not the same for all of them:
//!
//! | Inbound | From → To | Tree | Answered on | Prüfende Rolle |
//! |---|---|---|---|---|
//! | 55238 Anmeldung in Modell 2 | NB (LPB) → NB (VNB) | [`pruefe_direkt_ablehnbar`] `E_0513`, then [`pruefe_anmeldung`] `E_0510` | 55239 | NB (VNB) |
//! | 55240 Beendigung der Zuordnung zur MaLo | NB (VNB) → LF | [`pruefe_beendigung`] `E_0511` | 55241 | **LF** |
//! | 55242 Abmeldung aus dem Modell 2 | NB (LPB) → NB (VNB) | [`pruefe_abmeldung`] `E_0512` | 55243 | NB (VNB) |
//!
//! `E_0514` is named but publishes nothing — see
//! [`codes::EBD_BEENDIGUNG_ANSTOSSEN`].
//!
//! # What is *not* here
//!
//! The Zuordnung des Zählpunkts der NGZ zur NZR (55235 / 55236 / 55237) belongs
//! to the MaBiS-Ergänzung, not to Kapitel 17, and its answers are `E_0102`
//! (Zuordnung) and `E_0103` (Beendigung der Zuordnung) — both already in
//! [`crate::mabis`]. A deployment that runs Modell 2 therefore needs
//! `role-mabis` as well as `role-emob`; there are no Modell-2-specific trees
//! for those three Prüfidentifikatoren to write.
//!
//! # Sources
//!
//! - BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 (23.06.2026) Kap. 17
//! - BDEW AWH „Zum Modell 2 zur ladevorgangscharfen bilanziellen
//!   Energiemengenzuordnungsmöglichkeit" V1.3 (01.04.2025) Kap. 2.1.2 / 2.2.2
//! - EDI@Energy Anwendungsübersicht der Prüfidentifikatoren 4.0, Lfd. Nr.
//!   19000 / 19010 / 19020 / 19030 / 19050 / 19060
//! - Anlage 6 zum Beschluss BK6-20-160 („NZR-EMob")

pub mod codes;
pub mod modellwechsel;
pub mod types;

pub use codes::{
    EBD_ABMELDUNG, EBD_ANMELDUNG, EBD_BEENDIGUNG, EBD_BEENDIGUNG_ANSTOSSEN, EBD_DIREKT_ABLEHNBAR,
};
pub use modellwechsel::{
    Anmeldung, Fehler, pruefe_abmeldung, pruefe_anmeldung, pruefe_beendigung,
    pruefe_direkt_ablehnbar,
};
pub use types::{EmobAntwort, EmobEntscheidung};

/// The Prüfidentifikator an answer to `trigger_pid` rides on, and the tree that
/// decides it.
///
/// Both Modell-2 answers use **one** PID per direction rather than a
/// Bestätigungs-/Ablehnungs-pair: 55239 carries `E_0510` *and* `E_0513`, 55243
/// carries `E_0512`, and the agreement lives in `SG4 STS+E01` DE 9013 with the
/// tree named in DE 1131. A caller that derives an answer PID from the cluster
/// — the GPKE habit — would look for a PID that does not exist.
#[must_use]
pub fn antwort_pid(trigger_pid: u32) -> Option<(u32, &'static [&'static str])> {
    Some(match trigger_pid {
        55_238 => (55_239, &[EBD_DIREKT_ABLEHNBAR, EBD_ANMELDUNG]),
        55_240 => (55_241, &[EBD_BEENDIGUNG]),
        55_242 => (55_243, &[EBD_ABMELDUNG]),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_pids_match_the_anwendungsuebersicht() {
        assert_eq!(antwort_pid(55_238).unwrap().0, 55_239);
        assert_eq!(antwort_pid(55_240).unwrap().0, 55_241);
        assert_eq!(antwort_pid(55_242).unwrap().0, 55_243);
    }

    /// The answer PIDs are answers; they never start a process of their own.
    #[test]
    fn answer_pids_are_not_triggers() {
        for pid in [55_239, 55_241, 55_243] {
            assert!(
                antwort_pid(pid).is_none(),
                "{pid} is an answer, not a trigger"
            );
        }
    }

    /// 55235–55237 are MaBiS, and deliberately absent here.
    #[test]
    fn ngz_zuordnung_is_not_a_modell_2_tree() {
        for pid in [55_235, 55_236, 55_237] {
            assert!(antwort_pid(pid).is_none());
        }
    }

    #[test]
    fn the_anmeldung_answer_names_both_of_its_trees() {
        let (_, trees) = antwort_pid(55_238).unwrap();
        assert_eq!(trees, &[EBD_DIREKT_ABLEHNBAR, EBD_ANMELDUNG]);
    }
}
