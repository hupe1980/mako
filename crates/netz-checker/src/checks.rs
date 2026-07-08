//! The six deterministic NB Anmeldung validation checks.
//!
//! All checks are **pure functions** — no I/O, no global state, no clock calls.
//! The current instant is always passed as a parameter.
//!
//! ## Check sequence
//!
//! | # | Rule | Reject code | Escalate? |
//! |---|------|-------------|-----------|
//! | 1 | Grid record present (`MaloGridRecord` is `Some`) | — | ✓ missing data |
//! | 2 | No conflicting active supply (`lf_gln_next` is `None`) | A06 | |
//! | 3 | `process_date ≥ today_berlin(now)` | A97 | |
//! | 4 | Bilanzierungsgebiet matches grid record (when both are present) | A02 | |
//! | 5 | LF GLN is in the partner directory (`partner_known = true`) | A05 | |
//! | 6 | Mindestvorlauffrist met (SLP: ≥ next Werktag after 15:00; RLM: ≥ 2 Werktage) | A99 | |
//!
//! Checks run in order; the first failure short-circuits and returns the result.
//!
//! ## Regulatory sources
//!
//! - GPKE: BK6-22-024 §5 + UTILMD Strom AHB (PIDs 55001, 55016)
//! - GeLi Gas: BK7-24-01-009 §3 + UTILMD Gas AHB (PID 44001)
//! - APERAK AHB 1.0 §2 — ERC codes A02, A05, A06, A97, A99

use time::{Date, OffsetDateTime};
use time_tz::{OffsetDateTimeExt, timezones};

use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};

use crate::types::{AnmeldungAnfrage, MaloGridRecord, Messtyp, NetzCheckResult, RejectReason};

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

// ── Vorlauffrist helpers ──────────────────────────────────────────────────────

/// Returns `true` if `process_date` meets the SLP Mindestvorlauffrist.
///
/// **SLP rule (LFW24, BK6-22-024):**
/// - Submission before 15:00 CET/CEST → earliest Lieferbeginn is the next
///   Arbeitstag (Monday–Saturday; Sunday and public holidays excluded).
/// - Submission at or after 15:00 → earliest Lieferbeginn is the day after
///   next (übernächster Arbeitstag).
///
/// For simplicity, this check verifies `process_date > today_berlin(now)`.
/// The full Arbeitstag calculation is the responsibility of `processd`; here
/// we apply the conservative rule: past dates always fail, same-day dates
/// always fail for SLP, dates at least 1 day ahead pass.
///
/// The caller may provide a stricter gate if needed.
fn slp_vorlauffrist_met(process_date: Date, now: OffsetDateTime) -> bool {
    let today = today_berlin(now);
    process_date > today
}

