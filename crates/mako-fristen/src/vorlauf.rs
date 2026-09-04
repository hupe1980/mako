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
    /// „Spätester ÜT ist der **Tag vor dem letzten Werktag vor** dem …"
    ///
    /// GPKE Teil 2 states the Beendigung der Zuordnung this way rather than in
    /// Werktagen, and the two differ: the anchor is the last Werktag strictly
    /// before the date, and the deadline is the calendar day before *that* —
    /// which may itself be a Sunday.
    TagVorDemLetztenWerktagVor,
    /// „Spätester ÜT liegt `n` Monat(e) vor dem …"
    ///
    /// A calendar interval, not a Werktag count: GPKE Teil 2 § 2.5.2 Nr. 1
    /// uses it for EEG-Marktlokationen and Tranchen of EEG-Marktlokationen.
    /// Clamped to the last day of the month when the anchor day does not exist
    /// there (§ 188 Abs. 3 BGB).
    LatestMonateBefore(u32),
    /// „Spätester ÜT liegt `monate` Monate **und** `werktage` WT vor dem …"
    ///
    /// The two units compose in that order — the calendar interval first, then
    /// the Werktage back from the day it lands on. WiM Strom Teil 1 Kap. 3.5.2
    /// Nr. 1 states the Vorabinformation zum Ersteinbau eines iMS this way, and
    /// collapsing it to either unit alone moves the deadline by up to a week.
    LatestMonateUndWerktageBefore {
        /// Calendar months before the anchor.
        monate: u32,
        /// Werktage before the date the months land on.
        werktage: u32,
    },
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
    /// The Zahlungsziel the Rechnung itself carries (`DTM+265`).
    Zahlungsziel,
    /// The day a Preisblatt takes effect.
    Inkrafttreten,
    /// The day the Messlokation is planned to be re-equipped.
    Umstellungszeitpunkt,
    /// The **frühestmöglicher Sperrtermin** the Lieferant names in its
    /// Sperrauftrag, or — once the NB has confirmed the order — the Sperrtermin
    /// the NB set. Both are the same slot on the wire (`DTM+469` an earliest
    /// start, `DTM+203` a fixed date), and every Sperrung Vorlauffrist counts
    /// back from it.
    Sperrtermin,
    /// The day the invoiced service ended — the Beendigung der temporären
    /// Fortführung, the Überlassung der Einrichtung, the Ende des
    /// Abrechnungszeitraums or the Versand der Zusatz-/Kontrollablesung.
    Leistungsende,
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
            Self::Zahlungsziel => "Zahlungsziel",
            Self::Inkrafttreten => "Inkrafttreten",
            Self::Umstellungszeitpunkt => "Umstellungszeitpunkt",
            Self::Sperrtermin => "Sperrtermin",
            Self::Leistungsende => "Leistungsende",
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
            Self::TagVorDemLetztenWerktagVor => {
                let letzter_wt = crate::sub_werktage(anchor, 1, cal);
                let latest_uet = letzter_wt.previous_day().unwrap_or(letzter_wt);
                if uebertragungstag <= latest_uet {
                    VorlaufVerdict::Ok
                } else {
                    VorlaufVerdict::TooLate {
                        shortfall_wt: crate::werktage_between(latest_uet, uebertragungstag, cal),
                        // The earliest Zuordnungsende this ÜT can still reach:
                        // the day after the next Werktag.
                        earliest_possible: crate::next_werktag(uebertragungstag, cal)
                            .next_day()
                            .unwrap_or(uebertragungstag),
                    }
                }
            }
            Self::LatestMonateUndWerktageBefore { monate, werktage } => not_after(
                uebertragungstag,
                crate::sub_werktage(subtract_months(anchor, monate), werktage, cal),
                add_months(crate::add_werktage(uebertragungstag, werktage, cal), monate),
                cal,
            ),
            Self::LatestMonateBefore(n) => not_after(
                uebertragungstag,
                subtract_months(anchor, n),
                add_months(uebertragungstag, n),
                cal,
            ),
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

/// `Ok` while `uebertragungstag` is at or before `latest_uet`, else `TooLate`
/// naming `earliest_possible` as the first anchor date it could still reach.
fn not_after(
    uebertragungstag: Date,
    latest_uet: Date,
    earliest_possible: Date,
    cal: HolidayCalendar,
) -> VorlaufVerdict {
    if uebertragungstag <= latest_uet {
        VorlaufVerdict::Ok
    } else {
        VorlaufVerdict::TooLate {
            shortfall_wt: crate::werktage_between(latest_uet, uebertragungstag, cal),
            earliest_possible,
        }
    }
}

/// Shift a date `n` calendar months back, clamping to the month's last day.
fn subtract_months(date: Date, n: u32) -> Date {
    shift_months(date, -i32::try_from(n).unwrap_or(i32::MAX))
}

/// Shift a date `n` calendar months forward, clamping to the month's last day.
fn add_months(date: Date, n: u32) -> Date {
    shift_months(date, i32::try_from(n).unwrap_or(i32::MAX))
}

