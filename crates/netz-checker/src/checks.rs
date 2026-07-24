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

use time::{Date, OffsetDateTime, Weekday};
use time_tz::{OffsetDateTimeExt, timezones};

use mako_markt::domain::Sparte;
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};

use crate::types::{AnmeldungAnfrage, MaloGridRecord, Messtyp, NetzCheckResult, RejectReason};

// ── Regulatory constants ──────────────────────────────────────────────────────

/// Gas Lieferantenwechsel (E03): minimum lead, Anmeldung → Lieferbeginn
/// (AWH GeLi Gas 2.0 V1.2, SD Lieferbeginn Prozessschritt 1).
pub const GAS_WECHSEL_VORLAUF_WT: u32 = 10;

/// Gas non-Wechsel retroactive window: 6 weeks (AWH GeLi Gas 2.0 Kap. 2.2
/// Grundregel 3a — „bis zu sechs Wochen zzgl. einer zu berücksichtigenden
/// Bearbeitungsfrist nach An- oder Abmeldedatum").
pub const GAS_RUECKWIRKUNG_WOCHEN: i64 = 6;

/// Gas Bearbeitungsfrist added to the 6-week window. The AWH quantifies it
/// only for the Ersatz-/Grundversorgung (3 WT); the same default is applied
/// to An-/Abmeldungen (flagged ambiguity — see crate README).
pub const GAS_BEARBEITUNGSFRIST_WT: u32 = 3;

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
// Mon–Fri approximation: BDEW holidays are NOT considered here (this crate is
// pure and calendar-free by design). Holiday-blindness errs toward *accepting*
// a date the exact calendar would push out — an operational risk, never a
// discriminatory auto-reject (§20 EnWG). The BDEW-MaKo holiday calendar lives
// in `mako-engine::fristen` for services that need exactness.

/// `true` when `d` is a Werktag under the Mon–Fri approximation.
fn is_werktag(d: Date) -> bool {
    !matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday)
}

/// `true` when at least one Werktag lies **strictly between** `a` and `b`.
///
/// This is the operational form of the LFW24 rule „spätester ÜT ist der Tag
/// vor dem letzten WT vor dem Zuordnungsbeginn": an Anmeldung received on
/// day `a` may carry Zuordnungsbeginn `b` iff a full Werktag separates them.
fn has_werktag_strictly_between(a: Date, b: Date) -> bool {
    let mut d = a.next_day();
    while let Some(cur) = d {
        if cur >= b {
            return false;
        }
        if is_werktag(cur) {
            return true;
        }
        d = cur.next_day();
    }
    false
}

/// The date `n` Werktage after `d` (Mon–Fri approximation).
fn add_werktage(d: Date, n: u32) -> Date {
    let mut cur = d;
    let mut remaining = n;
    while remaining > 0 {
        cur = cur.next_day().unwrap_or(cur);
        if is_werktag(cur) {
            remaining -= 1;
        }
    }
    cur
}

// ── Date plausibility (check 4) ───────────────────────────────────────────────

