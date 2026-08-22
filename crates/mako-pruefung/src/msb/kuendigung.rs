//! `E_0200` — the **abgebender MSB's** answer to a Kündigung des
//! Messstellenbetriebsvertrags (UTILMD 55039, MSBN → MSBA).
//!
//! # This process does not involve the Netzbetreiber
//!
//! WiM Strom Teil 1 Kap. 2.1.3: „Die Durchführung des Use-Cases ‚Kündigung
//! Messstellenbetrieb' ist ebenfalls kein konstitutiver Bestandteil zur
//! Herbeiführung eines MSB-Wechsels. Sie dient den beteiligten Marktpartnern
//! allein dazu, in einer massengeschäftstauglichen Art und Weise auf die
//! Zivilrechtslage Einfluss zu nehmen." The MSBN terminates, in the
//! Anschlussnutzer's name, the contract the MSBA holds with that customer.
//!
//! Every Prüfschritt is therefore a question about the MSBA's **own contract**.
//! There is no grid registry to consult and no Marktpartnerverzeichnis to
//! check: refusing a Kündigung because a Messlokation is absent from a network
//! register answers a question the process never asks, and the MSBA — which
//! may be a wettbewerblicher MSB with no network at all — has no such register.
//!
//! # The Termin decides the shape of the answer
//!
//! Kap. 2.2.1 gives two rules that pull in opposite directions:
//!
//! > - Hat der MSBN auf einen fixen Zeitpunkt gekündigt und wird dieser vom
//! >   MSBA nicht bestätigt, so teilt der MSBA den nächstmöglichen Zeitpunkt,
//! >   zu dem eine Kündigung erfolgen kann, und die Kündigungsfrist in der
//! >   **Ablehnung** mit.
//! > - Hat der MSBN auf den nächstmöglichen Zeitpunkt gekündigt, so
//! >   **bestätigt** der MSBA die Kündigung unter Angabe dieses Zeitpunkts.
//!
//! Same contract, same date, opposite cluster — the difference is only which
//! of the two the MSBN asked for. That is why
//! [`Kuendigungstermin`] is an enum rather
//! than an `Option<Date>`.
//!
//! # Kap. 2.2.3 — a contract already terminated
//!
//! The Festlegung tabulates four constellations against the existing
//! Vertragsende, and [`pruefe_kuendigung`] is that table:
//!
//! | Kündigung durch MSBN … | Antwort MSBA |
//! |---|---|
//! | … auf denselben Termin | Bestätigung |
//! | … auf einen fixen früheren Termin, Vertragslage lässt ihn zu | Bestätigung zum früheren Termin |
//! | … auf einen fixen früheren Termin, Vertragslage lässt ihn nicht zu | Ablehnung mit dem bestehenden Vertragsende |
//! | … auf einen fixen späteren Termin | Ablehnung — ein wirksam gekündigter Vertrag wird nicht verlängert |
//! | … auf den nächstmöglichen Termin | wie die beiden fixen früheren Fälle |
//!
//! The last row of the table carries the reason: „Ein bereits wirksam
//! gekündigtes Vertragsverhältnis kann nicht — auch nicht bei Zustimmung des
//! MSBA — durch eine schlichte Kündigung zu einem späteren Zeitpunkt wieder
//! verlängert werden."

use time::Date;

use crate::antwort::RejectReason;
use crate::codes::{EBD_KUENDIGUNG_MSB, lookup};

use super::types::{KuendigungMsb, Kuendigungstermin, MsbEntscheidung, Vertragslage};