fn shift_months(date: Date, delta: i32) -> Date {
    let total = i32::from(u8::from(date.month())) - 1 + delta;
    let year = date.year() + total.div_euclid(12);
    let month = time::Month::try_from(u8::try_from(total.rem_euclid(12) + 1).unwrap_or(1))
        .unwrap_or(time::Month::January);
    // § 188 Abs. 3 BGB: a day the target month does not have becomes its last.
    let last = time::util::days_in_month(month, year);
    Date::from_calendar_date(year, month, date.day().min(last)).unwrap_or(date)
}

/// One Prozessschritt's Vorlauffrist, with its Fundstelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VorlaufObligation {
    /// Stable slug, used as the lookup key and in operator-facing reasons.
    pub key: &'static str,
    /// The **Strom** Prüfidentifikator carrying the message, where the step has
    /// one.
    pub pid: Option<u32>,
    /// The **Gas** Prüfidentifikator, where it differs from [`Self::pid`].
    ///
    /// `None` means one of two things and the difference matters: either the
    /// step has no PID at all (`pid` is `None` too), or Gas runs it on the very
    /// same Prüfidentifikator as Strom — every ORDERS/ORDRSP/IFTSTA leg of WiM
    /// does, because those AHBs are Sparte-neutral. Only the UTILMD legs split,
    /// 55xxx against 44xxx.
    pub pid_gas: Option<u32>,
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

/// Werktage before the Zahlungsziel by which the **NB** must answer the
/// Rechnung „Messstellenbetrieb mit iMS gegenüber dem NB"
/// (WiM Teil 1 Kap. 6.2 Nr. 2).
pub const ANTWORT_IMS_RECHNUNG_NB_WT: u32 = 4;

/// Werktage before the Zahlungsziel by which the MSB must state that the
/// invoice the NB refused was correct after all (WiM Teil 1 Kap. 6.2 Nr. 3).
pub const MITTEILUNG_RECHNUNG_KORREKT_WT: u32 = 2;

/// Werktage before the Zahlungsziel by which the **ESA** must answer the MSB's
/// Rechnung (WiM Strom **Teil 2** Kap. 4.5.2 Nr. 2).
///
/// The same lead the NB gets on the iMS-Rechnung, and deliberately not the
/// LF's: WiM Teil 2 states the ESA's window as „spätester ÜT ist der 4. WT vor
/// dem Zahlungsziel in der Rechnung", while WiM Teil 1 Kap. 3.6.3.8.2 has the
/// LF answer *zum* Zahlungsziel. Both invoices arrive as PID 31009 and the
/// message body does not say which Use-Case it belongs to — the recipient's
/// Marktrolle does.
pub const ANTWORT_ESA_RECHNUNG_WT: u32 = 4;

/// Werktage before the Zahlungsziel by which the MSB must tell the **ESA** that
/// the invoice it refused was correct after all — the COMDIS 29001 of
/// WiM Strom Teil 2 Kap. 4.5.2 Nr. 3 (`E_0265`).
pub const MITTEILUNG_ESA_RECHNUNG_KORREKT_WT: u32 = 2;

/// Werktage the Zahlungsziel of a WiM-Rechnung may not fall short of, counted
/// from the day the invoice is received (WiM Teil 1 Kap. 3.6.3.8.2 / 3.7.2 /
/// 6.2 Nr. 1, AWH WiM Gas 2.0 Kap. 4.7.2 Nr. 1).
///
/// A floor on the *sender*, not a window on the receiver: it constrains the
/// date the invoice may carry in `DTM+265`, and the answer window is then
/// measured against that date.
pub const ZAHLUNGSZIEL_MINDEST_WT: u32 = 10;

// ── Sperrung / Entsperrung (GPKE Teil 2 § 3.5) ───────────────────────────────
//
// Three windows that all count **backwards from the Sperrtermin**, which is why
// they live here and not in `antwort`: none of them is measured against an
// arrival, and reading any of them as „n Werktage nach Eingang" gives an answer
// that is wrong by the whole lead time.
//
// They also compose. Prozessschritt 2 obliges the NB to set the Sperrtermin so
// that Nr. 3 and Nr. 4 still fit — „Sofern keine generelle Zustimmung des MSB
// … vorliegt, ist der Sperrtermin vom NB so festzulegen, dass dem MSB noch eine
// fristgerechte Antwort auf Anfrage vor dem Sperrtermin möglich ist" — so the
// earliest admissible Sperrtermin without a standing MSB-Zustimmung is
// [`SPERRUNG_MSB_ANFRAGE_WT`] Werktage out, and the MSB's own answer window
// (3 WT, `antwort`) has to close inside it.

/// Mindestvorlaufzeit of an **untermingebundener** Sperrauftrag, in Werktagen
/// before the frühestmöglicher Sperrtermin (GPKE Teil 2 § 3.5.1.2 Nr. 1).
pub const SPERRAUFTRAG_VORLAUF_WT: u32 = 6;

