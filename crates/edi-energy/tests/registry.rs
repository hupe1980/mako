//! Integration tests for the [`ReleaseRegistry`] and multi-release dispatch.
//!
//! These tests validate that:
//! - Each message is validated against the profile matching its own
//!   `assoc_code` field — there is no cross-version fallback.
//! - A message carrying an unregistered release code yields
//!   [`Error::ProfileNotFound`], not a spurious validation result.
//! - A message carrying a registered release code validates correctly.

// All constants are guarded by #[cfg(feature = "...")] — no blanket lint
// suppression needed.

// Imports are used exclusively in feature-gated test fns.
#[allow(unused_imports)]
use edi_energy::{EdiEnergyMessage, Error, MessageType, Release};

// ── Multi-release coexistence fixtures ───────────────────────────────────────

/// Well-formed UTILMD message with the registered release S2.1 (fv20251001 Strom).
#[cfg(feature = "utilmd")]
const UTILMD_S2_1: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20240101:102'\
RFF+Z13:REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+51238696781::'\
UNT+8+1'\
UNZ+1+1'";

/// Same structure but with a hypothetical release 5.5.4a that has no
/// registered profile — represents a future / unknown release version.
#[cfg(feature = "utilmd")]
const UTILMD_554A_UNKNOWN: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240101:0000+2'\
UNH+1+UTILMD:D:11A:UN:5.5.4a'\
BGM+E01:::+11001+9'\
DTM+137:20240101:102'\
RFF+ACE:REF-001:::'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z18+51238696781::'\
UNT+8+1'\
UNZ+1+2'";

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A message whose `assoc_code` matches a registered profile validates
/// successfully — all validation layers are applied.
#[test]
#[cfg(feature = "utilmd")]
fn registered_release_validates_against_own_profile() {
    let msg = edi_energy::Platform::with_all_profiles()
        .parse(UTILMD_S2_1)
        .expect("parse must succeed");
    let release = msg.detect_release().expect("release must be detected");
    assert_eq!(release.as_str(), "S2.1");

    let report = msg.validate().expect("validated release must not error");
    // The fixture is deliberately minimal and valid; expect no errors.
    assert_eq!(
        report.errors().len(),
        0,
        "valid S2.1 fixture should produce no validation errors"
    );
}

/// A message whose `assoc_code` refers to an unregistered release yields
/// `Error::ProfileNotFound` — it is *not* silently validated against
/// an incorrect profile and does *not* panic.
#[test]
#[cfg(feature = "utilmd")]
fn unregistered_release_returns_profile_not_found() {
    let msg = edi_energy::Platform::with_all_profiles()
        .parse(UTILMD_554A_UNKNOWN)
        .expect("parse must succeed");
    let release = msg.detect_release().expect("release must be detected");
    assert_eq!(release.as_str(), "5.5.4a");

    match msg.validate() {
        Err(Error::ProfileNotFound {
            message_type,
            release: r,
        }) => {
            assert_eq!(message_type, MessageType::Utilmd);
            assert_eq!(r.as_str(), "5.5.4a");
        }
        other => panic!("expected ProfileNotFound, got {other:?}"),
    }
}

/// Two messages carrying different release codes are each validated against
/// their own profile — there is no shared mutable state and no cross-release
/// contamination between calls.
///
/// This test deliberately alternates between a registered and an unregistered
/// release to verify independence: validating message A does not affect the
/// result for message B.
#[test]
#[cfg(feature = "utilmd")]
fn mixed_release_validates_independently() {
    // Parse both as separate interchanges (full UNB/UNZ envelopes).
    let known = edi_energy::Platform::with_all_profiles()
        .parse(UTILMD_S2_1)
        .expect("parse must succeed");
    let unknown = edi_energy::Platform::with_all_profiles()
        .parse(UTILMD_554A_UNKNOWN)
        .expect("parse must succeed");

    // Validating the registered release first.
    assert!(
        known.validate().is_ok(),
        "S2.1 must validate OK before the unknown release is touched"
    );

    // Validating the unregistered release returns ProfileNotFound, not Ok.
    assert!(
        matches!(
            unknown.validate(),
            Err(Error::ProfileNotFound {
                message_type: MessageType::Utilmd,
                ..
            })
        ),
        "5.5.4a must yield ProfileNotFound even after validating a known release"
    );

    // Validating the registered release again still succeeds — no side effects.
    assert!(
        known.validate().is_ok(),
        "S2.1 must still validate OK after the unknown release was rejected"
    );
}

