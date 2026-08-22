//! **Vorlauffristen** — the windows anchored on a date the message *carries*.
//!
//! [`antwort`](crate::antwort) answers one question: how long may the receiver
//! take? This module answers the other one: was the sender allowed to ask for
//! that date at all?
//!
//! The two are different in kind and mixing them loses one of them. An
//! Antwortfrist runs forward from the Übertragungstag and produces a deadline;
//! a Vorlauffrist runs *backward* from a Wunschtermin inside the payload and
//! produces a verdict on the message. WiM Strom Teil 1 states both for almost
//! every Use-Case, and the Netzbetreiber is required to check the second —
//! Kap. 2.3.2 Nr. 2 lists „Zulässiger Zuordnungsbeginn: Einhaltung der
//! Mindestvorlaufzeit gem. Prozessschritt 1" as one of three checks on an
//! Anmeldung MSB. Without it every Anmeldung is confirmed on the date the
//! counterparty picked, and `E17` (Ablehnung wg. Fristüberschreitung) — a code
//! the Entscheidungsbaum publishes for exactly this — can never be reached.
//!
//! ## The five shapes
//!
//! | Shape | Wording |
//! |---|---|
//! | [`VorlaufShape::LatestWerktageBefore`] | „Spätester ÜT ist der *n*. WT vor dem …" |
//! | [`VorlaufShape::WindowWerktageBefore`] | „Frühester ÜT ist der *a*. WT und spätester der *b*. WT vor dem …" |
//! | [`VorlaufShape::LatestWerktageAfter`] | „Spätester ÜT ist der *n*. WT nach dem …" |
//! | [`VorlaufShape::EarliestWerktageAfter`] | „… frühestens am *n*. auf diese Aktion folgenden WT" |
//! | [`VorlaufShape::Korridor`] | „… muss in einem Zeitraum vom *n*. WT vor bis zum *n*. WT nach dem … liegen" |
//!
//! The Korridor is not two independent bounds: WiM Teil 1 Kap. 2.3.2 Nr. 5/6
//! calls it the *Realisierungskorridor* and it is symmetric by construction.
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2a — WiM Strom Teil 1 (Lesefassung), Kap. 2.3–3.3
//! - EDI@Energy Entscheidungsbaum-Diagramme und Codelisten 4.3 — `E17`

use time::Date;

use crate::HolidayCalendar;

/// The shape of a Vorlauffrist, in Werktagen relative to its anchor date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VorlaufShape {
    /// „Spätester ÜT ist der `n`. WT **vor** dem …" — a minimum lead time.
    LatestWerktageBefore(u32),
    /// „Frühester ÜT ist der `earliest`. WT und spätester der `latest`. WT
    /// **vor** dem …" — a send window with both ends closed.
    ///
    /// `earliest` is the *larger* number: the 8. WT vor a date is earlier than
    /// the 5. WT vor the same date.
    WindowWerktageBefore {
        /// Werktage before the anchor at which the window opens (the larger).
        earliest: u32,
        /// Werktage before the anchor at which it closes (the smaller).
        latest: u32,
    },
    /// „Spätester ÜT ist der `n`. WT **nach** dem …" — a reporting deadline
    /// anchored on a business date rather than on an arrival instant.
    LatestWerktageAfter(u32),
    /// „… frühestens am `n`. auf diese Aktion folgenden WT" — a minimum
    /// notice period the *requested* date must respect.
    EarliestWerktageAfter(u32),
    /// „… muss in einem Zeitraum vom `n`. WT vor bis zum `n`. WT nach dem …
    /// liegen" — the WiM Realisierungskorridor.
    Korridor(u32),
}

