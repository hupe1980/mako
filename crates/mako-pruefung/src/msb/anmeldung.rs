//! `E_0201` — the Netzbetreiber's answer to an **Anmeldung Messstellenbetrieb**
//! (UTILMD 55042, MSBN → NB).
//!
//! WiM Strom Teil 1 Kap. 2.3.2 Nr. 2 states the checks verbatim:
//!
//! > Der NB prüft die eingegangene Anmeldung auf Vollständigkeit der
//! > übermittelten Angaben. Weiter prüft er:
//! > 1. Vorliegen der Versicherung über die Beauftragung des MSBN durch den AN.
//! > 2. Zulässiger Zuordnungsbeginn: Einhaltung der Mindestvorlaufzeit gem.
//! >    Prozessschritt 1.
//! > 3. Vorliegen eines Vertrages nach § 9 Abs. 1 Nr. 3 MsbG mit dem MSBN.
//!
//! Those three, and no more. In particular:
//!
//! - **An existing iMSys is not a ground to refuse.** § 5 MsbG gives the
//!   Anschlussnutzer a free choice of Messstellenbetreiber and § 14 MsbG the
//!   right to switch; the grundzuständiger MSB is the default, not a
//!   monopolist, and nothing in `E_0201` lets the NB refuse or defer on the
//!   metering technology. Escalating every iMSys Anmeldung would, at the
//!   rollout's end state, escalate every Anmeldung.
//! - **An unknown MSBN is not a ground either.** `E_0201` publishes no
//!   „Marktpartner unbekannt" code. What Kap. 2.3.2 Nr. 2 Ziff. 3 makes
//!   checkable is the *Rahmenvertrag* — and a missing Rahmenvertrag is a
//!   commercial fact the NB knows, not an inference from a directory lookup.

use mako_fristen::HolidayCalendar;
use mako_fristen::vorlauf::{VorlaufVerdict, anmeldung_vorlauf};
use time::Date;

use crate::antwort::RejectReason;
use crate::codes::lookup;

use super::types::{AnmeldungMsb, MsbEntscheidung};

