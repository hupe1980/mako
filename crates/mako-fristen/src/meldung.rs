//! **Meldepflichten** — messages a Festlegung obliges a party to *send*, with
//! no answer of their own.
//!
//! [`antwort`](crate::antwort) covers the other shape: an inbound message
//! arrives and a Bestätigung or Ablehnung closes the step. A Meldepflicht has no
//! pair, which is what makes it easy to omit — nobody waits for a reply, so a
//! missed one surfaces only as a counterparty's stale view of who supplies a
//! Marktlokation.
//!
//! The Lieferbeginn carries three per Sparte, all owed by the NB to a *third*
//! party:
//!
//! | Sparte | PID | Message | NB → | Prozessschritt |
//! |---|---|---|---|---|
//! | Strom | 55036 | Information über existierende Zuordnung | LFN | Nr. 2 |
//! | Strom | 55037 | Beendigung der Zuordnung | LFA | Nr. 10 |
//! | Strom | 55038 | Aufhebung einer zukünftigen Zuordnung | LFZ | Nr. 13 |
//! | Gas | 44036 | Informationsmeldung über existierende Zuordnung | LFN | Nr. 2 |
//! | Gas | 44037 | Informationsmeldung zur Beendigung der Zuordnung | LFA | Nr. 6 |
//! | Gas | 44038 | Informationsmeldung zur Aufhebung einer zuk. Zuordnung | LFZ | Nr. 7 |
//!
//! 55036 / 44036 is what tells the new supplier **who the old supplier is** —
//! „Hierbei teilt der NB dem LFN insbesondere die Identität des LFA … mit".
//!
//! # These windows belong to the Lieferbeginn, not to the PID
//!
//! The same Prüfidentifikator recurs in other Sequenzdiagramme with a
//! *different* window, so a caller must key on the process it is running and
//! not on the message it is about to send. The Anwendungsübersicht der
//! Prüfidentifikatoren 4.0 lists, beside the six rows above:
//!
//! | PID | Sequenzdiagramm | Nr. | Spätester ÜZ |
//! |---|---|---|---|
//! | 55037 | Fall 2 / 3 / 4: LF-Zuordnung bei EEG-/KWKG-Anlagen | 8 | **17:00 Uhr am ÜT** von Nr. 1 |
//! | 55038 | Lieferende von NB an LF | 8 | **07:00 Uhr** des 1. WT nach dem ÜT von Nr. 1 |
//!
//! Those two run from the NB's **own** initiating message rather than from an
//! inbound one, and they would need a second entry under the same PID —
//! [`meldepflicht`] resolves one per Prüfidentifikator, so the catalogue cannot
//! hold both windows yet. `ROADMAP.md` carries the work.
//!
//! [`GPKE_LIEFERENDE`] is the same shape and *is* catalogued, because 55611
//! appears in no other Sequenzdiagramm: it is anchored on
//! [`MeldungAnchor::EigeneAnkuendigung`].
//!
//! `services/makod/tests/meldepflicht_coverage.rs` pins the catalogue against
//! what the PID router actually handles, so a new entry here is either routed
//! or declared missing with a reason. No deadline is registered for a message
//! that cannot be rendered.
//!
//! # Anchors
//!
//! The clock does not always start where the process does. The Strom windows run
//! from the **Eingang der Anmeldung**; the Gas Beendigung and Aufhebung run from
//! the **Antwort** („am selben Tag wie in Prozessschritt 5"). [`MeldungAnchor`]
//! names which, so a caller cannot resolve one against the other.

use time::OffsetDateTime;

use crate::HolidayCalendar;
use crate::antwort::{Family, FristShape};

/// Which instant a Meldepflicht's window runs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeldungAnchor {
    /// The arrival of the message that opened the process — for the Lieferbeginn,
    /// the ÜT der Anmeldung.
    Eingang,
    /// The instant the process's own Antwort went out. Used where the
    /// Sequenzdiagramm says „am selben Tag wie in Prozessschritt N" rather than
    /// counting from the Eingang.
    Antwort,
    /// The instant **this party's own initiating message** went out.
    ///
    /// Not every Meldepflicht hangs off something that arrived. The SD
    /// „Lieferende von NB an LF" is opened by the NB itself (55007, Nr. 1), and
    /// the notifications it owes downstream count „nach dem ÜT von Nr. 1" —
    /// which is the NB's own dispatch. Resolving those against an arrival
    /// resolves them against nothing.
    EigeneAnkuendigung,
}

