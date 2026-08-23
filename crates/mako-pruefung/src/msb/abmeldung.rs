//! `E_0202` — the Netzbetreiber's answer to an **Ende Messstellenbetrieb**
//! (UTILMD 55051, MSBA → NB).
//!
//! # The Mindestvorlauffrist here is not a rejection
//!
//! WiM Strom Teil 1 Kap. 2.4.2 Nr. 2 is explicit about what happens when the
//! MSB names a Zuordnungsende inside the 20-Werktage lead time:
//!
//! > Hat der MSB ein Zuordnungsende benannt, das die Mindestvorlauffrist nach
//! > Prozessschritt 1 unterschreitet, so **setzt der NB das Zuordnungsende auf
//! > das nächstmögliche Zuordnungsende** unter Beachtung der
//! > Mindestvorlauffrist.
//!
//! So the answer is a Bestätigung with `Z01` („Zustimmung mit Terminänderung")
//! stating the date the NB set — not an `E17`. `E_0202` publishes `E17` only
//! for the Aufhebung einer zukünftigen Zuordnung (Transaktionsgrund
//! `ZG9`/`ZH1`/`ZH2`), which is a different message.
//!
//! The confirmation is **vorläufig** in either case. Kap. 2.4.2 Nr. 2 lists
//! three ways the date can still move afterwards: an Anmeldung by an MSBN
//! (which takes precedence and can pull it arbitrarily far forward), the
//! ±9-Werktage Realisierungskorridor, and a Weiterverpflichtung of this very
//! MSB. A caller that treats 55052 as final will disagree with the NB about
//! when the Zuordnung ended.
//!
//! # Außerbetriebnahme has no lead time at all
//!
//! Kap. 2.4.2 Nr. 1: for a Stilllegung the Abmeldung goes out „unverzüglich
//! nach Außerbetriebnahme der Messlokation" and the Zuordnungsende is fixed to
//! „der Folgetag 00:00 des Geräteausbaudatums" — a date in the *past*.
//! Measuring 20 Werktage against it manufactures a rejection on every
//! Stilllegung.

use mako_fristen::HolidayCalendar;
use mako_fristen::vorlauf::{ABMELDUNG_WT, VorlaufShape, VorlaufVerdict};
use time::Date;

use crate::antwort::{AntwortDetail, RejectReason};
use crate::codes::{EBD_ABMELDUNG_MSB, lookup};

use super::types::{AbmeldungMsb, MsbEntscheidung};

/// Decide the NB's answer to an Ende Messstellenbetrieb.
///
/// The `Accept` may carry a Zuordnungsende the NB moved — read
/// [`MsbEntscheidung::abweichender_termin`] and confirm to *that* date.
///
/// # Panics
///
/// Only if the `E_0202` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn pruefe_abmeldung(
    anfrage: &AbmeldungMsb,
    eingangsdatum: Date,
    cal: HolidayCalendar,
) -> MsbEntscheidung {
    let tree = super::baum::abmeldung(anfrage.sparte);
    let code = |c: &str| lookup(tree, c).expect("code is published in the Abmeldung tree");

    let Some(zuordnung_besteht) = anfrage.zuordnung_besteht else {
        return MsbEntscheidung::Escalate {
            reason: format!(
                "Zuordnung des MSB {} zur Messlokation {} konnte nicht nachgeschlagen werden",
                anfrage.msba_mp_id, anfrage.melo_id
            ),
        };
    };
    if !zuordnung_besteht {
        // Kap. 2.4.1 names „Die Messlokation war dem MSB nicht zugeordnet" as
        // the Fehlerfall, but `E_0202` publishes no code for it — the tree has
        // no ZC9. So it escalates rather than borrowing a code from E_0201.
        return MsbEntscheidung::Escalate {
            reason: format!(
                "MSB {} ist der Messlokation {} nicht zugeordnet (Fehlerfall nach Kap. 2.4.1); \
                 E_0202 veröffentlicht hierfür keinen Ablehnungscode",
                anfrage.msba_mp_id, anfrage.melo_id
            ),
        };
    }

    if !anfrage.grund.hat_mindestvorlauffrist() {
        return MsbEntscheidung::accept(tree, code("E15"));
    }

    match VorlaufShape::LatestWerktageBefore(ABMELDUNG_WT).check(
        eingangsdatum,
        anfrage.gewuenschtes_zuordnungsende,
        cal,
    ) {
        // Kap. 2.4.2 Nr. 2: the NB *sets* the date rather than refusing.
        VorlaufVerdict::TooLate {
            earliest_possible, ..
        } => MsbEntscheidung::Accept(
            AntwortDetail::new(tree, code("Z01")).mit_termin(earliest_possible),
        ),
        // More lead time than required is lawful and needs no correction.
        VorlaufVerdict::Ok | VorlaufVerdict::TooEarly { .. } => {
            MsbEntscheidung::accept(tree, code("E15"))
        }
    }
}

