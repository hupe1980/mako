//! `E_0233` — the **wettbewerblicher MSB's** answer to a Vorabinformation über
//! den Ersteinbau eines iMS (IFTSTA 21029, gMSB → wMSB; answered with
//! IFTSTA 21030 / 21031).
//!
//! # What this process is for
//!
//! The grundzuständiger MSB carries the rollout obligation of § 29 MsbG, and it
//! reaches Messlokationen a *wettbewerblicher* MSB operates. WiM Strom Teil 1
//! Kap. 3.5 is how the two settle who installs: the gMSB announces the planned
//! Umstellungszeitpunkt three months and three Werktage ahead, and the wMSB has
//! three Werktage to say whether the rollout may proceed at all.
//!
//! Two statutory facts can stop it, and both belong to the wMSB alone — the
//! gMSB cannot look them up:
//!
//! - **§ 19 Abs. 5 MsbG Bestandsschutz.** A moderne Messeinrichtung installed
//!   before the rollout obligation took effect may stay for the rest of its
//!   Eichfrist. The wMSB may waive it; nobody else may waive it for them.
//! - **Selbsteinbau.** The wMSB may intend to install the iMS or the mME
//!   itself, which is the whole point of § 5 MsbG.
//!
//! # `A04` is not a soft yes
//!
//! „Zum jetzigen Zeitpunkt noch keine Aussage hinsichtlich Selbsteinbau
//! möglich" reads like a deferral, and the BDEW clusters it as an **Ablehnung**
//! anyway. It rides 21031 and the gMSB may not roll out. Reading an undecided
//! wMSB as consent installs a device at a Messlokation the wMSB operates
//! without its agreement — which is a § 5 MsbG problem, not a scheduling one.
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2a, WiM Strom Teil 1 Kap. 3.5
//! - *Entscheidungsbaum-Diagramme und Codelisten* 4.3 Kap. 8.8.2
//! - §§ 5, 19 Abs. 5, 29 MsbG
//! - Anwendungsübersicht der Prüfidentifikatoren 4.0, lfd. Nr. 30790–30890

use serde::{Deserialize, Serialize};

use crate::antwort::RejectReason;
use crate::codes::{EBD_ERSTEINBAU_IMS, lookup};

use super::types::MsbEntscheidung;

/// The Prüfidentifikator of the gMSB's Vorabinformation zum Ersteinbau.
///
/// One PID, four Prozessschritte and three different recipients: the wMSB
/// (Kap. 3.5.2 Nr. 1), the LF (Nr. 3) and the NB (Nr. 4). Only the wMSB owes an
/// answer.
pub const VORABINFORMATION_PID: u32 = 21_029;

/// IFTSTA 21030 „iMS-Ersteinbauzustimmung" — the wMSB consents.
pub const ZUSTIMMUNG_PID: u32 = 21_030;

/// IFTSTA 21031 „Bestandsschutz / Eigenausbau iMS" — the wMSB does not.
pub const ABLEHNUNG_PID: u32 = 21_031;

/// Whether the wMSB's answer to a Vorabinformation rides 21030 or 21031.
///
/// The cluster decides, as everywhere in this crate — never a boolean the
/// caller passes beside the code. `A03` is the only Zustimmung `E_0233`
/// publishes.
#[must_use]
pub fn antwort_pid(entscheidung: &MsbEntscheidung) -> Option<u32> {
    match entscheidung {
        MsbEntscheidung::Accept(_) => Some(ZUSTIMMUNG_PID),
        MsbEntscheidung::Reject(_) => Some(ABLEHNUNG_PID),
        MsbEntscheidung::Escalate { .. } => None,
    }
}