/// What a date was checked against, so a rejection can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The Zuordnungsbeginn the sender wishes for (Anmeldung MSB).
    GewuenschterZuordnungsbeginn,
    /// The Zuordnungsende the sender wishes for (Ende MSB).
    GewuenschtesZuordnungsende,
    /// The Zuordnungsbeginn the NB confirmed in its Anmeldebestätigung.
    BestaetigterZuordnungsbeginn,
    /// The Zuordnungsende the NB confirmed — possibly moved by a
    /// Weiterverpflichtung („verschobenes Zuordnungsende").
    BestaetigtesZuordnungsende,
    /// The Übertragungstag of the message that opened the step.
    Uebertragungstag,
    /// The Änderungstermin of a Messlokationsänderung.
    Aenderungstermin,
    /// The Gerätewechseltermin announced in the Gerätewechselabsicht.
    Gerätewechseltermin,
}

impl Anchor {
    /// The wire spelling, for operator-facing rejection reasons.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GewuenschterZuordnungsbeginn => "gewünschter Zuordnungsbeginn",
            Self::GewuenschtesZuordnungsende => "gewünschtes Zuordnungsende",
            Self::BestaetigterZuordnungsbeginn => "bestätigter Zuordnungsbeginn",
            Self::BestaetigtesZuordnungsende => "bestätigtes Zuordnungsende",
            Self::Uebertragungstag => "Übertragungstag",
            Self::Aenderungstermin => "Änderungstermin",
            Self::Gerätewechseltermin => "Gerätewechseltermin",
        }
    }
}

/// The verdict of a Vorlauffrist check.
///
/// `TooLate` is the one that carries a published Antwortcode — `E17`,
/// „Ablehnung wg. Fristüberschreitung" (EBD 4.3 `S_0056` / `S_0060` /
/// `S_0064`). `TooEarly` has none: no WiM Ablehnungsgrund says „you asked too
/// far ahead", so a caller must escalate rather than invent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VorlaufVerdict {
    /// The date respects the window.
    Ok,
    /// The Übertragungstag is later than the window allows. `shortfall_wt` is
    /// how many Werktage of lead time are missing; `earliest_possible` is the
    /// next date that *would* satisfy it, which several Use-Cases require the
    /// answer to name.
    TooLate {
        /// Werktage of lead time missing.
        shortfall_wt: u32,
        /// The earliest anchor date this Übertragungstag could still reach.
        earliest_possible: Date,
    },
    /// The Übertragungstag is earlier than the window opens.
    TooEarly {
        /// Werktage before the window opens.
        excess_wt: u32,
    },
}