/// Mindestvorlaufzeit of a **termingebundener** Sperrauftrag, in Werktagen
/// before the Sperrtermin (GPKE Teil 2 § 3.5.1.2 Nr. 1).
///
/// Twice the ordinary lead and then some: a termingebundener Auftrag fixes
/// date, time and place — the Festlegung's example is a Gerichtsvollzieher — so
/// the NB cannot move it to fit its own scheduling, and the LF has to give it
/// room instead. Treating both cases as 6 WT accepts an order the NB then
/// cannot execute as instructed.
pub const SPERRAUFTRAG_TERMINGEBUNDEN_VORLAUF_WT: u32 = 12;

/// Latest ÜT of the NB's **Anfrage Sperrung** to the MSB (ORDERS 17116), in
/// Werktagen before the Sperrtermin (GPKE Teil 2 § 3.5.1.2 Nr. 3).
///
/// Owed only „sofern keine generelle Zustimmung des MSB zur
/// Sperrung/Entsperrung durch den NB erteilt wurde". Not to be confused with
/// the MSB's *answer* window, which is 3 WT **after** the ÜT of this message
/// (`antwort`) — the two numbers are equal and measured from opposite ends.
pub const SPERRUNG_MSB_ANFRAGE_WT: u32 = 3;

// ── GeLi Gas — An-/Abmeldung ─────────────────────────────────────────────────
//
// These three live here rather than beside the `E_0622`/`E_0609` Prüfschritte
// that read them because they are Fristen, and every Frist in mako has one
// home: a role-gated crate cannot be the source of a window a role-agnostic
// caller (a config default, an operator queue) also needs.

/// Mindestvorlaufzeit for a Gas Lieferantenwechsel, in Werktagen
/// (AWH GeLi Gas 2.0 V1.2, SD „Lieferbeginn" Prozessschritt 1).
pub const GAS_WECHSEL_VORLAUF_WT: u32 = 10;

/// How far back a Gas An-/Abmeldung that is **not** a Wechsel may reach, in
/// weeks (AWH GeLi Gas 2.0 Kap. 2.2 Grundregel 3a — „bis zu sechs Wochen zzgl.
/// einer zu berücksichtigenden Bearbeitungsfrist nach An- oder
/// Abmeldedatum").
pub const GAS_RUECKWIRKUNG_WOCHEN: i64 = 6;

/// The Bearbeitungsfrist added to [`GAS_RUECKWIRKUNG_WOCHEN`], in Werktagen.
///
/// The AWH quantifies it only for the Ersatz-/Grundversorgung (3 WT) and the
/// same value is applied to An-/Abmeldungen. It is a **default**, not a
/// published Frist: operators override it in configuration, which is why it is
/// the one constant in this module a caller may legitimately ignore.
pub const GAS_BEARBEITUNGSFRIST_WT_DEFAULT: u32 = 3;

/// Who received a WiM-Rechnung — the only thing that decides its answer window.
///
/// The **same PID 31009** carries three different Use-Cases with three
/// different windows, and the message body says which one it is nowhere:
///
/// | Empfänger | Fundstelle | Spätester ÜT der Antwort |
/// |---|---|---|
/// | NB | WiM Teil 1 Kap. 6.2 Nr. 2 | 4. WT **vor** dem Zahlungsziel |
/// | **ESA** | WiM Teil **2** Kap. 4.5.2 Nr. 2 | 4. WT **vor** dem Zahlungsziel |
/// | LF / MSB | WiM Teil 1 Kap. 3.6.3.8.2, 3.7.2 | **zum** Zahlungsziel |
///
/// The recipient's Marktrolle is what tells them apart (BDEW Allgemeine
/// Festlegungen §2.13: one MP-ID per Marktrolle). Folding the ESA into the LF
/// arm gave it four Werktage it does not have, and the breach only becomes
/// visible once the MSB has already stopped waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RechnungEmpfaenger {
    /// Netzbetreiber — Rechnung „Messstellenbetrieb mit iMS gegenüber dem NB",
    /// WiM Teil 1 Kap. 6.
    Netzbetreiber,
    /// Energieserviceanbieter — „Abrechnung einer für den ESA erbrachten
    /// Leistung", WiM Teil 2 Kap. 4.5. Its tree is `E_0264`, not `E_0406`.
    Esa,
    /// Lieferant or ESA — Abrechnung Messstellenbetrieb gegenüber dem LF,
    /// WiM Teil 1 Kap. 3.6.3.8, and the Sparte-neutral Abrechnung von
    /// Dienstleistungen, Kap. 3.7.
    LieferantOderMsb,
}

/// PID of the MSB-Rechnung — the only WiM invoice with a recipient-dependent
/// answer window (INVOIC AHB 1.0b; WiM Teil 1 Kap. 3.6.3.8 and Kap. 6).
pub const MSB_RECHNUNG_PID: u32 = 31_009;

