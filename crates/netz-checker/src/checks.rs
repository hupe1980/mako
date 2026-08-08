//! The six deterministic NB Anmeldung validation checks.
//!
//! All checks are **pure functions** — no I/O, no global state, no clock calls.
//! The current instant is always passed as a parameter.
//!
//! ## Check sequence (aligned with EBD E_0622 "Prüfen, ob Anmeldung direkt
//! ablehnbar" / GeLi Gas codeliste G_0011)
//!
//! | # | Rule | Reject code | Escalate? |
//! |---|------|-------------|-----------|
//! | 1 | Grid record present (`MaloGridRecord` is `Some`) | — | ✓ missing data |
//! | 2 | MaLo participates in market communication (not Stillgelegt/Ruhend) | A02 | |
//! | 3 | No conflicting active supply (`lf_mp_id_next` is `None`) | A06 | |
//! | 4 | Date plausibility (Strom: LFW24 future rule; Gas: Transaktionsgrund-aware incl. 6-week retroactive window) | A07 (Strom) / E17 (Gas) | ✓ Gas backdated without Transaktionsgrund |
//! | 5 | Bilanzierungsgebiet matches grid record (when both are present) | A05 | ✓ grid record incomplete |
//! | 6 | LF GLN is in the partner directory (`partner_known = true`) | A05 | |
//!
//! Checks run in order; the first failure short-circuits and returns the result.
//!
//! ## Date plausibility (check 4) — the normative rules
//!
//! **Strom (GPKE per BK6-24-174, Teil 2 SD Lieferbeginn Prozessschritt 1):**
//! „Unverzüglich nach Vorliegen des Anmeldegrundes, jedoch spätester ÜT ist
//! der Tag vor dem letzten WT vor dem Zuordnungsbeginn." Retroactive
//! Anmeldungen were **abolished with LFW24 for every Transaktionsgrund**
//! (E01 Ein-/Auszug and E03 Wechsel follow the same rule; the pre-LFW24
//! 6-week Ein-/Auszug window no longer exists). Operationally: the
//! Zuordnungsbeginn is valid iff at least one full Werktag lies strictly
//! between the receipt day and the Zuordnungsbeginn. There is **no
//! time-of-day cutoff**. Violation → **A07** „Vorlauffrist wurde nicht
//! eingehalten" (EBD 4.2 E_0622 Prüfschritt 15). The only retroactive
//! Strom assignment is the NB-initiated Ersatz-/Grundversorgung
//! (PID 55013, `mako-gpke::eog`) — not this check's scope.
//!
//! **Strom EEG-/KWKG-MaLo Zuordnung (§10c EEG):** when the Anmeldung carries the
//! `ZW3` „Erzeugende Marktlokation" Transaktionsgrundergänzung (surfaced as
//! [`AnmeldungAnfrage::ist_erzeugende_marktlokation`](crate::AnmeldungAnfrage))
//! the Zuordnung of an Einspeise-MaLo to a Bilanzkreis is a monatsscharfer
//! Prozess: the
//! Zuordnungsbeginn must be a **Monatserster** and lie at least one whole month
//! ahead (configurable). Violation → **A07**.
//!
//! **Werktag arithmetic** uses the BDEW-MaKo holiday calendar
//! (`mako-engine::fristen`, injected via [`NetzCheckConfig::holiday_calendar`]),
//! not a bare Mon–Fri approximation.
//!
//! **Gas (AWH GeLi Gas 2.0 V1.2 Kap. 2.2, in force since 01.04.2026):**
//! - Lieferantenwechsel (**E03**): future-only, Anmeldung ≥ **10 WT** before
//!   the Lieferbeginn. Violation → **E17** „Ablehnung wg. Fristüberschreitung".
//! - Non-Wechsel (**E01** Ein-/Auszug, **E02** Einzug in Neuanlage) with
//!   SLP metering (kME/nME): **retroactive registrations are permitted** up
//!   to **6 weeks + Bearbeitungsfrist (3 WT)** before receipt. Beyond the
//!   window → **E17**.
//! - Non-Wechsel with RLM (hourly balancing) or SMGW-attached metering:
//!   dates must lie strictly after receipt (no retroactivity).
//! - Backdated Anmeldung without a readable Transaktionsgrund → **Escalate**
//!   (never auto-reject a potentially lawful move-in — §20 EnWG).
//!
//! ## Regulatory sources
//!
//! - GPKE Teil 2 (BK6-24-174) SD Lieferbeginn; UTILMD AHB Strom 2.1
//! - EBD 4.2 E_0622 (A02/A05/A06/A07 semantics)
//! - AWH GeLi Gas 2.0 V1.2 (26.03.2026) Kap. 2.2; EBD 4.2 Kap. 13.6 G_0011 (E17)
//!
//! Note: `A97` is **not** a date code (it was the pre-LFW24 "AHB-Prüfung"
//! result code, deleted in EBD 4.x), and `A99` „Sonstiges" ends 01.10.2026 —
//! neither is used here.