/// Decide the NB's answer to an Anmeldung Messstellenbetrieb.
///
/// `eingangsdatum` is the Übertragungstag — the day the message arrived, in
/// German local time. It is passed in rather than read from a clock so the
/// decision is reproducible from the audit log.
///
/// # Panics
///
/// Only if the `E_0201` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn pruefe_anmeldung(
    anfrage: &AnmeldungMsb,
    eingangsdatum: Date,
    cal: HolidayCalendar,
) -> MsbEntscheidung {
    let tree = super::baum::anmeldung(anfrage.sparte);
    let code = |c: &str| lookup(tree, c).expect("code is published in the Anmeldung tree");

    // A lookup that could not be performed is not a finding. `ZC9` on a
    // Messlokation that exists refuses a lawful § 5 MsbG registration, and the
    // MSBN has no way to tell that apart from a genuine one.
    let Some(melo_bekannt) = anfrage.melo_bekannt else {
        return MsbEntscheidung::Escalate {
            reason: format!(
                "Messlokation {} konnte nicht nachgeschlagen werden — ohne Auskunft darf \
                 kein ZC9 gesendet werden",
                anfrage.melo_id
            ),
        };
    };
    if !melo_bekannt {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("ZC9"),
            2,
            format!(
                "Messlokation {} ist dem Netzbetreiber nicht bekannt",
                anfrage.melo_id
            ),
        ));
    }

    // Kap. 2.3.2 Nr. 2 Ziff. 1 — the Versicherung.
    if !anfrage.versicherung_liegt_vor {
        return MsbEntscheidung::Reject(RejectReason::new(
            tree,
            code("ZB6"),
            2,
            "Die Versicherung über die Beauftragung des MSBN durch den Anschlussnutzer \
             (bzw. über die Übernahme aufgrund des Umbaus auf iMS) fehlt",
        ));
    }

    // Kap. 2.3.2 Nr. 2 Ziff. 2 — the Mindestvorlaufzeit.
    let shape = anmeldung_vorlauf(anfrage.einrichtungsart.ist_erstmalig());
    match shape.check(eingangsdatum, anfrage.gewuenschter_zuordnungsbeginn, cal) {
        VorlaufVerdict::Ok => {}
        VorlaufVerdict::TooLate {
            shortfall_wt,
            earliest_possible,
        } => {
            return MsbEntscheidung::Reject(
                RejectReason::new(
                    tree,
                    code("E17"),
                    2,
                    format!(
                        "Mindestvorlaufzeit um {shortfall_wt} Werktage unterschritten; \
                         frühestmöglicher Zuordnungsbeginn ist {earliest_possible}"
                    ),
                )
                .mit_termin(earliest_possible),
            );
        }
        // „Zu früh" is not a published Ablehnungsgrund. An Anmeldung far ahead
        // of its Zuordnungsbeginn is unusual but lawful, so it goes to an
        // operator rather than being refused with a code that means something
        // else.
        VorlaufVerdict::TooEarly { excess_wt } => {
            return MsbEntscheidung::Escalate {
                reason: format!(
                    "Zuordnungsbeginn liegt {excess_wt} Werktage weiter in der Zukunft als \
                     üblich — E_0201 kennt hierfür keinen Ablehnungscode"
                ),
            };
        }
    }

    // Kap. 2.3.2 Nr. 2 Ziff. 3 — the Rahmenvertrag nach § 9 Abs. 1 Nr. 3 MsbG.
    match anfrage.msb_rahmenvertrag {
        Some(true) => {}
        Some(false) => {
            return MsbEntscheidung::Escalate {
                reason: format!(
                    "Kein Vertrag nach § 9 Abs. 1 Nr. 3 MsbG mit MSB {} — E_0201 veröffentlicht \
                     hierfür keinen Code; der Vertragsschluss ist zu klären, bevor die \
                     Anmeldung beantwortet wird",
                    anfrage.msbn_mp_id
                ),
            };
        }
        None => {
            return MsbEntscheidung::Escalate {
                reason: format!(
                    "Vertragslage nach § 9 Abs. 1 Nr. 3 MsbG mit MSB {} ist unbekannt",
                    anfrage.msbn_mp_id
                ),
            };
        }
    }

    MsbEntscheidung::accept(tree, code("E15"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::EBD_ANMELDUNG_MSB;
    use crate::msb::types::Einrichtungsart;
    use crate::msb::types::Sparte;
    use time::Month;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn anfrage(art: Einrichtungsart, beginn: Date) -> AnmeldungMsb {
        AnmeldungMsb {
            sparte: Sparte::Strom,
            melo_id: "DE0000000001234567890000000000001".to_owned(),
            msbn_mp_id: "9900000000003".to_owned(),
            gewuenschter_zuordnungsbeginn: beginn,
            einrichtungsart: art,
            versicherung_liegt_vor: true,
            melo_bekannt: Some(true),
            msb_rahmenvertrag: Some(true),
        }
    }

    #[test]
    fn a_compliant_anmeldung_is_confirmed_with_e15() {
        let beginn = d(2026, Month::June, 1);
        let uet = mako_fristen::sub_werktage(beginn, 15, CAL);
        let e = pruefe_anmeldung(
            &anfrage(Einrichtungsart::BestehenderMessstellenbetrieb, beginn),
            uet,
            CAL,
        );
        assert_eq!(e.antwortcode(), Some("E15"));
        assert_eq!(e.ebd(), Some("E_0201"));
    }

    /// A Zuordnungsbeginn inside the 15-Werktage lead time is refused with
    /// `E17`, and the answer names the earliest date the MSBN could still
    /// reach.
    #[test]
    fn a_short_vorlauffrist_is_e17_and_names_the_next_possible_date() {
        let beginn = d(2026, Month::June, 1);
        let uet = mako_fristen::sub_werktage(beginn, 5, CAL);
        let e = pruefe_anmeldung(
            &anfrage(Einrichtungsart::BestehenderMessstellenbetrieb, beginn),
            uet,
            CAL,
        );
        assert_eq!(e.antwortcode(), Some("E17"));
        assert_eq!(
            e.abweichender_termin(),
            Some(mako_fristen::add_werktage(uet, 15, CAL))
        );
    }

    /// The same message is lawful when it sets the Messstellenbetrieb up for
    /// the first time — 7 Werktage, not 15.
    #[test]
    fn erstmalige_einrichtung_takes_the_short_window() {
        let beginn = d(2026, Month::June, 1);
        let uet = mako_fristen::sub_werktage(beginn, 8, CAL);
        assert_eq!(
            pruefe_anmeldung(
                &anfrage(Einrichtungsart::ErstmaligeEinrichtung, beginn),
                uet,
                CAL
            )
            .antwortcode(),
            Some("E15")
        );
        assert_eq!(
            pruefe_anmeldung(
                &anfrage(Einrichtungsart::BestehenderMessstellenbetrieb, beginn),
                uet,
                CAL
            )
            .antwortcode(),
            Some("E17")
        );
    }

    #[test]
    fn a_missing_versicherung_is_zb6() {
        let beginn = d(2026, Month::June, 1);
        let mut a = anfrage(Einrichtungsart::BestehenderMessstellenbetrieb, beginn);
        a.versicherung_liegt_vor = false;
        let uet = mako_fristen::sub_werktage(beginn, 15, CAL);
        assert_eq!(pruefe_anmeldung(&a, uet, CAL).antwortcode(), Some("ZB6"));
    }

    /// A transport failure is not evidence of absence.
    #[test]
    fn an_unanswerable_lookup_escalates_rather_than_rejecting() {
        let beginn = d(2026, Month::June, 1);
        let mut a = anfrage(Einrichtungsart::BestehenderMessstellenbetrieb, beginn);
        a.melo_bekannt = None;
        let uet = mako_fristen::sub_werktage(beginn, 15, CAL);
        assert!(matches!(
            pruefe_anmeldung(&a, uet, CAL),
            MsbEntscheidung::Escalate { .. }
        ));
    }

    /// § 5 MsbG: the Anschlussnutzer chooses freely, so the metering
    /// technology at the Messlokation is not part of this decision at all.
    /// `E_0201` publishes no code for it, and there is no input for it here.
    #[test]
    fn e_0201_has_no_metering_technology_ground() {
        let tree_codes = crate::codes::CODELISTEN
            .iter()
            .find(|(id, _)| *id == EBD_ANMELDUNG_MSB)
            .expect("registered")
            .1;
        for c in tree_codes {
            let b = c.bedeutung.to_lowercase();
            assert!(
                !b.contains("imsys") && !b.contains("ims ") && !b.contains("messsystem"),
                "{} names a metering technology: {}",
                c.code,
                c.bedeutung
            );
        }
    }
}