/// Strom LFW24 date rule — see module docs. `None` = valid.
fn check_date_strom(anfrage: &AnmeldungAnfrage, today: Date) -> Option<NetzCheckResult> {
    if has_werktag_strictly_between(today, anfrage.process_date) {
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

/// Gas date rule (Transaktionsgrund-aware) — see module docs. `None` = valid.
fn check_date_gas(anfrage: &AnmeldungAnfrage, today: Date) -> Option<NetzCheckResult> {
    let d = anfrage.process_date;
    match anfrage.transaktionsgrund.as_deref() {
        // Lieferantenwechsel: future-only, ≥ 10 WT lead.
        Some("E03") => {
            let earliest = add_werktage(today, GAS_WECHSEL_VORLAUF_WT);
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
                    let window_end = add_werktage(
                        d.saturating_add(time::Duration::weeks(GAS_RUECKWIRKUNG_WOCHEN)),
                        GAS_BEARBEITUNGSFRIST_WT,
                    );
                    if today <= window_end {
                        None // lawful retroactive move-in/move-out
                    } else {
                        Some(NetzCheckResult::Reject(RejectReason {
                            erc_code: "E17".to_owned(),
                            detail: format!(
                                "Fristüberschreitung: retroactive Gas Anmeldung {d} is \
                                 outside the 6-week window (+{GAS_BEARBEITUNGSFRIST_WT} WT \
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
    // If `lf_mp_id_next` is already set, another Anmeldung is in Bearbeitung
    // for this MaLo. EBD E_0622 Prüfschritt 70 → A06 „Andere Anmeldung in
    // Bearbeitung".
    if let Some(vs) = versorgung {
        if vs.lf_mp_id_next.is_some() {
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
            Sparte::Strom => check_date_strom(anfrage, today),
            Sparte::Gas => check_date_gas(anfrage, today),
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
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn strom_reject_next_day() {
        // ÜT Wed → D Thu: no Werktag strictly between → A07.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 9));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn strom_reject_past_date_even_for_einzug() {
        // LFW24 abolished retroactive Anmeldungen for ALL Transaktionsgründe —
        // an E01 (Ein-/Auszug) backdated Strom Anmeldung is rejected A07 too.
        let mut anfrage = make_anfrage(55001, d(2026, Month::July, 7));
        anfrage.transaktionsgrund = Some("E01".to_owned());
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn strom_reject_today() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 8));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");
    }

    #[test]
    fn strom_weekend_pushes_earliest_start() {
        // ÜT Fri 07-10 → D Mon 07-13: only Sat/Sun between → A07.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 13));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW_FRIDAY);
        assert_eq!(result.erc_code(), Some("A07"), "got {result:?}");

        // D Tue 07-14: Mon 07-13 lies between → Accept.
        let anfrage = make_anfrage(55001, d(2026, Month::July, 14));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW_FRIDAY);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn strom_rule_is_messtyp_independent() {
        // Under LFW24 there is one date rule for all metering types.
        let mut anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        anfrage.messtyp = Messtyp::Rlm;
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── Gas date rules (check 4, E17 / Escalate) ─────────────────────────────

    #[test]
    fn gas_wechsel_requires_10_wt_lead() {
        // 10 WT after Wed 07-08 is Wed 07-22.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let ok = make_gas_anfrage(Some("E03"), d(2026, Month::July, 22));
        assert!(evaluate(&ok, Some(&vs), Some(&make_grid()), true, NOW).is_accept());
        let short = make_gas_anfrage(Some("E03"), d(2026, Month::July, 21));
        assert_eq!(
            evaluate(&short, Some(&vs), Some(&make_grid()), true, NOW).erc_code(),
            Some("E17")
        );
    }

    #[test]
    fn gas_einzug_retroactive_within_six_weeks_accepted() {
        // Lawful retroactive move-in: D 2026-06-01; window ends
        // 06-01 + 6 weeks = 07-13, + 3 WT = 07-16 ≥ today 07-08.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::June, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn gas_einzug_retroactive_beyond_window_rejected() {
        // D 2026-05-20: window ends 05-20 + 6w = 07-01, +3 WT = 07-06 < today.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::May, 20));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("E17"), "got {result:?}");
    }

    #[test]
    fn gas_einzug_neuanlage_e02_retroactive_accepted() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(Some("E02"), d(2026, Month::July, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn gas_rlm_retroactive_rejected_regardless_of_grund() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let mut anfrage = make_gas_anfrage(Some("E01"), d(2026, Month::July, 1));
        anfrage.messtyp = Messtyp::Rlm;
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("E17"), "got {result:?}");
    }

    #[test]
    fn gas_backdated_without_transaktionsgrund_escalates() {
        // §20 EnWG: never auto-reject a potentially lawful move-in.
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(None, d(2026, Month::July, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert!(result.is_escalate(), "expected Escalate, got {result:?}");
    }

    #[test]
    fn gas_future_without_transaktionsgrund_accepted() {
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let anfrage = make_gas_anfrage(None, d(2026, Month::August, 1));
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── Check 2: MaLo participates in MaKo (A02) ─────────────────────────────

    #[test]
    fn stillgelegt_malo_rejected_a02() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Stillgelegt, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("A02"), "got {result:?}");
    }

    #[test]
    fn ruhende_malo_rejected_a02() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Ruhend, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("A02"), "got {result:?}");
    }

    // ── Remaining checks ──────────────────────────────────────────────────────

    #[test]
    fn escalate_missing_grid() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), None, true, NOW);
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
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
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
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), true, NOW);
        assert_eq!(result.erc_code(), Some("A06"), "got {result:?}");
    }

    #[test]
    fn reject_bilanzierungsgebiet_mismatch_a05() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let mut grid = make_grid();
        grid.bilanzierungsgebiet = Some("11YB-AMPRION----W".to_owned());
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert_eq!(result.erc_code(), Some("A05"), "got {result:?}");
    }

    #[test]
    fn reject_unknown_lf_a05() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&make_grid()), false, NOW);
        assert_eq!(result.erc_code(), Some("A05"), "got {result:?}");
    }

    #[test]
    fn no_versorgung_record_still_passes() {
        let anfrage = make_anfrage(55001, d(2026, Month::July, 10));
        let result = evaluate(&anfrage, None, Some(&make_grid()), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    // ── Werktag helpers ───────────────────────────────────────────────────────

    #[test]
    fn werktag_between_examples_from_gpke() {
        // ÜT Montag → frühester Lieferbeginn Mittwoch (GPKE example).
        let mon = d(2026, Month::July, 6);
        assert!(!has_werktag_strictly_between(mon, d(2026, Month::July, 7)));
        assert!(has_werktag_strictly_between(mon, d(2026, Month::July, 8)));
        // ÜT Freitag → frühester Lieferbeginn Dienstag.
        let fri = d(2026, Month::July, 10);
        assert!(!has_werktag_strictly_between(fri, d(2026, Month::July, 13)));
        assert!(has_werktag_strictly_between(fri, d(2026, Month::July, 14)));
        // Past dates never validate.
        assert!(!has_werktag_strictly_between(mon, d(2026, Month::July, 3)));
    }

    #[test]
    fn add_werktage_skips_weekends() {
        // Wed 07-08 + 10 WT = Wed 07-22.
        assert_eq!(
            add_werktage(d(2026, Month::July, 8), 10),
            d(2026, Month::July, 22)
        );
    }
}