use time::{Date, Duration, OffsetDateTime};
use time_tz::{OffsetDateTimeExt, timezones};

use mako_engine::fristen::{self, HolidayCalendar};
use mako_markt::domain::Sparte;
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};

use crate::config::NetzCheckConfig;
use crate::types::{AnmeldungAnfrage, MaloGridRecord, Messtyp, NetzCheckResult, RejectReason};

// ── Regulatory constants ──────────────────────────────────────────────────────

/// Gas Lieferantenwechsel (E03): minimum lead, Anmeldung → Lieferbeginn
/// (AWH GeLi Gas 2.0 V1.2, SD Lieferbeginn Prozessschritt 1).
pub const GAS_WECHSEL_VORLAUF_WT: u32 = 10;

/// Gas non-Wechsel retroactive window: 6 weeks (AWH GeLi Gas 2.0 Kap. 2.2
/// Grundregel 3a — „bis zu sechs Wochen zzgl. einer zu berücksichtigenden
/// Bearbeitungsfrist nach An- oder Abmeldedatum").
pub const GAS_RUECKWIRKUNG_WOCHEN: i64 = 6;

/// Default Gas Bearbeitungsfrist (in Werktage) added to the 6-week window.
/// The AWH quantifies it only for the Ersatz-/Grundversorgung (3 WT); the same
/// default is applied to An-/Abmeldungen. Operators may override it via
/// [`NetzCheckConfig::gas_bearbeitungsfrist_wt`].
pub const GAS_BEARBEITUNGSFRIST_WT_DEFAULT: u32 = 3;

/// Default EEG-MaLo Zuordnungs-Vorlauf, in whole months (§10c EEG). Override
/// via [`NetzCheckConfig::eeg_zuordnung_vorlauf_monate`].
pub const EEG_ZUORDNUNG_VORLAUF_MONATE_DEFAULT: u32 = 1;

/// The UTILMD SG4 STS Transaktionsgrundergänzung (DE9013) that marks an
/// **Erzeugende Marktlokation** (EEG-/KWKG-Einspeise-MaLo), verified against
/// `UTILMD_AHB_Strom_2.2` Kap. 3 (Codeliste DE9013).
///
/// **Correction (2026-07):** the previous `["A27".."A32"]` were placeholders
/// that do not exist in the UTILMD AHB — DE9013 uses `E`/`Z`-codes only, so the
/// EEG Monatserster rule never fired. The real signal is `ZW3`; it arrives as a
/// *second* STS+7 (Transaktionsgrundergänzung) alongside the main Anmeldegrund
/// (`E01`/`E03`), which the `makod` adapter surfaces as
/// [`AnmeldungAnfrage::ist_erzeugende_marktlokation`](crate::AnmeldungAnfrage).
pub const EEG_ERZEUGENDE_MARKTLOKATION_CODE: &str = "ZW3";

// ── Berlin timezone helper ────────────────────────────────────────────────────

/// Current calendar date in Germany (CET/CEST).
///
/// All deadline arithmetic uses German local time.  An off-by-one-hour error
/// at DST transitions would be a regulatory deadline violation.
#[must_use]
fn today_berlin(now: OffsetDateTime) -> Date {
    let berlin = timezones::db::europe::BERLIN;
    now.to_timezone(berlin).date()
}

// ── Werktag helpers ───────────────────────────────────────────────────────────
//
// The crate stays pure and clock-free, but Werktag arithmetic now delegates to
// the real BDEW-MaKo holiday calendar in `mako-engine::fristen` (injected via
// `NetzCheckConfig::holiday_calendar`). The former Mon–Fri approximation
// silently accepted dates the exact calendar would push past a Feiertag; using
// the BDEW calendar closes that gap while keeping the §20 EnWG guarantee that a
// widening of the window can never turn an Accept into an auto-reject.

