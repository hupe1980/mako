//! The Netzbetreiber's Anmeldung decision — `E_0622` (Strom) and `G_0011` (Gas).
//!
//! All checks are **pure functions** — no I/O, no global state, no clock calls.
//! The current instant is always passed as a parameter.
//!
//! # Three trees, three alphabets
//!
//! An Anmeldung is not one decision. `E_0622` Prüfschritt 10 splits Strom into
//! two branches that share **no** Antwortcode, and Gas answers from an entirely
//! different Codeliste:
//!
//! | Anwendungsfall | Tree | „andere Anmeldung in Bearbeitung" | Fristüberschreitung |
//! |---|---|---|---|
//! | Strom, verbrauchende / ruhende MaLo | `E_0622` Prüfschritte 15–70 | `A06` | `A07` |
//! | Strom, erzeugende MaLo / Tranche | `E_0622` Prüfschritte 220–830 | `A45` | `A34`/`A28`/`A29`/`A30`/`A32`/`A35`/`A44` |
//! | Gas | `E_3005` / `G_0011` | `ZC5` | `E17` |
//!
//! Putting `A06` on a 44003 is not a wrong reason — it is a code the Gas
//! Codeliste does not define, and the counterparty cannot act on it. Every code
//! this module emits is therefore resolved through [`crate::codes`] against the
//! tree that publishes it.
//!
//! # The Bestätigung carries a code too
//!
//! `E_0622` is a **Vorprüfung**: every code it publishes is an Ablehnung. A
//! message that survives it is confirmed out of `E_0623` („Lieferbeginn
//! prüfen") with `A51` (verbrauchende / ruhende MaLo) or `A58` (erzeugende);
//! Gas confirms with `E15` out of `G_0012`. `SG4 STS+E01` is Muss on every
//! Antwortnachricht, so [`NbEntscheidung::Accept`] carries the code rather than
//! leaving the caller to invent one.
//!
//! # Vorlauffristen
//!
//! **Strom, verbrauchende / ruhende Marktlokation** (GPKE Teil 2 § 2.1.1
//! Prozessschritt 1): „Unverzüglich nach Vorliegen des Anmeldegrundes, jedoch
//! spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn."
//! Retroactive Anmeldungen were abolished with LFW24 for **every**
//! Transaktionsgrund. Operationally: at least one full Werktag must lie
//! strictly between the receipt day and the Zuordnungsbeginn. There is no
//! time-of-day cutoff. Violation → `A07` (Prüfschritt 15).
//!
//! **Strom, erzeugende Marktlokation**: six published Fristen, chosen by
//! Geschäftsvorfall and by the pair (bestehende, angemeldete)
//! Veräußerungsform — GPKE Teil 2 § 2.1.1 „Fristen für die Anmeldung
//! (Prozessschritt 1) bei EEG-Marktlokationen und Tranchen von
//! EEG-Marktlokationen", walked as `E_0622` Prüfschritte 300–830:
//!
//! | Fall | Regel | Code |
//! |---|---|---|
//! | Nicht-EEG-/-KWKG, GV 1 / 2 / 3 | Tag vor dem letzten WT | `A34` / `A30` / `A35` |
//! | EEG, GV 1, Veräußerungsformwechsel | Monatserster + 1 Monat | `A27` / `A28` |
//! | EEG, GV 1, Wechsel aus der Ausfallvergütung | Monatserster + 5 WT | `A27` / `A29` |
//! | EEG, GV 2 | Monatserster + 1 Monat | `A31` / `A32` |
//! | EEG, GV 3 | 1 Monat | `A44` |
//!
//! The Monatserster rule is **§ 21b Abs. 1 EEG 2023** with the notice period in
//! **§ 21c** — not § 10c, which is the Solarpaket-I „Zuordnung geringfügiger
//! Verbräuche".
//!
//! **Gas** (AWH GeLi Gas 2.0 V1.2 Kap. 2.2, in force since 01.04.2026):
//! - Lieferantenwechsel (`E03`): future-only, ≥ **10 WT** before the
//!   Lieferbeginn. Violation → `E17`.
//! - Non-Wechsel (`E01` Ein-/Auszug, `E02` Einzug in Neuanlage) with SLP
//!   metering: retroactive up to **6 Wochen** + Bearbeitungsfrist (3 WT).
//! - Non-Wechsel with RLM or SMGW-attached metering: future-only.
//! - Backdated without a readable Transaktionsgrund → **Escalate** (never
//!   auto-reject a potentially lawful move-in — § 20 EnWG).
//!
//! # Where the sources disagree
//!
//! For an EEG-Marktlokation in Geschäftsvorfall 1 or 2 **without** a
//! Veräußerungsformwechsel, GPKE Teil 2's Fristentabelle permits an
//! untermonatlichen Zuordnungsbeginn („DV mit Marktprämie → DV mit Marktprämie")
//! which `E_0622` Prüfschritt 410 / 620 would refuse. That case escalates and
//! names both sources: auto-rejecting against the Festlegung's own table is the
//! § 20 EnWG-unsafe direction.
//!
//! # Regulatory sources
//!
//! - BK6-24-174 GPKE Teil 2 § 2.1 (UC/SD Lieferbeginn) und § 2.1.1 Fristentabelle
//! - Entscheidungsbaum-Diagramme und Codelisten **4.3** (01.04.2026),
//!   Kap. 6.6.1 (`E_0622`), 6.6.4 (`E_0623`), 13.6.1 (`E_3005` / `G_0011`),
//!   13.6.4 (`E_3007` / `G_0012`)
//! - UTILMD MIG Strom S2.2 — `SG10 CCI+Z22` DE 7037 (Veräußerungsform)
//! - AWH GeLi Gas 2.0 V1.2 (26.03.2026) Kap. 2.2
//! - §§ 21b, 21c EEG 2023

use time::{Date, Duration, OffsetDateTime};

use mako_fristen::{self as fristen, HolidayCalendar};
use mako_markt::domain::Sparte;
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};

use crate::codes::{
    self, AntwortCode, EBD_ANMELDUNG_DIREKT_ABLEHNBAR, EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS,
    EBD_LIEFERBEGINN, EBD_LIEFERBEGINN_GAS,
};

use super::config::NetzCheckConfig;
use super::types::{
    AnmeldungAnfrage, ErzeugungsAnmeldung, Geschaeftsvorfall, MaloGridRecord, Messtyp,
    NbEntscheidung, RejectReason, Veraeusserungsform,
};

// ── Regulatory constants ──────────────────────────────────────────────────────

/// Gas Lieferantenwechsel (E03): minimum lead, Anmeldung → Lieferbeginn
/// (AWH GeLi Gas 2.0 V1.2, SD Lieferbeginn Prozessschritt 1).
///
/// Re-exported from [`mako_fristen::vorlauf`], which owns every Frist. This
/// module is behind `role-nb`; the same windows are read by callers that
/// compile no Marktrolle at all.
pub use mako_fristen::vorlauf::{
    GAS_BEARBEITUNGSFRIST_WT_DEFAULT, GAS_RUECKWIRKUNG_WOCHEN, GAS_WECHSEL_VORLAUF_WT,
};

/// The Vorlauffrist of an EEG-Veräußerungsformwechsel, in whole months
/// (§ 21c EEG 2023; GPKE Teil 2 § 2.1.1 „Spätester ÜT liegt 1 Monat vor dem
/// Zuordnungsbeginn"). Override via
/// [`NetzCheckConfig::eeg_zuordnung_vorlauf_monate`].
pub const EEG_ZUORDNUNG_VORLAUF_MONATE_DEFAULT: u32 = 1;