/// One message a party is obliged to send, with no answer expected back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meldepflicht {
    /// The Prüfidentifikator of the message that must go out.
    pub pid: u32,
    /// Human-readable name, as the Anwendungsübersicht spells it.
    pub name: &'static str,
    /// The Marktrolle that owes the message.
    pub sent_by: &'static str,
    /// The Marktrolle that receives it.
    pub sent_to: &'static str,
    /// The inbound Prüfidentifikatoren whose arrival can open this obligation.
    ///
    /// More than one because the Lieferbeginn runs the same Sequenzdiagramm for
    /// a verbrauchende Marktlokation, a Tranche and an erzeugende Marktlokation.
    pub triggered_by: &'static [u32],
    /// Which instant [`Meldepflicht::frist`] is resolved against.
    pub anchor: MeldungAnchor,
    /// The window shape.
    pub frist: FristShape,
    /// Which Festlegung family states it.
    pub family: Family,
    /// Citation, for the audit trail.
    pub source: &'static str,
}

impl Meldepflicht {
    /// The instant by which this message must have been sent.
    ///
    /// `anchor_at` must be the instant [`Meldepflicht::anchor`] names — the
    /// arrival of the Anmeldung for [`MeldungAnchor::Eingang`], the dispatch of
    /// the Antwort for [`MeldungAnchor::Antwort`]. Passing the wrong one
    /// resolves a real window against a fictional start.
    #[must_use]
    pub fn due_at(&self, anchor_at: OffsetDateTime, cal: HolidayCalendar) -> OffsetDateTime {
        self.frist.due_at(anchor_at, cal)
    }
}

const fn at(hour: u8) -> time::Time {
    match time::Time::from_hms(hour, 0, 0) {
        Ok(t) => t,
        Err(_) => panic!("whole hour is a valid Time"),
    }
}

/// GPKE Strom — the three notifications the NB owes around a Lieferbeginn.
///
/// All three run from the **Eingang der Anmeldung** (Nr. 1), so all three are
/// resolvable the moment it arrives. Nr. 2's **07:00 Uhr** closes four hours
/// before the 11:00 answer to the same message: the LFN is meant to learn the
/// LFA's identity with a Werktag left to act on it.
pub const GPKE: &[Meldepflicht] = &[
    Meldepflicht {
        pid: 55_036,
        name: "Information über existierende Zuordnung",
        sent_by: "NB",
        sent_to: "LFN",
        triggered_by: &[55_001, 55_077],
        anchor: MeldungAnchor::Eingang,
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(7),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 2 — „Unverzüglich, jedoch \
                 spätester ÜZ ist 07:00 Uhr des 1. WT nach dem ÜT von Nr. 1\"; „Hierbei teilt \
                 der NB dem LFN insbesondere die Identität des LFA an der Marktlokation bzw. \
                 Tranche … mit\". Die Information ist auch dann zu versenden, sofern LFA und \
                 LFN identisch sind.",
    },
    Meldepflicht {
        pid: 55_037,
        name: "Beendigung der Zuordnung",
        sent_by: "NB",
        sent_to: "LFA",
        triggered_by: &[55_001, 55_077],
        anchor: MeldungAnchor::Eingang,
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(12),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 10 — „Unverzüglich nach dem \
                 ÜZ von Nr. 5, jedoch spätester ÜZ ist 12:00 Uhr des 1. WT nach dem ÜT von \
                 Nr. 1\"",
    },
    Meldepflicht {
        pid: 55_038,
        name: "Aufhebung einer zukünftigen Zuordnung",
        sent_by: "NB",
        sent_to: "LFZ",
        triggered_by: &[55_001, 55_077],
        anchor: MeldungAnchor::Eingang,
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(12),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 13 — „Unverzüglich nach dem \
                 ÜZ von Nr. 5, jedoch spätester ÜZ ist 12:00 Uhr des 1. WT nach dem ÜT von \
                 Nr. 1\". Enthält die Anmeldung des LFN ein Zuordnungsende, ist die Zuordnung \
                 des LFZ nur aufzuheben, wenn der Zuordnungsbeginn des LFZ kleiner dem \
                 Zuordnungsende des LFN ist.",
    },
];