/// The latest Übertragungstag for the REMADV answering a WiM-Rechnung.
///
/// The 4-Werktage lead applies to two of the three 31009 Use-Cases — the one
/// received by a **Netzbetreiber** (WiM Teil 1 Kap. 6.2 Nr. 2) and the one
/// received by an **ESA** (WiM Teil 2 Kap. 4.5.2 Nr. 2). The LF answers *zum*
/// Zahlungsziel, and so does every other WiM invoice PID.
///
/// # Panics
///
/// Panics only if date arithmetic overflows the Gregorian calendar.
#[must_use]
pub fn rechnung_antwort_spaetester_uet(
    pid: u32,
    empfaenger: RechnungEmpfaenger,
    zahlungsziel: Date,
    cal: HolidayCalendar,
) -> Date {
    if pid != MSB_RECHNUNG_PID {
        return zahlungsziel;
    }
    match empfaenger {
        RechnungEmpfaenger::Netzbetreiber => {
            crate::sub_werktage(zahlungsziel, ANTWORT_IMS_RECHNUNG_NB_WT, cal)
        }
        RechnungEmpfaenger::Esa => crate::sub_werktage(zahlungsziel, ANTWORT_ESA_RECHNUNG_WT, cal),
        RechnungEmpfaenger::LieferantOderMsb => zahlungsziel,
    }
}

/// The latest Übertragungstag for the **COMDIS 29001** with which the MSB tells
/// an ESA that the invoice it refused was correct after all.
///
/// WiM Teil 2 Kap. 4.5.2 Nr. 3: „spätester ÜT ist der 2. WT vor dem
/// Zahlungsziel in der Rechnung". The answer to *that* message (Nr. 4) is due
/// **zum** Zahlungsziel — [`esa_zweite_antwort_spaetester_uet`].
///
/// # Panics
///
/// Panics only if date arithmetic overflows the Gregorian calendar.
#[must_use]
pub fn esa_comdis_spaetester_uet(zahlungsziel: Date, cal: HolidayCalendar) -> Date {
    crate::sub_werktage(zahlungsziel, MITTEILUNG_ESA_RECHNUNG_KORREKT_WT, cal)
}

/// The latest Übertragungstag for the ESA's **second** REMADV — its answer to
/// the MSB's COMDIS 29001 (WiM Teil 2 Kap. 4.5.2 Nr. 4, `E_0266`).
///
/// „spätester ÜT ist zum Zahlungsziel in der Rechnung" — no lead this time,
/// because there is no further round: a second refusal ends the automation and
/// the two parties clear it bilaterally.
#[must_use]
pub const fn esa_zweite_antwort_spaetester_uet(zahlungsziel: Date) -> Date {
    zahlungsziel
}

