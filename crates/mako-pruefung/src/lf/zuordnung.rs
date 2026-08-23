//! **Ankündigung Zuordnung LF** — the NB assigns a supplier to an *erzeugende*
//! Marktlokation or Tranche, and the supplier answers.
//!
//! | Inbound | EBD | Anwendungsfall | Answers |
//! |---|---|---|---|
//! | 55607 | `E_0603` | EEG-MaLo **ohne** DV-Pflicht bzw. KWKG-MaLo ohne DV-Pflicht | 55608 / 55609 |
//! | 55607 | `E_0604` | EEG-MaLo **mit** DV-Pflicht | 55608 / 55609 |
//! | 55607 | `E_0605` | KWKG-MaLo mit DV-Pflicht bzw. Nicht-EEG-/Nicht-KWKG-MaLo, nicht-tranchiert | 55608 / 55609 |
//! | 55607 | `E_0606` | dieselben Fälle, **tranchiert** abgebildet | 55608 / 55609 |
//!
//! Strom only: assigning a supplier to an erzeugende Marktlokation is a
//! Bilanzkreis mechanic GeLi Gas has no counterpart for.
//!
//! Every erzeugende Marktlokation must be assigned to exactly one Bilanzkreis at
//! every instant; when it is not, the NB restores the 100 % assignment and
//! announces it. **Silence is consent:** Prozessschritt 3 of all four
//! Sequenzdiagramme is „Zuordnung … **aufgrund fehlender Antwort**" — past the
//! window (15:00 Uhr am ÜT, [`mako_fristen::antwort`]) the NB assigns anyway,
//! using whichever Bilanzkreis the supplier last deposited.
//!
//! All four trees are a single Prüfschritt — „Ist ein zuvor nicht spezifizierter
//! Fehler aufgetreten?", ja → `A99`, nein → `A01`. There is no published ground
//! to refuse on, so the substance of the answer is the **Bilanzkreis**, not the
//! code: see [`Bilanzkreisart`] for which one the Anwendungsfall calls for.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 6.50–6.53;
//! BK6-24-174 GPKE Teil 2 § 2.4.

use crate::codes::{AntwortCode, E_0603_CODES, E_0604_CODES, E_0605_CODES, E_0606_CODES};
use crate::lf::types::{LfAnfrage, LfEntscheidung};

/// Which of the four Anwendungsfälle the inbound 55607 belongs to.
///
/// The message names its EBD in `SG4 STS+E01` DE 1131, and the four differ only
/// in the Bilanzkreis the Zustimmung must name — so the caller resolves the id
/// rather than the walk guessing from Stammdaten it may not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ZuordnungsFall {
    /// `E_0603` — EEG-Marktlokation ohne DV-Pflicht bzw. KWKG-Marktlokation
    /// ohne DV-Pflicht. The Zustimmung names the EEG-BK or the KWKG-BK.
    EegOhneDvPflicht,
    /// `E_0604` — EEG-Marktlokation mit DV-Pflicht. The Zustimmung names the
    /// EEG-BK.
    EegMitDvPflicht,
    /// `E_0605` — KWKG-Marktlokation mit DV-Pflicht bzw. Nicht-EEG-/Nicht-KWKG-
    /// Marktlokation, nicht-tranchiert abgebildet.
    KwkgNichtTranchiert,
    /// `E_0606` — dieselben Fälle, tranchiert abgebildet. Runs once per Tranche.
    KwkgTranchiert,
}

/// Which of a supplier's Bilanzkreise an announced Zuordnung is balanced in.
///
/// A Lieferant holds several, and the answer is a **choice** among those the
/// BKV has authorised: MaBiS (BK6-24-174 § 10.2.1) grants the
/// Zuordnungsermächtigung „je ZRT, BG, BK und LF", so for one Zeitreihentyp in
/// one Bilanzierungsgebiet a supplier may have more than one admissible BK.
///
/// What is *not* free is the regime. GPKE Teil 2 § 2.4.2.2 Nr. 2 prescribes the
/// EEG-BK for an EEG-Marktlokation and the KWKG-BK for a KWKG one; directly
/// marketed and Nicht-EEG-/Nicht-KWKG generation carries no regime BK and goes
/// into an ordinary one. This enum is that prescribed *kind* — the concrete BK
/// within it is the supplier's to pick, and mako cannot check the pick: the
/// Zuordnungsermächtigung is held by the NB, so an unauthorised BK comes back
/// as a rejected Zuordnung rather than a validation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Bilanzkreisart {
    /// The EEG-Bilanzkreis — generation under the EEG Veräußerungsform.
    Eeg,
    /// The KWKG-Bilanzkreis.
    Kwkg,
    /// The supplier's ordinary Bilanzkreis: directly marketed generation and
    /// Nicht-EEG-/Nicht-KWKG-Marktlokationen carry no separate regime BK.
    Standard,
}