/// GeLi Gas — the same three notifications, on Gas's day-granular windows.
///
/// Nr. 2 counts from the Eingang der Anmeldung like its Strom twin; Nr. 6 and
/// Nr. 7 are „am selben Tag wie in Prozessschritt 5, wenn die Anmeldung
/// bestätigt wurde" — anchored on the **Antwort**, and owed only on a
/// confirmation.
pub const GELI_GAS: &[Meldepflicht] = &[
    Meldepflicht {
        pid: 44_036,
        name: "Informationsmeldung über existierende Zuordnung",
        sent_by: "NB",
        sent_to: "LFN",
        triggered_by: &[44_001],
        anchor: MeldungAnchor::Eingang,
        frist: FristShape::EndOfWerktag(4),
        family: Family::GeliGas,
        source: "AWH GeLi Gas V1.2 Kap. 2.5.2 SD Lieferbeginn Nr. 2 — „Unverzüglich, jedoch \
                 spätestens bis zum Ablauf des 4. WT nach Eingang der Anmeldung\"; „Hierbei \
                 teilt der NB dem LFN insbesondere die Identität des LFA mit\". Die \
                 Informationsmeldung kann nicht als Antwortnachricht in Bezug auf mögliche \
                 Stornierungen interpretiert werden.",
    },
    Meldepflicht {
        pid: 44_037,
        name: "Informationsmeldung zur Beendigung der Zuordnung",
        sent_by: "NB",
        sent_to: "LFA",
        triggered_by: &[44_001],
        anchor: MeldungAnchor::Antwort,
        frist: FristShape::SameDay,
        family: Family::GeliGas,
        source: "AWH GeLi Gas V1.2 Kap. 2.5.2 SD Lieferbeginn Nr. 6 — „Am selben Tag wie in \
                 Prozessschritt 5, wenn die Anmeldung bestätigt wurde\"",
    },
    Meldepflicht {
        pid: 44_038,
        name: "Informationsmeldung zur Aufhebung einer zukünftigen Zuordnung",
        sent_by: "NB",
        sent_to: "LFZ",
        triggered_by: &[44_001],
        anchor: MeldungAnchor::Antwort,
        frist: FristShape::SameDay,
        family: Family::GeliGas,
        source: "AWH GeLi Gas V1.2 Kap. 2.5.2 SD Lieferbeginn Nr. 7 — „Am selben Tag wie in \
                 Prozessschritt 5, wenn die Anmeldung bestätigt wurde\"",
    },
];

/// GPKE Strom — the notification the NB owes the **MSB** when it ends a
/// Zuordnung of its own accord.
///
/// A different Sequenzdiagramm from the three above: „Lieferende von NB an LF"
/// is opened by the NB (55007, Nr. 1), not by an inbound Anmeldung, so this runs
/// from [`MeldungAnchor::EigeneAnkuendigung`]. One PID, two Prozessschritte and
/// two recipients — Nr. 11 tells the **MSB** its Zuordnung ends (`ZC8`), Nr. 13
/// tells the **MSBZ** a future one is cancelled (`ZH1`) — on the same window.
///
/// It is the one message in the Zuordnungs-Meldung family that may name a
/// **Messlokation**: „Der MSB ist ausschließlich dem Objekt Messlokation
/// zugeordnet" (WiM Strom Teil 1 Kap. 2.1.2 d), so `SG5 LOC` carries `Z16` or
/// `Z17` where the other three carry `Z16` or `Z21`.
pub const GPKE_LIEFERENDE: &[Meldepflicht] = &[Meldepflicht {
    pid: 55_611,
    name: "Beendigung der Zuordnung des MSB zur MaLo / MeLo",
    sent_by: "NB",
    sent_to: "MSB / MSBZ",
    triggered_by: &[55_007],
    anchor: MeldungAnchor::EigeneAnkuendigung,
    frist: FristShape::WerktagAt {
        werktage: 1,
        at: at(7),
    },
    family: Family::Gpke,
    source: "BK6-24-174 GPKE Teil 2 § 2.5.2 SD Lieferende von NB an LF Nr. 11 und Nr. 13 — \
             \"Unverzüglich nach dem ÜZ von Nr. 2, sofern es sich um eine Zustimmung handelt, \
             bzw. nach dem ÜZ von Nr. 3, jedoch spätester ÜZ ist 07:00 Uhr des 1. WT nach dem \
             ÜT von Nr. 1\". Nr. 11 beendet die Zuordnung des MSB (STS+7++ZC8), Nr. 13 hebt \
             die des MSBZ auf (ZH1).",
}];

const TABLES: &[&[Meldepflicht]] = &[GPKE, GPKE_LIEFERENDE, GELI_GAS];

/// Every catalogued Meldepflicht, across all families.
pub fn all() -> impl Iterator<Item = &'static Meldepflicht> {
    TABLES.iter().copied().flatten()
}

/// The Meldepflicht carrying `pid`, if one is catalogued.
#[must_use]
pub fn meldepflicht(pid: u32) -> Option<&'static Meldepflicht> {
    all().find(|m| m.pid == pid)
}