/// The `E_0202` rejection for an implausible Transaktionsgrund, for callers
/// that detect one outside the date logic.
///
/// Split out because `Z09` is the one `E_0202` Ablehnung a projection of the
/// NB's own records can reach; `E17` needs the Transaktionsgrund of an
/// Aufhebung einer zukünftigen Zuordnung, which is a different Anwendungsfall.
///
/// # Panics
///
/// Only if `Z09` leaves the `E_0202` Codeliste.
#[must_use]
pub fn transaktionsgrund_unplausibel(detail: impl Into<String>) -> MsbEntscheidung {
    MsbEntscheidung::Reject(RejectReason::new(
        EBD_ABMELDUNG_MSB,
        lookup(EBD_ABMELDUNG_MSB, "Z09").expect("Z09 is published in E_0202"),
        2,
        detail,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msb::types::Abmeldegrund;
    use crate::msb::types::Sparte;
    use time::Month;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn a(grund: Abmeldegrund, ende: Date) -> AbmeldungMsb {
        AbmeldungMsb {
            sparte: Sparte::Strom,
            melo_id: "DE0000000001234567890000000000001".to_owned(),
            msba_mp_id: "9900000000003".to_owned(),
            gewuenschtes_zuordnungsende: ende,
            grund,
            zuordnung_besteht: Some(true),
        }
    }

    #[test]
    fn a_compliant_abmeldung_is_e15() {
        let ende = d(2026, Month::September, 1);
        let uet = mako_fristen::sub_werktage(ende, 20, CAL);
        let e = pruefe_abmeldung(&a(Abmeldegrund::VertragsEnde, ende), uet, CAL);
        assert_eq!(e.antwortcode(), Some("E15"));
        assert_eq!(e.abweichender_termin(), None);
    }

    /// The rule the tree makes unmistakable: a short lead time moves the date,
    /// it does not refuse the message. `E_0202` has no `E17` for this case.
    #[test]
    fn a_short_lead_time_is_confirmed_to_the_next_possible_date() {
        let ende = d(2026, Month::September, 1);
        let uet = mako_fristen::sub_werktage(ende, 5, CAL);
        let e = pruefe_abmeldung(&a(Abmeldegrund::VertragsEnde, ende), uet, CAL);
        assert_eq!(e.antwortcode(), Some("Z01"));
        assert_eq!(
            e.abweichender_termin(),
            Some(mako_fristen::add_werktage(uet, 20, CAL))
        );
    }

    /// A Stilllegung is reported after the fact — the Zuordnungsende is in the
    /// past by construction and must still be confirmed.
    #[test]
    fn an_ausserbetriebnahme_has_no_lead_time() {
        let ende = d(2026, Month::March, 2);
        let uet = d(2026, Month::March, 3);
        let e = pruefe_abmeldung(&a(Abmeldegrund::Ausserbetriebnahme, ende), uet, CAL);
        assert_eq!(e.antwortcode(), Some("E15"));
        // …and the same message under any other Grund would have moved.
        assert_eq!(
            pruefe_abmeldung(&a(Abmeldegrund::VertragsEnde, ende), uet, CAL).antwortcode(),
            Some("Z01")
        );
    }

    #[test]
    fn an_unknown_assignment_escalates() {
        let ende = d(2026, Month::September, 1);
        let mut anfrage = a(Abmeldegrund::VertragsEnde, ende);
        anfrage.zuordnung_besteht = None;
        assert!(matches!(
            pruefe_abmeldung(&anfrage, d(2026, Month::June, 1), CAL),
            MsbEntscheidung::Escalate { .. }
        ));
    }

    /// The Weiterverpflichtungszeitraum depends on why the MSB deregistered:
    /// three months on an Anschlussnutzerwechsel, one otherwise.
    #[test]
    fn the_weiterverpflichtung_cap_follows_the_abmeldegrund() {
        assert_eq!(
            Abmeldegrund::AnschlussnutzerWechsel.max_weiterverpflichtung_monate(),
            3
        );
        assert_eq!(
            Abmeldegrund::VertragsEnde.max_weiterverpflichtung_monate(),
            1
        );
    }
}
