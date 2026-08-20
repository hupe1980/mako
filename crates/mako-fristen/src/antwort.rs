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
//! - BK6-24-174 WiM Strom Teil 1, Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2
//! - BK7-24-01-009 / AWH WiM Gas V2.0
//! - EDI@Energy Anwendungsübersicht der Prüfidentifikatoren 4.0 — roles, EBDs

use time::{Duration, OffsetDateTime, Time};

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
     (11:00 / 06:00 / 05:00 / 09:00), never a 24-hour duration — BK6-24-174 Teil 2";

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
    /// WiM Strom (Messstellenbetrieb) — BK6-24-174 Teil 1.
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
    /// „Unverzüglich, jedoch spätester ÜZ ist `HH:MM` Uhr des 1. WT nach dem ÜT."
    ///
    /// A wall-clock instant in German local time on the first Werktag strictly
    /// after the arrival day.
    NextWerktagAt(Time),
    /// „…bis zum **Ablauf** des `n`. Werktags nach Eingang."
    ///
    /// Day-granular: the Frist runs to the end of that Werktag. The arrival day
    /// does not count (§ 187 Abs. 1 BGB).
    EndOfWerktag(u32),
    /// „…spätester ÜT ist der `n`. WT nach dem ÜT", resolved to the 17:00
    /// Europe/Berlin MaKo cut-off on that Werktag.
    WerktageAtCutoff(u32),
}

impl FristShape {
    /// Resolve this Frist against the instant the message arrived.
    #[must_use]
    pub fn due_at(self, received: OffsetDateTime, cal: HolidayCalendar) -> OffsetDateTime {
        match self {
            Self::NextWerktagAt(at) => crate::next_werktag_at(received, at, cal),
            Self::EndOfWerktag(n) => crate::end_of_werktag_after(received, n, cal),
            Self::WerktageAtCutoff(n) => crate::deadline_at_werktage(received, n, cal),
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
pub const GPKE: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 55_001,
        name: "Anmeldung verb. Marktlokation (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (55_002, 55_003),
        ebd: Some("E_0622"),
        frist: FristShape::NextWerktagAt(at(11)),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
    },
    AntwortObligation {
        trigger_pid: 55_077,
        name: "Anmeldung erz. Marktlokation (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (55_078, 55_080),
        ebd: Some("E_0622"),
        frist: FristShape::NextWerktagAt(at(11)),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritte 5/6",
    },
    AntwortObligation {
        trigger_pid: 55_004,
        name: "Abmeldung (Lieferende von LF an NB)",
        answered_by: "NB",
        antwort_pids: (55_005, 55_006),
        ebd: Some("E_0607"),
        frist: FristShape::NextWerktagAt(at(6)),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferende von LF an NB Prozessschritte 2/3",
    },
    AntwortObligation {
        trigger_pid: 55_007,
        name: "Ankündigung der Beendigung der Zuordnung (Lieferende von NB an LF)",
        answered_by: "LF",
        antwort_pids: (55_008, 55_009),
        ebd: Some("E_0609"),
        frist: FristShape::NextWerktagAt(at(5)),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferende von NB an LF Prozessschritt 2",
    },
    AntwortObligation {
        trigger_pid: 55_010,
        name: "Anfrage zur Beendigung der Zuordnung",
        answered_by: "LFA",
        antwort_pids: (55_011, 55_012),
        ebd: Some("E_0624"),
        frist: FristShape::NextWerktagAt(at(9)),
        family: Family::Gpke,
        source: "BK6-24-174 GPKE Teil 2, SD Lieferbeginn Prozessschritt 4",
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
];

/// GeLi Gas — every inbound PID whose answer Frist the Festlegung quantifies.
///
/// | PID | Process | Answerer | Frist |
/// |---|---|---|---|
/// | 44001 | Anmeldung NN (Lieferbeginn) | NB | Ablauf des 4. WT |
/// | 44004 | Abmeldung NN (Lieferende) | NB | Ablauf des 3. WT |
/// | 44013 | Zuordnung Ersatz-/Grundversorgung | E/G | Ablauf des 2. WT |
/// | 44016 | Kündigung beim Altlieferanten | LFA | Ablauf des 3. WT |
///
/// The Abmeldungsanfrage (44010), the GNB-initiated Abmeldung NN (44007) and the
/// Änderungsmeldung (44020) have Fristen set per Netzbetreiber under Kap. 2.6,
/// so they are absent — *unknown*, never *unbounded*.
pub const GELI_GAS: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 44_001,
        name: "Anmeldung NN (Lieferbeginn)",
        answered_by: "NB",
        antwort_pids: (44_002, 44_003),
        ebd: None,
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
        ebd: None,
        frist: FristShape::EndOfWerktag(3),
        family: Family::GeliGas,
        source: "GeLi Gas 3.0 Kap. 3.2.2 — „spätestens jedoch bis zum Ablauf des 3. Werktags \
                 nach Eingang der Abmeldung\"",
    },
    AntwortObligation {
        trigger_pid: 44_013,
        name: "Zuordnung Ersatz-/Grundversorgung",
        answered_by: "E/G",
        antwort_pids: (44_014, 44_015),
        ebd: None,
        frist: FristShape::EndOfWerktag(2),
        family: Family::GeliGas,
        source: "GeLi Gas 3.0 Kap. 3.3.2 — „spätestens bis zum Ablauf des 2. Werktages\"",
    },
    AntwortObligation {
        trigger_pid: 44_016,
        name: "Kündigung beim Altlieferanten",
        answered_by: "LFA",
        antwort_pids: (44_017, 44_018),
        ebd: None,
        frist: FristShape::EndOfWerktag(3),
        family: Family::GeliGas,
        source: "GeLi Gas 3.0 Kap. 3.1 — „spätestens jedoch bis zum Ablauf des 3. Werktages \
                 nach Eingang der Kündigung\"",
    },
];