/// `validate_against` with an explicit release bypasses the message's own
/// assoc_code and uses the caller-supplied profile instead.
#[test]
#[cfg(feature = "utilmd")]
fn validate_against_explicit_release_overrides_assoc_code() {
    // Parse a 5.5.4a message (no registered profile for that release).
    let msg = edi_energy::Platform::with_all_profiles()
        .parse(UTILMD_554A_UNKNOWN)
        .expect("parse must succeed");

    // Pinning to the registered S2.1 profile allows validation to proceed.
    let pinned = Release::new("S2.1");
    assert!(
        msg.validate_against(&pinned).is_ok(),
        "explicit S2.1 profile should be found even for a 5.5.4a message"
    );

    // Pinning to another unregistered release still yields ProfileNotFound.
    let also_unknown = Release::new("5.5.0a");
    assert!(
        matches!(
            msg.validate_against(&also_unknown),
            Err(Error::ProfileNotFound { .. })
        ),
        "explicit unknown release must still yield ProfileNotFound"
    );
}

// ── Grace-period boundary tests ───────────────────────────────────────────────

/// `is_acceptable_on` pre-launch boundary: a date before `valid_from` must
/// return `false`.
#[cfg(feature = "mscons")]
#[test]
fn is_acceptable_on_rejects_before_valid_from() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    // fv20261001 wire is "2.5", valid_from is 2026-10-01.
    let release_25 = Release::new("2.5");
    assert!(
        !reg.is_acceptable_on(MessageType::Mscons, &release_25, date!(2026 - 09 - 30)),
        "2.5 must NOT be acceptable on 2026-09-30 (one day before valid_from)"
    );
    assert!(
        reg.is_acceptable_on(MessageType::Mscons, &release_25, date!(2026 - 10 - 01)),
        "2.5 must be acceptable on 2026-10-01 (exactly valid_from)"
    );
}

/// `is_acceptable_on` post-expiry boundary: a date beyond `valid_until +
/// TRANSITION_GRACE_DAYS` must return `false`.
///
/// Uses MSCONS fv20251001 (wire 2.4c, `valid_until = 2026-09-30`).
/// Grace window: 2026-09-30 + 7 days = 2026-10-07.
/// On 2026-10-08 (grace +1) the 2.4c profile must be rejected because
/// fv20261001 (wire 2.5) takes over.
#[cfg(feature = "mscons")]
#[test]
fn is_acceptable_on_rejects_after_grace_expires() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    // fv20251001 — wire 2.4c, valid_until 2026-09-30; grace ends 2026-10-07.
    let p = reg
        .profiles_for(MessageType::Mscons)
        .find(|p| p.valid_until() == Some(date!(2026 - 09 - 30)))
        .expect("fv20251001 must have valid_until 2026-09-30");

    let vu = p.valid_until().unwrap();
    let grace_end = vu + time::Duration::days(edi_energy::TRANSITION_GRACE_DAYS);

    let release = p.release().clone();

    // On exactly grace_end: acceptable.
    assert!(
        reg.is_acceptable_on(MessageType::Mscons, &release, grace_end),
        "must be acceptable on valid_until + TRANSITION_GRACE_DAYS (day {grace_end})"
    );
    // One day after grace_end: expired.
    let after = grace_end.next_day().expect("date arithmetic must succeed");
    assert!(
        !reg.is_acceptable_on(MessageType::Mscons, &release, after),
        "must NOT be acceptable on valid_until + TRANSITION_GRACE_DAYS + 1 (day {after})"
    );
}

