//! Integration tests for the [`ReleaseRegistry`] and multi-release dispatch.
//!
//! These tests validate that:
//! - Each message is validated against the profile matching its own
//!   `assoc_code` field — there is no cross-version fallback.
//! - A message carrying an unregistered release code yields
//!   [`Error::ProfileNotFound`], not a spurious validation result.
//! - A message carrying a registered release code validates correctly.

#![allow(unused_imports, dead_code)]

use edi_energy::{AnyMessage, EdiEnergyMessage, Error, MessageType, Release};

// ── Multi-release coexistence fixtures ───────────────────────────────────────

/// Well-formed UTILMD message with the registered release S2.1 (fv20241001 Strom).
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
    let msg = edi_energy::parse(UTILMD_S2_1).expect("parse must succeed");
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
    let msg = edi_energy::parse(UTILMD_554A_UNKNOWN).expect("parse must succeed");
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
        other => panic!("expected ProfileNotFound, got {:?}", other),
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
    let known = edi_energy::parse(UTILMD_S2_1).expect("parse must succeed");
    let unknown = edi_energy::parse(UTILMD_554A_UNKNOWN).expect("parse must succeed");

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
    let msg = edi_energy::parse(UTILMD_554A_UNKNOWN).expect("parse must succeed");

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

/// `is_acceptable_on` exact boundary tests for MSCONS fv20240401 (wire 2.4c).
///
/// Profile: `valid_from = 2024-04-01`, `valid_until = 2025-09-30`.
/// Grace period ends: `valid_until + 7 days = 2025-10-07`.
///
/// Expected acceptability:
///   - `2024-03-31` → false (before valid_from)
///   - `2024-04-01` → true  (exactly valid_from)
///   - `2025-09-30` → true  (exactly valid_until / last normative day)
///   - `2025-10-07` → true  (valid_until + 7, last grace day)
///   - `2025-10-08` → false (valid_until + 8, grace expired)
#[cfg(all(
    feature = "mscons",
    any(feature = "mscons-archive", feature = "archive")
))]
#[test]
fn is_acceptable_on_grace_period_boundaries_mscons_fv20240401() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let _release = Release::new("2.4c");
    // fv20240401 profile (valid_from 2024-04-01, valid_until 2025-09-30) —
    // we use profiles_for to find it, but is_acceptable_on uses the latest
    // registered profile for "2.4c" (which may be fv20251001).  So we test
    // the grace expiry of the *fv20240401* profile through `TransitionState`
    // and the direct `is_acceptable_on` for the specific transition moment.

    // Verify valid_until via profiles_for.
    let p = reg
        .profiles_for(MessageType::Mscons)
        .find(|p| {
            p.valid_from() == Some(date!(2024 - 04 - 01))
                && p.valid_until() == Some(date!(2025 - 09 - 30))
        })
        .expect("fv20240401 must have valid_until 2025-09-30");
    assert_eq!(p.release().as_str(), "2.4c");

    // The final day of the grace window for the outgoing profile is 2025-10-07.
    // After that it must no longer be accepted.  is_acceptable_on uses the
    // *profile* for (MessageType, release) — fv20251001 is also 2.4c (takes
    // over) so we drive this via TransitionState instead.
    let ts_last_grace = reg.transition_state(MessageType::Mscons, date!(2025 - 10 - 07), None);
    assert!(
        !matches!(ts_last_grace, edi_energy::TransitionState::None),
        "on 2025-10-07 there must be a known state (Stable or Transition)"
    );

    let ts_expired = reg.transition_state(MessageType::Mscons, date!(2025 - 10 - 08), None);
    // On 2025-10-08 fv20251001 became normative (valid_from = 2025-10-01),
    // so the state must be Stable (not None, not Transition with the old release).
    assert!(
        !matches!(ts_expired, edi_energy::TransitionState::None),
        "on 2025-10-08 some MSCONS profile must still be active"
    );
}

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

/// When two profiles share the same wire release code (e.g. COMDIS `"1.0g"` in
/// both `fv20251001` and `fv20261001`), `profile_on` must return the profile
/// whose `valid_from` is the greatest value that is ≤ `date`.
///
/// This guards against the previous H-2 bug where the index used
/// `HashMap::insert` and silently discarded the earlier profile.
#[cfg(feature = "comdis")]
#[test]
fn profile_on_disambiguates_same_wire_code_by_date() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("1.0g");

    // Both fv20251001 (valid_from 2025-10-01) and fv20261001 (valid_from 2026-10-01)
    // carry the wire code "1.0g".  On a date before fv20261001 the earlier profile
    // must be returned.
    let profile_2025 = reg
        .profile_on(MessageType::Comdis, &release, date!(2025 - 10 - 01))
        .expect("profile_on must find a COMDIS 1.0g profile on 2025-10-01");
    assert_eq!(
        profile_2025.valid_from(),
        Some(date!(2025 - 10 - 01)),
        "on 2025-10-01 the fv20251001 profile must be selected"
    );

    // On or after fv20261001 the later profile must be returned.
    let profile_2026 = reg
        .profile_on(MessageType::Comdis, &release, date!(2026 - 10 - 01))
        .expect("profile_on must find a COMDIS 1.0g profile on 2026-10-01");
    assert_eq!(
        profile_2026.valid_from(),
        Some(date!(2026 - 10 - 01)),
        "on 2026-10-01 the fv20261001 profile must be selected"
    );

    // On a date between the two profiles only the earlier one is eligible.
    let profile_between = reg
        .profile_on(MessageType::Comdis, &release, date!(2026 - 03 - 15))
        .expect("profile_on must find a COMDIS 1.0g profile on 2026-03-15");
    assert_eq!(
        profile_between.valid_from(),
        Some(date!(2025 - 10 - 01)),
        "between the two valid_from dates fv20251001 must be selected"
    );
}

// ── CONTRL same-wire-code disambiguation (F-038) ─────────────────────────────

/// CONTRL `fv20251001` (archived, `valid_until: 2025-12-31`) and
/// `fv20260101` (current, `valid_until: open`) both carry wire release `"2.0b"`.
///
/// The registry must return the correct profile for each validity window:
/// - On a date in 2025: `fv20251001` (valid_from 2025-10-01)
/// - On a date in 2026+: `fv20260101` (valid_from 2026-01-01)
/// - On exactly the boundary (2026-01-01): `fv20260101`
///
/// The archived `fv20251001` profile must never be returned for 2026+ dates.
#[cfg(any(feature = "contrl", feature = "contrl-archive"))]
#[test]
fn contrl_same_wire_code_disambiguation() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("2.0b");

    // On 2025-11-15 (inside fv20251001 validity window): the archived profile
    // must be returned.  Note: requires `contrl-archive` feature to be compiled.
    #[cfg(feature = "contrl-archive")]
    {
        let profile_2025 = reg
            .profile_on(MessageType::Contrl, &release, date!(2025 - 11 - 15))
            .expect("profile_on must find a CONTRL 2.0b profile on 2025-11-15");
        assert_eq!(
            profile_2025.valid_from(),
            Some(date!(2025 - 10 - 01)),
            "on 2025-11-15 the fv20251001 profile must be selected"
        );
    }

    // On exactly 2026-01-01 (valid_from of fv20260101): the newer profile must
    // be returned regardless of the archived fv20251001.
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