/// WiM Strom — the MSB-Wechsel family and the REQOTE Preisanfrage.
///
/// The MSB-Wechsel windows are **not** one flat number: 3 / 5 / 7 / 1 Werktage
/// („Unverzüglich, jedoch spätester ÜT ist der *n*. WT nach dem ÜT von Nr. 1",
/// BK6-24-174 WiM Teil 1 Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2). A flat window
/// fires early for the Kündigung and late for the Abmeldung.
///
/// 35003 is deliberately absent: it is the ESA „Anfrage von Werten", not a
/// Preisanfrage, and must never be answered with a PreisblattMessung quote.
pub const WIM: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 55_039,
        name: "Kündigung MSB",
        answered_by: "MSBA",
        antwort_pids: (55_040, 55_041),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(3),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom Teil 1 Kap. 2.2.2 Nr. 2 — 3 Werktage",
    },
    AntwortObligation {
        trigger_pid: 55_042,
        name: "Anmeldung MSB",
        answered_by: "NB",
        antwort_pids: (55_043, 55_044),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(5),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom Teil 1 Kap. 2.3.2 Nr. 2 — 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 55_051,
        name: "Abmeldung MSB",
        answered_by: "NB",
        antwort_pids: (55_052, 55_053),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(7),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom Teil 1 Kap. 2.4.2 Nr. 2 — 7 Werktage",
    },
    AntwortObligation {
        trigger_pid: 55_168,
        name: "Verpflichtungsanfrage",
        answered_by: "MSB",
        antwort_pids: (55_169, 55_170),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(1),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom Teil 1 Kap. 2.5.2 Nr. 4 — 1 Werktag",
    },
    AntwortObligation {
        trigger_pid: 35_001,
        name: "Preisanfrage (REQOTE)",
        answered_by: "MSB",
        antwort_pids: (15_001, 15_001),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(PREISANFRAGE_WERKTAGE),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom — REQOTE Preisanfrage, 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 35_002,
        name: "Preisanfrage Rechnungsabwicklung MSB über LF (REQOTE)",
        answered_by: "MSB",
        antwort_pids: (15_002, 15_002),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(PREISANFRAGE_WERKTAGE),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom — REQOTE Preisanfrage, 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 35_004,
        name: "Preisanfrage (REQOTE)",
        answered_by: "MSB",
        antwort_pids: (15_004, 15_004),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(PREISANFRAGE_WERKTAGE),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom — REQOTE Preisanfrage, 5 Werktage",
    },
    AntwortObligation {
        trigger_pid: 35_005,
        name: "Preisanfrage (REQOTE)",
        answered_by: "MSB",
        antwort_pids: (15_005, 15_005),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(PREISANFRAGE_WERKTAGE),
        family: Family::Wim,
        source: "BK6-24-174 WiM Strom — REQOTE Preisanfrage, 5 Werktage",
    },
];