impl VorlaufVerdict {
    /// Whether the check passed.
    #[must_use]
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

impl VorlaufShape {
    /// Check an Übertragungstag against an anchor date.
    ///
    /// `uebertragungstag` is the day the message was transmitted;
    /// `anchor` is the date it carries (or, for the anchored-after shapes, the
    /// business date the report follows).
    ///
    /// # Panics
    ///
    /// Panics only if date arithmetic overflows the Gregorian calendar.
    #[must_use]
    pub fn check(
        self,
        uebertragungstag: Date,
        anchor: Date,
        cal: HolidayCalendar,
    ) -> VorlaufVerdict {
        match self {
            Self::LatestWerktageBefore(n) => {
                let latest_uet = crate::sub_werktage(anchor, n, cal);
                if uebertragungstag <= latest_uet {
                    VorlaufVerdict::Ok
                } else {
                    VorlaufVerdict::TooLate {
                        shortfall_wt: crate::werktage_between(latest_uet, uebertragungstag, cal),
                        earliest_possible: crate::add_werktage(uebertragungstag, n, cal),
                    }
                }
            }
            Self::WindowWerktageBefore { earliest, latest } => {
                let opens = crate::sub_werktage(anchor, earliest, cal);
                let closes = crate::sub_werktage(anchor, latest, cal);
                if uebertragungstag < opens {
                    VorlaufVerdict::TooEarly {
                        excess_wt: crate::werktage_between(uebertragungstag, opens, cal),
                    }
                } else if uebertragungstag > closes {
                    VorlaufVerdict::TooLate {
                        shortfall_wt: crate::werktage_between(closes, uebertragungstag, cal),
                        earliest_possible: crate::add_werktage(uebertragungstag, latest, cal),
                    }
                } else {
                    VorlaufVerdict::Ok
                }
            }
            Self::LatestWerktageAfter(n) => {
                let latest_uet = crate::add_werktage(anchor, n, cal);
                if uebertragungstag <= latest_uet {
                    VorlaufVerdict::Ok
                } else {
                    VorlaufVerdict::TooLate {
                        shortfall_wt: crate::werktage_between(latest_uet, uebertragungstag, cal),
                        earliest_possible: anchor,
                    }
                }
            }
            Self::EarliestWerktageAfter(n) => {
                // Here `anchor` is the *requested* date and `uebertragungstag`
                // the day the notice went out: the requested date must be at
                // least `n` Werktage away.
                let earliest_possible = crate::add_werktage(uebertragungstag, n, cal);
                if anchor >= earliest_possible {
                    VorlaufVerdict::Ok
                } else {
                    VorlaufVerdict::TooLate {
                        shortfall_wt: crate::werktage_between(anchor, earliest_possible, cal),
                        earliest_possible,
                    }
                }
            }
            Self::Korridor(n) => {
                // `anchor` is the confirmed Zuordnungstermin, `uebertragungstag`
                // the requested Übernahme-/Wechselzeitpunkt.
                let opens = crate::sub_werktage(anchor, n, cal);
                let closes = crate::add_werktage(anchor, n, cal);
                if uebertragungstag < opens {
                    VorlaufVerdict::TooEarly {
                        excess_wt: crate::werktage_between(uebertragungstag, opens, cal),
                    }
                } else if uebertragungstag > closes {
                    VorlaufVerdict::TooLate {
                        shortfall_wt: crate::werktage_between(closes, uebertragungstag, cal),
                        earliest_possible: opens,
                    }
                } else {
                    VorlaufVerdict::Ok
                }
            }
        }
    }
}

/// One Prozessschritt's Vorlauffrist, with its Fundstelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VorlaufObligation {
    /// Stable slug, used as the lookup key and in operator-facing reasons.
    pub key: &'static str,
    /// The Prüfidentifikator carrying the message, where the step has one.
    pub pid: Option<u32>,
    /// Human-readable Prozessschritt name.
    pub name: &'static str,
    /// What the window is measured against.
    pub anchor: Anchor,
    /// The window.
    pub shape: VorlaufShape,
    /// Citation, for the audit trail.
    pub source: &'static str,
}

/// Mindestvorlaufzeit for an Anmeldung MSB **at first setup** of the
/// Messstellenbetrieb, in Werktagen (WiM Teil 1 Kap. 2.3.2 Nr. 1).
pub const ANMELDUNG_ERSTMALIG_WT: u32 = 7;

/// Mindestvorlaufzeit for an ordinary Anmeldung MSB, in Werktagen
/// (WiM Teil 1 Kap. 2.3.2 Nr. 1).
pub const ANMELDUNG_WT: u32 = 15;

/// Mindestvorlaufzeit for an Abmeldung (Ende MSB), in Werktagen
/// (WiM Teil 1 Kap. 2.4.2 Nr. 1).
///
/// Does **not** apply when the Abmeldegrund is Außerbetriebnahme der
/// Messlokation: that case is „unverzüglich nach Außerbetriebnahme", with the
/// gewünschtes Zuordnungsende fixed to the Folgetag 00:00 des
/// Geräteausbaudatums.
pub const ABMELDUNG_WT: u32 = 20;

/// The WiM Realisierungskorridor, in Werktagen either side of the confirmed
/// Zuordnungstermin (WiM Teil 1 Kap. 2.3.2 Nr. 5/6 and 2.5.2 Nr. 1/2).
pub const REALISIERUNGSKORRIDOR_WT: u32 = 9;