/// Every Meldepflicht that an inbound `trigger_pid` puts on the receiver.
///
/// Returns the obligations in Prozessschritt order, which is also the order
/// their windows close.
pub fn meldepflichten_for(trigger_pid: u32) -> impl Iterator<Item = &'static Meldepflicht> {
    all().filter(move |m| m.triggered_by.contains(&trigger_pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::antwort;
    use time::{Date, Month, Time};

    fn utc(y: i32, m: Month, d: u8, h: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(y, m, d).expect("valid date"),
            Time::from_hms(h, 0, 0).expect("valid time"),
        )
    }

    /// A Meldepflicht is never also an Antwort: the two tables describe
    /// different obligations, and a PID in both would be counted twice by every
    /// consumer that walks them.
    #[test]
    fn no_meldepflicht_is_also_an_antwort_pid() {
        for m in all() {
            assert!(
                antwort::antwort_obligation(m.pid).is_none(),
                "{} ({}) is in both tables",
                m.pid,
                m.name
            );
            for &trigger in m.triggered_by {
                assert!(
                    antwort::antwort_obligation(trigger).is_some(),
                    "{} is triggered by {trigger}, which has no published Antwortfrist — \
                     the process it hangs off does not exist",
                    m.pid
                );
            }
        }
    }

    /// The Strom Information über existierende Zuordnung closes **before** the
    /// answer to the same Anmeldung — 07:00 against 11:00.
    #[test]
    fn the_existierende_zuordnung_closes_before_the_anmeldung_is_answered() {
        let received = utc(2026, Month::March, 2, 9); // Monday
        let info = meldepflicht(55_036).expect("catalogued");
        let info_due = info.due_at(received, HolidayCalendar::BdewMaKo);
        let antwort_due =
            antwort::antwort_deadline(55_001, received).expect("55001 has a published Frist");
        assert!(
            info_due < antwort_due,
            "55036 is due 07:00 and 55001's answer 11:00 on the same Werktag: {info_due} vs {antwort_due}"
        );
    }

    /// A Friday Anmeldung puts the Monday-morning obligations on Monday, not on
    /// the weekend.
    #[test]
    fn a_friday_anmeldung_is_notified_on_monday() {
        let friday = utc(2026, Month::March, 6, 14);
        for pid in [55_036_u32, 55_037, 55_038] {
            let due = meldepflicht(pid)
                .expect("catalogued")
                .due_at(friday, HolidayCalendar::BdewMaKo);
            assert_eq!(
                due.date(),
                Date::from_calendar_date(2026, Month::March, 9).expect("valid date"),
                "PID {pid}"
            );
        }
    }

    /// The Gas Beendigung and Aufhebung are anchored on the **Antwort**, and
    /// resolving them against the Eingang would give them a different day
    /// whenever the GNB uses more than a few hours of its four Werktage.
    #[test]
    fn the_gas_beendigung_is_anchored_on_the_answer_not_the_arrival() {
        for pid in [44_037_u32, 44_038] {
            let m = meldepflicht(pid).expect("catalogued");
            assert_eq!(m.anchor, MeldungAnchor::Antwort, "PID {pid}");
        }
        let m = meldepflicht(44_036).expect("catalogued");
        assert_eq!(m.anchor, MeldungAnchor::Eingang);

        // Anmeldung Monday, answered on the 3rd Werktag: „am selben Tag" is
        // Wednesday, not Monday.
        let antwort_sent = utc(2026, Month::March, 4, 15);
        let due = meldepflicht(44_037)
            .expect("catalogued")
            .due_at(antwort_sent, HolidayCalendar::BdewMaKo);
        assert_eq!(
            due.date(),
            Date::from_calendar_date(2026, Month::March, 4).expect("valid date")
        );
    }

    /// Each Sparte's three notifications address three different parties —
    /// LFN, LFA and LFZ.
    #[test]
    fn each_family_notifies_three_distinct_parties() {
        for table in [GPKE, GELI_GAS] {
            let mut to: Vec<&str> = table.iter().map(|m| m.sent_to).collect();
            to.sort_unstable();
            to.dedup();
            assert_eq!(to.len(), 3, "expected LFN, LFA and LFZ, got {to:?}");
        }
    }

    /// Every entry carries a Fundstelle naming a document and a Prozessschritt.
    #[test]
    fn every_entry_cites_a_prozessschritt() {
        for m in all() {
            assert!(
                m.source.contains("Nr.") && m.source.len() > 40,
                "{} has no Prozessschritt citation: {}",
                m.pid,
                m.source
            );
        }
    }
}