/// The **verkürzte** Vorlauffrist, in Werktage, for a plant leaving the
/// Ausfallvergütung (§ 21 Abs. 1 Nr. 2 EEG 2023) — GPKE Teil 2 § 2.1.1 letzte
/// Zeile, `E_0622` Prüfschritt 440 („Vorgabe nach EEG: 5 WT vor
/// Zuordnungsbeginn").
pub const EEG_VERKUERZTER_WECHSEL_WT: u32 = 5;

/// The UTILMD `SG4 STS+7` DE 9013 Transaktionsgrundergänzung marking an
/// **erzeugende Marktlokation**, verified against `UTILMD_AHB_Strom_2.2` Kap. 3.
///
/// The caller maps it — together with PID 55077, which *is* the Anwendungsfall
/// „Anmeldung erzeugende Marktlokation" — onto
/// [`Marktlokationsart::Erzeugend`](super::types::Marktlokationsart::Erzeugend).
pub const EEG_ERZEUGENDE_MARKTLOKATION_CODE: &str = "ZW3";

// ── Code resolution ───────────────────────────────────────────────────────────

/// Resolve a published Antwortcode, or panic naming the tree.
///
/// Every call site passes a literal pair that a `codes` unit test also asserts,
/// so a typo fails the test suite rather than reaching the wire.
fn code(ebd: &'static str, code: &'static str) -> &'static AntwortCode {
    codes::lookup(ebd, code)
        .unwrap_or_else(|| panic!("{code} is not published by {ebd} — see crate::codes"))
}

/// An `E_0622` Ablehnung.
fn strom(c: &'static str, pruefschritt: u16, detail: String) -> NbEntscheidung {
    NbEntscheidung::Reject(RejectReason::new(
        EBD_ANMELDUNG_DIREKT_ABLEHNBAR,
        code(EBD_ANMELDUNG_DIREKT_ABLEHNBAR, c),
        pruefschritt,
        detail,
    ))
}

/// A `G_0011` (Gas) Ablehnung. The Gas Codelisten carry no DE 1131, so the
/// resolved code reports `ebd: None` even though it is looked up by tree.
fn gas(c: &'static str, pruefschritt: u16, detail: String) -> NbEntscheidung {
    NbEntscheidung::Reject(RejectReason::new(
        EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS,
        code(EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS, c),
        pruefschritt,
        detail,
    ))
}

// ── Werktag helpers ───────────────────────────────────────────────────────────
//
// The crate stays pure and clock-free; Werktag arithmetic delegates to the real
// BDEW-MaKo holiday calendar in `mako-fristen` (injected via
// `NetzCheckConfig::holiday_calendar`).

/// `true` when at least one Werktag lies **strictly between** `a` and `b`.
///
/// The operational form of „spätester ÜT ist der Tag vor dem letzten WT vor dem
/// Zuordnungsbeginn": an Anmeldung received on day `a` may carry Zuordnungs-
/// beginn `b` iff a full Werktag separates them.
pub(crate) fn has_werktag_strictly_between(a: Date, b: Date, cal: HolidayCalendar) -> bool {
    // The first Werktag strictly after `a` is the first Werktag on-or-after the
    // day following `a`. A Werktag lies strictly between iff it precedes `b`.
    let Some(day_after) = a.next_day() else {
        return false;
    };
    fristen::next_werktag(day_after, cal) < b
}

/// The date `n` Werktage after `d` under the given holiday calendar.
fn add_werktage(d: Date, n: u32, cal: HolidayCalendar) -> Date {
    fristen::add_werktage(d, n, cal)
}

/// The date `n` Werktage **before** `d` — „spätester ÜT ist der n. WT vor dem
/// Zuordnungsbeginn".
fn werktage_before(d: Date, n: u32, cal: HolidayCalendar) -> Date {
    let mut current = d;
    let mut remaining = n;
    while remaining > 0 {
        let Some(prev) = current.previous_day() else {
            return current;
        };
        current = prev;
        if fristen::is_werktag(current, cal) {
            remaining -= 1;
        }
    }
    current
}

/// The same calendar day `months` whole months earlier, clamped to the length
/// of the target month (31.03. minus one month is 28./29.02.).
///
/// „Spätester ÜT liegt 1 Monat vor dem Zuordnungsbeginn" is a calendar-month
/// span, not 30 days.
pub(crate) fn months_before(d: Date, months: u32) -> Date {
    let mut year = d.year();
    let mut month = d.month();
    for _ in 0..months {
        month = month.previous();
        if month == time::Month::December {
            year -= 1;
        }
    }
    let last = time::util::days_in_month(month, year);
    Date::from_calendar_date(year, month, d.day().min(last))
        .expect("day is clamped to the month length")
}

// ── evaluate ──────────────────────────────────────────────────────────────────

/// Decide one inbound Anmeldung.
///
/// # Parameters
///
/// - `anfrage` — parsed fields from the `de.mako.process.initiated` CloudEvent.
/// - `versorgung` — current supply state from `GET /api/v1/versorgung/{malo_id}`
///   on `marktd`. `None` if `marktd` returned 404 (MaLo unknown).
/// - `grid` — NB grid topology record from `GET /api/v1/malos/{id}/grid`.
///   `None` when the NB's NIS/GIS data has not yet been imported.
/// - `partner_known` — `true` if the requesting LF MP-ID is in the operator's
///   partner directory.
/// - `now` — current UTC instant (injected by the caller for testability).
/// - `config` — tunables (holiday calendar, Gas Bearbeitungsfrist, EEG lead).
///
/// # Returns
///
/// [`NbEntscheidung::Accept`] with the Zustimmungscode, [`NbEntscheidung::Reject`]
/// with the Ablehnungscode of the tree that governs this Anwendungsfall, or
/// [`NbEntscheidung::Escalate`] when a Prüfschritt needs a fact the projection
/// does not carry.
///
/// # Notes
///
/// This function is **synchronous** and **infallible** — it never panics on
/// caller data and never returns an `Err`. A `marktd` connectivity error is the
/// caller's to retry; do not call `evaluate` with incomplete data.
#[must_use]
pub fn evaluate(
    anfrage: &AnmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    grid: Option<&MaloGridRecord>,
    partner_known: bool,
    now: OffsetDateTime,
    config: &NetzCheckConfig,
) -> NbEntscheidung {
    // Grid record present. Not a Prüfschritt: the tree assumes the NB knows its
    // own Netzgebiet, so a missing record is a data gap in mako, not a ground
    // for refusing the counterparty (§ 20 EnWG).
    let Some(grid) = grid else {
        return NbEntscheidung::Escalate {
            reason: format!(
                "No grid record found for MaLo {} in the NB's grid topology. \
                 Import NIS/GIS data or provision the record manually via \
                 PUT /api/v1/malos/{}/grid.",
                anfrage.malo_id, anfrage.malo_id
            ),
        };
    };

    let today = mako_fristen::berlin_date(now);

    match (anfrage.sparte, anfrage.marktlokationsart) {
        (Sparte::Gas, _) => g_0011(anfrage, versorgung, grid, partner_known, today, *config),
        (Sparte::Strom, art) if art.ist_verbrauchend_oder_ruhend() => {
            e_0622_verbrauchend(anfrage, versorgung, grid, partner_known, today, *config)
        }
        (Sparte::Strom, _) => {
            e_0622_erzeugend(anfrage, versorgung, grid, partner_known, today, *config)
        }
    }
}

// ── E_0622, verbrauchende / ruhende Marktlokation (Prüfschritte 15–70) ────────