/// `transition_state` must return `Transition { outgoing, incoming }` with
/// BOTH profiles populated when the outgoing profile's `valid_until` has been
/// reached AND the incoming profile's `valid_from` is imminent.
///
/// Algorithm note: `transition_state` selects the profile with the greatest
/// `valid_from ≤ date` as "current". Because fv20261001 has `valid_from =
/// 2026-10-01`, the outgoing fv20251001 is only "current" on dates ≤
/// 2026-09-30. The grace-window condition (`date ≥ valid_until`) fires on
/// exactly 2026-09-30 for this pair. On 2026-10-01 and later, fv20261001 is
/// already the stable current profile.
///
/// For recipients who need to accept EITHER format during the full 7-day
/// post-boundary window (2026-10-01 through 2026-10-07), use
/// `is_acceptable_on()` with both release codes — that API explicitly
/// extends acceptance by `TRANSITION_GRACE_DAYS` beyond `valid_until`.
#[cfg(feature = "mscons")]
#[test]
fn transition_state_returns_both_profiles_during_grace_period() {
    use edi_energy::{MessageType, TransitionState, registry::ReleaseRegistry};
    use time::macros::date;

    let reg = ReleaseRegistry::global();

    // On exactly valid_until (2026-09-30): fv20251001 is the "current" profile
    // (it has the largest valid_from ≤ date among all MSCONS profiles with
    // valid_from ≤ 2026-09-30). The grace-window condition fires, and fv20261001
    // is found as the incoming profile (valid_from = 2026-10-01 is within 7 days).
    let boundary = date!(2026 - 09 - 30);
    let ts = reg.transition_state(MessageType::Mscons, boundary, None);

    match ts {
        TransitionState::Transition { outgoing, incoming } => {
            assert_eq!(
                outgoing.release().as_str(),
                "2.4c",
                "outgoing release must be 2.4c (fv20251001)"
            );
            assert_eq!(
                incoming.release().as_str(),
                "2.5",
                "incoming release must be 2.5 (fv20261001)"
            );
        }
        other => {
            panic!("expected Transition {{ outgoing, incoming }} on {boundary}, got {other:?}")
        }
    }

    // One day before valid_until (2026-09-29): Stable on the outgoing profile.
    // The grace-window condition (date >= valid_until) does not yet fire.
    let before_boundary = date!(2026 - 09 - 29);
    let ts_before = reg.transition_state(MessageType::Mscons, before_boundary, None);
    assert!(
        matches!(ts_before, TransitionState::Stable { .. }),
        "must be Stable before valid_until, got {ts_before:?}"
    );

    // After valid_from of incoming (2026-10-01+): Stable on the incoming profile.
    // fv20261001 is now the most-recently-started profile.
    let after_switch = date!(2026 - 10 - 01);
    let ts_after = reg.transition_state(MessageType::Mscons, after_switch, None);
    match ts_after {
        TransitionState::Stable { profile } => {
            assert_eq!(
                profile.release().as_str(),
                "2.5",
                "must be Stable on release 2.5 after valid_from of incoming"
            );
        }
        other => panic!("expected Stable on {after_switch}, got {other:?}"),
    }
}

// ── Same-wire-code disambiguation ────────────────────────────────────────────