/// The REQOTE Preisanfrage answer window, in Werktage (BK6-24-174).
pub const PREISANFRAGE_WERKTAGE: u32 = 5;

/// WiM Gas — the 10-Werktage window, on the PIDs that start the clock.
///
/// Answer PIDs are deliberately absent. This is the one family where a flat
/// number is correct, which is how it came to cover GeLi Gas as well.
pub const WIM_GAS: &[AntwortObligation] = &[
    AntwortObligation {
        trigger_pid: 44_039,
        name: "Kündigung MSB Gas",
        answered_by: "MSBA",
        antwort_pids: (44_040, 44_041),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(WIM_GAS_WERKTAGE),
        family: Family::WimGas,
        source: "BK7-24-01-009 / AWH WiM Gas V2.0 — 10 Werktage",
    },
    AntwortObligation {
        trigger_pid: 44_042,
        name: "Anmeldung neuer MSB Gas",
        answered_by: "NB",
        antwort_pids: (44_043, 44_044),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(WIM_GAS_WERKTAGE),
        family: Family::WimGas,
        source: "BK7-24-01-009 / AWH WiM Gas V2.0 — 10 Werktage",
    },
    AntwortObligation {
        trigger_pid: 44_051,
        name: "Ende MSB Gas / Vorläufige Abmeldung",
        answered_by: "MSBA",
        antwort_pids: (44_052, 44_053),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(WIM_GAS_WERKTAGE),
        family: Family::WimGas,
        source: "BK7-24-01-009 / AWH WiM Gas V2.0 — 10 Werktage",
    },
    AntwortObligation {
        trigger_pid: 44_168,
        name: "Verpflichtungsanfrage Gas",
        answered_by: "gMSB",
        antwort_pids: (44_169, 44_170),
        ebd: None,
        frist: FristShape::WerktageAtCutoff(WIM_GAS_WERKTAGE),
        family: Family::WimGas,
        source: "BK7-24-01-009 / AWH WiM Gas V2.0 — 10 Werktage",
    },
];

/// The WiM Gas answer window, in Werktage.
pub const WIM_GAS_WERKTAGE: u32 = 10;

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

    /// 35003 is the ESA Werteanfrage, not a Preisanfrage, and must not carry a
    /// PreisblattMessung answer window.
    #[test]
    fn the_esa_werteanfrage_is_not_a_preisanfrage() {
        assert!(antwort_obligation(35_003).is_none());
        for pid in [35_001_u32, 35_002, 35_004, 35_005] {
            assert_eq!(
                antwort_obligation(pid).map(|o| o.frist),
                Some(FristShape::WerktageAtCutoff(PREISANFRAGE_WERKTAGE)),
                "PID {pid}"
            );
        }
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
        // 44010 Abmeldungsanfrage — Frist set per Netzbetreiber under Kap. 2.6.
        assert!(antwortfrist(44_010, received).is_none());
        for pid in [31_001_u32, 37_000, 23_001, 55_557, 99_999, 0] {
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
    #[test]
    fn no_trigger_is_also_an_answer() {
        let answers: BTreeSet<u32> = all()
            .flat_map(|o| [o.antwort_pids.0, o.antwort_pids.1])
            .collect();
        for o in all() {
            assert!(
                !answers.contains(&o.trigger_pid),
                "{} is listed both as a trigger and as an answer",
                o.trigger_pid
            );
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
