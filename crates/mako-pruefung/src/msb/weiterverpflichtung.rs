//! `E_0203` — the **abgebender MSB's** answer to a Weiterverpflichtung
//! (ORDERS 17002, NB → MSBA; answered with ORDRSP 19003 / 19004).
//!
//! # What this process is for
//!
//! An Ende Messstellenbetrieb with no successor would leave the Messlokation
//! unassigned, and WiM Teil 1 Kap. 2.1.1 forbids that: „Ist eine Messlokation
//! zu einem Zeitpunkt in Bezug auf den Messstellenbetrieb nicht einem wMSB
//! zugeordnet, so ist sie dem gMSB zuzuordnen." Between the Verpflichtungsanfrage
//! (Kap. 2.4.2 Nr. 3) and the gMSB taking over, the NB may keep the outgoing
//! MSB in place — that is this order.
//!
//! # The cap is per Abmeldegrund, and the answer to overshooting it depends on
//! whether it is the first ask
//!
//! Kap. 2.4.2 Nr. 4 caps the Weiterverpflichtung at „längstens drei Monate" on
//! an Anschlussnutzerwechsel and „längstens einem Monat" in every other case.
//! `E_0203` then splits the overshoot in two:
//!
//! - `Z14` „Zustimmung mit Terminänderung" — Bedingung: „Termin war außerhalb
//!   des max. möglichen Weiterverpflichtungszeitraums. Der korrigierte
//!   Abmeldetermin ist im DTM DE2380 anzugeben." The MSBA **agrees**, to the
//!   capped date.
//! - `Z22` „Ablehnung wegen Überschreiten des Weiterverpflichtungszeitraums" —
//!   Bedingung: „Nur möglich bei geforderter Verlängerung der
//!   Weiterverpflichtung über eine weitere ORDERS **nach Erreichen** des max.
//!   möglichen Weiterverpflichtungszeitraums."
//!
//! Refusing the first ask with `Z22` states a Bedingung that is not met and
//! leaves the Messlokation heading for the Zuordnungslücke the process exists
//! to prevent.

use time::{Date, Duration};

use crate::antwort::{AntwortDetail, RejectReason};
use crate::codes::lookup;

use super::types::{MsbEntscheidung, WeiterverpflichtungAuftrag};

/// Decide the MSBA's answer to a Weiterverpflichtung.
///
/// # Panics
///
/// Only if the `E_0203` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn pruefe_weiterverpflichtung(auftrag: &WeiterverpflichtungAuftrag) -> MsbEntscheidung {
    let tree = super::baum::weiterverpflichtung(auftrag.sparte);
    let code =
        |c: &str| lookup(tree, c).expect("code is published in the Weiterverpflichtung tree");

    let cap = max_termin(
        auftrag.bestaetigtes_zuordnungsende,
        auftrag.grund.max_weiterverpflichtung_monate(),
    );

    if auftrag.verschobenes_zuordnungsende <= cap {
        return MsbEntscheidung::accept(tree, code("Z13"));
    }

    if auftrag.bereits_ausgeschoepft {
        return MsbEntscheidung::Reject(
            RejectReason::new(
                tree,
                code("Z22"),
                6,
                format!(
                    "Der maximale Weiterverpflichtungszeitraum von {} Monat(en) ab dem \
                     bestätigten Zuordnungsende {} ist mit {cap} bereits ausgeschöpft",
                    auftrag.grund.max_weiterverpflichtung_monate(),
                    auftrag.bestaetigtes_zuordnungsende,
                ),
            )
            .mit_termin(cap),
        );
    }

    MsbEntscheidung::Accept(AntwortDetail::new(tree, code("Z14")).mit_termin(cap))
}