/// `true` when at least one Werktag lies **strictly between** `a` and `b`.
///
/// This is the operational form of the LFW24 rule „spätester ÜT ist der Tag
/// vor dem letzten WT vor dem Zuordnungsbeginn": an Anmeldung received on
/// day `a` may carry Zuordnungsbeginn `b` iff a full Werktag separates them.
fn has_werktag_strictly_between(a: Date, b: Date, cal: HolidayCalendar) -> bool {
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

// ── Date plausibility (check 4) ───────────────────────────────────────────────

/// Strom date rule. Dispatches to the EEG-MaLo Monatserster rule when the
/// Transaktionsgrund marks an EEG-/KWKG-Zuordnung, otherwise applies the LFW24
/// Vorlauffrist rule. `None` = valid. See module docs.
fn check_date_strom(
    anfrage: &AnmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NetzCheckResult> {
    if anfrage.ist_erzeugende_marktlokation {
        return check_date_eeg(anfrage, today, config);
    }

    if has_werktag_strictly_between(today, anfrage.process_date, config.holiday_calendar) {
        return None;
    }
    Some(NetzCheckResult::Reject(RejectReason {
        erc_code: "A07".to_owned(),
        detail: format!(
            "Vorlauffrist not met: requested Zuordnungsbeginn {} vs receipt day {}. \
             GPKE (BK6-24-174, Teil 2 SD Lieferbeginn): spätester ÜT ist der Tag \
             vor dem letzten WT vor dem Zuordnungsbeginn — at least one full \
             Werktag must lie between receipt and Lieferbeginn; retroactive \
             Anmeldungen are not permitted under LFW24 for any Transaktionsgrund \
             (EBD E_0622 Prüfschritt 15 → A07).",
            anfrage.process_date, today
        ),
        check_number: 4,
    }))
}

/// First day of the month `months` whole months after `d`'s month.
fn first_of_month_after(d: Date, months: u32) -> Date {
    let mut year = d.year();
    let mut month = d.month();
    for _ in 0..months {
        month = month.next();
        if month == time::Month::January {
            year += 1;
        }
    }
    Date::from_calendar_date(year, month, 1).expect("day 1 is always valid")
}

/// EEG-/KWKG-MaLo Zuordnung date rule (§10c EEG): the Zuordnungsbeginn must be
/// the **first of a month** and lie at least `eeg_zuordnung_vorlauf_monate`
/// whole months ahead of receipt. `None` = valid.
///
/// The assignment of an EEG-Einspeise-MaLo to a Bilanzkreis is a
/// monatsscharfer Prozess: mid-month starts are impossible and the NB needs a
/// full month of lead to arrange the bilanzielle Zuordnung. A violation is a
/// Vorlauffrist/Fristverletzung → **A07** (the same Fristcode the GPKE date
/// tree uses; the detail names the EEG specifics for the operator audit log).
fn check_date_eeg(
    anfrage: &AnmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NetzCheckResult> {
    let d = anfrage.process_date;
    let grund = anfrage.transaktionsgrund.as_deref().unwrap_or("");

    // Rule 1: must be a Monatserster.
    if d.day() != 1 {
        return Some(NetzCheckResult::Reject(RejectReason {
            erc_code: "A07".to_owned(),
            detail: format!(
                "EEG-MaLo Zuordnung (Transaktionsgrund {grund}) requested for {d}, \
                 which is not a Monatserster. The bilanzielle Zuordnung einer \
                 EEG-/KWKG-Marktlokation is only possible zum Monatsersten \
                 (§10c EEG; UTILMD AHB Strom).",
            ),
            check_number: 4,
        }));
    }

    // Rule 2: at least N whole months of lead — earliest valid start is the
    // first of the month N months after the receipt month.
    let earliest = first_of_month_after(today, config.eeg_zuordnung_vorlauf_monate);
    if d >= earliest {
        None
    } else {
        Some(NetzCheckResult::Reject(RejectReason {
            erc_code: "A07".to_owned(),
            detail: format!(
                "Vorlauffrist not met: EEG-MaLo Zuordnung (Transaktionsgrund \
                 {grund}) requested for {d}, but the earliest admissible \
                 Monatserster is {earliest} — at least {} month(s) lead is \
                 required (§10c EEG; UTILMD AHB Strom). Receipt {today}.",
                config.eeg_zuordnung_vorlauf_monate
            ),
            check_number: 4,
        }))
    }
}

/// Gas date rule (Transaktionsgrund-aware) — see module docs. `None` = valid.
fn check_date_gas(
    anfrage: &AnmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NetzCheckResult> {
    let cal = config.holiday_calendar;
    let d = anfrage.process_date;
    match anfrage.transaktionsgrund.as_deref() {
        // Lieferantenwechsel: future-only, ≥ 10 WT lead.
        Some("E03") => {
            let earliest = add_werktage(today, GAS_WECHSEL_VORLAUF_WT, cal);
            if d >= earliest {
                None
            } else {
                Some(NetzCheckResult::Reject(RejectReason {
                    erc_code: "E17".to_owned(),
                    detail: format!(
                        "Fristüberschreitung: Gas Lieferantenwechsel (E03) requires \
                         ≥ {GAS_WECHSEL_VORLAUF_WT} WT lead — requested Lieferbeginn {d}, \
                         earliest {earliest} (receipt {today}). Wechsel sind nur in die \
                         Zukunft gerichtet möglich (AWH GeLi Gas 2.0 Kap. 2.2; G_0011 E17).",
                    ),
                    check_number: 4,
                }))
            }
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
                    if today <= window_end {
                        None // lawful retroactive move-in/move-out
                    } else {
                        Some(NetzCheckResult::Reject(RejectReason {
                            erc_code: "E17".to_owned(),
                            detail: format!(
                                "Fristüberschreitung: retroactive Gas Anmeldung {d} is \
                                 outside the 6-week window (+{bearbeitungsfrist} WT \
                                 Bearbeitungsfrist, ends {window_end}; receipt {today}). \
                                 Lieferbeginn kann nur noch für die Zukunft realisiert \
                                 werden (AWH GeLi Gas 2.0 Kap. 2.2 Grundregel 3b).",
                            ),
                            check_number: 4,
                        }))
                    }
                }
                // Hourly-balanced (RLM) and SMGW-attached metering: dates may
                // only lie after the receipt date (Grundregel 2).
                Messtyp::Rlm | Messtyp::Imsys => Some(NetzCheckResult::Reject(RejectReason {
                    erc_code: "E17".to_owned(),
                    detail: format!(
                        "Fristüberschreitung: retroactive Gas Anmeldung {d} is not \
                         permitted for {} metering — An- und Abmeldedatum können nur \
                         nach dem Eingangsdatum liegen (AWH GeLi Gas 2.0 Kap. 2.2 \
                         Grundregel 2).",
                        anfrage.messtyp
                    ),
                    check_number: 4,
                })),
            }
        }
        // No readable Transaktionsgrund: never auto-reject a backdated
        // Anmeldung that may be a lawful move-in (§20 EnWG) — escalate.
        None => {
            if d >= today {
                None
            } else {
                Some(NetzCheckResult::Escalate {
                    reason: format!(
                        "Backdated Gas Anmeldung ({d}, receipt {today}) without a \
                         readable SG4 STS Transaktionsgrund — cannot distinguish a \
                         lawful retroactive Ein-/Auszug (6-week window) from an \
                         impermissible retroactive Wechsel. Operator review required.",
                    ),
                })
            }
        }
    }
}

// ── Evaluate ──────────────────────────────────────────────────────────────────

/// Run all six NB Anmeldung checks and return a single decision.
///
/// # Parameters
///
/// - `anfrage` — parsed fields from the `de.mako.process.initiated` CloudEvent.
/// - `versorgung` — current supply state from `GET /api/v1/versorgung/{malo_id}`
///   on `marktd`.  `None` if `marktd` returned 404 (MaLo unknown).
/// - `grid` — NB grid topology record from `GET /api/v1/malo/{id}/grid` on
///   `marktd`.  `None` when the NB's NIS/GIS data has not yet been imported.
/// - `partner_known` — `true` if the requesting LF GLN is in the operator's
///   partner directory (`GET /api/v1/partners/{mp_id}` returned 200).
/// - `now` — current UTC instant (injected by caller for testability).
/// - `config` — tunables (holiday calendar, Gas Bearbeitungsfrist, EEG lead).
///   Use [`NetzCheckConfig::default`] for the regulatory defaults.
///
/// # Returns
///
/// [`NetzCheckResult::Accept`] — all checks passed; auto-accept is permissible.
/// [`NetzCheckResult::Reject`] — a deterministic rule failed; dispatch `ablehnen`.
/// [`NetzCheckResult::Escalate`] — data is insufficient; alert the operator.
///
/// # Notes
///
/// - This function is **synchronous** and **infallible** — it never panics and
///   never returns an `Err`.
/// - The caller is responsible for retrying after a `marktd` connectivity error
///   rather than calling `evaluate` with incomplete data.
#[must_use]
#[allow(clippy::too_many_lines)] // 6 regulatory checks are inherently verbose
pub fn evaluate(
    anfrage: &AnmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    grid: Option<&MaloGridRecord>,
    partner_known: bool,
    now: OffsetDateTime,
    config: &NetzCheckConfig,
) -> NetzCheckResult {
    // ── Check 1: Grid record present ─────────────────────────────────────────
    let Some(grid) = grid else {
        return NetzCheckResult::Escalate {
            reason: format!(
                "No grid record found for MaLo {} in the NB's grid topology. \
                 Import NIS/GIS data or provision the record manually via \
                 PUT /api/v1/malo/{}/grid.",
                anfrage.malo_id, anfrage.malo_id
            ),
        };
    };

    // ── Check 2: MaLo participates in market communication ───────────────────
    //
    // A stillgelegte MaLo takes no further part in MaKo; a ruhende MaLo
    // (Kundenanlagen-Modell 2, § 20 Abs. 1d EnWG) likewise. EBD E_0622
    // Prüfschritt 30 → A02 „Marktlokation nimmt nicht an der
    // Marktkommunikation teil".
    if let Some(vs) = versorgung
        && matches!(
            vs.lieferstatus,
            LieferStatus::Stillgelegt | LieferStatus::Ruhend
        )
    {
        return NetzCheckResult::Reject(RejectReason {
            erc_code: "A02".to_owned(),
            detail: format!(
                "MaLo {} does not participate in market communication \
                 (lieferstatus = {:?}). EBD E_0622 → A02.",
                anfrage.malo_id, vs.lieferstatus
            ),
            check_number: 2,
        });
    }

    // ── Check 3: No conflicting active supply ─────────────────────────────────
    //
    // If `lf_mp_id_next` is already set by a *different* LF, another Anmeldung
    // is in Bearbeitung for this MaLo. EBD E_0622 Prüfschritt 70 → A06
    // „Andere Anmeldung in Bearbeitung".
    //
    // The GLN comparison is load-bearing, not a refinement: `marktd`'s
    // `event_ingest` calls `announce_lf_next` while ingesting the
    // `process.initiated` CloudEvent — i.e. *before* it fans the event out to
    // `processd`. By the time this check runs, the Anmeldung under evaluation
    // has already written its own `lf_mp_id_next`. A bare `is_some()` test
    // therefore rejects every first-time Anmeldung against itself with A06.
    if let Some(vs) = versorgung {
        if vs
            .lf_mp_id_next
            .as_deref()
            .is_some_and(|next| next != anfrage.new_supplier_gln.as_str())
        {
            return NetzCheckResult::Reject(RejectReason {
                erc_code: "A06".to_owned(),
                detail: format!(
                    "MaLo {} already has a pending Lieferbeginn (lf_mp_id_next = {:?}). \
                     Andere Anmeldung in Bearbeitung (EBD E_0622 → A06).",
                    anfrage.malo_id, vs.lf_mp_id_next
                ),
                check_number: 3,
            });
        }

        // When the MaLo is already Beliefert by the same LF, it's also a
        // duplicate — reject with A06.
        if vs.lieferstatus == LieferStatus::Beliefert
            && vs.lf_mp_id.as_deref() == Some(anfrage.new_supplier_gln.as_str())
        {
            return NetzCheckResult::Reject(RejectReason {
                erc_code: "A06".to_owned(),
                detail: format!(
                    "MaLo {} is already supplied by LF {} (duplicate Anmeldung).",
                    anfrage.malo_id, anfrage.new_supplier_gln
                ),
                check_number: 3,
            });
        }
    }

    // ── Check 4: Date plausibility (Transaktionsgrund-aware) ─────────────────
    {
        let today = today_berlin(now);
        let violation = match anfrage.sparte {
            Sparte::Strom => check_date_strom(anfrage, today, *config),
            Sparte::Gas => check_date_gas(anfrage, today, *config),
        };
        if let Some(result) = violation {
            return result;
        }
    }

    // ── Check 5: Bilanzierungsgebiet consistent ───────────────────────────────
    //
    // When both the UTILMD message and the grid record carry a
    // Bilanzierungsgebiet, they must match. A mismatch means the Anmeldung's
    // balancing-group assignment cannot be fulfilled — EBD E_0622
    // Prüfschritt 60 → A05 „Anforderungen können nicht erfüllt werden"
    // (insb. Zuordnungsermächtigung Bilanzkreis/Bilanzierungsgebiet).
    if let (Some(req_big), Some(grid_big)) =
        (&anfrage.bilanzierungsgebiet, &grid.bilanzierungsgebiet)
        && req_big != grid_big
    {
        return NetzCheckResult::Reject(RejectReason {
            erc_code: "A05".to_owned(),
            detail: format!(
                "Bilanzierungsgebiet mismatch: UTILMD contains '{}' but grid \
                 record for MaLo {} has '{}'. Anforderungen können nicht erfüllt \
                 werden (EBD E_0622 → A05).",
                req_big, anfrage.malo_id, grid_big
            ),
            check_number: 5,
        });
    }

    // Also escalate when the UTILMD provides a Bilanzierungsgebiet but the grid
    // record has none — we cannot confirm the assertion.
    if anfrage.bilanzierungsgebiet.is_some() && grid.bilanzierungsgebiet.is_none() {
        return NetzCheckResult::Escalate {
            reason: format!(
                "UTILMD message provides Bilanzierungsgebiet {:?} but grid record \
                 for MaLo {} has no Bilanzierungsgebiet — cannot confirm consistency. \
                 Update the grid record via PUT /api/v1/malo/{}/grid.",
                anfrage.bilanzierungsgebiet, anfrage.malo_id, anfrage.malo_id
            ),
        };
    }

    // ── Check 6: LF known in partner directory ────────────────────────────────
    //
    // The requesting LF must be registered as a trading partner.  Without a
    // valid partner record, AS4 delivery of the response is impossible —
    // the Anmeldung's requirements cannot be fulfilled (A05).
    if !partner_known {
        return NetzCheckResult::Reject(RejectReason {
            erc_code: "A05".to_owned(),
            detail: format!(
                "LF GLN {} is not registered in the partner directory. \
                 The LF must publish their MP-ID at bdew-codes.de and register \
                 an AS4 channel before initiating a Lieferbeginn.",
                anfrage.new_supplier_gln
            ),
            check_number: 6,
        });
    }

    NetzCheckResult::Accept
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};
    use time::{Date, Month, OffsetDateTime, macros::datetime};
    use uuid::Uuid;

    use crate::types::Messtyp;
    use mako_markt::domain::Sparte;

    fn make_anfrage(pid: u32, process_date: Date) -> AnmeldungAnfrage {
        AnmeldungAnfrage {
            pid,
            process_id: Uuid::new_v4(),
            malo_id: "51238696780".to_owned(),
            new_supplier_gln: "9900357000004".to_owned(),
            grid_operator_gln: "9900000000002".to_owned(),
            bilanzierungsgebiet: Some("11YB-TENNET-----W".to_owned()),
            process_date,
            sparte: Sparte::Strom,
            messtyp: Messtyp::Slp,
            transaktionsgrund: Some("E03".to_owned()),
            ist_erzeugende_marktlokation: false,
        }
    }

    fn make_gas_anfrage(transaktionsgrund: Option<&str>, process_date: Date) -> AnmeldungAnfrage {
        let mut a = make_anfrage(44001, process_date);
        a.sparte = Sparte::Gas;
        a.transaktionsgrund = transaktionsgrund.map(ToOwned::to_owned);
        a
    }

    fn make_grid() -> MaloGridRecord {
        MaloGridRecord {
            malo_id: "51238696780".to_owned(),
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
            malo_id: "51238696780".parse().unwrap(),
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

    // 2026-07-08 10:00 UTC → today_berlin = Wed 2026-07-08.
    const NOW: OffsetDateTime = datetime!(2026-07-08 10:00 UTC);
    // Friday receipt for weekend-crossing cases.
    const NOW_FRIDAY: OffsetDateTime = datetime!(2026-07-10 10:00 UTC);

    // ── Strom LFW24 date rule (check 4, A07) ─────────────────────────────────

    #[test]
    fn strom_accept_full_werktag_between() {
        // ÜT Wed 07-08 → D Fri 07-10: Thu 07-09 lies between. Accept.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn strom_reject_next_day() {
        // ÜT Wed → D Thu: no Werktag strictly between → A07.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 9));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn strom_reject_past_date_even_for_einzug() {
        // LFW24 abolished retroactive Anmeldungen for ALL Transaktionsgründe —
        // an E01 (Ein-/Auszug) backdated Strom Anmeldung is rejected A07 too.
        let mut anfrage = make_anfrage(55001, d(2026, Month::July, 7));
        anfrage.transaktionsgrund = Some("E01".to_owned());
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn strom_reject_today() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 8));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn strom_weekend_pushes_earliest_start() {
        // ÜT Fri 07-10 → D Mon 07-13: only Sat/Sun between → A07.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 13));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(
            &anfrage,
            Some(&vs),
            Some(&make_grid()),
            true,
            NOW_FRIDAY,
            &cfg(),
        );
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");

        // D Tue 07-14: Mon 07-13 lies between → Accept.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 14));
        let result = evaluate(
            &anfrage,
            Some(&vs),
            Some(&make_grid()),
            true,
            NOW_FRIDAY,
            &cfg(),
        );
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn strom_rule_is_messtyp_independent() {
        // Under LFW24 there is one date rule for all metering types.
        let mut anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        anfrage.messtyp = Messtyp::Rlm;
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── Gas date rules (check 4, E17 / Escalate) ─────────────────────────────

    #[test]
    fn gas_wechsel_requires_10_wt_lead() {
        // 10 WT after Wed 07-08 is Wed 07-22.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let ok = make_gas_anfrage(Some("E03"), d(2026, Month::July, 22));
        assert!(evaluate(&ok, Some(&vs), Some(&make_grid()), true, NOW, &cfg()).is_accept());
        let short = make_gas_anfrage(Some("E03"), d(2026, Month::July, 21));
        assert_eq!(
            evaluate(&short, Some(&vs), Some(&make_grid()), true, NOW, &cfg()).erc_code(),
            Some("E17")
        );
    }

    #[test]
    fn gas_einzug_retroactive_within_six_weeks_accepted() {
        // Lawful retroactive move-in: D 2026-06-01; window ends
        // 06-01 + 6 weeks = 07-13, + 3 WT = 07-16 ≥ today 07-08.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::June, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn gas_einzug_retroactive_beyond_window_rejected() {
        // D 2026-05-20: window ends 05-20 + 6w = 07-01, +3 WT = 07-06 < today.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::May, 20));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("E17"), "got {result:?}");
    }

    #[test]
    fn gas_einzug_neuanlage_e02_retroactive_accepted() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E02"), d(2026, Month::July, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn gas_rlm_retroactive_rejected_regardless_of_grund() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::July, 1));
        anfrage.messtyp = Messtyp::Rlm;
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("E17"), "got {result:?}");
    }

    #[test]
    fn gas_backdated_without_transaktionsgrund_escalates() {
        // §20 EnWG: never auto-reject a potentially lawful move-in.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(None, d(2026, Month::July, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_escalate(), "expected Escalate, got {result:?}");
    }

    #[test]
    fn gas_future_without_transaktionsgrund_accepted() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(None, d(2026, Month::August, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── EEG-MaLo Zuordnung date rule (check 4, A07) ──────────────────────────

    #[test]
    fn eeg_zuordnung_monatserster_with_lead_accepted() {
        // Receipt Wed 2026-07-08, requested 2026-09-01 (Monatserster, >1 month
        // ahead) → Accept.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_anfrage(55016, d(2026, Month::September, 1));
        anfrage.ist_erzeugende_marktlokation = true;
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn eeg_zuordnung_non_monatserster_rejected() {
        // 2026-09-15 is not a Monatserster → A07.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_anfrage(55016, d(2026, Month::September, 15));
        anfrage.ist_erzeugende_marktlokation = true;
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn eeg_zuordnung_insufficient_lead_rejected() {
        // Receipt 2026-07-08; 2026-08-01 is only < 1 whole month ahead of the
        // receipt month (July → earliest admissible is 2026-08-01)… actually the
        // first-of-month one month after July is August, so 08-01 is the
        // earliest. 2026-07-01 (same month, past) must be rejected.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_anfrage(55016, d(2026, Month::July, 1));
        anfrage.ist_erzeugende_marktlokation = true;
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn eeg_zuordnung_earliest_monatserster_accepted() {
        // Earliest admissible Monatserster for a July receipt with 1-month lead
        // is 2026-08-01 → Accept.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_anfrage(55016, d(2026, Month::August, 1));
        anfrage.ist_erzeugende_marktlokation = true;
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── Configurable Gas Bearbeitungsfrist ───────────────────────────────────

    #[test]
    fn gas_bearbeitungsfrist_is_configurable() {
        // D 2026-05-20: window base = 05-20 + 6w = 07-01. With the default 3 WT
        // it ends 07-06 < today (07-08) → E17. Widening the Bearbeitungsfrist to
        // 5 WT pushes window_end to 07-08 = today → Accept.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::May, 20));

        assert_eq!(
            evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg()).erc_code(),
            Some("E17"),
        );

        let wide = NetzCheckConfig {
            gas_bearbeitungsfrist_wt: 5,
            ..NetzCheckConfig::default()
        };
        assert!(
            evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &wide).is_accept(),
            "widened Bearbeitungsfrist should admit the retroactive Anmeldung",
        );
    }

    // ── BDEW holiday calendar (Werktag math) ─────────────────────────────────

    #[test]
    fn add_werktage_observes_bdew_holidays() {
        // 2026-05-01 (Tag der Arbeit, bundesweiter Feiertag) is a Friday. One
        // Werktag after Thu 2026-04-30 must skip both the holiday and the
        // weekend, landing on Mon 2026-05-04 — never Fri 05-01.
        assert_eq!(
            add_werktage(d(2026, Month::April, 30), 1, CAL),
            d(2026, Month::May, 4),
        );
    }

    // ── Check 2: MaLo participates in MaKo (A02) ─────────────────────────────

    #[test]
    fn stillgelegt_malo_rejected_a02() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Stillgelegt, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A02"), "got {result:?}");
    }

    #[test]
    fn ruhende_malo_rejected_a02() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Ruhend, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A02"), "got {result:?}");
    }

    // ── Remaining checks ──────────────────────────────────────────────────────

    #[test]
    fn escalate_missing_grid() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), None, true, NOW, &cfg());
        assert!(result.is_escalate(), "expected Escalate, got {result:?}");
    }

    #[test]
    fn reject_conflicting_supply() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(
            LieferStatus::Unbeliefert,
            None,
            Some("9900999000001".to_owned()),
        );
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A06"), "got {result:?}");
    }

    #[test]
    fn reject_same_lf_already_active() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(
            LieferStatus::Beliefert,
            Some("9900357000004".to_owned()),
            None,
        );
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A06"), "got {result:?}");
    }

    #[test]
    fn reject_bilanzierungsgebiet_mismatch_a05() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let mut grid = make_grid();
        grid.bilanzierungsgebiet = Some("11YB-AMPRION----W".to_owned());
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A05"), "got {result:?}");
    }

    // ── Check 3: A06 only for a *foreign* pending Anmeldung ──────────────────

    /// `marktd` writes `lf_mp_id_next` while ingesting the `process.initiated`
    /// event, before `processd` evaluates it — so the Anmeldung under
    /// evaluation always sees its own reservation. It must not self-reject.
    #[test]
    fn own_pending_lieferbeginn_is_not_a06() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(
            LieferStatus::Unbeliefert,
            None,
            Some(anfrage.new_supplier_gln.clone()),
        );
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_ne!(result.erc_code(), Some("A06"), "got {result:?}");
    }

    /// A pending Anmeldung from a *different* LF is a genuine
    /// EBD E_0622 Prüfschritt 70 conflict.
    #[test]
    fn foreign_pending_lieferbeginn_is_a06() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(
            LieferStatus::Unbeliefert,
            None,
            Some("9900000000009".to_owned()),
        );
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A06"), "got {result:?}");
    }

    #[test]
    fn reject_unknown_lf_a05() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), false, NOW, &cfg());
        assert_eq!(result.erc_code(), Some("A05"), "got {result:?}");
    }

    #[test]
    fn no_versorgung_record_still_passes() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let result = evaluate(&anfrage, None, Some(&make_grid()), true, NOW, &cfg());
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── Werktag helpers ───────────────────────────────────────────────────────

    #[test]
    fn werktag_between_examples_from_gpke() {
        // ÜT Montag → frühester Lieferbeginn Mittwoch (GPKE example).
        let mon = d(2026, Month::July, 6);
        assert!(!has_werktag_strictly_between(
            mon,
            d(2026, Month::July, 7),
            CAL
        ));
        assert!(has_werktag_strictly_between(
            mon,
            d(2026, Month::July, 8),
            CAL
        ));
        // ÜT Freitag → frühester Lieferbeginn Dienstag.
        let fri = d(2026, Month::July, 10);
        assert!(!has_werktag_strictly_between(
            fri,
            d(2026, Month::July, 13),
            CAL
        ));
        assert!(has_werktag_strictly_between(
            fri,
            d(2026, Month::July, 14),
            CAL
        ));
        // Past dates never validate.
        assert!(!has_werktag_strictly_between(
            mon,
            d(2026, Month::July, 3),
            CAL
        ));
    }

    #[test]
    fn add_werktage_skips_weekends() {
        // Wed 07-08 + 10 WT = Wed 07-22.
        assert_eq!(
            add_werktage(d(2026, Month::July, 8), 10, CAL),
            d(2026, Month::July, 22)
        );
    }
}