impl ZuordnungsFall {
    /// Which Bilanzkreis the Zustimmung must name, when the Anwendungsfall
    /// fixes it.
    ///
    /// `None` for [`ZuordnungsFall::EegOhneDvPflicht`]: the Sequenzdiagramm
    /// covers „EEG-Marktlokation ohne DV-Pflicht **bzw.** KWKG-Marktlokation
    /// ohne DV-Pflicht" in one Fall, and its Hinweis names the EEG-BK for the
    /// first and the KWKG-BK for the second. The Anwendungsfall alone does not
    /// say which, so the caller must — and cannot guess, because the two are
    /// different balancing circles with different settlement.
    #[must_use]
    pub const fn bilanzkreisart(self) -> Option<Bilanzkreisart> {
        match self {
            Self::EegOhneDvPflicht => None,
            Self::EegMitDvPflicht => Some(Bilanzkreisart::Eeg),
            Self::KwkgNichtTranchiert | Self::KwkgTranchiert => Some(Bilanzkreisart::Standard),
        }
    }

    /// Resolve from the EBD id the inbound message named.
    #[must_use]
    pub fn from_ebd(ebd: &str) -> Option<Self> {
        match ebd {
            "E_0603" => Some(Self::EegOhneDvPflicht),
            "E_0604" => Some(Self::EegMitDvPflicht),
            "E_0605" => Some(Self::KwkgNichtTranchiert),
            "E_0606" => Some(Self::KwkgTranchiert),
            _ => None,
        }
    }

    /// The EBD id this Anwendungsfall answers from — `SG4 STS+E01` DE 1131.
    #[must_use]
    pub const fn ebd(self) -> &'static str {
        match self {
            Self::EegOhneDvPflicht => "E_0603",
            Self::EegMitDvPflicht => "E_0604",
            Self::KwkgNichtTranchiert => "E_0605",
            Self::KwkgTranchiert => "E_0606",
        }
    }

    const fn codes(self) -> &'static [AntwortCode] {
        match self {
            Self::EegOhneDvPflicht => E_0603_CODES,
            Self::EegMitDvPflicht => E_0604_CODES,
            Self::KwkgNichtTranchiert => E_0605_CODES,
            Self::KwkgTranchiert => E_0606_CODES,
        }
    }
}

/// What the supplier can say about an announced assignment.
///
/// Two facts, and only one of them is a judgement. `bilanzkreis` is what makes
/// a Zustimmung an answer; `fehler` is the operator's channel for the `A99`
/// „zuvor nicht spezifizierter Fehler" the walk cannot detect about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZuordnungsLage {
    /// The concrete Bilanzkreis this generation will be balanced in, already
    /// picked by the caller for the Marktlokation's **Bilanzierungsgebiet** —
    /// the key the Zuordnungsermächtigung is granted on — and for the
    /// [`Bilanzkreisart`] the Anwendungsfall prescribes.
    ///
    /// `None` escalates: GPKE Teil 2 § 2.4.2.2 Nr. 2 makes naming it part of
    /// the Zustimmung, and a Bestätigung without one assigns generation to no
    /// balancing circle at all.
    pub bilanzkreis: Option<String>,
    /// An operator-supplied reason to refuse. Rides `A99` with its `FTX+ACB`
    /// Erläuterung, which the EBD makes mandatory („Das identifizierte Problem
    /// ist in der Antwort zu beschreiben/benennen").
    pub fehler: Option<String>,
}