/// WiM — every Prozessschritt whose window is anchored on a date in the payload
/// rather than on the arrival instant, **in both Sparten**.
///
/// AWH WiM Gas 2.0 restates WiM Strom Teil 1 Vorlauffrist for Vorlauffrist:
/// 15 / 7 WT on the Anmeldung, 20 WT on der Abmeldung, the 8.–5. WT window on
/// the Verpflichtungsanfrage, ±9 WT Realisierungskorridor, the 10./11. WT
/// Gesamtvorgang pair, 4 WT to the Gerätewechseltermin and 2 WT before it for
/// the answer. One table therefore serves both, with the Gas UTILMD PID beside
/// the Strom one.
///
/// Deliberately keyed by slug rather than by PID: three of these steps share a
/// PID with a differently-anchored one (55168 is both the Verpflichtungsanfrage
/// and the Aufforderung; 17011 covers the NB and the LF variant), and one —
/// the Anmeldung — has two windows selected by a payload flag rather than by
/// its PID. A PID alone cannot pick the right row.
pub const WIM: &[VorlaufObligation] = &[
    VorlaufObligation {
        key: "wim.anmeldung-msb",
        pid: Some(55_042),
        pid_gas: Some(44_042),
        name: "Anmeldung MSB",
        anchor: Anchor::GewuenschterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageBefore(ANMELDUNG_WT),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 1 — spätester ÜT ist der 15. WT vor dem \
                 gewünschten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.anmeldung-msb.erstmalige-einrichtung",
        pid: Some(55_042),
        pid_gas: Some(44_042),
        name: "Anmeldung MSB (erstmalige Einrichtung des Messstellenbetriebes)",
        anchor: Anchor::GewuenschterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageBefore(ANMELDUNG_ERSTMALIG_WT),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 1 — bei erstmaliger Einrichtung des \
                 Messstellenbetriebes: spätester ÜT ist der 7. WT",
    },
    VorlaufObligation {
        key: "wim.ende-msb",
        pid: Some(55_051),
        pid_gas: Some(44_051),
        name: "Ende MSB (Abmeldung)",
        anchor: Anchor::GewuenschtesZuordnungsende,
        shape: VorlaufShape::LatestWerktageBefore(ABMELDUNG_WT),
        source: "WiM Strom Teil 1 Kap. 2.4.2 Nr. 1 — spätester ÜT ist der 20. WT vor dem \
                 gewünschten Zuordnungsende",
    },
    VorlaufObligation {
        key: "wim.verpflichtungsanfrage",
        pid: Some(55_168),
        pid_gas: Some(44_168),
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
        pid_gas: None,
        name: "Anstoß Gerätewechsel/Geräteübernahme durch den gMSB",
        anchor: Anchor::BestaetigtesZuordnungsende,
        shape: VorlaufShape::LatestWerktageBefore(4),
        source: "WiM Strom Teil 1 Kap. 2.5.2 Nr. 1/2 — spätester ÜT ist der 4. WT vor dem \
                 vorläufig bestätigten bzw. verschobenen Zuordnungsende",
    },
    VorlaufObligation {
        key: "wim.realisierungskorridor",
        pid: None,
        pid_gas: None,
        name: "Realisierungskorridor Übernahme-/Wechselzeitpunkt",
        anchor: Anchor::BestaetigterZuordnungsbeginn,
        shape: VorlaufShape::Korridor(REALISIERUNGSKORRIDOR_WT),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 5/6 — vom 9. WT vor bis zum 9. WT nach dem \
                 vom NB bestätigten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.mitteilung-gesamtvorgang",
        pid: Some(21_009),
        pid_gas: Some(21_009),
        name: "Mitteilung über Gesamtvorgang (MSBN → NB)",
        anchor: Anchor::BestaetigterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageAfter(10),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 7 — spätester ÜT ist der 10. WT nach dem vom \
                 NB bestätigten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.scheitern-gesamtvorgang",
        pid: Some(21_013),
        pid_gas: Some(21_013),
        name: "Mitteilung über das Scheitern des Gesamtvorgangs (NB → MSBN)",
        anchor: Anchor::BestaetigterZuordnungsbeginn,
        shape: VorlaufShape::LatestWerktageAfter(11),
        source: "WiM Strom Teil 1 Kap. 2.3.2 Nr. 16 — spätester ÜT ist der 11. WT nach dem vom \
                 NB bestätigten Zuordnungsbeginn",
    },
    VorlaufObligation {
        key: "wim.geraetewechsel-termin",
        pid: Some(17_009),
        pid_gas: Some(17_009),
        name: "Gerätewechseltermin nach Anzeige der Gerätewechselabsicht",
        anchor: Anchor::Gerätewechseltermin,
        shape: VorlaufShape::EarliestWerktageAfter(4),
        source: "WiM Strom Teil 1 Kap. 3.1.2 Nr. 1 — frühestens am 4. auf die Anzeige \
                 folgenden WT",
    },
    VorlaufObligation {
        key: "wim.antwort-geraetewechselabsicht",
        pid: Some(19_015),
        pid_gas: Some(19_015),
        name: "Antwort auf die Gerätewechselabsicht (Eigenausbau ja/nein)",
        anchor: Anchor::Gerätewechseltermin,
        shape: VorlaufShape::LatestWerktageBefore(2),
        source: "WiM Strom Teil 1 Kap. 3.1.2 Nr. 2 — spätester ÜT ist der 2. WT vor dem \
                 Gerätewechseltermin",
    },
    VorlaufObligation {
        key: "wim.beauftragung-aenderung-technik",
        pid: Some(17_011),
        pid_gas: None,
        name: "Beauftragung Änderung der Technik an der Messlokation",
        anchor: Anchor::Aenderungstermin,
        shape: VorlaufShape::LatestWerktageBefore(20),
        source: "WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 1 — spätester ÜT ist der 20. WT \
                 vor dem gewünschten Änderungstermin",
    },
    VorlaufObligation {
        key: "wim.scheitern-aenderung-technik",
        pid: None,
        pid_gas: None,
        name: "Scheitern der Änderung der Technik",
        anchor: Anchor::Aenderungstermin,
        shape: VorlaufShape::LatestWerktageAfter(3),
        source: "WiM Strom Teil 1 Kap. 3.3.1.2 / 3.3.2.2 Nr. 5 — spätester ÜT ist der 3. WT \
                 nach dem ursprünglich bestätigten Änderungstermin",
    },
    VorlaufObligation {
        key: "wim.vorabinformation-ersteinbau-ims",
        pid: Some(21_029),
        pid_gas: None,
        name: "Vorabinformation zum Gerätewechsel (Ersteinbau iMS, gMSB → wMSB)",
        anchor: Anchor::Umstellungszeitpunkt,
        shape: VorlaufShape::LatestMonateUndWerktageBefore {
            monate: 3,
            werktage: 3,
        },
        source: "WiM Strom Teil 1 Kap. 3.5.2 Nr. 1 — spätester ÜT liegt 3 Monate und 3 WT vor \
                 dem geplanten Zeitpunkt der Ausstattung der Messlokation",
    },
    VorlaufObligation {
        // The answer leg. `pid` names the *Zustimmung* 21030; the Ablehnung
        // 21031 rides the same window, and `crate::antwort::WIM` carries the
        // pair keyed on the trigger 21029 — this entry exists so a caller
        // holding the answer PID finds the same three Werktage.
        key: "wim.information-bestandsschutz-eigenausbau",
        pid: Some(21_030),
        pid_gas: None,
        name: "Information Bestandsschutz / Eigenausbau iMS (wMSB → gMSB)",
        anchor: Anchor::Uebertragungstag,
        shape: VorlaufShape::LatestWerktageAfter(3),
        source: "WiM Strom Teil 1 Kap. 3.5.2 Nr. 2 — spätester ÜT ist der 3. WT nach dem ÜT \
                 der Vorabinformation",
    },
    VorlaufObligation {
        key: "wim.vorabinformation-ersteinbau-ims.an-lf-und-nb",
        pid: None,
        pid_gas: None,
        name: "Vorabinformation zum Gerätewechsel an LF und NB (Ersteinbau iMS)",
        anchor: Anchor::Umstellungszeitpunkt,
        shape: VorlaufShape::LatestMonateBefore(3),
        source: "WiM Strom Teil 1 Kap. 3.5.2 Nr. 3/4 — spätester ÜT liegt 3 Monate vor dem \
                 geplanten Zeitpunkt der Ausstattung der Messlokation",
    },
    VorlaufObligation {
        key: "wim.angebot-rechnungsabwicklung",
        pid: Some(15_002),
        pid_gas: None,
        name: "Angebot zur Rechnungsabwicklung des Messstellenbetriebes über den LF",
        anchor: Anchor::Uebertragungstag,
        shape: VorlaufShape::LatestWerktageAfter(3),
        source: "WiM Strom Teil 1 Kap. 3.6.3.4.2 Nr. 1 — spätester ÜT ist der 3. WT nach dem \
                 ÜT der Mitteilung einer neuen LF-Zuordnung vom NB an den MSB",
    },
    VorlaufObligation {
        key: "wim.preisblatt-lf",
        pid: Some(27_002),
        pid_gas: None,
        name: "Übermittlung Preisblatt MSB an LF (Änderung bestehender Preisschlüsselstämme)",
        anchor: Anchor::Inkrafttreten,
        shape: VorlaufShape::LatestMonateBefore(3),
        source: "WiM Strom Teil 1 Kap. 3.6.2.3.2 Nr. 1 — spätester ÜT liegt 3 Monate vor dem \
                 Wirksamwerden der geänderten Preise zu bestehenden Preisschlüsselstämmen",
    },
    VorlaufObligation {
        key: "wim.preisblatt-nb.initial",
        pid: Some(27_002),
        pid_gas: None,
        name: "Preisblatt „Messstellenbetrieb mit iMS gegenüber dem NB\" (initial)",
        anchor: Anchor::Uebertragungstag,
        shape: VorlaufShape::LatestWerktageAfter(3),
        source: "WiM Strom Teil 1 Kap. 5.2 Nr. 1 — spätester ÜT ist der 3. WT, nachdem die \
                 EDIFACT-Kommunikation aufgebaut wurde",
    },
    VorlaufObligation {
        key: "wim.preisblatt-nb.aenderung",
        pid: Some(27_002),
        pid_gas: None,
        name: "Preisblatt „Messstellenbetrieb mit iMS gegenüber dem NB\" (Änderung)",
        anchor: Anchor::Inkrafttreten,
        shape: VorlaufShape::LatestWerktageBefore(20),
        source: "WiM Strom Teil 1 Kap. 5.2 Nr. 1 — spätester ÜT ist der 20. WT vor \
                 Inkrafttreten des geänderten Preisblatts",
    },
    VorlaufObligation {
        key: "wim.rechnung-dienstleistungen",
        pid: Some(31_003),
        pid_gas: Some(31_003),
        name: "Rechnung über Dienstleistungen im Messwesen",
        anchor: Anchor::Leistungsende,
        shape: VorlaufShape::LatestWerktageAfter(20),
        source: "WiM Strom Teil 1 Kap. 3.7.2 Nr. 1 — spätester ÜT ist der 20. WT nach \
                 Beendigung der Leistung",
    },
    VorlaufObligation {
        key: "wim.antwort-rechnung",
        pid: Some(33_001),
        pid_gas: Some(33_001),
        name: "Antwort auf eine WiM-Rechnung (REMADV)",
        anchor: Anchor::Zahlungsziel,
        shape: VorlaufShape::LatestWerktageBefore(0),
        source: "WiM Strom Teil 1 Kap. 3.6.3.8.2 Nr. 2/4 und Kap. 3.7.2 Nr. 2/4 — spätester \
                 ÜT ist zum Zahlungsziel in der Rechnung",
    },
    VorlaufObligation {
        key: "wim.antwort-rechnung-ims-nb",
        pid: Some(33_001),
        pid_gas: None,
        name: "Antwort des NB auf die iMS-Rechnung (REMADV)",
        anchor: Anchor::Zahlungsziel,
        shape: VorlaufShape::LatestWerktageBefore(ANTWORT_IMS_RECHNUNG_NB_WT),
        source: "WiM Strom Teil 1 Kap. 6.2 Nr. 2 — spätester ÜT ist der 4. WT vor dem \
                 Zahlungsziel in der Rechnung",
    },
    VorlaufObligation {
        key: "wim.mitteilung-rechnung-korrekt",
        pid: Some(29_001),
        pid_gas: None,
        name: "Mitteilung, dass die ursprüngliche Rechnung korrekt war (COMDIS)",
        anchor: Anchor::Zahlungsziel,
        shape: VorlaufShape::LatestWerktageBefore(MITTEILUNG_RECHNUNG_KORREKT_WT),
        source: "WiM Strom Teil 1 Kap. 6.2 Nr. 3 — spätester ÜT ist der 2. WT vor dem \
                 Zahlungsziel in der Rechnung",
    },
    VorlaufObligation {
        key: "gpke.sperrauftrag",
        pid: Some(17_115),
        pid_gas: None,
        name: "Sperrauftrag (nicht termingebunden)",
        anchor: Anchor::Sperrtermin,
        shape: VorlaufShape::LatestWerktageBefore(SPERRAUFTRAG_VORLAUF_WT),
        source: "BK6-24-174 GPKE Teil 2 § 3.5.1.2 SD Unterbrechung der Anschlussnutzung \
                 Nr. 1 — „Auftrag ist nicht termingebunden: Unverzüglich, jedoch spätester \
                 ÜT ist der 6. WT vor dem frühestmöglichen Sperrtermin\"",
    },
    VorlaufObligation {
        key: "gpke.sperrauftrag.termingebunden",
        pid: Some(17_115),
        pid_gas: None,
        name: "Sperrauftrag (termingebunden)",
        anchor: Anchor::Sperrtermin,
        shape: VorlaufShape::LatestWerktageBefore(SPERRAUFTRAG_TERMINGEBUNDEN_VORLAUF_WT),
        source: "BK6-24-174 GPKE Teil 2 § 3.5.1.2 Nr. 1 — „Auftrag ist termingebunden \
                 (z.B. der Gerichtsvollzieher gibt den Sperrtermin (Datum, Uhrzeit, Ort) \
                 vor): … spätester ÜT ist der 12. WT vor dem Sperrtermin\"",
    },
    VorlaufObligation {
        key: "gpke.sperrung.anfrage-msb",
        pid: Some(17_116),
        pid_gas: None,
        name: "Anfrage Sperrung an den MSB",
        anchor: Anchor::Sperrtermin,
        shape: VorlaufShape::LatestWerktageBefore(SPERRUNG_MSB_ANFRAGE_WT),
        source: "BK6-24-174 GPKE Teil 2 § 3.5.1.2 Nr. 3 — „Unverzüglich, jedoch spätester \
                 ÜT ist der 3. WT vor dem Sperrtermin\"; owed nur „sofern keine generelle \
                 Zustimmung des MSB … erteilt wurde\". Nr. 2 obliges the NB to set the \
                 Sperrtermin so that this and the MSB\u{2019}s 3-WT answer still fit.",
    },
];