/// Decide the MSBA's answer to a Kündigung Messstellenbetrieb.
///
/// # Panics
///
/// Only if the `E_0200` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn pruefe_kuendigung(anfrage: &KuendigungMsb) -> MsbEntscheidung {
    let tree = EBD_KUENDIGUNG_MSB;
    let code = |c: &str| lookup(tree, c).expect("code is published in E_0200");

    match anfrage.vertragslage {
        Vertragslage::Unbekannt => MsbEntscheidung::Escalate {
            reason: format!(
                "Vertragslage zur Messlokation {} ist nicht feststellbar — ohne Auskunft \
                 darf weder bestätigt noch mit ZC9 abgelehnt werden",
                anfrage.melo_id
            ),
        },

        Vertragslage::KeineZuordnung => MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("ZC9"),
            2,
            format!(
                "Zur Messlokation {} besteht kein Messstellenbetriebsverhältnis dieses MSB",
                anfrage.melo_id
            ),
        )),

        Vertragslage::Beendet => MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("Z29"),
            2,
            format!(
                "Das Vertragsverhältnis zur Messlokation {} wurde bereits zu einem früheren \
                 Zeitpunkt beendet",
                anfrage.melo_id
            ),
        )),

        Vertragslage::Laufend { naechstmoeglich } => match anfrage.kuendigungstermin {
            // „Hat der MSBN auf den nächstmöglichen Zeitpunkt gekündigt, so
            // bestätigt der MSBA die Kündigung unter Angabe dieses Zeitpunkts."
            Kuendigungstermin::Naechstmoeglich => MsbEntscheidung::Accept(
                crate::antwort::AntwortDetail::new(tree, code("Z01")).mit_termin(naechstmoeglich),
            ),
            Kuendigungstermin::Fix(t) if t >= naechstmoeglich => {
                MsbEntscheidung::accept(tree, code("E15"))
            }
            // „…und wird dieser vom MSBA nicht bestätigt, so teilt der MSBA den
            // nächstmöglichen Zeitpunkt … in der Ablehnung mit."
            Kuendigungstermin::Fix(t) => MsbEntscheidung::Reject(
                RejectReason::new(
                    tree,
                    code("Z12"),
                    2,
                    format!(
                        "Der Messstellenbetriebsvertrag ist zum {t} noch gebunden; \
                         nächstmöglicher Kündigungstermin ist {naechstmoeglich}"
                    ),
                )
                .mit_termin(naechstmoeglich),
            ),
        },

        // ── Kap. 2.2.3 ────────────────────────────────────────────────────
        Vertragslage::BereitsGekuendigt {
            vertragsende,
            frueher_moeglich,
        } => bereits_gekuendigt(anfrage, vertragsende, frueher_moeglich),
    }
}