/// The latest date a Weiterverpflichtung may reach.
///
/// Calendar months from the confirmed Zuordnungsende — „längstens drei Monate"
/// is a Monatsfrist (§ 188 Abs. 2 BGB), not 90 days. Where the target month is
/// short, the last day of that month is taken, which is the same rule
/// § 188 Abs. 3 BGB states.
fn max_termin(von: Date, monate: i64) -> Date {
    let mut jahr = von.year();
    let mut monat = i64::from(von.month() as u8) + monate;
    while monat > 12 {
        monat -= 12;
        jahr += 1;
    }
    let monat =
        time::Month::try_from(u8::try_from(monat).unwrap_or(12)).unwrap_or(time::Month::December);
    let mut tag = von.day();
    loop {
        if let Ok(d) = Date::from_calendar_date(jahr, monat, tag) {
            return d;
        }
        // § 188 Abs. 3 BGB — a month that has no such day ends on its last.
        tag -= 1;
        if tag == 0 {
            return von + Duration::days(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msb::types::Abmeldegrund;
    use crate::msb::types::Sparte;
    use time::Month;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn auftrag(grund: Abmeldegrund, bis: Date, ausgeschoepft: bool) -> WeiterverpflichtungAuftrag {
        WeiterverpflichtungAuftrag {
            sparte: Sparte::Strom,
            melo_id: "DE0000000001234567890000000000001".to_owned(),
            bestaetigtes_zuordnungsende: d(2026, Month::April, 1),
            verschobenes_zuordnungsende: bis,
            grund,
            bereits_ausgeschoepft: ausgeschoepft,
        }
    }

    #[test]
    fn inside_the_cap_is_a_plain_z13() {
        let e = pruefe_weiterverpflichtung(&auftrag(
            Abmeldegrund::VertragsEnde,
            d(2026, Month::April, 20),
            false,
        ));
        assert_eq!(e.antwortcode(), Some("Z13"));
    }

    /// The Anschlussnutzerwechsel gets three months where every other reason
    /// gets one — same date, opposite answer.
    #[test]
    fn the_cap_depends_on_the_abmeldegrund() {
        let bis = d(2026, Month::June, 1);
        assert_eq!(
            pruefe_weiterverpflichtung(&auftrag(Abmeldegrund::AnschlussnutzerWechsel, bis, false))
                .antwortcode(),
            Some("Z13")
        );
        assert_eq!(
            pruefe_weiterverpflichtung(&auftrag(Abmeldegrund::VertragsEnde, bis, false))
                .antwortcode(),
            Some("Z14")
        );
    }

    /// The first overshoot is agreed to, at the capped date — refusing it
    /// would leave the Messlokation heading for a Zuordnungslücke.
    #[test]
    fn a_first_overshoot_is_z14_with_the_capped_date() {
        let e = pruefe_weiterverpflichtung(&auftrag(
            Abmeldegrund::VertragsEnde,
            d(2026, Month::August, 1),
            false,
        ));
        assert_eq!(e.antwortcode(), Some("Z14"));
        assert_eq!(e.abweichender_termin(), Some(d(2026, Month::May, 1)));
    }

    /// `Z22`'s Bedingung is „eine weitere ORDERS nach Erreichen des max.
    /// möglichen Weiterverpflichtungszeitraums" — only then.
    #[test]
    fn a_further_overshoot_after_the_cap_is_z22() {
        let e = pruefe_weiterverpflichtung(&auftrag(
            Abmeldegrund::VertragsEnde,
            d(2026, Month::August, 1),
            true,
        ));
        assert_eq!(e.antwortcode(), Some("Z22"));
        assert_eq!(e.abweichender_termin(), Some(d(2026, Month::May, 1)));
    }

    /// „Längstens drei Monate" is a Monatsfrist, and § 188 Abs. 3 BGB decides
    /// what happens when the target month is shorter.
    #[test]
    fn the_cap_is_calendar_months_not_ninety_days() {
        assert_eq!(
            max_termin(d(2026, Month::January, 31), 1),
            d(2026, Month::February, 28)
        );
        assert_eq!(
            max_termin(d(2026, Month::November, 30), 3),
            d(2027, Month::February, 28)
        );
        assert_eq!(
            max_termin(d(2026, Month::April, 1), 3),
            d(2026, Month::July, 1)
        );
    }
}