/// WiM Strom Teil 1 — every Prozessschritt whose window is anchored on a date
/// in the payload rather than on the arrival instant.
///
/// Deliberately keyed by slug rather than by PID: three of these steps share a
/// PID with a differently-anchored one (55168 is both the Verpflichtungsanfrage
/// and the Aufforderung; 17011 covers the NB and the LF variant), and one —
/// the Anmeldung — has two windows selected by a payload flag rather than by
/// its PID. A PID alone cannot pick the right row.
pub const WIM_STROM: &[VorlaufObligation] = &[
    VorlaufObligation {
        key: "wim.anmeldung-msb",
        pid: Some(55_042),
        name: "Anmeldung MSB",
        anchor: Anchor::GewuenschterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageBefore(ANMELDUNG_WT),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 1 — spätester ÜT ist der 15. WT vor dem \
                 gewünschten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.anmeldung-msb.erstmalige-einrichtung",
        pid: Some(55_042),
        name: "Anmeldung MSB (erstmalige Einrichtung des Messstellenbetriebes)",
        anchor: Anchor::GewuenschterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageBefore(ANMELDUNG_ERSTMALIG_WT),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 1 — bei erstmaliger Einrichtung des \
                 Messstellenbetriebes: spätester ÜT ist der 7. WT",
    },
    VorlaufObligation {
        key: "wim.ende-msb",
        pid: Some(55_051),
        name: "Ende MSB (Abmeldung)",
        anchor: Anchor::GewuenschtesZuordnungsende,
        shape: VorlaufShape::LatestWerktageBefore(ABMELDUNG_WT),
        source: "WiM Strom Teil 1 Kap. 2.4.2 Nr. 1 — spätester ÜT ist der 20. WT vor dem \
                 gewünschten Zuordnungsende",
    },
    VorlaufObligation {
        key: "wim.verpflichtungsanfrage",
        pid: Some(55_168),
        name: "Verpflichtungsanfrage an den gMSB",
        anchor: Anchor::BestaetigtesZuordnungsende,
        shape: VorlaufShape::WindowWerktageBefore {
            earliest: 8,
            latest: 5,
        },
        source: "WiM Strom Teil 1 Kap. 2.4.2 Nr. 3 — frühester ÜT ist der 8. WT und spätester \
                 der 5. WT vor dem vorläufig bestätigten Zuordnungsende",
    },
    VorlaufObligation {
        key: "wim.gmsb-uebernahme-anstoss",
        pid: None,
        name: "Anstoß Gerätewechsel/Geräteübernahme durch den gMSB",
        anchor: Anchor::BestaetigtesZuordnungsende,
        shape: VorlaufShape::LatestWerktageBefore(4),
        source: "WiM Strom Teil 1 Kap. 2.5.2 Nr. 1/2 — spätester ÜT ist der 4. WT vor dem \
                 vorläufig bestätigten bzw. verschobenen Zuordnungsende",
    },
    VorlaufObligation {
        key: "wim.realisierungskorridor",
        pid: None,
        name: "Realisierungskorridor Übernahme-/Wechselzeitpunkt",
        anchor: Anchor::BestaetigterZuordnungsbeginn,
        shape: VorlaufShape::Korridor(REALISIERUNGSKORRIDOR_WT),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 5/6 — vom 9. WT vor bis zum 9. WT nach dem \
                 vom NB bestätigten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.mitteilung-gesamtvorgang",
        pid: Some(21_009),
        name: "Mitteilung über Gesamtvorgang (MSBN → NB)",
        anchor: Anchor::BestaetigterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageAfter(10),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 7 — spätester ÜT ist der 10. WT nach dem vom \
                 NB bestätigten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.scheitern-gesamtvorgang",
        pid: Some(21_013),
        name: "Mitteilung über das Scheitern des Gesamtvorgangs (NB → MSBN)",
        anchor: Anchor::BestaetigterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageAfter(11),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 16 — spätester ÜT ist der 11. WT nach dem vom \
                 NB bestätigten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.geraetewechsel-termin",
        pid: Some(17_009),
        name: "Gerätewechseltermin nach Anzeige der Gerätewechselabsicht",
        anchor: Anchor::Gerätewechseltermin,
        shape: VorlaufShape::EarliestWerktageAfter(4),
        source: "WiM Strom Teil 1 Kap. 3.1.2 Nr. 1 — frühestens am 4. auf die Anzeige \
                 folgenden WT",
    },
    VorlaufObligation {
        key: "wim.antwort-geraetewechselabsicht",
        pid: Some(19_015),
        name: "Antwort auf die Gerätewechselabsicht (Eigenausbau ja/nein)",
        anchor: Anchor::Gerätewechseltermin,
        shape: VorlaufShape::LatestWerktageBefore(2),
        source: "WiM Strom Teil 1 Kap. 3.1.2 Nr. 2 — spätester ÜT ist der 2. WT vor dem \
                 Gerätewechseltermin",
    },
    VorlaufObligation {
        key: "wim.beauftragung-aenderung-technik",
        pid: Some(17_011),
        name: "Beauftragung Änderung der Technik an der Messlokation",
        anchor: Anchor::Aenderungstermin,
        shape: VorlaufShape::LatestWerktageBefore(20),
        source: "WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 1 — spätester ÜT ist der 20. WT \
                 vor dem gewünschten Änderungstermin",
    },
    VorlaufObligation {
        key: "wim.scheitern-aenderung-technik",
        pid: None,
        name: "Scheitern der Änderung der Technik",
        anchor: Anchor::Aenderungstermin,
        shape: VorlaufShape::LatestWerktageAfter(3),
        source: "WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 5 — spätester ÜT ist der 3. WT \
                 nach dem ursprünglich bestätigten Änderungstermin",
    },
];