/// The Kap. 2.2.3 table.
fn bereits_gekuendigt(
    anfrage: &KuendigungMsb,
    vertragsende: Date,
    frueher_moeglich: Option<Date>,
) -> MsbEntscheidung {
    let tree = EBD_KUENDIGUNG_MSB;
    let code = |c: &str| lookup(tree, c).expect("code is published in E_0200");

    let mehrfach = |termin: Date| {
        MsbEntscheidung::Reject(
            RejectReason::new(
                tree,
                code("Z34"),
                2,
                format!(
                    "Der Vertrag zur Messlokation {} ist bereits wirksam zum {vertragsende} \
                     gekündigt; eine Kündigung zum {termin} verlängert ihn nicht",
                    anfrage.melo_id
                ),
            )
            .mit_termin(vertragsende),
        )
    };

    match anfrage.kuendigungstermin {
        Kuendigungstermin::Fix(t) if t == vertragsende => {
            MsbEntscheidung::accept(tree, code("E15"))
        }
        // A later date cannot revive a terminated contract — the one row of the
        // table with no „Fall 1 / Fall 2" split, because the answer does not
        // depend on the contract situation at all.
        Kuendigungstermin::Fix(t) if t > vertragsende => mehrfach(t),
        // Earlier than the existing Vertragsende, or „nächstmöglich": confirm
        // to the earlier date when the contract allows one, otherwise refuse
        // and name the Vertragsende already in force.
        Kuendigungstermin::Fix(t) => match frueher_moeglich {
            Some(frueher) if t >= frueher => MsbEntscheidung::Accept(
                crate::antwort::AntwortDetail::new(tree, code("Z01")).mit_termin(t),
            ),
            _ => mehrfach(t),
        },
        Kuendigungstermin::Naechstmoeglich => match frueher_moeglich {
            Some(frueher) => MsbEntscheidung::Accept(
                crate::antwort::AntwortDetail::new(tree, code("Z01")).mit_termin(frueher),
            ),
            None => mehrfach(vertragsende),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn k(termin: Kuendigungstermin, lage: Vertragslage) -> KuendigungMsb {
        KuendigungMsb {
            melo_id: "DE0000000001234567890000000000001".to_owned(),
            msbn_mp_id: "9900000000003".to_owned(),
            kuendigungstermin: termin,
            vertragslage: lage,
        }
    }

    #[test]
    fn a_fixed_date_the_contract_allows_is_confirmed_plainly() {
        let e = pruefe_kuendigung(&k(
            Kuendigungstermin::Fix(d(2026, Month::September, 1)),
            Vertragslage::Laufend {
                naechstmoeglich: d(2026, Month::July, 1),
            },
        ));
        assert_eq!(e.antwortcode(), Some("E15"));
        assert_eq!(e.abweichender_termin(), None);
    }

    /// „Hat der MSBN auf den nächstmöglichen Zeitpunkt gekündigt, so bestätigt
    /// der MSBA die Kündigung **unter Angabe dieses Zeitpunkts**."
    #[test]
    fn naechstmoeglich_is_confirmed_with_the_date() {
        let e = pruefe_kuendigung(&k(
            Kuendigungstermin::Naechstmoeglich,
            Vertragslage::Laufend {
                naechstmoeglich: d(2026, Month::July, 1),
            },
        ));
        assert_eq!(e.antwortcode(), Some("Z01"));
        assert_eq!(e.abweichender_termin(), Some(d(2026, Month::July, 1)));
    }

    /// The same contract and the same date, refused — because the MSBN asked
    /// for a fixed one the Kündigungsfrist does not reach.
    #[test]
    fn a_fixed_date_inside_the_binding_is_z12_naming_the_next_one() {
        let e = pruefe_kuendigung(&k(
            Kuendigungstermin::Fix(d(2026, Month::May, 1)),
            Vertragslage::Laufend {
                naechstmoeglich: d(2026, Month::July, 1),
            },
        ));
        assert_eq!(e.antwortcode(), Some("Z12"));
        assert_eq!(e.abweichender_termin(), Some(d(2026, Month::July, 1)));
    }

    // ── Kap. 2.2.3 ────────────────────────────────────────────────────────

    #[test]
    fn the_same_termin_on_an_already_terminated_contract_is_confirmed() {
        let e = pruefe_kuendigung(&k(
            Kuendigungstermin::Fix(d(2026, Month::August, 1)),
            Vertragslage::BereitsGekuendigt {
                vertragsende: d(2026, Month::August, 1),
                frueher_moeglich: None,
            },
        ));
        assert_eq!(e.antwortcode(), Some("E15"));
    }

    #[test]
    fn an_earlier_termin_the_contract_allows_is_confirmed_to_that_date() {
        let e = pruefe_kuendigung(&k(
            Kuendigungstermin::Fix(d(2026, Month::July, 1)),
            Vertragslage::BereitsGekuendigt {
                vertragsende: d(2026, Month::August, 1),
                frueher_moeglich: Some(d(2026, Month::June, 1)),
            },
        ));
        assert_eq!(e.antwortcode(), Some("Z01"));
        assert_eq!(e.abweichender_termin(), Some(d(2026, Month::July, 1)));
    }

    #[test]
    fn an_earlier_termin_the_contract_forbids_is_z34_naming_the_vertragsende() {
        let e = pruefe_kuendigung(&k(
            Kuendigungstermin::Fix(d(2026, Month::July, 1)),
            Vertragslage::BereitsGekuendigt {
                vertragsende: d(2026, Month::August, 1),
                frueher_moeglich: None,
            },
        ));
        assert_eq!(e.antwortcode(), Some("Z34"));
        assert_eq!(e.abweichender_termin(), Some(d(2026, Month::August, 1)));
    }

    /// „Ein bereits wirksam gekündigtes Vertragsverhältnis kann nicht — auch
    /// nicht bei Zustimmung des MSBA — durch eine schlichte Kündigung zu einem
    /// späteren Zeitpunkt wieder verlängert werden."
    #[test]
    fn a_later_termin_never_extends_a_terminated_contract() {
        for frueher in [None, Some(d(2026, Month::June, 1))] {
            let e = pruefe_kuendigung(&k(
                Kuendigungstermin::Fix(d(2026, Month::October, 1)),
                Vertragslage::BereitsGekuendigt {
                    vertragsende: d(2026, Month::August, 1),
                    frueher_moeglich: frueher,
                },
            ));
            assert_eq!(
                e.antwortcode(),
                Some("Z34"),
                "frueher_moeglich = {frueher:?}"
            );
        }
    }

    #[test]
    fn an_ended_contract_is_z29_and_a_foreign_melo_is_zc9() {
        assert_eq!(
            pruefe_kuendigung(&k(
                Kuendigungstermin::Naechstmoeglich,
                Vertragslage::Beendet
            ))
            .antwortcode(),
            Some("Z29")
        );
        assert_eq!(
            pruefe_kuendigung(&k(
                Kuendigungstermin::Naechstmoeglich,
                Vertragslage::KeineZuordnung
            ))
            .antwortcode(),
            Some("ZC9")
        );
    }

    /// „Unbekannt" is not „KeineZuordnung". Answering `ZC9` because a lookup
    /// failed refuses a lawful Kündigung and leaves the customer bound.
    #[test]
    fn an_unknown_contract_position_escalates() {
        assert!(matches!(
            pruefe_kuendigung(&k(
                Kuendigungstermin::Naechstmoeglich,
                Vertragslage::Unbekannt
            )),
            MsbEntscheidung::Escalate { .. }
        ));
    }
}
