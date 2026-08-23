//! The **business answer Frist** per inbound Prüfidentifikator.
//!
//! Four services need this number and must agree on it: `makod` registers the
//! deadline on the process, `processd` sizes its operator queue by it, `obsd`
//! raises the breach alert and reports the KPI, and `agentd`'s deadline
//! specialist classifies what `obsd` returns. One table, so they cannot disagree.
//!
//! Two plausible misreadings are recorded as constants because both are easy to
//! re-introduce: [`GPKE_IS_NOT_TWENTY_FOUR_HOURS`] and
//! [`TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST`].
//!
//! ## The rule
//!
//! [`antwortfrist`] returns `None` for a PID whose window the Festlegungen do
//! not quantify. That is **unknown** — never *unbounded*, never *no deadline*.
//! A caller that needs an instant regardless asks [`operator_window`], which
//! says out loud (`is_regulatory: false`) that its answer is an operating
//! convention rather than a citation.
//!
//! ## Why the tables live here rather than in the domain crates
//!
//! They are data: a PID, a window shape and a Fundstelle. Beside the workflows
//! they would sit above `mako-engine`, so no crate could hold all four families
//! and each service would aggregate them itself. What *does* belong beside a
//! workflow is the cross-check that the table agrees with it — every trigger is
//! a PID the workflow spawns from, and the answer PIDs match its own derivation.
//! Those tests live in `mako-gpke` and `mako-geli-gas`.
//!
//! # Sources
//!
//! - BK6-24-174 GPKE Teil 2 — the SD Fristen per Prozessschritt
//! - BK7-24-01-009 GeLi Gas 3.0, Kap. 2.6 / 3.1 / 3.2.2 / 3.2.3 / 3.3.2
//! - BK6-22-024 Anlage 2a — WiM Strom Teil 1, Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2
//! - BK7-24-01-009 / AWH WiM Gas V2.0
//! - EDI@Energy Anwendungsübersicht der Prüfidentifikatoren 4.0 — roles, EBDs

use time::{Duration, OffsetDateTime, Time};
use time_tz::{OffsetDateTimeExt, timezones};

use crate::HolidayCalendar;

/// Why a flat 24-hour GPKE window is wrong, kept where the mistake was made.
///
/// GPKE Teil 2 states every answer window as a wall-clock instant in German
/// local time on the first Werktag *after* the Übertragungstag — 11:00 for a
/// Lieferbeginn, 06:00 for an Abmeldung, 05:00 for the NB-seitiges Lieferende,
/// 09:00 for the Anfrage zur Beendigung der Zuordnung. It is not a duration.
///
/// A message arriving Friday afternoon is answerable until Monday morning; one
/// arriving Tuesday evening has under sixteen hours. A flat 24 h is therefore
/// both too tight and too loose, and the loose direction is the silent one: it
/// reports a lapsed Frist as still running, while the tight direction raises a
/// breach against a counterparty still inside its window.
pub const GPKE_IS_NOT_TWENTY_FOUR_HOURS: &str = "GPKE Strom answer windows are clock times on the 1. Werktag after the ÜT \
     (11:00 / 06:00 / 05:00 / 09:00), never a 24-hour duration — GPKE Teil 2";

/// Why a flat 10-Werktage GeLi Gas window is wrong.
///
/// The familiar „10 Werktage" is the **supplier's** minimum lead time
/// („mindestens 10 Werktage vor Aufnahme der Belieferung", GeLi Gas 3.0
/// Kap. 3.2.3) — how far ahead the LF must send. The Netzbetreiber's *answer*
/// window for the same message is 4 Werktage; the Abmeldung pairs a 7-Werktage
/// lead time with a 3-Werktage answer window.
pub const TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST: &str = "GeLi Gas 10 Werktage is the LF's Vorlauffrist, not the NB's Antwortfrist \
     (4 / 3 / 2 / 3 Werktage) — BK7-24-01-009 Kap. 3.1–3.3";

/// Which Festlegung family a window comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    /// GPKE Strom — BK6-24-174 Teil 2.
    Gpke,
    /// GeLi Gas — BK7-24-01-009.
    GeliGas,
    /// WiM Strom (Messstellenbetrieb) — BK6-22-024 Anlage 2a/2b.
    Wim,
    /// WiM Gas (Messstellenbetrieb Gas) — BK7-24-01-009 / AWH WiM Gas V2.0.
    WimGas,
}

impl Family {
    /// The wire spelling every consumer groups by.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gpke => "gpke",
            Self::GeliGas => "geli-gas",
            Self::Wim => "wim",
            Self::WimGas => "wim-gas",
        }
    }
}

/// The shape of an answer Frist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FristShape {
    /// „Unverzüglich, jedoch spätester ÜZ ist `HH:MM` Uhr des `werktage`. WT
    /// nach dem ÜT."
    ///
    /// A wall-clock instant in German local time on the `werktage`-th Werktag
    /// strictly after the arrival day. `werktage = 1` is the ordinary GPKE
    /// Teil 2 shape (11:00 / 06:00 / 05:00 / 09:00); the Neuanlage answer is
    /// the same shape at `00:00 Uhr des 61. WT`.
    WerktagAt {
        /// How many Werktage after the ÜT the deadline falls on.
        werktage: u32,
        /// The wall-clock time on that Werktag, in German local time.
        at: Time,
    },
    /// „…bis zum **Ablauf** des `n`. Werktags nach Eingang."
    ///
    /// Day-granular: the Frist runs to the end of that Werktag. The arrival day
    /// does not count (§ 187 Abs. 1 BGB).
    EndOfWerktag(u32),
    /// „…spätester ÜT ist der `n`. WT nach dem ÜT", resolved to the 17:00
    /// Europe/Berlin MaKo cut-off on that Werktag.
    WerktageAtCutoff(u32),
    /// „Spätester ÜZ ist `HH:MM` Uhr **am ÜT**" — a wall-clock instant on the
    /// arrival day itself, not on a Werktag after it.
    ///
    /// GPKE Teil 2 uses it where the answer has to be back before the NB's own
    /// same-day cut-off; the Beginn der Ersatz-/Grundversorgung is the case
    /// that matters to a supplier.
    SameDayAt(Time),
}

impl FristShape {
    /// Resolve this Frist against the instant the message arrived.
    #[must_use]
    pub fn due_at(self, received: OffsetDateTime, cal: HolidayCalendar) -> OffsetDateTime {
        match self {
            Self::WerktagAt { werktage, at } => crate::nth_werktag_at(received, werktage, at, cal),
            Self::EndOfWerktag(n) => crate::end_of_werktag_after(received, n, cal),
            Self::WerktageAtCutoff(n) => crate::deadline_at_werktage(received, n, cal),
            Self::SameDayAt(at) => {
                let berlin = received.to_timezone(timezones::db::europe::BERLIN);
                crate::berlin_at(berlin.date(), at)
            }
        }
    }
}

/// One inbound PID's answer obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AntwortObligation {
    /// The **inbound** Prüfidentifikator that starts the clock. Never an answer
    /// PID — a process is only ever spawned from an inbound message, and an
    /// answer discharges a Frist rather than starting one.
    pub trigger_pid: u32,
    /// Human-readable process name, for operator-facing queue reasons and logs.
    pub name: &'static str,
    /// The Marktrolle that owes the answer, as the Anwendungsübersicht names it.
    pub answered_by: &'static str,
    /// Outbound PIDs carrying the positive and negative answer.
    pub antwort_pids: (u32, u32),
    /// The Entscheidungsbaum that decides the answer, where one is published.
    pub ebd: Option<&'static str>,
    /// The window shape.
    pub frist: FristShape,
    /// Which Festlegung family states it.
    pub family: Family,
    /// Citation, for the audit trail.
    pub source: &'static str,
}

/// The MSB's window to answer an Anfrage einer Konfiguration with a QUOTES
/// Angebot or an IFTSTA Ablehnung, in Werktage (GPKE Teil 3 SD Prozessschritt 2).
///
/// **Not a WiM Preisanfrage window.** REQOTE 35004 opens the GPKE Teil 3
/// Konfigurationsprozess, where the MSB answers in two Werktagen; folding it
/// into the WiM family gave it five and hid three days of breach.
pub const KONFIGURATIONSANGEBOT_WERKTAGE: u32 = 2;

/// The Neuanlage answer window, in Werktage after the ÜT.
///
/// `E_0608` Prüfschritte 110 / 590 give the NB **60 Werktage** of daily
/// re-identification before it may refuse a newly commissioned Marktlokation,
/// and GPKE Teil 2 § 2.2.2 states the answer window as 00:00 Uhr des 61. WT.
pub const NEUANLAGE_WERKTAGE: u32 = 61;

/// Bearbeitungsstand / Rückmeldung on Abrechnungsdaten, in Werktage
/// (GPKE Teil 2 §§ 3.1.1.2 / 3.1.2.2 / 3.1.3.2).
pub const ABRECHNUNGSDATEN_WERKTAGE: u32 = 2;