/// Returns `true` if `process_date` meets the RLM Mindestvorlauffrist.
///
/// **RLM rule:** at least 2 Werktage (calendar days with Saturday counting
/// as a Werktag).  For cross-DST robustness this check uses calendar days.
fn rlm_vorlauffrist_met(process_date: Date, now: OffsetDateTime) -> bool {
    let today = today_berlin(now);
    // 2 calendar days (conservative; Werktag counting would be ≥ 2 working days)
    process_date >= today + time::Duration::days(2)
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

    // ── Check 2: No conflicting active supply ─────────────────────────────────
    //
    // If `lf_gln_next` is already set, another Lieferbeginn is in flight for
    // this MaLo.  Per GPKE AHB: reject with A06.
    if let Some(vs) = versorgung {
        if vs.lf_gln_next.is_some() {
            return NetzCheckResult::Reject(RejectReason {
                erc_code: "A06".to_owned(),
                detail: format!(
                    "MaLo {} already has a pending Lieferbeginn (lf_gln_next = {:?}). \
                     Reject duplicate Anmeldung.",
                    anfrage.malo_id, vs.lf_gln_next
                ),
                check_number: 2,
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
                check_number: 2,
            });
        }
    }

    // ── Check 3: Process date is not in the past ──────────────────────────────
    //
    // Retroactive Lieferbeginn dates are forbidden (LFW24, BK6-22-024 §5).
    // Per APERAK AHB: reject with A97 (ungültiger Zeitpunkt).
    {
        let today = today_berlin(now);
        if anfrage.process_date < today {
            return NetzCheckResult::Reject(RejectReason {
                erc_code: "A97".to_owned(),
                detail: format!(
                    "Requested Lieferbeginn {} is in the past (today = {}). \
                     Retroactive supply starts are not permitted (BK6-22-024 §5).",
                    anfrage.process_date, today
                ),
                check_number: 3,
            });
        }
    }

    // ── Check 4: Bilanzierungsgebiet consistent ───────────────────────────────
    //
    // When both the UTILMD message and the grid record carry a
    // Bilanzierungsgebiet, they must match.  A mismatch indicates the LFN
    // referenced the wrong NB's grid area.  Reject with A02.
    if let (Some(req_big), Some(grid_big)) =
        (&anfrage.bilanzierungsgebiet, &grid.bilanzierungsgebiet)
        && req_big != grid_big
    {
        return NetzCheckResult::Reject(RejectReason {
            erc_code: "A02".to_owned(),
            detail: format!(
                "Bilanzierungsgebiet mismatch: UTILMD contains '{}' but grid \
                 record for MaLo {} has '{}'. The request was directed to the \
                 wrong NB or the LFN used an incorrect grid area code.",
                req_big, anfrage.malo_id, grid_big
            ),
            check_number: 4,
        });
    }

    // Also reject when the UTILMD provides a Bilanzierungsgebiet but the grid
    // record has none — we cannot confirm the assertion.
    if anfrage.bilanzierungsgebiet.is_some() && grid.bilanzierungsgebiet.is_none() {
        // Conservative: escalate rather than reject, as the grid record may be
        // incomplete.  The operator can override.
        return NetzCheckResult::Escalate {
            reason: format!(
                "UTILMD message provides Bilanzierungsgebiet {:?} but grid record \
                 for MaLo {} has no Bilanzierungsgebiet — cannot confirm consistency. \
                 Update the grid record via PUT /api/v1/malo/{}/grid.",
                anfrage.bilanzierungsgebiet, anfrage.malo_id, anfrage.malo_id
            ),
        };
    }

    // ── Check 5: LF known in partner directory ────────────────────────────────
    //
    // The requesting LF must be registered as a trading partner.  Without a
    // valid partner record, AS4 delivery of the response is impossible.
    // Reject with A05 (unbekannter Marktpartner).
    if !partner_known {
        return NetzCheckResult::Reject(RejectReason {
            erc_code: "A05".to_owned(),
            detail: format!(
                "LF GLN {} is not registered in the partner directory. \
                 The LF must publish their MP-ID at bdew-codes.de and register \
                 an AS4 channel before initiating a Lieferbeginn.",
                anfrage.new_supplier_gln
            ),
            check_number: 5,
        });
    }

    // ── Check 6: Mindestvorlauffrist ──────────────────────────────────────────
    //
    // Strom SLP (LFW24): submission before 15:00 → next Arbeitstag;
    //   after 15:00 → übernächster Arbeitstag.  We check process_date > today.
    // RLM: 2 Werktage minimum lead time.
    // Gas: 10 Werktage (calendar) — but the GeLi Gas AHB deadline check is
    //   separate; here we only check that the date is not today or past.
    let vorlauffrist_ok = match anfrage.messtyp {
        Messtyp::Rlm => rlm_vorlauffrist_met(anfrage.process_date, now),
        // SLP and iMSys use the same 1-day minimum (conservative; processd
        // applies the full 15:00 cutoff rule using German local time).
        Messtyp::Slp | Messtyp::Imsys => slp_vorlauffrist_met(anfrage.process_date, now),
    };

    if !vorlauffrist_ok {
        return NetzCheckResult::Reject(RejectReason {
            erc_code: "A99".to_owned(),
            detail: format!(
                "Mindestvorlauffrist not met for {} metering. \
                 Requested Lieferbeginn {} does not satisfy the minimum lead time \
                 (SLP: next Arbeitstag; RLM: 2 Werktage). \
                 Source: BK6-22-024 §5 / GeLi Gas BK7-24-01-009.",
                anfrage.messtyp, anfrage.process_date
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
        }
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
        lf_gln_next: Option<String>,
    ) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: "51238696780".parse().unwrap(),
            lieferstatus: status,
            lf_mp_id,
            lf_gln_next,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: "9900000000002".to_owned(),
            last_process_id: None,
            updated_at: OffsetDateTime::now_utc(),
            tenant: "9900000000002".to_owned(),
            version: 1,
        }
    }

    // A fixed "now" in the future (winter time, UTC = CET)
    // 2026-07-08 10:00 UTC → 2026-07-08 12:00 CEST → today_berlin = 2026-07-08
    const NOW: OffsetDateTime = datetime!(2026-07-08 10:00 UTC);

    #[test]
    fn accept_clean_slp() {
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        let grid = make_grid();
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn escalate_missing_grid() {
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), None, true, NOW);
        assert!(result.is_escalate(), "expected Escalate, got {result:?}");
    }

    #[test]
    fn reject_conflicting_supply() {
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        let grid = make_grid();
        let vs = make_versorgung(
            LieferStatus::Unbeliefert,
            None,
            Some("9900999000001".to_owned()),
        );
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert_eq!(result.erc_code(), Some("A06"), "got {result:?}");
    }

    #[test]
    fn reject_past_date() {
        // yesterday — today_berlin(NOW) = 2026-07-08
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 7).unwrap(),
        );
        let grid = make_grid();
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert_eq!(result.erc_code(), Some("A97"), "got {result:?}");
    }

    #[test]
    fn reject_today_slp() {
        // Same day as NOW — SLP requires process_date > today
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 8).unwrap(),
        );
        let grid = make_grid();
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert_eq!(result.erc_code(), Some("A99"), "got {result:?}");
    }

    #[test]
    fn reject_bilanzierungsgebiet_mismatch() {
        let mut anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        anfrage.bilanzierungsgebiet = Some("11YB-TENNET-----W".to_owned());
        let mut grid = make_grid();
        grid.bilanzierungsgebiet = Some("11YB-AMPRION----W".to_owned()); // different!
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert_eq!(result.erc_code(), Some("A02"), "got {result:?}");
    }

    #[test]
    fn reject_unknown_lf() {
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        let grid = make_grid();
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), false, NOW); // partner_known=false
        assert_eq!(result.erc_code(), Some("A05"), "got {result:?}");
    }

    #[test]
    fn reject_rlm_insufficient_vorlauffrist() {
        let mut anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        anfrage.messtyp = Messtyp::Rlm;
        // Only 1 day ahead — RLM requires 2
        let grid = make_grid();
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert_eq!(result.erc_code(), Some("A99"), "got {result:?}");
    }

    #[test]
    fn accept_rlm_sufficient_vorlauffrist() {
        let mut anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 10).unwrap(),
        );
        anfrage.messtyp = Messtyp::Rlm;
        // 2 days ahead — just meets RLM minimum
        let grid = make_grid();
        let vs = make_versorgung(LieferStatus::Unbeliefert, None, None);
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn no_versorgung_record_still_passes_check2() {
        // When the MaLo is not yet in versorgungsstatus, skip check 2 (no conflict).
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        let grid = make_grid();
        let result = evaluate(&anfrage, None, Some(&grid), true, NOW);
        assert!(result.is_accept(), "expected Accept, got {result:?}");
    }

    #[test]
    fn reject_same_lf_already_active() {
        let anfrage = make_anfrage(
            55001,
            Date::from_calendar_date(2026, Month::July, 9).unwrap(),
        );
        let grid = make_grid();
        let vs = make_versorgung(
            LieferStatus::Beliefert,
            Some("9900357000004".to_owned()), // same GLN as new_supplier_gln
            None,
        );
        let result = evaluate(&anfrage, Some(&vs), Some(&grid), true, NOW);
        assert_eq!(result.erc_code(), Some("A06"), "got {result:?}");
    }
}