/// Look up a Vorlauffrist by its slug.
#[must_use]
pub fn vorlauf(key: &str) -> Option<&'static VorlaufObligation> {
    WIM_STROM.iter().find(|o| o.key == key)
}

/// The Mindestvorlaufzeit an Anmeldung MSB must respect.
///
/// The two windows are **not** interchangeable: an ordinary MSB-Wechsel needs
/// 15 Werktage, the erstmalige Einrichtung des Messstellenbetriebes 7. Picking
/// the ordinary one for a Neuanlage rejects a valid Anmeldung eight Werktage
/// early; picking the short one for a Wechsel confirms a date the NB cannot
/// honour, because the Realisierungskorridor around it no longer fits.
#[must_use]
pub const fn anmeldung_vorlauf(erstmalige_einrichtung: bool) -> VorlaufShape {
    VorlaufShape::LatestWerktageBefore(if erstmalige_einrichtung {
        ANMELDUNG_ERSTMALIG_WT
    } else {
        ANMELDUNG_WT
    })
}

/// The Realisierungskorridor around a confirmed Zuordnungstermin, as a closed
/// date range.
///
/// # Panics
///
/// Panics only if date arithmetic overflows the Gregorian calendar.
#[must_use]
pub fn realisierungskorridor(
    bestaetigter_termin: Date,
    cal: HolidayCalendar,
) -> std::ops::RangeInclusive<Date> {
    crate::sub_werktage(bestaetigter_termin, REALISIERUNGSKORRIDOR_WT, cal)
        ..=crate::add_werktage(bestaetigter_termin, REALISIERUNGSKORRIDOR_WT, cal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    #[test]
    fn anmeldung_fifteen_werktage_lead_time() {
        // Zuordnungsbeginn Mon 2025-02-03; 15 WT before is Mon 2025-01-13.
        let beginn = d(2025, Month::February, 3);
        let shape = anmeldung_vorlauf(false);
        assert_eq!(shape, VorlaufShape::LatestWerktageBefore(15));
        assert!(
            shape
                .check(d(2025, Month::January, 13), beginn, CAL)
                .is_ok()
        );
        assert!(
            !shape
                .check(d(2025, Month::January, 14), beginn, CAL)
                .is_ok()
        );
    }

    /// The Neuanlage window is shorter, and using the ordinary one refuses a
    /// valid Anmeldung eight Werktage early.
    #[test]
    fn erstmalige_einrichtung_uses_the_short_window() {
        let beginn = d(2025, Month::February, 3);
        let uet = d(2025, Month::January, 23);
        assert!(anmeldung_vorlauf(true).check(uet, beginn, CAL).is_ok());
        assert!(!anmeldung_vorlauf(false).check(uet, beginn, CAL).is_ok());
    }

    #[test]
    fn too_late_names_the_earliest_reachable_date() {
        let beginn = d(2025, Month::February, 3);
        let uet = d(2025, Month::January, 20);
        match anmeldung_vorlauf(false).check(uet, beginn, CAL) {
            VorlaufVerdict::TooLate {
                shortfall_wt,
                earliest_possible,
            } => {
                assert!(shortfall_wt > 0);
                assert_eq!(earliest_possible, crate::add_werktage(uet, 15, CAL));
            }
            other => panic!("expected TooLate, got {other:?}"),
        }
    }

    #[test]
    fn verpflichtungsanfrage_window_has_both_ends() {
        let ende = d(2025, Month::March, 3);
        let shape = vorlauf("wim.verpflichtungsanfrage")
            .expect("in table")
            .shape;
        let opens = crate::sub_werktage(ende, 8, CAL);
        let closes = crate::sub_werktage(ende, 5, CAL);
        assert!(shape.check(opens, ende, CAL).is_ok());
        assert!(shape.check(closes, ende, CAL).is_ok());
        assert!(matches!(
            shape.check(crate::sub_werktage(opens, 1, CAL), ende, CAL),
            VorlaufVerdict::TooEarly { .. }
        ));
        assert!(matches!(
            shape.check(crate::add_werktage(closes, 1, CAL), ende, CAL),
            VorlaufVerdict::TooLate { .. }
        ));
    }

    #[test]
    fn realisierungskorridor_is_symmetric() {
        let beginn = d(2025, Month::April, 1);
        let korridor = realisierungskorridor(beginn, CAL);
        assert_eq!(*korridor.start(), crate::sub_werktage(beginn, 9, CAL));
        assert_eq!(*korridor.end(), crate::add_werktage(beginn, 9, CAL));
        let shape = VorlaufShape::Korridor(REALISIERUNGSKORRIDOR_WT);
        assert!(shape.check(*korridor.start(), beginn, CAL).is_ok());
        assert!(shape.check(*korridor.end(), beginn, CAL).is_ok());
        assert!(
            !shape
                .check(crate::add_werktage(beginn, 10, CAL), beginn, CAL)
                .is_ok()
        );
    }

    #[test]
    fn geraetewechsel_termin_needs_four_werktage_notice() {
        let shape = vorlauf("wim.geraetewechsel-termin")
            .expect("in table")
            .shape;
        let anzeige = d(2025, Month::May, 5);
        let earliest = crate::add_werktage(anzeige, 4, CAL);
        assert!(shape.check(anzeige, earliest, CAL).is_ok());
        assert!(matches!(
            shape.check(anzeige, crate::sub_werktage(earliest, 1, CAL), CAL),
            VorlaufVerdict::TooLate { .. }
        ));
    }

    #[test]
    fn every_entry_cites_a_chapter_and_has_a_unique_key() {
        let mut keys: Vec<_> = WIM_STROM.iter().map(|o| o.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate Vorlauffrist key");
        for o in WIM_STROM {
            assert!(
                o.source.contains("Kap."),
                "{} cites no chapter: {}",
                o.key,
                o.source
            );
        }
    }

    #[test]
    fn sub_and_add_werktage_are_inverse() {
        let start = d(2026, Month::August, 21);
        for n in 0..40 {
            let forward = crate::add_werktage(start, n, CAL);
            assert_eq!(crate::werktage_between(start, forward, CAL), n);
            let back = crate::sub_werktage(forward, n, CAL);
            // `start` may be a Sunday, in which case walking back lands on the
            // preceding Werktag — the count is what round-trips, not the date.
            assert_eq!(crate::add_werktage(back, n, CAL), forward);
        }
    }
}