/// Rückmeldung auf eine Stammdatenänderung, in Werktage
/// (GPKE Teil 4 §§ 1.4.2–1.4.5 Prozessschritte 2 / 5).
///
/// The **Bestellung** einer Stammdatenänderung is a different window — GPKE
/// Teil 4 § 1.5.2 gives the NB 10 Werktage for the Bearbeitungsstand — and the
/// two must not be conflated.
pub const STAMMDATEN_RUECKMELDUNG_WERKTAGE: u32 = 2;

/// Bearbeitungsstand zur **Bestellung** einer Stammdatenänderung, in Werktage
/// (GPKE Teil 4 § 1.5.2 Prozessschritte 2 / 4 / 6).
pub const STAMMDATEN_BESTELLUNG_WERKTAGE: u32 = 10;

/// Antwort auf eine **Gas** Stammdatenänderung, in Werktage.
///
/// „Unverzüglich, spätestens jedoch bis zum Ablauf des 10. WT nach Eingang der
/// Änderung" — AWH GeLi Gas § 4.3.2 Nr. 2 / Nr. 4, and likewise for the LF- and
/// MSB-initiated directions. Five times the Strom window
/// ([`STAMMDATEN_RUECKMELDUNG_WERKTAGE`]), and genuinely so: GeLi Gas gives the
/// Berechtigter a real Zustimmung/Ablehnung rather than Strom's asynchronous
/// quality feedback.
pub const STAMMDATEN_ANTWORT_WERKTAGE_GAS: u32 = 10;

const fn at(hour: u8) -> Time {
    match Time::from_hms(hour, 0, 0) {
        Ok(t) => t,
        Err(_) => panic!("whole hour is a valid Time"),
    }
}