fn e_0622_verbrauchend(
    anfrage: &AnmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    grid: &MaloGridRecord,
    partner_known: bool,
    today: Date,
    config: NetzCheckConfig,
) -> NbEntscheidung {
    // ── 15: Wurde die Vorlauffrist eingehalten? ──────────────────────────────
    if !has_werktag_strictly_between(today, anfrage.process_date, config.holiday_calendar) {
        return strom(
            "A07",
            15,
            format!(
                "Vorlauffrist not met: requested Zuordnungsbeginn {} vs receipt day {today}. \
                 GPKE Teil 2 § 2.1.1 Prozessschritt 1: spätester ÜT ist der Tag vor dem \
                 letzten WT vor dem Zuordnungsbeginn — at least one full Werktag must lie \
                 between receipt and Lieferbeginn; retroactive Anmeldungen are not permitted \
                 under LFW24 for any Transaktionsgrund.",
                anfrage.process_date
            ),
        );
    }

    // ── 30: Nimmt die Marktlokation an der Marktkommunikation teil? ──────────
    //
    // The EBD's own Hinweis enumerates the set: „Marktlokationen, die
    // stillgelegt sind bzw. Marktlokationen, die dem Modell 2 zur
    // ladevorgangscharfen bilanziellen Energiemengenzuordnungsmöglichkeit
    // zugeordnet sind." A **ruhende** Marktlokation is not in it — Prüfschritte
    // 16–28 exist precisely to check one, so refusing it here would refuse the
    // Anwendungsfall the tree is built around.
    if let Some(vs) = versorgung
        && vs.lieferstatus == LieferStatus::Stillgelegt
    {
        return strom(
            "A02",
            30,
            format!(
                "MaLo {} does not participate in market communication \
                 (lieferstatus = {}).",
                anfrage.malo_id, vs.lieferstatus
            ),
        );
    }

    // ── 60: Sind alle zwingend notwendigen Anforderungen des LF erfüllt? ─────
    //
    // „Insbesondere die notwendige Zuordnungsermächtigung
    // (Bilanzkreis/Bilanzierungsverfahren) ist vorhanden."
    if let Some(reason) = anforderungen_nicht_erfuellt(anfrage, grid, partner_known) {
        return match reason {
            Anforderung::BilanzierungsgebietUnbekannt => NbEntscheidung::Escalate {
                reason: format!(
                    "UTILMD provides Bilanzierungsgebiet {:?} but the grid record for MaLo {} \
                     has none — the Zuordnungsermächtigung of E_0622 Prüfschritt 60 cannot be \
                     confirmed. Update the record via PUT /api/v1/malos/{}/grid.",
                    anfrage.bilanzierungsgebiet, anfrage.malo_id, anfrage.malo_id
                ),
            },
            Anforderung::Abweichung(detail) => strom("A05", 60, detail),
        };
    }

    // ── 70: Liegt bereits eine in Arbeit befindliche Anmeldung vor? ──────────
    if let Some(detail) = andere_anmeldung_in_bearbeitung(anfrage, versorgung) {
        return strom("A06", 70, detail);
    }

    // Survived the Vorprüfung: the Bestätigung comes out of E_0623.
    NbEntscheidung::accept(EBD_LIEFERBEGINN, code(EBD_LIEFERBEGINN, "A51"))
}

// ── E_0622, erzeugende Marktlokation / Tranche (Prüfschritte 220–830) ─────────

fn e_0622_erzeugend(
    anfrage: &AnmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    grid: &MaloGridRecord,
    partner_known: bool,
    today: Date,
    config: NetzCheckConfig,
) -> NbEntscheidung {
    // The date branch (300–830) is the whole point of this arm and it needs
    // facts the common projection does not carry. Escalating names what is
    // missing; guessing picks one of six published Fristen at random.
    let Some(erz) = anfrage.erzeugung.as_ref() else {
        return NbEntscheidung::Escalate {
            reason: format!(
                "Anmeldung erzeugender Marktlokation {} carries no Geschäftsvorfall and no \
                 Veräußerungsform (UTILMD SG10 CCI+Z22). E_0622 Prüfschritte 300–830 choose \
                 between six published Vorlauffristen on exactly those facts, so the decision \
                 cannot be made without them.",
                anfrage.malo_id
            ),
        };
    };

    // ── 260: Sind alle zwingend notwendigen Anforderungen des LF erfüllt? ────
    if let Some(reason) = anforderungen_nicht_erfuellt(anfrage, grid, partner_known) {
        return match reason {
            Anforderung::BilanzierungsgebietUnbekannt => NbEntscheidung::Escalate {
                reason: format!(
                    "UTILMD provides Bilanzierungsgebiet {:?} but the grid record for MaLo {} \
                     has none — the Zuordnungsermächtigung of E_0622 Prüfschritt 260 cannot be \
                     confirmed.",
                    anfrage.bilanzierungsgebiet, anfrage.malo_id
                ),
            },
            Anforderung::Abweichung(detail) => strom("A25", 260, detail),
        };
    }

    // ── 270: Liegt bereits eine in Arbeit befindliche Anmeldung vor? ─────────
    //
    // A45, not A06: the erzeugende branch has its own code for this condition.
    if let Some(detail) = andere_anmeldung_in_bearbeitung(anfrage, versorgung) {
        return strom("A45", 270, detail);
    }

    // ── 400 / 600: „Verändert sich die Veräußerungsform?" ────────────────────
    //
    // Geschäftsvorfall 1 and 2 branch on it, and the answer needs the
    // *bestehende* Veräußerungsform — the NB's own register, not the message.
    // Without it the Monatserster check of Prüfschritt 410 / 620 would refuse an
    // untermonatlichen Wechsel innerhalb derselben Veräußerungsform, which GPKE
    // Teil 2 § 2.1.1 permits. Geschäftsvorfall 3 does not ask the question.
    if matches!(
        erz.geschaeftsvorfall,
        Geschaeftsvorfall::Eins | Geschaeftsvorfall::Zwei
    ) && erz.ist_veraeusserungsformwechsel().is_none()
    {
        return NbEntscheidung::Escalate {
            reason: format!(
                "MaLo {}: the Veräußerungsform in force at the Zuordnungsbeginn is \
                 unknown, so E_0622 Prüfschritt {} (Veräußerungsformwechsel?) cannot \
                 be answered. The angemeldete Form is {} (SG10 CCI+Z22); the \
                 bestehende one comes from the NB's EEG-/KWKG-Register.",
                anfrage.malo_id,
                if erz.geschaeftsvorfall == Geschaeftsvorfall::Eins {
                    400
                } else {
                    600
                },
                erz.angemeldete_veraeusserungsform.wire_code(),
            ),
        };
    }

    // ── 300 / 310: Geschäftsvorfall, then the date branch ────────────────────
    match erz.geschaeftsvorfall {
        Geschaeftsvorfall::Eins => gv1(anfrage, erz, today, config),
        Geschaeftsvorfall::Zwei => gv2(anfrage, erz, today, config),
        Geschaeftsvorfall::Drei => gv3(anfrage, erz, today, config),
    }
    .unwrap_or_else(|| NbEntscheidung::accept(EBD_LIEFERBEGINN, code(EBD_LIEFERBEGINN, "A58")))
}