/// What the wettbewerblicher MSB knows about the Messlokation the gMSB wants to
/// roll out at.
///
/// Every field is a fact only the wMSB holds. `Option` is not decoration here:
/// `None` escalates rather than defaulting, because both defaults are wrong in
/// opposite directions — assuming no Bestandsschutz installs over a protected
/// device, and assuming one blocks a lawful rollout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErsteinbauVorabinformation {
    /// The Messlokation the gMSB named.
    pub melo_id: String,
    /// MP-ID of the grundzuständiger MSB that sent the Vorabinformation.
    pub gmsb_mp_id: String,
    /// Whether § 19 Abs. 5 MsbG Bestandsschutz applies at this Messlokation
    /// (Prüfschritt 1). `None` escalates.
    pub bestandsschutz: Option<bool>,
    /// Whether the wMSB waives that Bestandsschutz (Prüfschritt 2).
    ///
    /// Only read when [`Self::bestandsschutz`] is `Some(true)`. `None` there
    /// escalates: a waiver is a Willenserklärung and cannot be inferred.
    pub bestandsschutz_verzicht: Option<bool>,
    /// Whether the wMSB plans to install an iMS or an mME itself
    /// (Prüfschritt 3). `None` escalates.
    pub selbsteinbau_geplant: Option<bool>,
    /// Whether the wMSB waives that Selbsteinbau (Prüfschritt 4).
    ///
    /// Only read when [`Self::selbsteinbau_geplant`] is `Some(false)`, and the
    /// distinction Prüfschritt 4 draws is between *deciding not to* (`A03`,
    /// Zustimmung) and *not having decided* (`A04`, Ablehnung) — so `None` is a
    /// real answer here rather than an escalation.
    pub selbsteinbau_verzicht: Option<bool>,
}