/// Walk `E_0603`–`E_0606` „Zuordnung prüfen" for an inbound **55607**.
///
/// `fall` is `None` when the deployment cannot say which Anwendungsfall this
/// is, and that escalates. The inbound 55607 does **not** name an EBD — the
/// Anwendungsübersicht leaves the column empty for it and names `E_0603`–
/// `E_0606` only on the answers — so which one applies follows from the
/// Marktlokation being EEG or KWKG, with or without Direktvermarktungspflicht,
/// tranchiert or not. Guessing it would put a tree in DE 1131 that does not
/// govern this Sequenzdiagramm.
///
/// # Panics
///
/// If the resolved Anwendungsfall's Codeliste does not publish `A01`/`A99` —
/// a defect in [`crate::codes`], covered by
/// `every_landing_resolves_to_a_published_code`.
#[must_use]
pub fn pruefe_zuordnung(
    anfrage: &LfAnfrage,
    fall: Option<ZuordnungsFall>,
    lage: &ZuordnungsLage,
) -> LfEntscheidung {
    let Some(fall) = fall else {
        return LfEntscheidung::eskalation(
            10,
            format!(
                "Ankündigung Zuordnung LF für MaLo {}: der Anwendungsfall ist nicht bekannt \
                 (EEG/KWKG, mit oder ohne DV-Pflicht, tranchiert oder nicht — \
                 E_0603…E_0606), und die Antwort muss ihn in SG4 STS+E01 DE 1131 nennen. \
                 Ohne Antwort bis 15:00 Uhr am ÜT ordnet der NB den LF selbst zu \
                 (GPKE Teil 2 § 2.4.2.2 Nr. 3).",
                anfrage.malo_id
            ),
        );
    };
    let list = fall.codes();
    let find = |code: &str| {
        list.iter()
            .find(|c| c.code == code)
            .unwrap_or_else(|| panic!("{} does not publish {code}", fall.ebd()))
    };

    // Prüfschritt 10, ja-Kante. A walk cannot detect an "unspecified error"
    // about itself, so this edge is only ever taken from an operator decision.
    if let Some(fehler) = &lage.fehler {
        return LfEntscheidung::antwort(find("A99"), 10, anfrage.termin, Some(fehler.clone()));
    }

    // Prüfschritt 10, nein-Kante — but a Zustimmung is only an answer if it
    // names the Bilanzkreis (GPKE Teil 2 § 2.4.2.2 Nr. 2).
    let Some(bilanzkreis) = &lage.bilanzkreis else {
        return LfEntscheidung::eskalation(
            10,
            format!(
                "Zuordnung des LF zu MaLo {} ({}): der Bilanzkreis ist nicht bekannt, die \
                 Zustimmung muss ihn aber nennen (GPKE Teil 2 § 2.4.2.2 Nr. 2). Ohne Antwort \
                 ordnet der NB nach Ablauf der Frist selbst zu (Prozessschritt 3).",
                anfrage.malo_id,
                fall.ebd()
            ),
        );
    };

    let mut entscheidung = LfEntscheidung::antwort(find("A01"), 10, anfrage.termin, None);
    if let LfEntscheidung::Antwort(antwort) = &mut entscheidung {
        antwort.bemerkung = Some(bilanzkreis.clone());
    }
    entscheidung
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;
    use uuid::Uuid;

    fn anfrage() -> LfAnfrage {
        LfAnfrage {
            pid: 55_607,
            process_id: Uuid::nil(),
            malo_id: "51238696012".to_owned(),
            vorgangsnummer: Some("NNV1234".to_owned()),
            absender_mp_id: "9900357000004".to_owned(),
            empfaenger_mp_id: "9900000000001".to_owned(),
            lokationsart: Some(crate::lf::types::Lokationsart::ErzeugendeMalo),
            transaktionsgrund: None,
            termin: Some(time::macros::date!(2026 - 09 - 01)),
            terminart: crate::lf::types::Terminart::Fix,
            uet_lieferanmeldung: None,
            eingang: datetime!(2026-08-20 08:00 UTC),
        }
    }

    /// Every Anwendungsfall answers from its **own** EBD id — the four
    /// Codelisten publish the same two codes, and DE 1131 is what tells the NB
    /// which Sequenzdiagramm the answer belongs to.
    #[test]
    fn each_anwendungsfall_answers_from_its_own_ebd() {
        for fall in [
            ZuordnungsFall::EegOhneDvPflicht,
            ZuordnungsFall::EegMitDvPflicht,
            ZuordnungsFall::KwkgNichtTranchiert,
            ZuordnungsFall::KwkgTranchiert,
        ] {
            assert_eq!(ZuordnungsFall::from_ebd(fall.ebd()), Some(fall));
            let d = pruefe_zuordnung(
                &anfrage(),
                Some(fall),
                &ZuordnungsLage {
                    bilanzkreis: Some("11XBK-EEG-----1".to_owned()),
                    fehler: None,
                },
            );
            let a = d.as_antwort().expect("answer");
            assert_eq!(a.code, "A01");
            assert!(a.zustimmung);
            assert_eq!(a.ebd.as_deref(), Some(fall.ebd()));
        }
    }

    /// A Zustimmung carries the Bilanzkreis. Without one there is nothing to
    /// put in a Muss segment, and the generation would land in no balancing
    /// circle — so the decision escalates rather than confirming.
    #[test]
    fn a_zustimmung_without_a_bilanzkreis_escalates() {
        let d = pruefe_zuordnung(
            &anfrage(),
            Some(ZuordnungsFall::EegMitDvPflicht),
            &ZuordnungsLage::default(),
        );
        assert!(d.ist_eskalation(), "{d:?}");
        let d = pruefe_zuordnung(
            &anfrage(),
            Some(ZuordnungsFall::EegMitDvPflicht),
            &ZuordnungsLage {
                bilanzkreis: Some("11XBK-EEG-----1".to_owned()),
                fehler: None,
            },
        );
        assert_eq!(
            d.as_antwort().expect("answer").bemerkung.as_deref(),
            Some("11XBK-EEG-----1")
        );
    }

    /// `A99` is the only refusal the trees publish, and the EBD makes its
    /// Erläuterung mandatory.
    #[test]
    fn a_stated_error_rides_a99_with_its_erlaeuterung() {
        let d = pruefe_zuordnung(
            &anfrage(),
            Some(ZuordnungsFall::KwkgTranchiert),
            &ZuordnungsLage {
                bilanzkreis: Some("11XBK-EEG-----1".to_owned()),
                fehler: Some("Zuordnungsermächtigung liegt nicht vor".to_owned()),
            },
        );
        let a = d.as_antwort().expect("answer");
        assert_eq!(a.code, "A99");
        assert!(!a.zustimmung);
        assert_eq!(
            a.bemerkung.as_deref(),
            Some("Zuordnungsermächtigung liegt nicht vor")
        );
    }

    /// An unknown Anwendungsfall escalates: the answer must name a tree in
    /// DE 1131, and the inbound 55607 does not state one.
    #[test]
    fn an_unknown_anwendungsfall_escalates() {
        let d = pruefe_zuordnung(
            &anfrage(),
            None,
            &ZuordnungsLage {
                bilanzkreis: Some("11XBK-EEG-----1".to_owned()),
                fehler: None,
            },
        );
        assert!(d.ist_eskalation(), "{d:?}");
    }

    /// The Anwendungsfall fixes the *kind* of Bilanzkreis — except Fall 1,
    /// which covers EEG and KWKG plants in one Sequenzdiagramm and therefore
    /// cannot say which of the two BKs the answer must name.
    #[test]
    fn only_fall_one_leaves_the_bilanzkreisart_open() {
        assert_eq!(ZuordnungsFall::EegOhneDvPflicht.bilanzkreisart(), None);
        assert_eq!(
            ZuordnungsFall::EegMitDvPflicht.bilanzkreisart(),
            Some(Bilanzkreisart::Eeg)
        );
        for fall in [
            ZuordnungsFall::KwkgNichtTranchiert,
            ZuordnungsFall::KwkgTranchiert,
        ] {
            assert_eq!(fall.bilanzkreisart(), Some(Bilanzkreisart::Standard));
        }
    }

    /// An EBD id from another tree is not one of these four.
    #[test]
    fn a_foreign_ebd_resolves_to_no_anwendungsfall() {
        assert_eq!(ZuordnungsFall::from_ebd("E_0609"), None);
        assert_eq!(ZuordnungsFall::from_ebd(""), None);
    }
}