/// Look up a Vorlauffrist by its slug.
#[must_use]
pub fn vorlauf(key: &str) -> Option<&'static VorlaufObligation> {
    WIM.iter().find(|o| o.key == key)
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

    /// „3 Monate und 3 WT vor" is not „3 Monate vor": the Werktage move the
    /// deadline further back, and over a weekend they move it by five days.
    #[test]
    fn monate_und_werktage_compose_in_that_order() {
        // Umstellung Mon 2026-06-01 → 3 Monate back is Sun 2026-03-01,
        // 3 WT before that is Wed 2026-02-25.
        let umstellung = d(2026, Month::June, 1);
        let shape = VorlaufShape::LatestMonateUndWerktageBefore {
            monate: 3,
            werktage: 3,
        };
        assert!(
            shape
                .check(d(2026, Month::February, 25), umstellung, CAL)
                .is_ok()
        );
        assert!(
            !shape
                .check(d(2026, Month::February, 26), umstellung, CAL)
                .is_ok()
        );
        // The months-only shape would still accept 2026-03-01.
        assert!(
            VorlaufShape::LatestMonateBefore(3)
                .check(d(2026, Month::March, 1), umstellung, CAL)
                .is_ok()
        );
    }

    /// The ESA answers by the **4. WT vor dem Zahlungsziel** (WiM Teil 2
    /// Kap. 4.5.2 Nr. 2), not zum Zahlungsziel like the LF — the same lead the
    /// NB gets. Folding it into the LF arm handed the ESA four Werktage it does
    /// not have.
    #[test]
    fn the_esa_answers_four_werktage_before_the_zahlungsziel() {
        // Zahlungsziel Fri 2026-06-19; 4 WT before is Mon 2026-06-15.
        let ziel = d(2026, Month::June, 19);
        assert_eq!(
            rechnung_antwort_spaetester_uet(MSB_RECHNUNG_PID, RechnungEmpfaenger::Esa, ziel, CAL),
            d(2026, Month::June, 15)
        );
        assert_eq!(
            rechnung_antwort_spaetester_uet(
                MSB_RECHNUNG_PID,
                RechnungEmpfaenger::Netzbetreiber,
                ziel,
                CAL
            ),
            rechnung_antwort_spaetester_uet(MSB_RECHNUNG_PID, RechnungEmpfaenger::Esa, ziel, CAL),
            "Teil 1 Kap. 6.2 Nr. 2 and Teil 2 Kap. 4.5.2 Nr. 2 state the same lead"
        );
        assert_ne!(
            rechnung_antwort_spaetester_uet(MSB_RECHNUNG_PID, RechnungEmpfaenger::Esa, ziel, CAL),
            rechnung_antwort_spaetester_uet(
                MSB_RECHNUNG_PID,
                RechnungEmpfaenger::LieferantOderMsb,
                ziel,
                CAL
            ),
        );
    }

    /// The rest of the UC 4.5 round trip: the MSB's COMDIS is due by the 2. WT
    /// before the Zahlungsziel (Nr. 3) and the ESA's answer to it *zum*
    /// Zahlungsziel (Nr. 4) — three different windows on one invoice.
    #[test]
    fn the_esa_billing_round_trip_has_three_windows() {
        let ziel = d(2026, Month::June, 19);
        assert_eq!(
            esa_comdis_spaetester_uet(ziel, CAL),
            d(2026, Month::June, 17)
        );
        assert_eq!(esa_zweite_antwort_spaetester_uet(ziel), ziel);
        // Strictly ordered: answer → COMDIS → second answer.
        assert!(
            rechnung_antwort_spaetester_uet(MSB_RECHNUNG_PID, RechnungEmpfaenger::Esa, ziel, CAL)
                < esa_comdis_spaetester_uet(ziel, CAL)
        );
        assert!(esa_comdis_spaetester_uet(ziel, CAL) < esa_zweite_antwort_spaetester_uet(ziel));
    }

    /// Only 31009 to a Netzbetreiber gets the 4-Werktage lead; the same PID to
    /// an LF, and 31003 to anyone, is answered zum Zahlungsziel.
    #[test]
    fn only_the_ims_rechnung_to_the_nb_leads_the_zahlungsziel() {
        // Zahlungsziel Fri 2026-06-19; 4 WT before is Mon 2026-06-15.
        let ziel = d(2026, Month::June, 19);
        assert_eq!(
            rechnung_antwort_spaetester_uet(
                MSB_RECHNUNG_PID,
                RechnungEmpfaenger::Netzbetreiber,
                ziel,
                CAL
            ),
            d(2026, Month::June, 15)
        );
        assert_eq!(
            rechnung_antwort_spaetester_uet(
                MSB_RECHNUNG_PID,
                RechnungEmpfaenger::LieferantOderMsb,
                ziel,
                CAL
            ),
            ziel
        );
        assert_eq!(
            rechnung_antwort_spaetester_uet(31_003, RechnungEmpfaenger::Netzbetreiber, ziel, CAL),
            ziel
        );
    }

    /// Every row is reachable by its own key, and no key is claimed twice —
    /// `vorlauf()` returns the first match, so a duplicate silently shadows.
    #[test]
    fn every_key_is_unique_and_resolvable() {
        for o in WIM {
            assert_eq!(vorlauf(o.key).map(|f| f.name), Some(o.name), "{}", o.key);
        }
        let mut keys: Vec<_> = WIM.iter().map(|o| o.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate Vorlauffrist key");
    }

    /// Every convenience helper must return the shape the published table
    /// carries for the same Prozessschritt.
    ///
    /// The helpers build their shape from the same constants the table does, not
    /// from the table entry — so editing one and not the other leaves the
    /// checker silently disagreeing with the window this crate publishes, and
    /// with the Fundstelle a refusal cites. There is no runtime dependency
    /// between them to catch it, only this.
    #[test]
    fn the_helpers_agree_with_the_table_they_stand_for() {
        for (key, shape) in [
            ("wim.anmeldung-msb", anmeldung_vorlauf(false)),
            (
                "wim.anmeldung-msb.erstmalige-einrichtung",
                anmeldung_vorlauf(true),
            ),
        ] {
            let published = vorlauf(key).unwrap_or_else(|| panic!("{key} is catalogued"));
            assert_eq!(published.shape, shape, "{key}");
        }

        // `realisierungskorridor` returns a date range rather than a shape, so
        // it is held against the window the table states.
        let korridor = vorlauf("wim.realisierungskorridor").expect("catalogued");
        assert_eq!(
            korridor.shape,
            VorlaufShape::Korridor(REALISIERUNGSKORRIDOR_WT)
        );
        let termin = d(2025, Month::February, 3);
        let range = realisierungskorridor(termin, CAL);
        assert_eq!(
            *range.start(),
            crate::sub_werktage(termin, REALISIERUNGSKORRIDOR_WT, CAL)
        );
        assert_eq!(
            *range.end(),
            crate::add_werktage(termin, REALISIERUNGSKORRIDOR_WT, CAL)
        );
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
        let mut keys: Vec<_> = WIM.iter().map(|o| o.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate Vorlauffrist key");
        for o in WIM {
            // WiM and the AWH cite chapters; GPKE Teil 2 cites paragraphs.
            // Requiring „Kap." alone rejected a correctly-sourced GPKE entry.
            assert!(
                o.source.contains("Kap.") || o.source.contains('§'),
                "{} cites neither a Kapitel nor a §: {}",
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