/// `E_0622` Prüfschritte 400–440 — Geschäftsvorfall 1.
fn gv1(
    anfrage: &AnmeldungAnfrage,
    erz: &ErzeugungsAnmeldung,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NbEntscheidung> {
    // 400 → 405 → 406: a Nicht-EEG-/-KWKG-Marktlokation with no
    // Veräußerungsformwechsel takes the ordinary Werktag rule — and, once it
    // passes, the tree leaves for E_0621. It does not fall through to 410.
    match nicht_eeg_werktagsregel(anfrage, erz, today, config, "A34", 406) {
        Arm::NichtZutreffend => {}
        Arm::Bestanden => return None,
        Arm::Ablehnung(d) => return Some(d),
    }
    if let Some(d) = untermonatlich_erlaubt(anfrage, erz, today, 410) {
        return Some(d);
    }
    // 410: Monatserster?
    if anfrage.process_date.day() != 1 {
        return Some(strom(
            "A27",
            410,
            monatserster_detail(anfrage, erz, "Geschäftsvorfall 1"),
        ));
    }
    // 420: verkürzter Wechsel? → 440 (5 WT) : 430 (1 Monat)
    if erz.ausfallverguetung {
        let earliest_ut = werktage_before(
            anfrage.process_date,
            EEG_VERKUERZTER_WECHSEL_WT,
            config.holiday_calendar,
        );
        if today > earliest_ut {
            return Some(strom(
                "A29",
                440,
                format!(
                    "Verkürzte Vorlauffrist not met: Zuordnungsbeginn {}, latest ÜT \
                     {earliest_ut} ({EEG_VERKUERZTER_WECHSEL_WT} WT vor dem \
                     Zuordnungsbeginn), receipt {today}. The plant leaves the \
                     Ausfallvergütung (§ 21 Abs. 1 Nr. 2 EEG 2023), for which GPKE Teil 2 \
                     § 2.1.1 fixes the verkürzte Frist.",
                    anfrage.process_date
                ),
            ));
        }
        return None;
    }
    monatsfrist(anfrage, today, config, "A28", 430, "Geschäftsvorfall 1")
}

/// `E_0622` Prüfschritte 600–630 — Geschäftsvorfall 2.
fn gv2(
    anfrage: &AnmeldungAnfrage,
    erz: &ErzeugungsAnmeldung,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NbEntscheidung> {
    match nicht_eeg_werktagsregel(anfrage, erz, today, config, "A30", 610) {
        Arm::NichtZutreffend => {}
        Arm::Bestanden => return None,
        Arm::Ablehnung(d) => return Some(d),
    }
    if let Some(d) = untermonatlich_erlaubt(anfrage, erz, today, 620) {
        return Some(d);
    }
    // 620: Monatserster?
    if anfrage.process_date.day() != 1 {
        return Some(strom(
            "A31",
            620,
            monatserster_detail(anfrage, erz, "Geschäftsvorfall 2"),
        ));
    }
    // 630: Vorlauffrist von einem Monat. Geschäftsvorfall 2 has no verkürzter
    // Wechsel — the tree goes straight from 620 to 630.
    monatsfrist(anfrage, today, config, "A32", 630, "Geschäftsvorfall 2")
}

/// `E_0622` Prüfschritte 800–830 — Geschäftsvorfall 3.
///
/// The shape differs: a date that is **not** a Monatserster is not refused
/// outright (there is no `A27`/`A31` here). It is routed to the Nicht-EEG check
/// and, failing that, to the one-month Vorlauffrist.
fn gv3(
    anfrage: &AnmeldungAnfrage,
    erz: &ErzeugungsAnmeldung,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NbEntscheidung> {
    // 800 → 805 → 806: only reached when the date is not a Monatserster. A pass
    // here leaves the tree for E_0621; it does not continue to 810.
    if anfrage.process_date.day() != 1 && erz.nicht_eeg_kwkg {
        return (!has_werktag_strictly_between(
            today,
            anfrage.process_date,
            config.holiday_calendar,
        ))
        .then(|| {
            strom(
                "A35",
                806,
                nicht_eeg_detail(anfrage, today, "Geschäftsvorfall 3"),
            )
        });
    }

    // 810: Vorlauffrist von einem Monat.
    monatsfrist(anfrage, today, config, "A44", 810, "Geschäftsvorfall 3")
}

/// What a conditional arm of the tree did.
///
/// A three-way answer, because „passed" and „did not apply" lead to different
/// places: `406` leaves for `E_0621` — it does not fall through to the
/// Monatserster check the next Prüfschritt would run.
enum Arm {
    /// The arm's precondition is false; continue with the next Prüfschritt.
    NichtZutreffend,
    /// The arm applied and passed; the tree is finished with this branch.
    Bestanden,
    /// The arm applied and refused.
    Ablehnung(NbEntscheidung),
}

/// The `405`/`605` → `406`/`610` arm: a „Nicht-EEG-/-KWKG"-Marktlokation
/// without a Veräußerungsformwechsel keeps the ordinary Werktag rule.
fn nicht_eeg_werktagsregel(
    anfrage: &AnmeldungAnfrage,
    erz: &ErzeugungsAnmeldung,
    today: Date,
    config: NetzCheckConfig,
    reject_code: &'static str,
    pruefschritt: u16,
) -> Arm {
    // Prüfschritt 400/600 „ja" (a Veräußerungsformwechsel) skips this arm.
    if erz.ist_veraeusserungsformwechsel().unwrap_or(false) || !erz.nicht_eeg_kwkg {
        return Arm::NichtZutreffend;
    }
    if has_werktag_strictly_between(today, anfrage.process_date, config.holiday_calendar) {
        Arm::Bestanden
    } else {
        Arm::Ablehnung(strom(
            reject_code,
            pruefschritt,
            nicht_eeg_detail(anfrage, today, "Nicht-EEG-/-KWKG-Marktlokation"),
        ))
    }
}

/// The one place `E_0622` and GPKE Teil 2's Fristentabelle contradict each
/// other — see the module docs. Returns an `Escalate` rather than the
/// Monatserster refusal the EBD would give.
fn untermonatlich_erlaubt(
    anfrage: &AnmeldungAnfrage,
    erz: &ErzeugungsAnmeldung,
    today: Date,
    pruefschritt: u16,
) -> Option<NbEntscheidung> {
    if anfrage.process_date.day() == 1 {
        return None;
    }
    // Only the no-Wechsel case is contested; a Veräußerungsformwechsel is
    // Monatserster in both sources.
    if erz.ist_veraeusserungsformwechsel() != Some(false) {
        return None;
    }
    let permitted = matches!(
        erz.angemeldete_veraeusserungsform,
        Veraeusserungsform::Marktpraemie | Veraeusserungsform::SonstigeDirektvermarktung
    );
    permitted.then(|| NbEntscheidung::Escalate {
        reason: format!(
            "MaLo {}: untermonatlicher Zuordnungsbeginn {} without a Veräußerungsformwechsel \
             (bestehende und angemeldete Veräußerungsform sind beide {}). The published \
             sources disagree — GPKE Teil 2 § 2.1.1 permits it („Der Zuordnungsbeginn darf \
             ein Monatserster oder untermonatlich sein\" / „Spätester ÜT ist der Tag vor dem \
             letzten WT\"), while EBD 4.3 E_0622 Prüfschritt {pruefschritt} would refuse it. \
             mako does not auto-reject against the Festlegung's own Fristentabelle \
             (§ 20 EnWG). Receipt {today}.",
            anfrage.malo_id,
            anfrage.process_date,
            erz.angemeldete_veraeusserungsform.wire_code(),
        ),
    })
}

/// „Ist die Vorlauffrist von einem Monat eingehalten?"
fn monatsfrist(
    anfrage: &AnmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
    reject_code: &'static str,
    pruefschritt: u16,
    fall: &str,
) -> Option<NbEntscheidung> {
    let monate = config.eeg_zuordnung_vorlauf_monate;
    let latest_ut = months_before(anfrage.process_date, monate);
    (today > latest_ut).then(|| {
        strom(
            reject_code,
            pruefschritt,
            format!(
                "Vorlauffrist not met ({fall}): Zuordnungsbeginn {}, latest ÜT {latest_ut} \
                 ({monate} month(s) ahead per GPKE Teil 2 § 2.1.1 and § 21c EEG 2023), \
                 receipt {today}.",
                anfrage.process_date
            ),
        )
    })
}

fn monatserster_detail(
    anfrage: &AnmeldungAnfrage,
    erz: &ErzeugungsAnmeldung,
    fall: &str,
) -> String {
    format!(
        "Zuordnungsbeginn {} is not the first of a calendar month ({fall}, angemeldete \
         Veräußerungsform {}). A Wechsel der Veräußerungsform is only possible zum ersten \
         Kalendertag eines Monats (§ 21b Abs. 1 EEG 2023; GPKE Teil 2 § 2.1.1).",
        anfrage.process_date,
        erz.angemeldete_veraeusserungsform.wire_code(),
    )
}

fn nicht_eeg_detail(anfrage: &AnmeldungAnfrage, today: Date, fall: &str) -> String {
    format!(
        "Vorlauffrist not met ({fall}): Zuordnungsbeginn {} vs receipt day {today}. Spätester \
         ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn.",
        anfrage.process_date
    )
}

// ── Shared Prüfschritte ───────────────────────────────────────────────────────

/// Outcome of the „zwingend notwendige Anforderungen" Prüfschritt.
enum Anforderung {
    /// The message asserts a Bilanzierungsgebiet the grid record cannot confirm.
    BilanzierungsgebietUnbekannt,
    /// A named deviation — the EBD requires it to be spelled out.
    Abweichung(String),
}

fn anforderungen_nicht_erfuellt(
    anfrage: &AnmeldungAnfrage,
    grid: &MaloGridRecord,
    partner_known: bool,
) -> Option<Anforderung> {
    if let (Some(req), Some(have)) = (&anfrage.bilanzierungsgebiet, &grid.bilanzierungsgebiet)
        && req != have
    {
        return Some(Anforderung::Abweichung(format!(
            "Bilanzierungsgebiet mismatch: UTILMD contains '{req}' but the grid record for \
             MaLo {} has '{have}'. Die Zuordnungsermächtigung \
             (Bilanzkreis/Bilanzierungsverfahren) kann nicht erfüllt werden.",
            anfrage.malo_id
        )));
    }
    if anfrage.bilanzierungsgebiet.is_some() && grid.bilanzierungsgebiet.is_none() {
        return Some(Anforderung::BilanzierungsgebietUnbekannt);
    }
    if !partner_known {
        return Some(Anforderung::Abweichung(format!(
            "LF MP-ID {} is not registered in the partner directory, so the Anmeldung cannot \
             be answered over AS4. The LF must publish their MP-ID at bdew-codes.de and \
             register a channel before initiating a Lieferbeginn.",
            anfrage.new_supplier_gln
        )));
    }
    None
}

/// „Liegt für diese Marktlokation bereits eine gerade in Arbeit befindliche und
/// noch nicht beantwortete Anmeldung vor?"
///
/// The MP-ID comparison is load-bearing, not a refinement: `marktd`'s event
/// ingest calls `announce_lf_next` while ingesting the `process.initiated`
/// CloudEvent — before it fans the event out — so by the time this runs the
/// Anmeldung under evaluation has already written its own `lf_mp_id_next`. A
/// bare `is_some()` test rejects every first-time Anmeldung against itself.
fn andere_anmeldung_in_bearbeitung(
    anfrage: &AnmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
) -> Option<String> {
    let vs = versorgung?;
    if vs
        .lf_mp_id_next
        .as_deref()
        .is_some_and(|next| next != anfrage.new_supplier_gln.as_str())
    {
        return Some(format!(
            "MaLo {} already has a pending Lieferbeginn (lf_mp_id_next = {:?}).",
            anfrage.malo_id, vs.lf_mp_id_next
        ));
    }
    if vs.lieferstatus == LieferStatus::Beliefert
        && vs.lf_mp_id.as_deref() == Some(anfrage.new_supplier_gln.as_str())
    {
        return Some(format!(
            "MaLo {} is already supplied by LF {} (duplicate Anmeldung).",
            anfrage.malo_id, anfrage.new_supplier_gln
        ));
    }
    None
}

// ── G_0011 — Gas (E_3005) ─────────────────────────────────────────────────────

fn g_0011(
    anfrage: &AnmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    grid: &MaloGridRecord,
    partner_known: bool,
    today: Date,
    config: NetzCheckConfig,
) -> NbEntscheidung {
    // The AHB is explicit: „Die Prüfungen, die zu den Codes A03, A04, A16 und
    // A17 führen, sind zuerst durchzuführen." A16 is the Gas spelling of
    // „nimmt nicht an der Marktkommunikation teil"; A02 is not a G_0011 code.
    if let Some(vs) = versorgung
        && matches!(
            vs.lieferstatus,
            LieferStatus::Stillgelegt | LieferStatus::Ruhend
        )
    {
        return gas(
            "A16",
            10,
            format!(
                "Identifizierte Marktlokation {} nimmt nicht an der Marktkommunikation teil \
                 (lieferstatus = {}).",
                anfrage.malo_id, vs.lieferstatus
            ),
        );
    }
    if grid.nb_mp_id != anfrage.grid_operator_gln {
        return gas(
            "A04",
            10,
            format!(
                "Marktlokation {} is assigned to NB {} and not to {} at the Eingangsdatum of \
                 the message.",
                anfrage.malo_id, grid.nb_mp_id, anfrage.grid_operator_gln
            ),
        );
    }

    // Fristen — E17.
    if let Some(d) = gas_date_rule(anfrage, today, config) {
        return d;
    }

    // Bilanzierungsproblem — E13, not the Strom A05.
    if let (Some(req), Some(have)) = (&anfrage.bilanzierungsgebiet, &grid.bilanzierungsgebiet)
        && req != have
    {
        return gas(
            "E13",
            20,
            format!(
                "Bilanzierungsgebiet mismatch: UTILMD contains '{req}' but the grid record for \
                 MaLo {} has '{have}' — der Bilanzkreis bzw. der erforderliche Zeitreihentyp \
                 ist in der Zuordnungsermächtigung nicht aufgeführt.",
                anfrage.malo_id
            ),
        );
    }
    if anfrage.bilanzierungsgebiet.is_some() && grid.bilanzierungsgebiet.is_none() {
        return NbEntscheidung::Escalate {
            reason: format!(
                "UTILMD provides Bilanzierungsgebiet {:?} but the grid record for MaLo {} has \
                 none — the Zuordnungsermächtigung cannot be confirmed.",
                anfrage.bilanzierungsgebiet, anfrage.malo_id
            ),
        };
    }

    // Andere Anmeldung in Bearbeitung — ZC5; ein bereits bestätigter Vorgang — Z08.
    if let Some(vs) = versorgung {
        if vs
            .lf_mp_id_next
            .as_deref()
            .is_some_and(|next| next != anfrage.new_supplier_gln.as_str())
        {
            return gas(
                "ZC5",
                30,
                format!(
                    "MaLo {} already has a pending Lieferbeginn (lf_mp_id_next = {:?}).",
                    anfrage.malo_id, vs.lf_mp_id_next
                ),
            );
        }
        if vs.lieferstatus == LieferStatus::Beliefert
            && vs.lf_mp_id.as_deref() == Some(anfrage.new_supplier_gln.as_str())
        {
            return gas(
                "Z08",
                30,
                format!(
                    "MaLo {} is already supplied by LF {} — der angefragte Geschäftsvorfall \
                     wurde bereits bestätigt.",
                    anfrage.malo_id, anfrage.new_supplier_gln
                ),
            );
        }
    }

    if !partner_known {
        return gas(
            "E14",
            40,
            format!(
                "LF MP-ID {} is not registered in the partner directory, so the Anmeldung \
                 cannot be answered over AS4.",
                anfrage.new_supplier_gln
            ),
        );
    }

    NbEntscheidung::accept(EBD_LIEFERBEGINN_GAS, code(EBD_LIEFERBEGINN_GAS, "E15"))
}

/// Gas date rule (Transaktionsgrund-aware) — see module docs. `None` = valid.
fn gas_date_rule(
    anfrage: &AnmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NbEntscheidung> {
    let cal = config.holiday_calendar;
    let d = anfrage.process_date;
    match anfrage.transaktionsgrund.as_deref() {
        // Lieferantenwechsel: future-only, ≥ 10 WT lead.
        Some("E03") => {
            let earliest = add_werktage(today, GAS_WECHSEL_VORLAUF_WT, cal);
            (d < earliest).then(|| {
                gas(
                    "E17",
                    10,
                    format!(
                        "Fristüberschreitung: Gas Lieferantenwechsel (E03) requires \
                         ≥ {GAS_WECHSEL_VORLAUF_WT} WT lead — requested Lieferbeginn {d}, \
                         earliest {earliest} (receipt {today}). Wechsel sind nur in die \
                         Zukunft gerichtet möglich (AWH GeLi Gas 2.0 Kap. 2.2).",
                    ),
                )
            })
        }
        // Non-Wechsel (Ein-/Auszug E01, Einzug in Neuanlage E02, …):
        // retroactive permitted for SLP metering within the 6-week window.
        Some(_) => {
            if d >= today {
                return None; // future or same-day non-Wechsel is always plausible
            }
            match anfrage.messtyp {
                Messtyp::Slp => {
                    let bearbeitungsfrist = config.gas_bearbeitungsfrist_wt;
                    let window_end = add_werktage(
                        d.saturating_add(Duration::weeks(GAS_RUECKWIRKUNG_WOCHEN)),
                        bearbeitungsfrist,
                        cal,
                    );
                    (today > window_end).then(|| {
                        gas(
                            "E17",
                            10,
                            format!(
                                "Fristüberschreitung: retroactive Gas Anmeldung {d} is outside \
                                 the 6-week window (+{bearbeitungsfrist} WT Bearbeitungsfrist, \
                                 ends {window_end}; receipt {today}). Lieferbeginn kann nur \
                                 noch für die Zukunft realisiert werden (AWH GeLi Gas 2.0 \
                                 Kap. 2.2 Grundregel 3b).",
                            ),
                        )
                    })
                }
                // Hourly-balanced (RLM) and SMGW-attached metering: dates may
                // only lie after the receipt date (Grundregel 2).
                Messtyp::Rlm | Messtyp::Imsys => Some(gas(
                    "E17",
                    10,
                    format!(
                        "Fristüberschreitung: retroactive Gas Anmeldung {d} is not permitted \
                         for {} metering — An- und Abmeldedatum können nur nach dem \
                         Eingangsdatum liegen (AWH GeLi Gas 2.0 Kap. 2.2 Grundregel 2).",
                        anfrage.messtyp
                    ),
                )),
            }
        }
        // No readable Transaktionsgrund: never auto-reject a backdated
        // Anmeldung that may be a lawful move-in (§ 20 EnWG) — escalate.
        None => (d < today).then(|| NbEntscheidung::Escalate {
            reason: format!(
                "Backdated Gas Anmeldung ({d}, receipt {today}) without a readable SG4 STS \
                 Transaktionsgrund — cannot distinguish a lawful retroactive Ein-/Auszug \
                 (6-week window) from an impermissible retroactive Wechsel. Operator review \
                 required.",
            ),
        }),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};
    use time::{Date, Month, OffsetDateTime, macros::datetime};
    use uuid::Uuid;

    use mako_markt::domain::Sparte;

    use super::super::types::Marktlokationsart;

    fn make_anfrage(pid: u32, process_date: Date) -> AnmeldungAnfrage {
        AnmeldungAnfrage {
            pid,
            process_id: Uuid::new_v4(),
            malo_id: "51238696012".to_owned(),
            new_supplier_gln: "9900357000004".to_owned(),
            grid_operator_gln: "9900000000002".to_owned(),
            bilanzierungsgebiet: Some("11YB-TENNET-----W".to_owned()),
            process_date,
            sparte: Sparte::Strom,
            messtyp: Messtyp::Slp,
            transaktionsgrund: Some("E03".to_owned()),
            marktlokationsart: Marktlokationsart::Verbrauchend,
            erzeugung: None,
            abmeldeanfrage: crate::nb::Abmeldeanfrage::NichtErforderlich,
        }
    }

    fn make_gas_anfrage(transaktionsgrund: Option<&str>, process_date: Date) -> AnmeldungAnfrage {
        let mut a = make_anfrage(44001, process_date);
        a.sparte = Sparte::Gas;
        a.transaktionsgrund = transaktionsgrund.map(ToOwned::to_owned);
        a
    }

    /// An erzeugende Anmeldung with the facts `E_0622` 300–830 needs.
    fn make_erz_anfrage(
        process_date: Date,
        gv: Geschaeftsvorfall,
        bestehende: Veraeusserungsform,
        angemeldete: Veraeusserungsform,
    ) -> AnmeldungAnfrage {
        let mut a = make_anfrage(55_077, process_date);
        a.marktlokationsart = Marktlokationsart::Erzeugend;
        a.erzeugung = Some(ErzeugungsAnmeldung {
            geschaeftsvorfall: gv,
            angemeldete_veraeusserungsform: angemeldete,
            bestehende_veraeusserungsform: Some(bestehende),
            nicht_eeg_kwkg: false,
            ausfallverguetung: false,
        });
        a
    }

    fn make_grid() -> MaloGridRecord {
        MaloGridRecord {
            malo_id: "51238696012".to_owned(),
            nb_mp_id: "9900000000002".to_owned(),
            bilanzierungsgebiet: Some("11YB-TENNET-----W".to_owned()),
            netzgebiet: None,
        }
    }

    fn make_versorgung(
        status: LieferStatus,
        lf_mp_id: Option<String>,
        lf_mp_id_next: Option<String>,
    ) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: "51238696012".parse().unwrap(),
            lieferstatus: status,
            lf_mp_id,
            lf_mp_id_next,
            lf_next_lieferbeginn: None,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: "9900000000002".to_owned(),
            eog_seit: None,
            last_process_id: None,
            updated_at: OffsetDateTime::now_utc(),
            tenant: "9900000000002".to_owned(),
            version: 1,
        }
    }

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).unwrap()
    }

    fn cfg() -> NetzCheckConfig {
        NetzCheckConfig::default()
    }

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    // 2026-07-08 10:00 UTC → Berlin date Wed 2026-07-08.
    const NOW: OffsetDateTime = datetime!(2026-07-08 10:00 UTC);
    // Friday receipt for weekend-crossing cases.
    const NOW_FRIDAY: OffsetDateTime = datetime!(2026-07-10 10:00 UTC);

    fn run(
        anfrage: &AnmeldungAnfrage,
        vs: &VersorgungsStatusRecord,
        now: OffsetDateTime,
    ) -> NbEntscheidung {
        evaluate(anfrage, Some(vs), Some(&make_grid()), true, now, &cfg())
    }

    // ── Strom LFW24 date rule (Prüfschritt 15, A07) ──────────────────────────

    #[test]
    fn strom_accept_full_werktag_between() {
        // ÜT Wed 07-08 → D Fri 07-10: Thu 07-09 lies between. Accept.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    /// A Bestätigung is not the absence of a code: `SG4 STS+E01` is Muss, and
    /// the code comes from `E_0623`, not from the Vorprüfung `E_0622`.
    #[test]
    fn the_bestaetigung_states_a51_from_e0623() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("A51"));
        assert_eq!(result.ebd(), Some("E_0623"));
    }

    #[test]
    fn strom_reject_next_day() {
        // ÜT Wed → D Thu: no Werktag strictly between → A07.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 9));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("A07"), "got {result:?}");
        assert_eq!(result.ebd(), Some("E_0622"));
    }

    #[test]
    fn strom_reject_past_date_even_for_einzug() {
        // LFW24 abolished retroactive Anmeldungen for ALL Transaktionsgründe.
        let mut anfrage = make_anfrage(55001, d(2026, Month::July, 7));
        anfrage.transaktionsgrund = Some("E01".to_owned());
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        assert_eq!(run(&anfrage, &vs, NOW).antwortcode(), Some("A07"));
    }

    #[test]
    fn strom_reject_today() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 8));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        assert_eq!(run(&anfrage, &vs, NOW).antwortcode(), Some("A07"));
    }

    #[test]
    fn strom_weekend_pushes_earliest_start() {
        // ÜT Fri 07-10 → D Mon 07-13: only Sat/Sun between → A07.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 13));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        assert_eq!(run(&anfrage, &vs, NOW_FRIDAY).antwortcode(), Some("A07"));

        // D Tue 07-14: Mon 07-13 lies between → Accept.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 14));
        assert!(run(&anfrage, &vs, NOW_FRIDAY).is_accept());
    }

    #[test]
    fn strom_rule_is_messtyp_independent() {
        let mut anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        anfrage.messtyp = Messtyp::Rlm;
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        assert!(run(&anfrage, &vs, NOW).is_accept());
    }

    // ── Prüfschritt 30 — Marktkommunikationsteilnahme ────────────────────────

    #[test]
    fn stillgelegt_malo_rejected_a02() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Stillgelegt, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("A02"), "got {result:?}");
    }

    /// A ruhende Marktlokation **is** an Anmeldung subject: `E_0622`
    /// Prüfschritt 10 routes „verbrauchende oder ruhende" down the same branch
    /// and Prüfschritt 30's Hinweis names only stillgelegte Marktlokationen and
    /// the Modell-2-Zuordnung. Refusing it A02 refused the Anwendungsfall the
    /// tree's Prüfschritte 16–28 exist to check.
    #[test]
    fn a_ruhende_malo_is_not_rejected_a02() {
        let mut anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        anfrage.marktlokationsart = Marktlokationsart::Ruhend;
        let vs = make_versorgung(LieferStatus::Ruhend, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── Prüfschritt 70 / 270 — andere Anmeldung in Bearbeitung ───────────────

    #[test]
    fn reject_conflicting_supply() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(
            LieferStatus::Beliefert,
            Some("9900111111111".to_owned()),
            Some("9900222222222".to_owned()),
        );
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("A06"), "got {result:?}");
    }

    #[test]
    fn reject_same_lf_already_active() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(
            LieferStatus::Beliefert,
            Some("9900357000004".to_owned()),
            None,
        );
        assert_eq!(run(&anfrage, &vs, NOW).antwortcode(), Some("A06"));
    }

    /// The *same condition* on an erzeugende Marktlokation is `A45`. `A06` is
    /// not in that branch, and sending it would be an undefined code.
    #[test]
    fn the_erzeugende_branch_answers_a45_not_a06() {
        let anfrage = make_erz_anfrage(
            d(2026, Month::September, 1),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::Marktpraemie,
            Veraeusserungsform::Marktpraemie,
        );
        let vs = make_versorgung(
            LieferStatus::Beliefert,
            Some("9900111111111".to_owned()),
            Some("9900222222222".to_owned()),
        );
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("A45"), "got {result:?}");
    }

    #[test]
    fn reject_unknown_lf_a05() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), false, NOW, &cfg());
        assert_eq!(result.antwortcode(), Some("A05"), "got {result:?}");
    }

    // ── Erzeugende Marktlokation — the six Vorlauffristen ────────────────────

    /// Without the Geschäftsvorfall and the Veräußerungsformen there is no way
    /// to pick between six published Fristen — escalate rather than guess.
    #[test]
    fn an_erzeugende_anmeldung_without_facts_escalates() {
        let mut anfrage = make_anfrage(55_077, d(2026, Month::September, 1));
        anfrage.marktlokationsart = Marktlokationsart::Erzeugend;
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        assert!(run(&anfrage, &vs, NOW).is_escalate());
    }

    /// GV 1, Veräußerungsformwechsel sonstige DV → Marktprämie: Monatserster
    /// plus one month of lead. 2026-09-01 with receipt 2026-07-08 clears both.
    #[test]
    fn gv1_veraeusserungsformwechsel_accepts_a_monatserster_a_month_ahead() {
        let anfrage = make_erz_anfrage(
            d(2026, Month::September, 1),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::Marktpraemie,
        );
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert!(result.is_accept(), "got {result:?}");
        assert_eq!(result.antwortcode(), Some("A58"));
    }

    #[test]
    fn gv1_veraeusserungsformwechsel_rejects_a_mid_month_start_a27() {
        let anfrage = make_erz_anfrage(
            d(2026, Month::September, 15),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::Marktpraemie,
        );
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("A27"), "got {result:?}");
    }

    #[test]
    fn gv1_veraeusserungsformwechsel_rejects_a_short_lead_a28() {
        // Receipt 2026-07-08, Zuordnungsbeginn 2026-08-01 → latest ÜT 2026-07-01.
        let anfrage = make_erz_anfrage(
            d(2026, Month::August, 1),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::Marktpraemie,
        );
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("A28"), "got {result:?}");
    }

    /// A plant leaving the Ausfallvergütung takes the verkürzte 5-WT Frist, so
    /// the same short lead that produces `A28` above is admissible here.
    #[test]
    fn gv1_ausfallverguetung_takes_the_five_werktage_frist() {
        let mut anfrage = make_erz_anfrage(
            d(2026, Month::August, 1),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::Einspeiseverguetung,
            Veraeusserungsform::Marktpraemie,
        );
        anfrage.erzeugung.as_mut().unwrap().ausfallverguetung = true;
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        assert!(run(&anfrage, &vs, NOW).is_accept());
    }

    #[test]
    fn gv1_ausfallverguetung_rejects_a_late_ut_a29() {
        // Zuordnungsbeginn Mon 2026-08-03; 5 WT before is Mon 2026-07-27.
        let mut anfrage = make_erz_anfrage(
            d(2026, Month::August, 1),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::Einspeiseverguetung,
            Veraeusserungsform::Marktpraemie,
        );
        anfrage.erzeugung.as_mut().unwrap().ausfallverguetung = true;
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        // Receipt 2026-07-30 is inside the 5-WT window before 2026-08-01.
        let late = datetime!(2026-07-30 10:00 UTC);
        let result = run(&anfrage, &vs, late);
        assert_eq!(result.antwortcode(), Some("A29"), "got {result:?}");
    }

    /// GV 2 has its own codes for the same two questions.
    #[test]
    fn gv2_uses_a31_and_a32() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mid = make_erz_anfrage(
            d(2026, Month::September, 15),
            Geschaeftsvorfall::Zwei,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::Marktpraemie,
        );
        assert_eq!(run(&mid, &vs, NOW).antwortcode(), Some("A31"));

        let short = make_erz_anfrage(
            d(2026, Month::August, 1),
            Geschaeftsvorfall::Zwei,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::Marktpraemie,
        );
        assert_eq!(run(&short, &vs, NOW).antwortcode(), Some("A32"));
    }

    /// GV 3 has no Monatserster refusal — a mid-month start is checked against
    /// the one-month Vorlauffrist and answered `A44` when it is too late.
    #[test]
    fn gv3_has_no_monatserster_refusal_and_answers_a44() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let ok = make_erz_anfrage(
            d(2026, Month::September, 15),
            Geschaeftsvorfall::Drei,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::Marktpraemie,
        );
        assert!(run(&ok, &vs, NOW).is_accept(), "mid-month is fine in GV 3");

        let late = make_erz_anfrage(
            d(2026, Month::July, 20),
            Geschaeftsvorfall::Drei,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::Marktpraemie,
        );
        assert_eq!(run(&late, &vs, NOW).antwortcode(), Some("A44"));
    }

    /// A „Nicht-EEG-/-KWKG"-Marktlokation without a Veräußerungsformwechsel
    /// keeps the ordinary Werktag rule, and its refusal is `A34` in GV 1.
    #[test]
    fn a_nicht_eeg_malo_keeps_the_werktag_rule() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_erz_anfrage(
            d(2026, Month::July, 10),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::SonstigeDirektvermarktung,
            Veraeusserungsform::SonstigeDirektvermarktung,
        );
        anfrage.erzeugung.as_mut().unwrap().nicht_eeg_kwkg = true;
        assert!(run(&anfrage, &vs, NOW).is_accept(), "one full WT between");

        anfrage.process_date = d(2026, Month::July, 9);
        assert_eq!(run(&anfrage, &vs, NOW).antwortcode(), Some("A34"));
    }

    /// Without the bestehende Veräußerungsform, `E_0622` Prüfschritt 400 / 600
    /// cannot be answered — and the Monatserster check behind it would refuse an
    /// untermonatlichen Wechsel that GPKE Teil 2 § 2.1.1 permits.
    #[test]
    fn an_unknown_bestehende_veraeusserungsform_escalates_in_gv1_and_gv2() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        for gv in [Geschaeftsvorfall::Eins, Geschaeftsvorfall::Zwei] {
            let mut anfrage = make_erz_anfrage(
                d(2026, Month::September, 15),
                gv,
                Veraeusserungsform::Marktpraemie,
                Veraeusserungsform::Marktpraemie,
            );
            anfrage
                .erzeugung
                .as_mut()
                .unwrap()
                .bestehende_veraeusserungsform = None;
            let result = run(&anfrage, &vs, NOW);
            assert!(result.is_escalate(), "{gv:?}: got {result:?}");
        }
    }

    /// Geschäftsvorfall 3 never asks the Veräußerungsformwechsel question, so it
    /// decides without the register.
    #[test]
    fn gv3_decides_without_the_bestehende_veraeusserungsform() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_erz_anfrage(
            d(2026, Month::September, 15),
            Geschaeftsvorfall::Drei,
            Veraeusserungsform::Marktpraemie,
            Veraeusserungsform::Marktpraemie,
        );
        anfrage
            .erzeugung
            .as_mut()
            .unwrap()
            .bestehende_veraeusserungsform = None;
        assert!(run(&anfrage, &vs, NOW).is_accept());
    }

    /// The contested case: no Veräußerungsformwechsel, EEG plant, untermonatlich.
    /// GPKE Teil 2's Fristentabelle permits it; `E_0622` Prüfschritt 410 would
    /// refuse it `A27`. mako escalates rather than auto-rejecting.
    #[test]
    fn an_untermonatlicher_wechsel_within_the_marktpraemie_escalates() {
        let anfrage = make_erz_anfrage(
            d(2026, Month::September, 15),
            Geschaeftsvorfall::Eins,
            Veraeusserungsform::Marktpraemie,
            Veraeusserungsform::Marktpraemie,
        );
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = run(&anfrage, &vs, NOW);
        assert!(result.is_escalate(), "got {result:?}");
    }

    // ── Gas — G_0011, a different alphabet ───────────────────────────────────

    #[test]
    fn gas_wechsel_needs_ten_werktage() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E03"), d(2026, Month::July, 15));
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("E17"), "got {result:?}");

        let anfrage = make_gas_anfrage(Some("E03"), d(2026, Month::July, 24));
        assert!(run(&anfrage, &vs, NOW).is_accept());
    }

    #[test]
    fn gas_slp_einzug_may_be_backdated_six_weeks() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::June, 15));
        assert!(run(&anfrage, &vs, NOW).is_accept());

        let anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::April, 1));
        assert_eq!(run(&anfrage, &vs, NOW).antwortcode(), Some("E17"));
    }

    #[test]
    fn gas_rlm_may_not_be_backdated() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::June, 15));
        anfrage.messtyp = Messtyp::Rlm;
        assert_eq!(run(&anfrage, &vs, NOW).antwortcode(), Some("E17"));
    }

    #[test]
    fn gas_backdated_without_transaktionsgrund_escalates() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(None, d(2026, Month::June, 15));
        assert!(run(&anfrage, &vs, NOW).is_escalate());
    }

    /// Every Gas refusal must come from `G_0011`. The Strom codes are not
    /// defined there, so a 44003 carrying `A02` / `A05` / `A06` is a message
    /// the counterparty cannot interpret.
    #[test]
    fn gas_never_answers_with_a_strom_code() {
        let vs_still = make_versorgung(LieferStatus::Stillgelegt, None, None);
        let anfrage = make_gas_anfrage(Some("E03"), d(2026, Month::August, 1));
        let result = run(&anfrage, &vs_still, NOW);
        assert_eq!(result.antwortcode(), Some("A16"), "not A02");

        let vs_conflict = make_versorgung(
            LieferStatus::Beliefert,
            Some("9900111111111".to_owned()),
            Some("9900222222222".to_owned()),
        );
        let result = run(&anfrage, &vs_conflict, NOW);
        assert_eq!(result.antwortcode(), Some("ZC5"), "not A06");

        let mut grid = make_grid();
        grid.bilanzierungsgebiet = Some("11YB-OTHER------W".to_owned());
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW, &cfg());
        assert_eq!(result.antwortcode(), Some("E13"), "not A05");
    }

    #[test]
    fn the_gas_bestaetigung_states_e15() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E03"), d(2026, Month::August, 1));
        let result = run(&anfrage, &vs, NOW);
        assert_eq!(result.antwortcode(), Some("E15"));
        // The Gas Codelisten are not named in STS DE 1131.
        assert_eq!(result.ebd(), None);
    }

    // ── Escalation on a missing grid record ──────────────────────────────────

    #[test]
    fn missing_grid_record_escalates() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), None, true, NOW, &cfg());
        assert!(result.is_escalate());
    }

    // ── Date helpers ─────────────────────────────────────────────────────────

    #[test]
    fn werktag_between_examples_from_gpke() {
        // Wed → Fri: Thu lies between.
        assert!(has_werktag_strictly_between(
            d(2026, Month::July, 8),
            d(2026, Month::July, 10),
            CAL
        ));
        // Wed → Thu: nothing between.
        assert!(!has_werktag_strictly_between(
            d(2026, Month::July, 8),
            d(2026, Month::July, 9),
            CAL
        ));
        // Fri → Mon: only the weekend between.
        assert!(!has_werktag_strictly_between(
            d(2026, Month::July, 10),
            d(2026, Month::July, 13),
            CAL
        ));
    }

    #[test]
    fn months_before_clamps_to_the_month_length() {
        assert_eq!(
            months_before(d(2026, Month::March, 31), 1),
            d(2026, Month::February, 28)
        );
        assert_eq!(
            months_before(d(2026, Month::January, 15), 1),
            d(2025, Month::December, 15)
        );
    }

    #[test]
    fn werktage_before_skips_weekends() {
        // Mon 2026-08-03 minus 1 WT is Fri 2026-07-31.
        assert_eq!(
            werktage_before(d(2026, Month::August, 3), 1, CAL),
            d(2026, Month::July, 31)
        );
    }

    #[test]
    fn months_before_rolls_the_year() {
        assert_eq!(
            months_before(d(2027, Month::January, 20), 1),
            d(2026, Month::December, 20)
        );
    }
}