/// Decide the wMSB's answer to a Vorabinformation über den Ersteinbau eines iMS.
///
/// Walks `E_0233` Prüfschritte 1–4 in the published order.
///
/// # Panics
///
/// Only if the `E_0233` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn pruefe_ersteinbau(info: &ErsteinbauVorabinformation) -> MsbEntscheidung {
    let tree = EBD_ERSTEINBAU_IMS;
    let code = |c: &str| lookup(tree, c).expect("code is published in E_0233");

    // Prüfschritt 1 — liegt ein Bestandsschutz nach § 19 Abs. 5 MsbG vor?
    let Some(bestandsschutz) = info.bestandsschutz else {
        return MsbEntscheidung::Escalate {
            reason: format!(
                "Bestandsschutz nach § 19 Abs. 5 MsbG für die Messlokation {} ist nicht \
                 feststellbar (E_0233 Prüfschritt 1)",
                info.melo_id
            ),
        };
    };

    if bestandsschutz {
        // Prüfschritt 2 — wird darauf verzichtet?
        match info.bestandsschutz_verzicht {
            Some(false) => {
                return MsbEntscheidung::Reject(RejectReason::new(
                    tree,
                    code("A01"),
                    2,
                    format!(
                        "Für die Messlokation {} besteht Bestandsschutz nach § 19 Abs. 5 MsbG, \
                         auf den nicht verzichtet wird",
                        info.melo_id
                    ),
                ));
            }
            None => {
                return MsbEntscheidung::Escalate {
                    reason: format!(
                        "Ob auf den Bestandsschutz nach § 19 Abs. 5 MsbG für die Messlokation {} \
                         verzichtet wird, ist eine Willenserklärung des wMSB und steht nicht in \
                         den Stammdaten (E_0233 Prüfschritt 2)",
                        info.melo_id
                    ),
                };
            }
            Some(true) => {}
        }
    }

    // Prüfschritt 3 — ist ein Selbsteinbau geplant?
    let Some(selbsteinbau) = info.selbsteinbau_geplant else {
        return MsbEntscheidung::Escalate {
            reason: format!(
                "Ob an der Messlokation {} ein Selbsteinbau eines iMS oder einer mME geplant ist, \
                 ist nicht feststellbar (E_0233 Prüfschritt 3)",
                info.melo_id
            ),
        };
    };

    if selbsteinbau {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A02"),
            3,
            format!(
                "Der wMSB plant den Selbsteinbau eines iMS oder einer mME an der Messlokation {}",
                info.melo_id
            ),
        ));
    }

    // Prüfschritt 4 — wird auf den Selbsteinbau verzichtet?
    //
    // `None` is „noch keine Aussage möglich", which is the `A04` branch and not
    // an escalation: the tree publishes the undecided case as a code.
    match info.selbsteinbau_verzicht {
        Some(true) => MsbEntscheidung::accept(tree, code("A03")),
        Some(false) | None => MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("A04"),
            4,
            format!(
                "Zum jetzigen Zeitpunkt ist für die Messlokation {} keine Aussage hinsichtlich \
                 des Selbsteinbaus möglich — der Ersteinbau durch den gMSB darf nicht erfolgen",
                info.melo_id
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> ErsteinbauVorabinformation {
        ErsteinbauVorabinformation {
            melo_id: "DE0000000001234567890000000000001".to_owned(),
            gmsb_mp_id: "9900000000001".to_owned(),
            bestandsschutz: Some(false),
            bestandsschutz_verzicht: None,
            selbsteinbau_geplant: Some(false),
            selbsteinbau_verzicht: Some(true),
        }
    }

    #[test]
    fn a_waived_rollout_consents_on_21030() {
        let e = pruefe_ersteinbau(&info());
        assert_eq!(e.antwortcode(), Some("A03"));
        assert_eq!(antwort_pid(&e), Some(ZUSTIMMUNG_PID));
    }

    #[test]
    fn an_unwaived_bestandsschutz_refuses_with_a01() {
        let mut i = info();
        i.bestandsschutz = Some(true);
        i.bestandsschutz_verzicht = Some(false);
        let e = pruefe_ersteinbau(&i);
        assert_eq!(e.antwortcode(), Some("A01"));
        assert_eq!(antwort_pid(&e), Some(ABLEHNUNG_PID));
    }

    /// A waived Bestandsschutz falls through to the Selbsteinbau branch rather
    /// than consenting on the spot — Prüfschritt 2's „ja" goes to 3, not to the
    /// answer.
    #[test]
    fn a_waived_bestandsschutz_still_asks_about_selbsteinbau() {
        let mut i = info();
        i.bestandsschutz = Some(true);
        i.bestandsschutz_verzicht = Some(true);
        i.selbsteinbau_geplant = Some(true);
        assert_eq!(pruefe_ersteinbau(&i).antwortcode(), Some("A02"));
    }

    #[test]
    fn a_planned_selbsteinbau_refuses_with_a02() {
        let mut i = info();
        i.selbsteinbau_geplant = Some(true);
        assert_eq!(pruefe_ersteinbau(&i).antwortcode(), Some("A02"));
    }

    /// The finding this tree exists to prevent: an undecided wMSB is an
    /// Ablehnung, not a deferral the gMSB may roll out through.
    #[test]
    fn an_undecided_selbsteinbau_is_a04_and_rides_the_ablehnungs_pid() {
        let mut i = info();
        i.selbsteinbau_verzicht = None;
        let e = pruefe_ersteinbau(&i);
        assert_eq!(e.antwortcode(), Some("A04"));
        assert_eq!(antwort_pid(&e), Some(ABLEHNUNG_PID));
        assert!(matches!(e, MsbEntscheidung::Reject(_)));
    }

    #[test]
    fn an_unknown_bestandsschutz_escalates_rather_than_guessing() {
        for (bestandsschutz, verzicht) in [(None, None), (Some(true), None)] {
            let mut i = info();
            i.bestandsschutz = bestandsschutz;
            i.bestandsschutz_verzicht = verzicht;
            assert!(
                matches!(pruefe_ersteinbau(&i), MsbEntscheidung::Escalate { .. }),
                "bestandsschutz={bestandsschutz:?} verzicht={verzicht:?} must escalate"
            );
        }
    }

    #[test]
    fn an_unknown_selbsteinbau_plan_escalates() {
        let mut i = info();
        i.selbsteinbau_geplant = None;
        assert!(matches!(
            pruefe_ersteinbau(&i),
            MsbEntscheidung::Escalate { .. }
        ));
    }

    /// Every code the walk can name must be in the Codeliste, or the `expect`
    /// above is a panic waiting for a rollout.
    #[test]
    fn every_named_code_is_published() {
        for c in ["A01", "A02", "A03", "A04"] {
            assert!(
                lookup(EBD_ERSTEINBAU_IMS, c).is_some(),
                "{c} left the E_0233 Codeliste"
            );
        }
    }
}