/// GPKE Strom — every inbound PID whose answer Frist Teil 2 states.
///
/// | PID | Process | Answerer | Frist |
/// |---|---|---|---|
/// | 55001 | Anmeldung verb. MaLo | NB | 11:00 des 1. WT nach dem ÜT |
/// | 55077 | Anmeldung erz. MaLo | NB | 11:00 des 1. WT nach dem ÜT |
/// | 55004 | Abmeldung (Lieferende von LF an NB) | NB | 06:00 des 1. WT nach dem ÜT |
/// | 55007 | Ankündigung der Beendigung der Zuordnung | LF | 05:00 des 1. WT nach dem ÜT |
/// | 55010 | Anfrage zur Beendigung der Zuordnung | LFA | 09:00 des 1. WT nach dem ÜT |
/// | 55016 | Kündigung | LFA | Ablauf des 1. WT nach dem ÜT |
/// | 55607 | Ankündigung Zuordnung LF (erz. MaLo / Tranche) | LFN | 15:00 Uhr am ÜT |
/// | 55600 | Anmeldung neuer verb. MaLo (Neuanlage) | NB | 00:00 des 61. WT nach dem ÜT |
/// | 55601 | Anmeldung neuer erz. MaLo (Neuanlage) | NB | 00:00 des 61. WT nach dem ÜT |
/// | 17115 | Sperrauftrag | NB | spätester ÜT ist der 1. WT nach dem ÜT |
/// | 17117 | Entsperrauftrag | NB | spätester ÜT ist der 1. WT nach dem ÜT |
/// | 39000 | Stornierung Sperr-/Entsperrauftrag | NB | spätester ÜT ist der 1. WT nach dem ÜT |
/// | 17116 | Anfrage Sperrung (NB → MSB) | MSB | spätester ÜT ist der 3. WT nach dem ÜT |
/// | 55156 | Rückmeldung/Bestellung Abr.-Daten BK-Abr. verb. MaLo | NB | 2. WT nach dem ÜT |
/// | 55673 | Rückmeldung/Bestellung Abr.-Daten BK-Abr. erz. MaLo | NB | 2. WT nach dem ÜT |
/// | 55220 | Rückmeldung/Bestellung Abr.-Daten NN-Abr. | NB | 2. WT nach dem ÜT |
/// | 55109 | Stammdatenänderung vom LF (Änderung der MaLo) | NB | 2. WT nach dem ÜT |
/// | 55230 | Änderung Blindabr.-Daten der NeLo (LF → NB) | NB | 2. WT nach dem ÜT |
/// | 55693 | Änderung Daten der TR (LF → NB) | NB | 2. WT nach dem ÜT |
/// | 55557 | Änderung MSB-Abrechnungsdaten der MaLo (MSB → NB) | NB | 2. WT nach dem ÜT |
/// | 55639–55643 | Stammdatenänderung vom MSB (NeLo/MaLo/SR/Tranche/MeLo) | NB | 2. WT nach dem ÜT |
///
/// **The Neuanlage window is not a mistake.** `E_0608` Prüfschritte 110 and 590
/// make the identification of a newly commissioned Marktlokation a *daily*
/// re-check for up to 60 Werktage before the NB may answer `A07` / `A16`, so
/// GPKE Teil 2 § 2.2.2 states the answer window as „spätester ÜZ ist 00:00 Uhr
/// des 61. WT nach dem ÜT". A 24-hour deadline on that process manufactures a
/// rejection roughly three months early.
pub const GPKE: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 55_001,
        name: "Anmeldung verb. Marktlokation (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (55_002, 55_003),
        ebd: Some("E_0622"),
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(11),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
    },
    AntwortObligation {
        trigger_pid: 55_077,
        name: "Anmeldung erz. Marktlokation (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (55_078, 55_080),
        ebd: Some("E_0622"),
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(11),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
    },
    AntwortObligation {
        trigger_pid: 55_004,
        name: "Abmeldung (Lieferende von LF an NB)",
        answered_by: "NB",
        antwort_pids: (55_005, 55_006),
        ebd: Some("E_0607"),
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(6),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferende von LF an NB Prozessschritte 2/3",
    },
    AntwortObligation {
        trigger_pid: 55_007,
        name: "Ankündigung der Beendigung der Zuordnung (Lieferende von NB an LF)",
        answered_by: "LF",
        antwort_pids: (55_008, 55_009),
        ebd: Some("E_0609"),
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(5),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferende von NB an LF Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_010,
        name: "Anfrage zur Beendigung der Zuordnung",
        answered_by: "LFA",
        antwort_pids: (55_011, 55_012),
        ebd: Some("E_0624"),
        frist: FristShape::WerktagAt {
            werktage: 1,
            at: at(9),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritt 4",
    },
    AntwortObligation {
        trigger_pid: 55_013,
        name: "Zuordnung Ersatz-/Grundversorgung",
        answered_by: "E/G",
        antwort_pids: (55_014, 55_015),
        ebd: Some("E_0615"),
        // GPKE Teil 2 states two windows for this step, selected by a date in
        // the payload: „spätester ÜZ ist 15:00 Uhr **am ÜT**" when the
        // Zuordnungsbeginn is in the future, and 15:00 Uhr des 1. WT nach dem
        // ÜT when it is not. Only the tighter one is safe to publish from a
        // table keyed on the PID alone — a queue sized by the looser window
        // reports a lapsed Frist as still running, which is the failure mode
        // this module exists to prevent. The Gas twin (44013) is a plain
        // 2-Werktage window and is listed separately.
        frist: FristShape::SameDayAt(at(15)),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 2.3.2.2 SD „Beginn der Ersatz-/Grundversorgung\" \
                 Nr. 2 Fall I — „Unverzüglich, jedoch spätester ÜZ ist 15:00 Uhr am ÜT von \
                 Nr. 1\" (Fall II, Zuordnungsbeginn nicht in der Zukunft: 15:00 Uhr des \
                 1. WT nach dem ÜT)",
    },
    AntwortObligation {
        trigger_pid: 55_607,
        name: "Ankündigung der Zuordnung des LF zur Marktlokation bzw. Tranche",
        answered_by: "LFN",
        antwort_pids: (55_608, 55_609),
        // Four Anwendungsfälle, four EBDs (`E_0603`–`E_0606`), one window. The
        // table names the first; `mako_pruefung::codes::EBD_ZUORDNUNG_LF` holds
        // all four, and the inbound message names the one it wants answered.
        ebd: Some("E_0603"),
        // Same two-window shape as the Ersatz-/Grundversorgung above, and the
        // same reason to publish the tighter one: keyed on the PID alone this
        // table cannot see whether the Zuordnungsbeginn is in the future.
        //
        // Missing it is not a lapsed obligation but a **balancing-circle
        // assignment**: Prozessschritt 3 has the NB assign the LFN to the
        // Marktlokation „aufgrund fehlender Antwort" anyway, using whatever BK
        // the LFN once communicated. Silence is consent here.
        frist: FristShape::SameDayAt(at(15)),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 2.4.2.2–2.4.2.5 SD „Herstellung einer 100% \
                 LF-Zuordnung zu einer erzeugenden Marktlokation\" Nr. 2 — „Unverzüglich, \
                 jedoch spätester ÜZ ist 15:00 Uhr am ÜT von Nr. 1\" (Fall II, \
                 Zuordnungsbeginn nicht in der Zukunft: 15:00 Uhr des 1. WT nach dem ÜT); \
                 Nr. 3 ordnet den LFN bei fehlender Antwort ohne weitere Rückfrage zu",
    },
    AntwortObligation {
        trigger_pid: 55_016,
        name: "Kündigung",
        answered_by: "LFA",
        antwort_pids: (55_017, 55_018),
        ebd: Some("E_0614"),
        frist: FristShape::EndOfWerktag(1),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Kündigung Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_600,
        name: "Anmeldung neuer verbrauchender Marktlokation (Neuanlage)",
        answered_by: "NB",
        antwort_pids: (55_602, 55_604),
        ebd: Some("E_0608"),
        frist: FristShape::WerktagAt {
            werktage: NEUANLAGE_WERKTAGE,
            at: at(0),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 2.2.2, SD Neuanlage Prozessschritte 2/3 — \
                 „spätester ÜZ ist 00:00 Uhr des 61. WT nach dem ÜT von Nr. 1\"",
    },
    AntwortObligation {
        trigger_pid: 55_601,
        name: "Anmeldung neuer erzeugender Marktlokation (Neuanlage)",
        answered_by: "NB",
        antwort_pids: (55_603, 55_605),
        ebd: Some("E_0608"),
        frist: FristShape::WerktagAt {
            werktage: NEUANLAGE_WERKTAGE,
            at: at(0),
        },
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 2.2.2, SD Neuanlage Prozessschritte 2/3",
    },
    AntwortObligation {
        trigger_pid: 17_115,
        name: "Sperrauftrag (Unterbrechung der Anschlussnutzung)",
        answered_by: "NB",
        antwort_pids: (19_116, 19_117),
        ebd: Some("E_0470"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 3.5.1.2 Prozessschritt 2 — „spätester ÜT ist der \
                 1. WT nach dem ÜT von Nr. 1\"",
    },
    AntwortObligation {
        trigger_pid: 17_117,
        name: "Entsperrauftrag (Wiederherstellung der Anschlussnutzung)",
        answered_by: "NB",
        antwort_pids: (19_116, 19_117),
        ebd: Some("E_0497"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 3.5.2.2 Prozessschritt 2 — „spätester ÜT ist der \
                 1. WT nach dem ÜT von Nr. 1\"",
    },
    AntwortObligation {
        trigger_pid: 39_000,
        name: "Stornierung eines Sperr-/Entsperrauftrags",
        answered_by: "NB",
        antwort_pids: (19_128, 19_129),
        ebd: Some("E_0468"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 3.5.3.2 Prozessschritt 2 — „spätester ÜT ist der \
                 1. WT nach dem ÜT\"",
    },
    AntwortObligation {
        trigger_pid: 17_116,
        name: "Anfrage Sperrung an den MSB",
        answered_by: "MSB",
        antwort_pids: (19_118, 19_119),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(3),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 § 3.5.1.2 Prozessschritt 4 — „spätester ÜT ist der \
                 3. WT nach dem ÜT von Nr. 3\"; Fristverstreichen gilt als Zustimmung",
    },
    AntwortObligation {
        trigger_pid: 55_156,
        name: "Rückmeldung / Bestellung Abrechnungsdaten Bilanzkreisabrechnung (verb. MaLo)",
        answered_by: "NB",
        antwort_pids: (21_047, 21_047),
        ebd: Some("E_0595"),
        frist: FristShape::WerktageAtCutoff(ABRECHNUNGSDATEN_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 §§ 3.1.2.2 / 3.1.3.2 — Bearbeitungsstand \
                 „spätester ÜT ist der 2. WT nach dem ÜT\"",
    },
    AntwortObligation {
        trigger_pid: 55_673,
        name: "Rückmeldung / Bestellung Abrechnungsdaten Bilanzkreisabrechnung (erz. MaLo)",
        answered_by: "NB",
        antwort_pids: (21_047, 21_047),
        ebd: Some("E_0595"),
        frist: FristShape::WerktageAtCutoff(ABRECHNUNGSDATEN_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 §§ 3.1.2.2 / 3.1.3.2",
    },
    AntwortObligation {
        trigger_pid: 55_220,
        name: "Rückmeldung / Bestellung Abrechnungsdaten Netznutzungsabrechnung",
        answered_by: "NB",
        antwort_pids: (21_047, 21_047),
        ebd: Some("E_0595"),
        frist: FristShape::WerktageAtCutoff(ABRECHNUNGSDATEN_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2 §§ 3.1.1.2 / 3.1.3.2",
    },
    AntwortObligation {
        trigger_pid: 55_109,
        name: "Stammdatenänderung vom LF — Daten der Marktlokation",
        answered_by: "NB",
        antwort_pids: (55_137, 55_137),
        ebd: Some("E_0410"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.3 Prozessschritt 2 — „spätester ÜT ist der \
                 2. WT nach dem ÜT von Nr. 1\"",
    },
    AntwortObligation {
        trigger_pid: 55_230,
        name: "Stammdatenänderung vom LF — Blindarbeits-Abrechnungsdaten der Netzlokation",
        answered_by: "NB",
        antwort_pids: (55_232, 55_232),
        ebd: Some("E_0410"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.3 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_693,
        name: "Stammdatenänderung vom LF — Daten der Technischen Ressource",
        answered_by: "NB",
        antwort_pids: (55_694, 55_694),
        ebd: Some("E_0410"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.3 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_557,
        name: "Stammdatenänderung vom MSB — MSB-Abrechnungsdaten der Marktlokation",
        answered_by: "NB",
        antwort_pids: (55_559, 55_559),
        ebd: Some("E_0415"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.4 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_639,
        name: "Stammdatenänderung vom MSB — Daten der Netzlokation",
        answered_by: "NB",
        antwort_pids: (55_644, 55_644),
        ebd: Some("E_0415"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.4 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_640,
        name: "Stammdatenänderung vom MSB — Daten der Marktlokation",
        answered_by: "NB",
        antwort_pids: (55_645, 55_645),
        ebd: Some("E_0415"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.4 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_641,
        name: "Stammdatenänderung vom MSB — Daten der Steuerbaren Ressource",
        answered_by: "NB",
        antwort_pids: (55_646, 55_646),
        ebd: Some("E_0415"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.4 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_642,
        name: "Stammdatenänderung vom MSB — Daten der Tranche",
        answered_by: "NB",
        antwort_pids: (55_647, 55_647),
        ebd: Some("E_0415"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.4 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_643,
        name: "Stammdatenänderung vom MSB — Daten der Messlokation",
        answered_by: "NB",
        antwort_pids: (55_648, 55_648),
        ebd: Some("E_0415"),
        frist: FristShape::WerktageAtCutoff(STAMMDATEN_RUECKMELDUNG_WERKTAGE),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 4 § 1.4.4 Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 35_004,
        name: "Anfrage einer Konfiguration (REQOTE)",
        answered_by: "MSB",
        antwort_pids: (15_004, 21_033),
        // Two trees on one PID: `E_0524` when the NB asked, `E_0531` when the
        // LF did. The PID cannot pick between them, so none is named here.
        ebd: None,
        frist: FristShape::WerktageAtCutoff(KONFIGURATIONSANGEBOT_WERKTAGE),
        family: Family::Gpke,
        source: "GPKE Teil 3 § 3.1 / § 3.2 SD Prozessschritt 2 — 2 Werktage",
    },
];

/// GeLi Gas — every inbound PID whose answer Frist the Festlegung quantifies.
///
/// | PID | Process | Answerer | Frist |
/// |---|---|---|---|
/// | PID | Process | Answerer | Frist |
/// |---|---|---|---|
/// | 44001 | Anmeldung NN (Lieferbeginn) | NB | Ablauf des 4. WT |
/// | 44004 | Abmeldung NN (Lieferende) | NB | Ablauf des 3. WT |
/// | 44007 | Abmeldung NN vom NB (Lieferende von NB an LF) | LF | Ablauf des 3. WT |
/// | 44010 | Abmeldeanfrage des NB | LFA | Ablauf des 3. WT |
/// | 44013 | Zuordnung Ersatz-/Grundversorgung | E/G | Ablauf des 2. WT |
/// | 44016 | Kündigung beim Altlieferanten | LFA | Ablauf des 3. WT |
///
/// The Änderungsmeldung zur Bestandsliste (44020) has no quantified Frist — it
/// is set per Netzbetreiber under Kap. 2.6 — so it is absent here: *unknown*,
/// never *unbounded*.
pub const GELI_GAS: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 44_001,
        name: "Anmeldung NN (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (44_002, 44_003),
        ebd: Some("E_3005"),
        frist: FristShape::EndOfWerktag(4),
        family: Family::GeliGas,
        source: "GeLi Gas 3.0 Kap. 3.2.3 — „spätestens bis zum Ablauf des 4. Werktages nach \
                 Eingang der Anmeldung\"",
    },
    AntwortObligation {
        trigger_pid: 44_004,
        name: "Abmeldung NN (Lieferende)",
        answered_by: "NB",
        antwort_pids: (44_005, 44_006),
        ebd: Some("E_3019"),
        frist: FristShape::EndOfWerktag(3),
        family: Family::GeliGas,
        source: "GeLi Gas 3.0 Kap. 3.2.2 — „spätestens jedoch bis zum Ablauf des 3. Werktags \
                 nach Eingang der Abmeldung\"",
    },
    AntwortObligation {
        trigger_pid: 44_007,
        name: "Abmeldung NN vom NB (Lieferende von NB an LF)",
        answered_by: "LF",
        antwort_pids: (44_008, 44_009),
        ebd: Some("E_3002"),
        frist: FristShape::EndOfWerktag(3),
        family: Family::GeliGas,
        source: "AWH GeLi Gas 2.0 Kap. 2.3.2 SD „Lieferende von NB an LF\" Nr. 2 — \
                 „Unverzüglich, jedoch spätestens bis zum Ablauf des 3. WT nach Eingang \
                 der Abmeldung\"",
    },
    AntwortObligation {
        trigger_pid: 44_010,
        name: "Abmeldeanfrage des NB",
        answered_by: "LFA",
        antwort_pids: (44_011, 44_012),
        ebd: Some("E_3020"),
        frist: FristShape::EndOfWerktag(3),
        family: Family::GeliGas,
        source: "AWH GeLi Gas 2.0 Kap. 2.5.2 SD „Lieferbeginn\" Nr. 4 — „Beantwortung der \
                 Abmeldeanfrage: Unverzüglich, jedoch Ablauf des 3. WT\"",
    },
    AntwortObligation {
        trigger_pid: 44_013,
        name: "Zuordnung Ersatz-/Grundversorgung",
        answered_by: "E/G",
        antwort_pids: (44_014, 44_015),
        ebd: Some("E_3008"),
        frist: FristShape::EndOfWerktag(2),
        family: Family::GeliGas,
        source: "GeLi Gas 3.0 Kap. 3.3.2 — „spätestens bis zum Ablauf des 2. Werktages\"",
    },
    AntwortObligation {
        trigger_pid: 44_016,
        name: "Kündigung beim Altlieferanten",
        answered_by: "LFA",
        antwort_pids: (44_017, 44_018),
        ebd: Some("E_3001"),
        frist: FristShape::EndOfWerktag(3),
        family: Family::GeliGas,
        source: "GeLi Gas 3.0 Kap. 3.1 — „spätestens jedoch bis zum Ablauf des 3. Werktages \
                 nach Eingang der Kündigung\"",
    },
];

/// WiM Strom — every inbound Prüfidentifikator whose answer window Teil 1
/// states.
///
/// | Trigger | Process | Answerer | Frist | EBD |
/// |---|---|---|---|---|
/// | 55039 | Kündigung MSB | MSBA | 3 WT | `E_0200` |
/// | 55042 | Anmeldung MSB | NB | 5 WT | `E_0201` |
/// | 55051 | Abmeldung (Ende MSB) | NB | 7 WT | `E_0202` |
/// | 55168 | Verpflichtungsanfrage | gMSB | 1 WT | `E_0240` |
/// | 17002 | Weiterverpflichtung des MSB | MSBA | 1 WT | `E_0203` |
/// | 21010 / 21009 | Mitteilung über Gesamtvorgang (erfolgreich / gescheitert) | NB | 1 WT | `E_0232` |
/// | 35001 | Anforderung Geräteübernahmeangebot | MSBA | **4 WT** | — |
/// | 17001 | Bestellung Geräteübernahmeangebot | MSBA | 2 WT | `E_0247` |
/// | 17009 | Anzeige Gerätewechselabsicht | MSBA | *2 WT vor dem Termin* | `E_0204` |
/// | 35002 | Anfrage Rechnungsabwicklung über LF | MSB | 5 WT | — |
/// | 15002 | Angebot Rechnungsabwicklung über LF | LF | 8 WT | `E_0205`/`E_0208` |
/// | 17006 | Beendigung Rechnungsabwicklung über LF | Gegenseite | 8 WT | `E_0206`/`E_0209` |
/// | 35005 | Anfrage Angebot Änderung Technik | MSB | 10 WT | — |
/// | 17011 | Beauftragung Änderung Technik | MSB | 10 WT | `E_0249`/`E_0250` ¹ |
///
/// **The MSB-Wechsel windows are not one flat number** — 3 / 5 / 7 / 1
/// Werktage („Unverzüglich, jedoch spätester ÜT ist der *n*. WT nach dem ÜT von
/// Nr. 1"). A flat window fires early for the Kündigung and late for the
/// Abmeldung.
///
/// **Nor is the REQOTE family one window.** The four REQOTE PIDs open four
/// different Use-Cases and only 35002 is answered in 5 Werktage: 35001 is the
/// Anforderung eines Geräteübernahmeangebots (Kap. 3.2.2 Nr. 2 — **4 WT**) and
/// 35005 opens the Messlokationsänderung (Kap. 3.3 — **10 WT**). Answering all
/// four in 5 gives the Geräteübernahme a day it does not have and lets a
/// Technikänderung run five Werktage past its window unflagged.
///
/// 17009 is in the table for completeness but its Frist is a
/// [`Vorlauffrist`](crate::vorlauf) in disguise: „spätester ÜT ist der 2. WT
/// **vor dem Gerätewechseltermin**" (Kap. 3.1.2 Nr. 2) is anchored on a date
/// the *request* carries, not on the arrival instant, so
/// [`antwortfrist`] deliberately reports it as unknown rather than as a
/// forward window it is not. Size it with
/// [`vorlauf::vorlauf("wim.antwort-geraetewechselabsicht")`](crate::vorlauf::vorlauf).
///
/// ¹ A PID answered by two trees names neither: `ebd` is `None` for 17011 and
/// 35004, and the caller picks the tree from the sender's Marktrolle.
///
/// **23001 has no row.** WiM Teil 2 Kap. 1.2 Nr. 2 states two windows for the
/// Störungsmeldung — 3 Werktage for a kME ohne RLM or an mME, 1 for a kME mit
/// RLM or an iMS — and the message does not say which applies. A PID-keyed
/// lookup would have to pick one; [`stoerungsmeldung_werktage`] takes the
/// Messtechnik instead.
///
/// 35003 is the ESA „Anfrage von Werten", **not** a Preisanfrage: it carries
/// its own 5-Werktage window here, and must never be routed into the
/// Preisanfrage auto-quote path — see [`ESA_WERTEANFRAGE_PID`].
pub const WIM: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 55_039,
        name: "Kündigung MSB",
        answered_by: "MSBA",
        antwort_pids: (55_040, 55_041),
        ebd: Some("E_0200"),
        frist: FristShape::WerktageAtCutoff(3),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 2.2.2 Nr. 2 — 3 Werktage",
    },
    AntwortObligation {
        trigger_pid: 55_042,
        name: "Anmeldung MSB",
        answered_by: "NB",
        antwort_pids: (55_043, 55_044),
        ebd: Some("E_0201"),
        frist: FristShape::WerktageAtCutoff(5),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 2 — 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 55_051,
        name: "Abmeldung MSB (Ende Messstellenbetrieb)",
        answered_by: "NB",
        antwort_pids: (55_052, 55_053),
        ebd: Some("E_0202"),
        frist: FristShape::WerktageAtCutoff(7),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 2.4.2 Nr. 2 — 7 Werktage",
    },
    AntwortObligation {
        trigger_pid: 55_168,
        name: "Verpflichtungsanfrage an den gMSB",
        answered_by: "gMSB",
        antwort_pids: (55_169, 55_170),
        ebd: Some("E_0240"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Wim,
        // Not Kap. 2.5: the Verpflichtungsanfrage is Prozessschritt 3 of the
        // *Ende Messstellenbetrieb*, and Kap. 2.5 („Verpflichtung gMSB") is the
        // separate Use-Case it can lead into.
        source: "WiM Strom Teil 1 Kap. 2.4.2 Nr. 4 — 1 Werktag",
    },
    AntwortObligation {
        trigger_pid: 17_002,
        name: "Weiterverpflichtung des MSB",
        answered_by: "MSBA",
        antwort_pids: (19_003, 19_004),
        ebd: Some("E_0203"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 2.4.2 Nr. 6 — 1 Werktag",
    },
    AntwortObligation {
        trigger_pid: 21_010,
        name: "Mitteilung über Gesamtvorgang (erfolgreich)",
        answered_by: "NB",
        antwort_pids: (21_012, 21_011),
        ebd: Some("E_0232"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 8 — 1 Werktag",
    },
    AntwortObligation {
        trigger_pid: 21_009,
        name: "Mitteilung über Gesamtvorgang (gescheitert)",
        answered_by: "NB",
        // A Scheitermeldung has no positive answer: `E_0232` publishes only
        // `Z66` „MSB-Scheitermeldung liegt vor", carried by 21011.
        antwort_pids: (21_011, 21_011),
        ebd: Some("E_0232"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 8 — 1 Werktag",
    },
    AntwortObligation {
        trigger_pid: 35_001,
        name: "Anforderung Geräteübernahmeangebot (REQOTE)",
        answered_by: "MSBA",
        antwort_pids: (15_001, 15_001),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(GERAETEUEBERNAHME_ANGEBOT_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 3.2.2 Nr. 2 — 4 Werktage",
    },
    AntwortObligation {
        trigger_pid: 17_001,
        name: "Bestellung Geräteübernahmeangebot",
        answered_by: "MSBA",
        antwort_pids: (19_001, 19_002),
        ebd: Some("E_0247"),
        frist: FristShape::WerktageAtCutoff(2),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 3.2.2 Nr. 4 — 2 Werktage",
    },
    AntwortObligation {
        trigger_pid: 35_002,
        name: "Anfrage Rechnungsabwicklung MSB über LF (REQOTE)",
        answered_by: "MSB",
        antwort_pids: (15_002, 21_033),
        ebd: Some("E_0207"),
        frist: FristShape::WerktageAtCutoff(RECHNUNGSABWICKLUNG_ANFRAGE_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 3.6.3.6.2 Nr. 2 — 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 15_002,
        name: "Angebot zur Rechnungsabwicklung des Messstellenbetriebes über den LF",
        answered_by: "LF",
        antwort_pids: (17_005, 21_032),
        ebd: Some("E_0205"),
        frist: FristShape::WerktageAtCutoff(RECHNUNGSABWICKLUNG_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 3.6.3.4.2 Nr. 2 — 8 Werktage",
    },
    AntwortObligation {
        trigger_pid: 17_006,
        name: "Beendigung Rechnungsabwicklung des Messstellenbetriebes über den LF",
        answered_by: "Gegenseite (LF oder MSB)",
        antwort_pids: (19_009, 19_010),
        ebd: Some("E_0206"),
        frist: FristShape::WerktageAtCutoff(RECHNUNGSABWICKLUNG_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 3.6.3.5.2 Nr. 2 — 8 Werktage",
    },
    AntwortObligation {
        trigger_pid: 17_132,
        name: "Geschäftsdatenanfrage (Anfrage Stammdaten Strom)",
        answered_by: "NB · MSB",
        antwort_pids: (17_133, 19_132),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(GESCHAEFTSDATENANFRAGE_WERKTAGE),
        family: Family::Gpke,
        source: "GPKE Teil 4 § 3.2 Nr. 2 — spätester ÜZ ist 1 WT nach dem ÜZ der Anfrage",
    },
    AntwortObligation {
        trigger_pid: 35_005,
        name: "Anfrage Angebot Änderung Technik (REQOTE)",
        answered_by: "MSB",
        antwort_pids: (15_005, 15_005),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(TECHNIKAENDERUNG_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 2 — 10 Werktage",
    },
    AntwortObligation {
        trigger_pid: 17_011,
        name: "Beauftragung Änderung der Technik an der Messlokation",
        answered_by: "MSB",
        antwort_pids: (19_005, 19_006),
        // Two trees on one PID: `E_0249` when the NB ordered the change,
        // `E_0250` when the LF did — the LF variant adds the Vollmacht
        // Prüfschritte `A03`/`A04`. The PID cannot pick between them, so none
        // is named here; `mako_pruefung::msb::technikaenderung_ebd` takes the
        // sender's Marktrolle instead.
        ebd: None,
        frist: FristShape::WerktageAtCutoff(TECHNIKAENDERUNG_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 2 — 10 Werktage",
    },
    // ── ESA Wertebestellung (WiM Strom Teil 2 Kap. 4) ───────────────────────
    //
    // All four are answered by the MSB serving the ESA. Only the Werteanfrage
    // has its own window (5 WT); the three order-level steps share the 2-WT
    // answer window of UC 4.1 Nr. 4 / Nr. 6 and UC 4.3 Nr. 2.
    AntwortObligation {
        trigger_pid: ESA_WERTEANFRAGE_PID,
        name: "Anfrage von Werten (ESA)",
        answered_by: "MSB",
        // One PID carries both the Angebot and the Ablehnung; they are told
        // apart by the Bindungsfrist (`DTM+273`), not by a second PID.
        antwort_pids: (15_003, 15_003),
        // `E_0253` „Angebot zur Anfrage prüfen" is published without a tree —
        // „derzeit ist für diese Entscheidung kein Entscheidungsbaum
        // notwendig, da keine Antwort gegeben wird" — so the Ablehnung carries
        // a free-text Begründung rather than an Antwortcode.
        ebd: None,
        frist: FristShape::WerktageAtCutoff(ESA_ANGEBOT_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 2 Kap. 4.1.2 Nr. 2 — 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 17_007,
        name: "Bestellung von Werten (ESA)",
        answered_by: "MSB",
        antwort_pids: (19_011, 19_012),
        ebd: Some("E_0256"),
        frist: FristShape::WerktageAtCutoff(ESA_ANTWORT_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 2 Kap. 4.1.2 Nr. 4 — 2 Werktage",
    },
    AntwortObligation {
        trigger_pid: 17_008,
        name: "Abbestellung von Werten (ESA)",
        answered_by: "MSB",
        // Same answer PIDs as the Bestellung; the `IMD+7081` on the answer is
        // what says which of the two it answers, and therefore which tree.
        antwort_pids: (19_011, 19_012),
        ebd: Some("E_0254"),
        frist: FristShape::WerktageAtCutoff(ESA_ANTWORT_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 2 Kap. 4.3.2 Nr. 2 — 2 Werktage",
    },
    AntwortObligation {
        trigger_pid: 39_002,
        name: "Stornierung der Bestellung von Werten (ESA)",
        answered_by: "MSB",
        antwort_pids: (19_013, 19_014),
        ebd: Some("E_0257"),
        frist: FristShape::WerktageAtCutoff(ESA_ANTWORT_WERKTAGE),
        family: Family::Wim,
        source: "WiM Strom Teil 2 Kap. 4.1.2 Nr. 6 — 2 Werktage",
    },
];

/// REQOTE „Anfrage von Werten" — the ESA Werteanfrage (WiM Teil 2 Kap. 4).
///
/// Named because it must never be routed into the Preisanfrage auto-quote
/// path: it asks for a Messprodukt from Codeliste der Konfigurationen Kap. 4.6,
/// not for a `PreisblattMessung`. `mako_wim::preisanfrage::REQOTE_PIDS` holds
/// the four that are Preisanfragen, and 35003 is not among them.
pub const ESA_WERTEANFRAGE_PID: u32 = 35_003;

/// Angebot window on an ESA Werteanfrage, in Werktage
/// (WiM Strom Teil 2 Kap. 4.1.2 Nr. 2).
pub const ESA_ANGEBOT_WERKTAGE: u32 = 5;

/// Answer window on an ESA Bestellung, Stornierung or Abbestellung, in
/// Werktage (WiM Strom Teil 2 Kap. 4.1.2 Nr. 4/6, Kap. 4.3.2 Nr. 2).
pub const ESA_ANTWORT_WERKTAGE: u32 = 2;

/// The Geräteübernahmeangebot window, in Werktage
/// (WiM Strom Teil 1 Kap. 3.2.2 Nr. 2).
pub const GERAETEUEBERNAHME_ANGEBOT_WERKTAGE: u32 = 4;

/// Answer window on an Anfrage zur Rechnungsabwicklung *durch den LF*, in
/// Werktage (WiM Strom Teil 1 Kap. 3.6.3.6.2 Nr. 2).
pub const RECHNUNGSABWICKLUNG_ANFRAGE_WERKTAGE: u32 = 5;

/// Answer window on an Angebot or a Beendigung of the Rechnungsabwicklung des
/// Messstellenbetriebes über den LF, in Werktage
/// (WiM Strom Teil 1 Kap. 3.6.3.4.2 / 3.6.3.5.2 Nr. 2).
pub const RECHNUNGSABWICKLUNG_WERKTAGE: u32 = 8;

/// Answer window on a Messlokationsänderung (Änderung der Technik), in
/// Werktage (WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 2).
pub const TECHNIKAENDERUNG_WERKTAGE: u32 = 10;

/// Answer window on a Geschäftsdatenanfrage, in Werktage
/// (GPKE Teil 4 § 3.2 Nr. 2/4).
///
/// One window for both legs: the Stammdaten anfrage an den NB and the Werte
/// anfrage an den MSB are stated identically.
pub const GESCHAEFTSDATENANFRAGE_WERKTAGE: u32 = 1;

/// Answer window on an INSRPT Störungsmeldung for a **kME ohne RLM or an mME**,
/// in Werktage (WiM Strom Teil 2 Kap. 1.2 Nr. 2).
pub const STOERUNGSMELDUNG_KME_WERKTAGE: u32 = 3;

/// Answer window on an INSRPT Störungsmeldung for a **kME mit RLM or an iMS**,
/// in Werktage (WiM Strom Teil 2 Kap. 1.2 Nr. 2).
pub const STOERUNGSMELDUNG_IMS_WERKTAGE: u32 = 1;

/// The INSRPT Störungsmeldung answer window for the Messtechnik at the
/// Messlokation.
///
/// WiM Strom Teil 2 Kap. 1.2 Nr. 2 states two numbers and the message does not
/// carry which applies — the MSB's own device registry decides it. That is why
/// 23001 has no row in [`WIM`]: a PID-keyed lookup would have to pick one, and
/// picking the longer one lets an iMS Störung run two Werktage past its window.
#[must_use]
pub const fn stoerungsmeldung_werktage(rlm_oder_ims: bool) -> u32 {
    if rlm_oder_ims {
        STOERUNGSMELDUNG_IMS_WERKTAGE
    } else {
        STOERUNGSMELDUNG_KME_WERKTAGE
    }
}

/// How a Messlokation is metered — the discriminator both INSRPT windows branch
/// on, and the only thing that decides them.
///
/// Neither the Störungsmeldung nor the Bestätigung carries it: WiM Strom Teil 2
/// states the Frist per Messtechnik and the message says nothing about the
/// device. The MSB's own registry is the source, which is why these are
/// functions of a caller-supplied classification and not a PID lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Messtechnik {
    /// kME ohne RLM, or an mME in Niederspannung — the slow branch.
    KmeOhneRlm,
    /// kME mit RLM, or an iMS bilanziert auf Basis von Viertelstundenwerten, in
    /// **Niederspannung**.
    RlmOderImsNs,
    /// kME mit RLM or iMS in **Mittel- oder Hochspannung** — the fast branch.
    RlmOderImsMsHs,
}

impl Messtechnik {
    /// Werktage the MSB has to answer an INSRPT Störungsmeldung (23001 →
    /// 23003/23004).
    ///
    /// WiM Strom Teil 2 Kap. 1.2 Nr. 2 splits only two ways here: everything
    /// with RLM or an iMS answers in 1 WT regardless of Spannungsebene.
    #[must_use]
    pub const fn stoerungsmeldung_werktage(self) -> u32 {
        match self {
            Self::KmeOhneRlm => STOERUNGSMELDUNG_KME_WERKTAGE,
            Self::RlmOderImsNs | Self::RlmOderImsMsHs => STOERUNGSMELDUNG_IMS_WERKTAGE,
        }
    }

    /// Werktage the MSB has to send the Ergebnisbericht (INSRPT 23008), counted
    /// from the ÜT of its own Bestätigung — **not** from the Störungsmeldung.
    ///
    /// WiM Strom Teil 2 Kap. 1.2 Nr. 7 splits three ways, and this is the one
    /// WiM window whose branch is the Spannungsebene:
    ///
    /// | Messtechnik | Frist |
    /// |---|---|
    /// | kME ohne RLM, mME (NS), iMS ohne ¼-h-Bilanzierung (NS) | **7 WT** |
    /// | kME mit RLM (NS), iMS mit ¼-h-Bilanzierung (NS) | **4 WT** |
    /// | kME mit RLM (MS/HS), iMS (MS/HS) | **2 WT** |
    #[must_use]
    pub const fn ergebnisbericht_werktage(self) -> u32 {
        match self {
            Self::KmeOhneRlm => ERGEBNISBERICHT_KME_WERKTAGE,
            Self::RlmOderImsNs => ERGEBNISBERICHT_RLM_NS_WERKTAGE,
            Self::RlmOderImsMsHs => ERGEBNISBERICHT_RLM_MSHS_WERKTAGE,
        }
    }
}

/// Ergebnisbericht window for a kME ohne RLM, an mME (NS) or an iMS ohne
/// ¼-h-Bilanzierung (NS), in Werktage (WiM Strom Teil 2 Kap. 1.2 Nr. 7).
pub const ERGEBNISBERICHT_KME_WERKTAGE: u32 = 7;

/// Ergebnisbericht window for a kME mit RLM or an iMS mit ¼-h-Bilanzierung, in
/// **Niederspannung**, in Werktage (WiM Strom Teil 2 Kap. 1.2 Nr. 7).
pub const ERGEBNISBERICHT_RLM_NS_WERKTAGE: u32 = 4;

/// Ergebnisbericht window for a kME mit RLM or an iMS in **Mittel- oder
/// Hochspannung**, in Werktage (WiM Strom Teil 2 Kap. 1.2 Nr. 7).
pub const ERGEBNISBERICHT_RLM_MSHS_WERKTAGE: u32 = 2;

/// Werktage the MSB has to send the Gas Ergebnisbericht, flat
/// (AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 4).
pub const ERGEBNISBERICHT_GAS_WERKTAGE: u32 = 7;

/// Werktage within which the Störungsinformation must reach every affected
/// Marktlokation, counted from the ÜT of the Information an die Messlokation
/// (WiM Strom Teil 2 Kap. 1.2 Nr. 4–6 and 9–11).
pub const STOERUNG_WEITERLEITUNG_WERKTAGE: u32 = 1;

/// The PIDs that are an answer in one Use-Case and a trigger in another, with
/// the Fundstelle that makes them one.
///
/// `trigger_pid` is documented as never being an answer, and for all but these
/// it holds. The Rechnungsabwicklung des Messstellenbetriebes über den LF is
/// specified twice with the roles swapped — the MSB may offer unprompted
/// (Kap. 3.6.3.4, where QUOTES 15002 is Prozessschritt 1 and the LF owes the
/// answer in 8 Werktage) and the LF may ask first (Kap. 3.6.3.6, where the same
/// 15002 is Prozessschritt 2 answering REQOTE 35002). Both are real, so the
/// table carries both and this list records why the invariant bends.
///
/// Adding a PID here is a claim about a Festlegung; the test enforces that it
/// is one that both starts a window and answers one.
pub const CHAINED_TRIGGERS: &[(u32, &str)] = &[(
    15_002,
    "WiM Strom Teil 1 Kap. 3.6.3.4.2 Nr. 1 (MSB-initiiert, Angebot) vs. Kap. 3.6.3.6.2 Nr. 2 \
     (LF-initiiert, Antwort auf die Anfrage)",
)];

/// WiM Gas — the Antwortfristen of the Wechselprozesse im Messwesen, Sparte Gas.
///
/// **WiM Gas is a structural mirror of WiM Strom, Frist for Frist.** AWH WiM Gas
/// 2.0 (gültig ab 01.10.2026) repeats the Strom Use-Cases on the 44xxx UTILMD
/// namespace with the identical windows — 3 / 5 / 7 / 1 Werktage — and the
/// identical Vorlauffristen (15 / 7 / 20 WT, Realisierungskorridor ±9 WT,
/// Gesamtvorgang 10./11. WT). What differs is the Zuordnungszeitpunkt (**06:00
/// Uhr**, the Gastag boundary, against 00:00 in Strom), the Codeliste namespace
/// (`G_00xx` against `S_00xx`) and the APERAK regime.
///
/// The ORDERS/ORDRSP/REQOTE/QUOTES/IFTSTA/INSRPT legs of the Gas Use-Cases run
/// on the **same Prüfidentifikatoren as Strom** (17001/17002/17009,
/// 19001–19004/19015/19016, 35001/15001, 21007–21013/21036, 23001–23008), so
/// they cannot be keyed by PID here — [`WIM`] carries them and the Sparte is
/// decided at the interchange, from the recipient MP-ID. The two Fristen that
/// genuinely differ are recorded in [`WIM_GAS_SPARTE_ABWEICHUNGEN`].
///
/// BK7-24-01-009 itself states no Antwortfrist; the AWH does, and it is the
/// only source for these four.
pub const WIM_GAS: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 44_039,
        name: "Kündigung MSB Gas",
        // MSBN → MSBA. The Kündigung runs on the contract layer between the two
        // MSB and never reaches the NB (AWH WiM Gas 2.0 Kap. 3.1.2 c).
        answered_by: "MSBA",
        antwort_pids: (44_040, 44_041),
        ebd: Some("E_2000"),
        frist: FristShape::WerktageAtCutoff(3),
        family: Family::WimGas,
        source: "AWH WiM Gas 2.0 Kap. 3.3.2 Nr. 2 — 3 Werktage",
    },
    AntwortObligation {
        trigger_pid: 44_042,
        name: "Anmeldung MSB Gas (Beginn Messstellenbetrieb)",
        answered_by: "NB",
        antwort_pids: (44_043, 44_044),
        ebd: Some("E_2002"),
        frist: FristShape::WerktageAtCutoff(5),
        family: Family::WimGas,
        source: "AWH WiM Gas 2.0 Kap. 3.5.2 Nr. 2 — 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 44_051,
        name: "Ende MSB Gas (Abmeldung vom MSB an NB)",
        answered_by: "NB",
        antwort_pids: (44_052, 44_053),
        ebd: Some("E_2005"),
        frist: FristShape::WerktageAtCutoff(7),
        family: Family::WimGas,
        source: "AWH WiM Gas 2.0 Kap. 3.6.2 Nr. 2 — 7 Werktage",
    },
    AntwortObligation {
        trigger_pid: 44_168,
        name: "Verpflichtungsanfrage an den gMSB (Gas)",
        answered_by: "gMSB",
        // **44170 does not exist.** PID-Übersicht 4.0 publishes 44168 and 44169
        // and no Ablehnungs-PID; the 44170 of PID 3.3 was withdrawn with
        // FV2026-10-01. `E_2006` still publishes `G_0071`, so an Ablehnung has
        // a code and no carrier — `mako-wim` escalates instead of inventing one.
        antwort_pids: (44_169, 44_169),
        ebd: Some("E_2006"),
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::WimGas,
        source: "AWH WiM Gas 2.0 Kap. 3.6.2 Nr. 4 — 1 Werktag",
    },
];

/// The Fristen where WiM Gas genuinely departs from WiM Strom, on a
/// Prüfidentifikator the two Sparten share.
///
/// Everything else in the two families is the same number, so only the
/// departures are listed: a full Gas mirror of [`WIM`] would be a second copy of
/// the same table, with two places to keep right.
///
/// | PID | Strom | Gas | Fundstelle |
/// |---|---|---|---|
/// | 23001 Störungsmeldung | 3 WT (kME ohne RLM, mME) / 1 WT (kME mit RLM, iMS) | **3 WT**, flat | AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 2 |
/// | 23008 Mitteilung Ergebnis | 7 / 4 / 2 WT nach Messtechnik | **7 WT**, flat | AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 4 |
/// | 17001 Bestellung Geräteübernahme | 2 WT | 2 WT | WiM Teil 1 Kap. 3.2.2 Nr. 4 / AWH WiM Gas 2.0 Kap. 4.2.2 Nr. 4 |
///
/// Gas has no iMS rollout obligation, which is why its Störungs-Fristen carry
/// no Messtechnik branch.
pub const WIM_GAS_SPARTE_ABWEICHUNGEN: &[(u32, u32, &str)] = &[
    (
        23_001,
        3,
        "AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 2 — 3 Werktage, ohne Messtechnik-Verzweigung",
    ),
    (
        23_008,
        7,
        "AWH WiM Gas 2.0 Kap. 4.3.2 Nr. 4 — 7 Werktage, ohne Messtechnik-Verzweigung",
    ),
];

/// The WiM Gas answer window for a Prüfidentifikator the two Sparten share, in
/// Werktagen — `None` when Gas states the same number as Strom.
#[must_use]
pub fn wim_gas_abweichung(trigger_pid: u32) -> Option<(u32, &'static str)> {
    WIM_GAS_SPARTE_ABWEICHUNGEN
        .iter()
        .find(|(pid, _, _)| *pid == trigger_pid)
        .map(|(_, wt, src)| (*wt, *src))
}

/// Every published obligation, in consult order.
const TABLES: &[&[AntwortObligation]] = &[GPKE, GELI_GAS, WIM, WIM_GAS];

/// Every published obligation across all families.
pub fn all() -> impl Iterator<Item = &'static AntwortObligation> {
    TABLES.iter().copied().flatten()
}

/// The published answer obligation for an inbound Prüfidentifikator.
///
/// `None` when no Festlegung this codebase has read quantifies the window. That
/// is **unknown**, not unbounded: a caller that must produce an instant anyway
/// should use [`operator_window`], which marks its fallback as a convention.
#[must_use]
pub fn antwort_obligation(trigger_pid: u32) -> Option<&'static AntwortObligation> {
    all().find(|o| o.trigger_pid == trigger_pid)
}

/// A published obligation, resolved against an arrival instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Antwortfrist {
    /// The instant by which the answer must have been sent.
    pub due_at: OffsetDateTime,
    /// Which Festlegung family states it.
    pub family: Family,
    /// Citation, for the operator-facing reason and the audit trail.
    pub source: &'static str,
}

/// Resolve the answer Frist for `trigger_pid` against its arrival instant.
#[must_use]
pub fn antwortfrist(trigger_pid: u32, received: OffsetDateTime) -> Option<Antwortfrist> {
    antwort_obligation(trigger_pid).map(|o| Antwortfrist {
        due_at: o.frist.due_at(received, HolidayCalendar::BdewMaKo),
        family: o.family,
        source: o.source,
    })
}

/// The instant alone, for callers with nowhere to put the citation.
#[must_use]
pub fn antwort_deadline(trigger_pid: u32, received: OffsetDateTime) -> Option<OffsetDateTime> {
    antwortfrist(trigger_pid, received).map(|f| f.due_at)
}

// ── Operator windows ─────────────────────────────────────────────────────────

/// Headroom subtracted from the regulatory Frist to give the answer time to
/// reach the counterparty after an operator acts.
///
/// An operator approving at the deadline itself produces a market message that
/// arrives late; expiring the entry an hour early is the difference between a
/// tight decision and a missed obligation.
pub const OPERATOR_HEADROOM: Duration = Duration::hours(1);

/// Fallback window for a process whose Frist is in no table.
///
/// Deliberately short. An entry that never expires is invisible in the overdue
/// queue, which is the one signal an operator has that a market message is
/// going unanswered.
const UNKNOWN_FRIST_FALLBACK: Duration = Duration::hours(24);

/// When an operator must have decided a queued process, and where the instant
/// came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorWindow {
    /// The answer deadline itself.
    pub deadline: OffsetDateTime,
    /// When the queue entry expires — `deadline` less [`OPERATOR_HEADROOM`].
    pub expires_at: OffsetDateTime,
    /// `true` when `deadline` came from a Festlegung table, `false` when it is
    /// the 24-hour operating convention.
    pub is_regulatory: bool,
    /// Citation for `deadline`, for the queue reason and the audit log.
    pub source: &'static str,
}

impl OperatorWindow {
    /// The fallback for a PID no table quantifies.
    #[must_use]
    pub fn unknown(received: OffsetDateTime) -> Self {
        let deadline = received + UNKNOWN_FRIST_FALLBACK;
        Self {
            deadline,
            expires_at: deadline - OPERATOR_HEADROOM,
            is_regulatory: false,
            source: "no Frist published for this Prüfidentifikator — 24 h operating \
                     convention, not a regulatory deadline",
        }
    }
}

/// The operator window for an inbound PID received at `received`.
#[must_use]
pub fn operator_window(trigger_pid: u32, received: OffsetDateTime) -> OperatorWindow {
    antwortfrist(trigger_pid, received).map_or_else(
        || OperatorWindow::unknown(received),
        |f| OperatorWindow {
            deadline: f.due_at,
            expires_at: f.due_at - OPERATOR_HEADROOM,
            is_regulatory: true,
            source: f.source,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use time::{Date, Month};

    fn utc(y: i32, m: Month, d: u8, h: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(y, m, d).expect("valid date"),
            Time::from_hms(h, 0, 0).expect("valid time"),
        )
    }

    /// A GPKE Anmeldung is due 11:00 on the next Werktag — **not** 24 hours on.
    ///
    /// The failure this pins is the one `obsd` shipped: a Friday-afternoon
    /// Anmeldung breached on Saturday, and a Tuesday-evening one reported
    /// healthy nine hours after its Frist lapsed.
    #[test]
    fn a_gpke_anmeldung_is_a_clock_time_not_a_duration() {
        let received = utc(2026, Month::March, 6, 14); // Friday
        let f = antwortfrist(55_001, received).expect("published");
        assert_eq!(f.family, Family::Gpke);
        assert_eq!(
            f.due_at.date(),
            Date::from_calendar_date(2026, Month::March, 9).expect("valid date"),
            "Friday's Anmeldung is answerable until Monday"
        );
        assert_ne!(
            f.due_at,
            received + Duration::hours(24),
            "{GPKE_IS_NOT_TWENTY_FOUR_HOURS}"
        );
    }

    /// 55001 is due 11:00 and 55004 06:00 of the same Werktag; a flat window
    /// collapses them and loses five hours on the Abmeldung.
    #[test]
    fn gpke_windows_are_per_pid() {
        let received = utc(2026, Month::March, 3, 8);
        assert!(
            antwort_deadline(55_004, received) < antwort_deadline(55_001, received),
            "06:00 must precede 11:00 on the same Werktag"
        );
    }

    /// A Gas Anmeldung is four Werktage, not ten.
    #[test]
    fn a_gas_anmeldung_is_four_werktage_not_ten() {
        let received = utc(2026, Month::March, 2, 9); // Monday
        let f = antwortfrist(44_001, received).expect("published");
        assert_eq!(f.family, Family::GeliGas);
        assert_eq!(
            f.due_at.date(),
            Date::from_calendar_date(2026, Month::March, 6).expect("valid date"),
            "{TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST}"
        );
    }

    /// The four WiM Strom MSB-Wechsel PIDs keep four different instants.
    #[test]
    fn wim_strom_stays_per_pid() {
        let received = utc(2026, Month::July, 14, 8);
        let all: BTreeSet<_> = [55_039_u32, 55_042, 55_051, 55_168]
            .into_iter()
            .map(|p| antwort_deadline(p, received).expect("published"))
            .collect();
        assert_eq!(all.len(), 4, "each WiM Strom PID carries its own Frist");
    }

    /// 35003 is the ESA Werteanfrage: it has its **own** 5-Werktage window
    /// (WiM Teil 2 Kap. 4.1.2 Nr. 2), answered with a QUOTES 15003 built from
    /// a Kapitel-4.6 Messprodukt — never from a `PreisblattMessung`.
    ///
    /// The distinction the routing depends on is that it is not one of the
    /// four Preisanfrage REQOTEs, not that it lacks a Frist: an ESA
    /// Werteanfrage left without a window is one no operator queue can size.
    #[test]
    fn the_esa_werteanfrage_has_its_own_window_and_is_not_a_preisanfrage() {
        let o = antwort_obligation(ESA_WERTEANFRAGE_PID).expect("published in WiM Teil 2");
        assert_eq!(o.frist, FristShape::WerktageAtCutoff(ESA_ANGEBOT_WERKTAGE));
        assert_eq!(o.antwort_pids, (15_003, 15_003));
        assert_eq!(o.ebd, None, "E_0253 is published without a tree");
        // Its window comes from Teil 2, not from the Teil 1 Preisanfrage
        // chapters — two processes may share a *length* (35002 is also 5 WT)
        // without sharing an obligation.
        assert!(
            o.source.contains("Teil 2"),
            "the ESA window is WiM Teil 2, got {:?}",
            o.source
        );
        for preisanfrage in [35_001_u32, 35_002, 35_005] {
            let p = antwort_obligation(preisanfrage).expect("published");
            assert!(
                p.source.contains("Teil 1"),
                "{preisanfrage} is a Teil 1 Preisanfrage, got {:?}",
                p.source
            );
        }
    }

    /// The three order-level steps share the 2-Werktage answer window and each
    /// names the tree its Antwortcode must come from.
    #[test]
    fn the_esa_order_steps_carry_their_own_ebd() {
        for (pid, ebd, antwort_pids) in [
            (17_007_u32, "E_0256", (19_011_u32, 19_012_u32)),
            (17_008, "E_0254", (19_011, 19_012)),
            (39_002, "E_0257", (19_013, 19_014)),
        ] {
            let o = antwort_obligation(pid).unwrap_or_else(|| panic!("{pid} published"));
            assert_eq!(o.ebd, Some(ebd), "{pid}");
            assert_eq!(o.antwort_pids, antwort_pids, "{pid}");
            assert_eq!(
                o.frist,
                FristShape::WerktageAtCutoff(ESA_ANTWORT_WERKTAGE),
                "{pid}"
            );
        }
    }

    /// The four WiM REQOTE PIDs open four different Use-Cases, so one flat
    /// window is wrong for three of them. 35004 is the GPKE Teil 3 Anfrage
    /// einer Konfiguration and has no WiM window at all.
    #[test]
    fn the_reqote_family_is_not_one_window() {
        assert_eq!(
            antwort_obligation(35_001).map(|o| o.frist),
            Some(FristShape::WerktageAtCutoff(
                GERAETEUEBERNAHME_ANGEBOT_WERKTAGE
            )),
            "35001 is the Anforderung Geräteübernahmeangebot — 4 WT"
        );
        assert_eq!(
            antwort_obligation(35_002).map(|o| o.frist),
            Some(FristShape::WerktageAtCutoff(
                RECHNUNGSABWICKLUNG_ANFRAGE_WERKTAGE
            )),
        );
        assert_eq!(
            antwort_obligation(35_005).map(|o| o.frist),
            Some(FristShape::WerktageAtCutoff(TECHNIKAENDERUNG_WERKTAGE)),
            "35005 opens the Messlokationsänderung — 10 WT"
        );
        // 35004 opens the GPKE Teil 3 Konfigurationsprozess, so it lives in
        // the GPKE table with **2** Werktage — not in the WiM family at five.
        let o = antwort_obligation(35_004).expect("published");
        assert_eq!(o.family, Family::Gpke);
        assert_eq!(
            o.frist,
            FristShape::WerktageAtCutoff(KONFIGURATIONSANGEBOT_WERKTAGE)
        );
    }

    /// The Antwort auf die Gerätewechselabsicht is anchored on the
    /// Gerätewechseltermin, not on the arrival instant, so it belongs to
    /// `vorlauf` and must not be reported as a forward window here.
    #[test]
    fn the_geraetewechselabsicht_answer_is_a_vorlauffrist() {
        assert!(antwort_obligation(17_009).is_none());
        assert!(crate::vorlauf::vorlauf("wim.antwort-geraetewechselabsicht").is_some());
    }

    /// Only WiM Gas *request* PIDs start a clock.
    #[test]
    fn wim_gas_answers_do_not_start_a_window() {
        let received = utc(2026, Month::March, 2, 9);
        for answer in [44_040_u32, 44_041, 44_043, 44_044, 44_052, 44_169, 44_170] {
            assert!(
                antwortfrist(answer, received).is_none(),
                "answer PID {answer} must not start a window"
            );
        }
    }

    /// A PID no Festlegung quantifies is unknown, never a guessed instant.
    #[test]
    fn an_unquantified_pid_is_unknown_rather_than_defaulted() {
        let received = utc(2026, Month::March, 2, 9);
        // 44020 Änderungsmeldung zur Bestandsliste — Frist set per
        // Netzbetreiber under GeLi Gas Kap. 2.6, so there is nothing to
        // compute. 44007 and 44010 *are* quantified (Ablauf des 3. WT), which
        // is why they are asserted present rather than absent.
        assert!(antwortfrist(44_020, received).is_none());
        assert!(antwortfrist(44_007, received).is_some());
        assert!(antwortfrist(44_010, received).is_some());
        for pid in [31_001_u32, 37_000, 23_001, 99_999, 0] {
            assert!(antwortfrist(pid, received).is_none(), "PID {pid}");
        }
    }

    /// No PID may appear in two tables, or the consult order silently decides
    /// which Festlegung applies.
    #[test]
    fn every_trigger_is_unique_across_the_tables() {
        let mut seen = BTreeSet::new();
        for o in all() {
            assert!(
                seen.insert(o.trigger_pid),
                "PID {} is claimed by two tables",
                o.trigger_pid
            );
        }
    }

    /// No trigger may also be an answer — that inversion is the recurring
    /// failure mode these tables exist to prevent.
    ///
    /// [`CHAINED_TRIGGERS`] is the one carve-out and it is enumerated, not
    /// inferred: a PID gets in only because a Festlegung gives it a
    /// Prozessschritt of its own in a second Use-Case.
    #[test]
    fn no_trigger_is_also_an_answer() {
        let answers: BTreeSet<u32> = all()
            .flat_map(|o| [o.antwort_pids.0, o.antwort_pids.1])
            .collect();
        for o in all() {
            if CHAINED_TRIGGERS
                .iter()
                .any(|(pid, _)| *pid == o.trigger_pid)
            {
                continue;
            }
            assert!(
                !answers.contains(&o.trigger_pid),
                "{} is listed both as a trigger and as an answer; if a Festlegung really \
                 gives it its own Prozessschritt, add it to CHAINED_TRIGGERS with the \
                 citation rather than deleting this assertion",
                o.trigger_pid
            );
        }
    }

    /// Every carve-out must actually be in the table and actually be an answer
    /// — otherwise the list quietly grows into a way of muting the check.
    #[test]
    fn chained_triggers_are_real() {
        let answers: BTreeSet<u32> = all()
            .flat_map(|o| [o.antwort_pids.0, o.antwort_pids.1])
            .collect();
        for (pid, source) in CHAINED_TRIGGERS {
            assert!(
                antwort_obligation(*pid).is_some(),
                "{pid} is exempted but starts no window"
            );
            assert!(
                answers.contains(pid),
                "{pid} is exempted but is not an answer anywhere — the exemption is dead"
            );
            assert!(source.contains("Kap."), "{pid} cites no chapter");
        }
    }

    /// The operator window is the regulatory instant less the headroom, and
    /// never inverts.
    #[test]
    fn the_operator_window_expires_before_the_deadline() {
        let received = utc(2026, Month::March, 2, 9);
        for o in all() {
            let w = operator_window(o.trigger_pid, received);
            assert!(w.is_regulatory, "PID {}", o.trigger_pid);
            assert_eq!(w.expires_at, w.deadline - OPERATOR_HEADROOM);
            assert!(
                w.expires_at > received,
                "PID {} expires before it arrives",
                o.trigger_pid
            );
        }
    }

    /// An unquantified PID still expires — on a window that says what it is.
    #[test]
    fn an_unknown_pid_still_expires_and_says_so() {
        let received = utc(2026, Month::March, 2, 9);
        let w = operator_window(99_999, received);
        assert!(!w.is_regulatory);
        assert!(w.expires_at > received);
        assert!(w.source.contains("operating convention"));
    }

    /// Every resolved instant lies strictly after arrival, on every day of a
    /// year — the property a holiday-table edit could break.
    #[test]
    fn every_window_is_in_the_future_all_year() {
        let mut day = utc(2026, Month::January, 1, 9);
        for _ in 0..365 {
            for o in all() {
                let due = antwort_deadline(o.trigger_pid, day).expect("published");
                assert!(
                    due > day,
                    "PID {} at {day} resolved to {due}",
                    o.trigger_pid
                );
            }
            day += Duration::days(1);
        }
    }
}