/// When two profiles share the same wire release code (e.g. INVOIC `"2.8e"` in
/// both `fv20251001` and `fv20260401`), `profile_on` must return the profile
/// whose `valid_from` is the greatest value that is ≤ `date`.
///
/// INVOIC MIG 2.8e covers Oct 2025 onward; `fv20260401` supersedes `fv20251001`
/// (AHB update only) but keeps the same wire code.
///
/// This guards against the previous H-2 bug where the index used
/// `HashMap::insert` and silently discarded the earlier profile.
#[cfg(feature = "invoic")]
#[test]
fn profile_on_disambiguates_same_wire_code_by_date() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("2.8e");

    // Both fv20251001 (valid_from 2025-10-01) and fv20260401 (valid_from 2026-04-01)
    // carry the wire code "2.8e".  On a date before fv20260401 the earlier profile
    // must be returned.
    let profile_2025 = reg
        .profile_on(MessageType::Invoic, &release, date!(2025 - 10 - 01))
        .expect("profile_on must find an INVOIC 2.8e profile on 2025-10-01");
    assert_eq!(
        profile_2025.valid_from(),
        Some(date!(2025 - 10 - 01)),
        "on 2025-10-01 the fv20251001 profile must be selected"
    );

    // On or after fv20260401 the later profile must be returned.
    let profile_2026 = reg
        .profile_on(MessageType::Invoic, &release, date!(2026 - 04 - 01))
        .expect("profile_on must find an INVOIC 2.8e profile on 2026-04-01");
    assert_eq!(
        profile_2026.valid_from(),
        Some(date!(2026 - 04 - 01)),
        "on 2026-04-01 the fv20260401 profile must be selected"
    );

    // On a date between the two profiles only the earlier one is eligible.
    let profile_between = reg
        .profile_on(MessageType::Invoic, &release, date!(2026 - 01 - 15))
        .expect("profile_on must find an INVOIC 2.8e profile on 2026-01-15");
    assert_eq!(
        profile_between.valid_from(),
        Some(date!(2025 - 10 - 01)),
        "between the two valid_from dates fv20251001 must be selected"
    );
}

// ── CONTRL same-wire-code disambiguation ─────────────────────────────

/// `fv20260101` carries wire release `"2.0b"`.
///
/// The registry must return `fv20260101` for all dates from 2026-01-01 onward.
#[cfg(feature = "contrl")]
#[test]
fn contrl_same_wire_code_disambiguation() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("2.0b");

    // On exactly 2026-01-01 (valid_from of fv20260101): the profile must be returned.
    let profile_boundary = reg
        .profile_on(MessageType::Contrl, &release, date!(2026 - 01 - 01))
        .expect("profile_on must find a CONTRL 2.0b profile on 2026-01-01");
    assert_eq!(
        profile_boundary.valid_from(),
        Some(date!(2026 - 01 - 01)),
        "on 2026-01-01 the fv20260101 profile must be selected"
    );

    // On a later date (2026-06-01): fv20260101 remains the active profile.
    let profile_2026 = reg
        .profile_on(MessageType::Contrl, &release, date!(2026 - 06 - 01))
        .expect("profile_on must find a CONTRL 2.0b profile on 2026-06-01");
    assert_eq!(
        profile_2026.valid_from(),
        Some(date!(2026 - 01 - 01)),
        "on 2026-06-01 the fv20260101 profile must be selected"
    );
}

// ── UTILMD Strom S2.1 boundary disambiguation ───────────────────────────────

/// `fv20251001` carries UTILMD Strom wire release `"S2.1"`.
///
/// S2.1 is only available from fv20251001 (valid_from 2025-10-01) onwards.
/// Before that date, `profile_on` must return `Err`.
/// This test guards the corrected profile release codes.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_strom_s2_1_boundary_selects_correct_profile() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("S2.1");

    // Before fv20251001 valid_from: S2.1 is not yet active — profile_on returns Err.
    let result_before = reg.profile_on(MessageType::Utilmd, &release, date!(2025 - 09 - 30));
    assert!(
        result_before.is_err(),
        "on 2025-09-30 (before S2.1 validity) profile_on must return Err"
    );

    // On the first day of fv20251001 validity: must select the 2025-10-01 profile.
    let profile_boundary = reg
        .profile_on(MessageType::Utilmd, &release, date!(2025 - 10 - 01))
        .expect("profile_on must find a UTILMD S2.1 profile on 2025-10-01");
    assert_eq!(
        profile_boundary.valid_from(),
        Some(date!(2025 - 10 - 01)),
        "on 2025-10-01 (first day of fv20251001) the 2025-10-01 profile must be selected"
    );

    // Well into fv20251001: still the 2025 profile.
    let profile_2026 = reg
        .profile_on(MessageType::Utilmd, &release, date!(2026 - 03 - 15))
        .expect("profile_on must find a UTILMD S2.1 profile on 2026-03-15");
    assert_eq!(
        profile_2026.valid_from(),
        Some(date!(2025 - 10 - 01)),
        "on 2026-03-15 the fv20251001 (valid_from 2025-10-01) profile must still be selected"
    );
}

/// UTILMD Strom S1.2 bridging profile (fv20250606) — gap coverage.
///
/// BK6-22-024 (LFW24 MpBNr2) created a transitional S1.2 release effective
/// 2025-06-06, bridging the period between S1.1a (deleted) and S2.1 (2025-10-01).
/// `profile_on` must:
///   - resolve S1.2 on dates 2025-06-06 … 2025-09-30 (inclusive)
///   - return Err before 2025-06-06 (no earlier S1.2 profile)
///
/// This guards against accidental deletion of the bridging profile and
/// ensures the 117-day gap (2025-06-06 → 2025-09-30) is covered.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_strom_s1_2_bridging_profile_covers_gap() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("S1.2");

    // Before fv20250606 valid_from: S1.2 not yet active.
    let result_before = reg.profile_on(MessageType::Utilmd, &release, date!(2025 - 06 - 05));
    assert!(
        result_before.is_err(),
        "on 2025-06-05 (before S1.2 validity) profile_on must return Err"
    );

    // On the exact start date: fv20250606 must be selected.
    let profile_start = reg
        .profile_on(MessageType::Utilmd, &release, date!(2025 - 06 - 06))
        .expect("profile_on must find a UTILMD S1.2 profile on 2025-06-06");
    assert_eq!(
        profile_start.valid_from(),
        Some(date!(2025 - 06 - 06)),
        "on 2025-06-06 (valid_from) the fv20250606 profile must be selected"
    );

    // Mid-gap date: still S1.2.
    let profile_mid = reg
        .profile_on(MessageType::Utilmd, &release, date!(2025 - 08 - 15))
        .expect("profile_on must find a UTILMD S1.2 profile on 2025-08-15");
    assert_eq!(
        profile_mid.valid_from(),
        Some(date!(2025 - 06 - 06)),
        "on 2025-08-15 (mid-gap) the fv20250606 profile must be selected"
    );

    // Last day of gap: still S1.2.
    let profile_end = reg
        .profile_on(MessageType::Utilmd, &release, date!(2025 - 09 - 30))
        .expect("profile_on must find a UTILMD S1.2 profile on 2025-09-30");
    assert_eq!(
        profile_end.valid_from(),
        Some(date!(2025 - 06 - 06)),
        "on 2025-09-30 (last day before S2.1) the fv20250606 profile must be selected"
    );
}

/// UTILMD Strom S1.2 bridging profile — known-PID detection is sound.
///
/// A PID that does not appear in the AHB profile JSON does not exist in that
/// release. PIDs 56001–56010 are absent from every BDEW PID list
/// (PID 3.3, PID 4.0) and must therefore never appear in any profile.
///
/// This test also verifies that the `has_pid` closure uses the correct
/// detection idiom (`pack.name() != "unknown-pid"`) rather than the broken
/// `rule_count() > 0` check, which returns `true` for every PID — including
/// unregistered ones — because the fallback pack always has exactly 1 rule.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_strom_s1_2_known_pid_detection_is_sound() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("S1.2");

    let profile = reg
        .profile_on(MessageType::Utilmd, &release, date!(2025 - 06 - 06))
        .expect("S1.2 profile must be present");

    // Correct detection: a PID is known iff its AHB rule pack is NOT the
    // "unknown-pid" fallback (which has exactly 1 rule for any unknown code).
    let has_pid = |code: u32| -> bool {
        let pid = edi_energy::Pruefidentifikator::new(code).unwrap();
        profile.ahb_rule_pack(Some(pid)).name() != "unknown-pid"
    };

    // Core GPKE supply PIDs must be registered in S1.2.
    for expected in [55001u32, 55002, 55003, 55004, 55005, 55006, 55555] {
        assert!(
            has_pid(expected),
            "PID {expected} (GPKE supply) must be present in UTILMD S1.2 AHB"
        );
    }

    // Phantom PIDs 56001–56004 must NOT be registered — they were never assigned
    // in any BDEW AHB document (PID 3.3, PID 4.0, or any UTILMD AHB PDF).
    for phantom in [56001u32, 56002, 56003, 56004] {
        assert!(
            !has_pid(phantom),
            "PID {phantom} (non-existent) must NOT be present in UTILMD S1.2 AHB"
        );
    }
}

/// UTILMD Gas G1.1 boundary.
///
/// Dual-release transition window test.
///
/// UTILMD Strom fv20251001 (S2.1) has `valid_until = 2026-09-30`.
/// UTILMD Strom fv20261001 (S2.2) has `valid_from  = 2026-10-01`.
///
/// # `transition_state` behaviour
///
/// The algorithm selects the profile with the **greatest** `valid_from ≤ date`
/// as "current", then checks whether it is in its outgoing grace window
/// (`date >= valid_until && date <= valid_until + GRACE`).
///
/// Because S2.2's `valid_from` (Oct 1) is exactly the day after S2.1's
/// `valid_until` (Sep 30), the Transition window is exactly **one day**
/// (2026-09-30): on that day S2.1 is still "current" (S2.2 not yet active),
/// its `valid_until` condition is met, and S2.2 qualifies as incoming.
///
/// | Date           | Expected `transition_state`                 |
/// |----------------|---------------------------------------------|
/// | 2026-09-29     | `Stable { "S2.1" }`  — before valid_until   |
/// | 2026-09-30     | `Transition { out="S2.1", in="S2.2" }`      |
/// | 2026-10-01     | `Stable { "S2.2" }`  — S2.2 is now current  |
///
/// # `profile_on` grace window
///
/// `profile_on` applies `is_acceptable` which extends acceptance of S2.1
/// wire messages until `valid_until + GRACE_DAYS = 2026-10-07`.  After that
/// date only S2.2 (no `valid_until`) can be looked up.
///
/// | Date       | `profile_on(S2.1)` | `profile_on(S2.2)` |
/// |------------|--------------------|--------------------|
/// | 2026-10-01 | Ok (still in grace)| Ok                 |
/// | 2026-10-07 | Ok (last grace day)| Ok                 |
/// | 2026-10-08 | Err (expired)      | Ok                 |
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_strom_transition_state_grace_window() {
    use edi_energy::registry::{ReleaseRegistry, TRANSITION_GRACE_DAYS, TransitionState};
    use time::macros::date;

    assert_eq!(
        TRANSITION_GRACE_DAYS, 7,
        "test assumes 7-day TRANSITION_GRACE_DAYS; update boundary dates if changed"
    );

    let reg = ReleaseRegistry::global();

    // ── Before valid_until of S2.1 ───────────────────────────────────────────
    // S2.1 is current; valid_until condition not yet met → Stable.
    let before = reg.transition_state(MessageType::Utilmd, date!(2026 - 09 - 29), Some("S"));
    match before {
        TransitionState::Stable { profile } => {
            assert_eq!(
                profile.release().as_str(),
                "S2.1",
                "day before valid_until: expected Stable S2.1"
            );
        }
        other => panic!("expected Stable on 2026-09-29, got {other:?}"),
    }

    // ── On valid_until of S2.1 (single transition day) ───────────────────────
    // S2.1 still "current"; in_outgoing_grace fires; S2.2 qualifies as incoming.
    let transition_day =
        reg.transition_state(MessageType::Utilmd, date!(2026 - 09 - 30), Some("S"));
    match transition_day {
        TransitionState::Transition { outgoing, incoming } => {
            assert_eq!(
                outgoing.release().as_str(),
                "S2.1",
                "2026-09-30: outgoing must be S2.1"
            );
            assert_eq!(
                incoming.release().as_str(),
                "S2.2",
                "2026-09-30: incoming must be S2.2"
            );
        }
        other => panic!("expected Transition on 2026-09-30 (transition day), got {other:?}"),
    }

    // ── Day of S2.2's valid_from: S2.2 is now "current" ─────────────────────
    // greatest valid_from ≤ 2026-10-01 is S2.2; S2.2 has no valid_until → Stable.
    let s2_2_active = reg.transition_state(MessageType::Utilmd, date!(2026 - 10 - 01), Some("S"));
    match s2_2_active {
        TransitionState::Stable { profile } => {
            assert_eq!(
                profile.release().as_str(),
                "S2.2",
                "2026-10-01: S2.2 must be current and Stable"
            );
        }
        other => panic!("expected Stable S2.2 on 2026-10-01, got {other:?}"),
    }

    // ── profile_on: S2.1 still accepted within grace window ──────────────────
    // profile_on only filters by valid_from ≤ date; it does NOT enforce valid_until.
    // Use is_acceptable_on to verify the 7-day grace window enforcement.
    let release_s2_1 = Release::new("S2.1");
    let release_s2_2 = Release::new("S2.2");

    // profile_on always resolves S2.1 as long as valid_from ≤ date.
    let p = reg
        .profile_on(MessageType::Utilmd, &release_s2_1, date!(2026 - 10 - 01))
        .expect("profile_on must resolve S2.1 by valid_from (no valid_until check)");
    assert_eq!(p.valid_from(), Some(date!(2025 - 10 - 01)));

    // is_acceptable_on enforces valid_until + GRACE: S2.1 valid last day is 2026-10-07.
    assert!(
        reg.is_acceptable_on(MessageType::Utilmd, &release_s2_1, date!(2026 - 10 - 01)),
        "S2.1 must be normatively acceptable on 2026-10-01 (within grace window)"
    );
    assert!(
        reg.is_acceptable_on(MessageType::Utilmd, &release_s2_1, date!(2026 - 10 - 07)),
        "S2.1 must be normatively acceptable on 2026-10-07 (last grace day)"
    );
    assert!(
        !reg.is_acceptable_on(MessageType::Utilmd, &release_s2_1, date!(2026 - 10 - 08)),
        "S2.1 must NOT be normatively acceptable on 2026-10-08 (grace window expired)"
    );

    // S2.2 is always acceptable from its valid_from onward (no valid_until set).
    let p = reg
        .profile_on(MessageType::Utilmd, &release_s2_2, date!(2026 - 10 - 01))
        .expect("S2.2 must be resolvable from its valid_from");
    assert_eq!(p.valid_from(), Some(date!(2026 - 10 - 01)));

    assert!(
        reg.is_acceptable_on(MessageType::Utilmd, &release_s2_2, date!(2026 - 10 - 08)),
        "S2.2 must be normatively acceptable after grace window"
    );
}

/// `fv20251001_gas` carries release "G1.1" (valid from 2025-10-01).
/// Before that date, `profile_on` must return `Err`.
/// This test guards the corrected profile release codes.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_gas_g1_1_boundary_selects_correct_profile() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("G1.1");

    // Before fv20251001_gas valid_from: G1.1 is not yet active — profile_on returns Err.
    let result_before = reg.profile_on(MessageType::Utilmd, &release, date!(2025 - 09 - 30));
    assert!(
        result_before.is_err(),
        "on 2025-09-30 (before G1.1 validity) profile_on must return Err"
    );

    // On the first day of fv20251001_gas validity: must select the newer profile.
    let profile_boundary = reg
        .profile_on(MessageType::Utilmd, &release, date!(2025 - 10 - 01))
        .expect("profile_on must find a UTILMD G1.1 profile on 2025-10-01");
    assert_eq!(
        profile_boundary.valid_from(),
        Some(date!(2025 - 10 - 01)),
        "on 2025-10-01 the fv20251001_gas (valid_from 2025-10-01) profile must be selected"
    );
}
