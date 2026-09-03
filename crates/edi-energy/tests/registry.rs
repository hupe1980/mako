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
use edi_energy::{EdiEnergyMessage, Error, MessageType, Release, ReleaseTrack};

// ── Multi-release coexistence fixtures ───────────────────────────────────────

/// Well-formed UTILMD message with the registered release S2.1 (fv20251001 Strom).
#[cfg(feature = "utilmd")]
const UTILMD_S2_1: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+261001:0700+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01+DOK55001'\
DTM+137:202610010000?+00:303'\
NAD+MS+4012345000023::9'\
NAD+MR+9900357000004::9'\
IDE+24+VORGANG0001'\
DTM+92:202610010000?+00:303'\
STS+7++E01+ZW4'\
LOC+Z16+51238696781'\
RFF+Z13:55001'\
SEQ+Z79+1'\
PIA+5+9991000002082:Z11'\
CCI+Z66'\
SEQ+ZH0+1'\
CCI+Z65+++Z01'\
SEQ+Z01'\
CCI+++Z15'\
SEQ+Z75'\
CCI+Z61++ZF9'\
CAV+ZU5'\
NAD+Z09+++Mustermann:::::Z01'\
NAD+Z04+++Mustermann:::::Z01+Musterstr. 1+Berlin+++DE'\
UNT+23+1'\
UNZ+1+1'";

/// Same structure but with a hypothetical release 5.5.4a that has no
/// registered profile — represents a future / unknown release version.
#[cfg(feature = "utilmd")]
const UTILMD_554A_UNKNOWN: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240101:0000+2'\
UNH+1+UTILMD:D:11A:UN:5.5.4a'\
BGM+E01:::+11001+9'\
DTM+137:202401010000?+00:303'\
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

// ── Format-boundary tests ─────────────────────────────────────────────────────
//
// EDIFACT changes format at a single Anwendungszeitpunkt (Allgemeine
// Festlegungen 6.1 §2.5): before it the old format applies, from it the new
// one. The 15-Werktage Übergangszeitraum in §8.5 is the XML rule and does not
// carry over, so the default receive tolerance is zero.

/// A format is not acceptable before its own `valid_from`, ever.
#[cfg(feature = "mscons")]
#[test]
fn a_format_is_not_acceptable_before_its_anwendungszeitpunkt() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    // fv20261001 wire is "2.5", valid_from is 2026-10-01.
    let release_25 = Release::new("2.5");
    assert!(
        !reg.is_acceptable_on(MessageType::Mscons, &release_25, date!(2026 - 09 - 30)),
        "2.5 must NOT be acceptable the day before its Anwendungszeitpunkt"
    );
    assert!(
        reg.is_acceptable_on(MessageType::Mscons, &release_25, date!(2026 - 10 - 01)),
        "2.5 must be acceptable on exactly its Anwendungszeitpunkt"
    );

    // Raising the receive tolerance must not open the leading edge: it is an
    // inbound allowance for a *superseded* format, not a licence to send early.
    let lenient = reg.clone().with_receive_tolerance_days(30);
    assert!(
        !lenient.is_acceptable_on(MessageType::Mscons, &release_25, date!(2026 - 09 - 30)),
        "tolerance must never make a format acceptable before it takes effect"
    );
}

/// With the default tolerance the superseded format stops on the boundary; with
/// a configured tolerance it stays acceptable for exactly that many days.
#[cfg(feature = "mscons")]
#[test]
fn the_superseded_format_expires_at_valid_until_plus_the_tolerance() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    assert_eq!(
        reg.receive_tolerance_days(),
        0,
        "the BDEW default is a hard cutover"
    );

    // fv20251001 — wire 2.4c, valid_until 2026-09-30.
    let p = reg
        .profiles_for(MessageType::Mscons)
        .find(|p| p.valid_until() == Some(date!(2026 - 09 - 30)))
        .expect("fv20251001 must have valid_until 2026-09-30");
    let valid_until = p.valid_until().unwrap();
    let release = p.release().clone();

    assert!(
        reg.is_acceptable_on(MessageType::Mscons, &release, valid_until),
        "2.4c is acceptable on its last day"
    );
    let next = valid_until
        .next_day()
        .expect("date arithmetic must succeed");
    assert!(
        !reg.is_acceptable_on(MessageType::Mscons, &release, next),
        "with a zero tolerance 2.4c stops on the boundary, not a week later"
    );

    // Three days of tolerance move the trailing edge and nothing else.
    let lenient = reg.clone().with_receive_tolerance_days(3);
    let last_tolerated = valid_until + time::Duration::days(3);
    assert!(lenient.is_acceptable_on(MessageType::Mscons, &release, last_tolerated));
    assert!(!lenient.is_acceptable_on(
        MessageType::Mscons,
        &release,
        last_tolerated.next_day().unwrap()
    ));
}

/// `transition_state` and `is_acceptable_on` must agree — they disagreed
/// before, one reporting an overlap the other refused to validate.
#[cfg(feature = "mscons")]
#[test]
fn transition_state_agrees_with_is_acceptable_on() {
    use edi_energy::{TransitionState, registry::ReleaseRegistry};
    use time::macros::date;

    for tolerance in [0, 3] {
        let reg = ReleaseRegistry::global()
            .clone()
            .with_receive_tolerance_days(tolerance);
        let mut day = date!(2026 - 09 - 25);
        while day <= date!(2026 - 10 - 12) {
            let state = reg.transition_state(MessageType::Mscons, day, None);
            let accepted: Vec<&str> = ["2.4c", "2.5"]
                .into_iter()
                .filter(|code| reg.is_acceptable_on(MessageType::Mscons, &Release::new(code), day))
                .collect();
            match state {
                TransitionState::Transition { outgoing, incoming } => assert_eq!(
                    accepted.len(),
                    2,
                    "{day} (tolerance {tolerance}): Transition reports {} and {} but only \
                     {accepted:?} validate",
                    outgoing.release().as_str(),
                    incoming.release().as_str()
                ),
                TransitionState::Stable { profile } => assert_eq!(
                    accepted,
                    vec![profile.release().as_str()],
                    "{day} (tolerance {tolerance}): Stable on {} but {accepted:?} validate",
                    profile.release().as_str()
                ),
                TransitionState::None => {
                    assert!(accepted.is_empty(), "{day}: None but {accepted:?} validate");
                }
            }
            day = day.next_day().unwrap();
        }
    }
}

/// A non-zero tolerance is what produces an overlap, and it is trailing.
#[cfg(feature = "mscons")]
#[test]
fn an_overlap_exists_only_with_a_tolerance_and_only_after_the_boundary() {
    use edi_energy::{TransitionState, registry::ReleaseRegistry};
    use time::macros::date;

    let strict = ReleaseRegistry::global();
    assert!(
        matches!(
            strict.transition_state(MessageType::Mscons, date!(2026 - 10 - 01), None),
            TransitionState::Stable { .. }
        ),
        "a hard cutover has no overlap on any date"
    );

    let lenient = strict.clone().with_receive_tolerance_days(7);
    match lenient.transition_state(MessageType::Mscons, date!(2026 - 10 - 01), None) {
        TransitionState::Transition { outgoing, incoming } => {
            assert_eq!(outgoing.release().as_str(), "2.4c");
            assert_eq!(incoming.release().as_str(), "2.5");
        }
        other => panic!("expected Transition inside the tolerance, got {other:?}"),
    }
    // Before the boundary there is nothing to overlap with.
    assert!(matches!(
        lenient.transition_state(MessageType::Mscons, date!(2026 - 09 - 30), None),
        TransitionState::Stable { .. }
    ));
}

// ── Same-wire-code disambiguation ────────────────────────────────────────────

/// When two profiles share the same wire release code (INVOIC `"2.8e"` in both
/// `fv20260401` and `fv20261001`), `profile_on` must return the profile whose
/// `valid_from` is the greatest value that is ≤ `date`.
///
/// The two are AHB 1.0a (published 01.10.2025, applies 01.04.2026) and AHB 1.0b
/// (published 01.04.2026, applies 01.10.2026) — the same MIG under two
/// Anwendungszeitpunkte, which is exactly why the wire code cannot disambiguate
/// them and the date must.
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

    // Both fv20260401 (valid_from 2026-04-01) and fv20261001 (valid_from
    // 2026-10-01) carry the wire code "2.8e".  Before the later
    // Anwendungszeitpunkt the earlier profile must be returned.
    let profile_2026_04 = reg
        .profile_on(MessageType::Invoic, &release, date!(2026 - 04 - 01))
        .expect("profile_on must find an INVOIC 2.8e profile on 2026-04-01");
    assert_eq!(
        profile_2026_04.valid_from(),
        Some(date!(2026 - 04 - 01)),
        "on 2026-04-01 the fv20260401 profile must be selected"
    );

    // On or after the later Anwendungszeitpunkt the successor must be returned.
    let profile_2026_10 = reg
        .profile_on(MessageType::Invoic, &release, date!(2026 - 10 - 01))
        .expect("profile_on must find an INVOIC 2.8e profile on 2026-10-01");
    assert_eq!(
        profile_2026_10.valid_from(),
        Some(date!(2026 - 10 - 01)),
        "on 2026-10-01 the fv20261001 profile must be selected"
    );

    // The day before the cutover still belongs to the predecessor — this is the
    // boundary a Publikationsdatum in `valid_from` moves six months early.
    let profile_before = reg
        .profile_on(MessageType::Invoic, &release, date!(2026 - 09 - 30))
        .expect("profile_on must find an INVOIC 2.8e profile on 2026-09-30");
    assert_eq!(
        profile_before.valid_from(),
        Some(date!(2026 - 04 - 01)),
        "on 2026-09-30 the fv20260401 profile must still be selected"
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

/// The UTILMD Strom boundary, which is a hard cutover: S2.1 runs to
/// 2026-09-30, S2.2 takes over on 2026-10-01, and no date accepts both.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_strom_changes_format_on_a_single_day() {
    use edi_energy::registry::{ReleaseRegistry, TransitionState};
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let s2_1 = Release::new("S2.1");
    let s2_2 = Release::new("S2.2");

    for (day, expected) in [
        (date!(2026 - 09 - 29), "S2.1"),
        (date!(2026 - 09 - 30), "S2.1"),
        (date!(2026 - 10 - 01), "S2.2"),
        (date!(2026 - 10 - 08), "S2.2"),
    ] {
        match reg.transition_state(MessageType::Utilmd, day, Some(ReleaseTrack::Strom)) {
            TransitionState::Stable { profile } => assert_eq!(
                profile.release().as_str(),
                expected,
                "{day}: expected {expected} in force"
            ),
            other => panic!("expected Stable on {day} under a hard cutover, got {other:?}"),
        }
    }

    // S2.1 is acceptable up to and including its last day, and not after it.
    assert!(reg.is_acceptable_on(MessageType::Utilmd, &s2_1, date!(2026 - 09 - 30)));
    assert!(!reg.is_acceptable_on(MessageType::Utilmd, &s2_1, date!(2026 - 10 - 01)));
    // S2.2 not before its Anwendungszeitpunkt, and from it onwards.
    assert!(!reg.is_acceptable_on(MessageType::Utilmd, &s2_2, date!(2026 - 09 - 30)));
    assert!(reg.is_acceptable_on(MessageType::Utilmd, &s2_2, date!(2026 - 10 - 01)));
    assert!(reg.is_acceptable_on(MessageType::Utilmd, &s2_2, date!(2026 - 10 - 08)));

    // `profile_on` resolves by `valid_from` alone and does not enforce
    // `valid_until`; `is_acceptable_on` is the one that answers "may I process
    // this today?".
    let p = reg
        .profile_on(MessageType::Utilmd, &s2_1, date!(2026 - 10 - 01))
        .expect("profile_on resolves S2.1 by valid_from");
    assert_eq!(p.valid_from(), Some(date!(2025 - 10 - 01)));
}

/// `fv20260401_gas` carries release "G1.1" — UTILMD AHB Gas 1.1, published
/// 01.10.2025 and therefore applying from 01.04.2026.
/// Before that date, `profile_on` must return `Err`.
/// This test guards the corrected profile release codes.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_gas_g1_1_boundary_selects_correct_profile() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let release = Release::new("G1.1");

    // Before fv20260401_gas valid_from: G1.1 is not yet active — profile_on returns Err.
    // 2025-10-01 is when the document was *published*, which changes nothing.
    let result_before = reg.profile_on(MessageType::Utilmd, &release, date!(2026 - 03 - 31));
    assert!(
        result_before.is_err(),
        "on 2026-03-31 (before G1.1 applies) profile_on must return Err"
    );

    // On the first day of fv20260401_gas validity: must select the newer profile.
    let profile_boundary = reg
        .profile_on(MessageType::Utilmd, &release, date!(2026 - 04 - 01))
        .expect("profile_on must find a UTILMD G1.1 profile on 2026-04-01");
    assert_eq!(
        profile_boundary.valid_from(),
        Some(date!(2026 - 04 - 01)),
        "on 2026-04-01 the fv20260401_gas profile must be selected"
    );
}

// ── Anwendungszeitpunkt, not Publikationsdatum ───────────────────────────────

/// A Formatversion applies six months after it is published.
///
/// Allgemeine Festlegungen 6.1d §2.5.1 („Änderungsmanagement zum 1. Oktober
/// eines Jahres") puts the *Veröffentlichungszeitpunkt der konsultierten
/// Dokumente* on 01.04. and their *Anwendungszeitpunkt* on 01.10. of the same
/// year; §2.5.2 mirrors it for the April changeover. The six months between are
/// the Umsetzungsphase, during which the **old** format is still the binding one.
///
/// REQOTE is the sharpest case: AHB 1.1 (published 01.04.2025) and AHB 1.2
/// (published 01.04.2026) both carry wire release `1.3c`, so nothing on the wire
/// distinguishes them and the date is the only input.
#[cfg(feature = "reqote")]
#[test]
fn reqote_switches_on_the_anwendungszeitpunkt_not_the_publikationsdatum() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();

    let on = |d: time::Date| {
        reg.profile_for_date_and_track(MessageType::Reqote, d, ReleaseTrack::Short)
            .unwrap_or_else(|| panic!("REQOTE must have an active profile on {d}"))
            .valid_from()
    };

    // The Publikationsdatum of AHB 1.2 changes nothing: AHB 1.1 stays binding
    // through its whole Umsetzungsphase.
    assert_eq!(
        on(date!(2026 - 04 - 01)),
        Some(date!(2025 - 10 - 01)),
        "01.04.2026 is when AHB 1.2 was published, not when it applies"
    );
    assert_eq!(
        on(date!(2026 - 09 - 30)),
        Some(date!(2025 - 10 - 01)),
        "the last day before the Anwendungszeitpunkt still belongs to AHB 1.1"
    );
    // The changeover is a single instant with no overlap (§2.5).
    assert_eq!(
        on(date!(2026 - 10 - 01)),
        Some(date!(2026 - 10 - 01)),
        "AHB 1.2 applies from 01.10.2026"
    );
}

/// Every profile that records a `publikationsdatum` must sit six months later.
///
/// The relation is enforced at codegen, but the generated tables are what the
/// runtime actually selects on, so it is re-checked here against the compiled
/// registry rather than against the JSON.
#[test]
fn no_profile_is_valid_from_its_publication_date() {
    use edi_energy::registry::ReleaseRegistry;

    for profile in ReleaseRegistry::global().all_profiles() {
        let Some(from) = profile.valid_from() else {
            continue;
        };
        assert!(
            matches!(
                (from.month() as u8, from.day()),
                (4, 1) | (10, 1) | (1, 1) | (6, 6)
            ),
            "{:?} {}: valid_from {from} is not an Anwendungszeitpunkt — the regular \
             changeovers are 01.04. and 01.10.; 01.01.2026 and 06.06.2025 are the two \
             ausserordentliche ones mako carries",
            profile.message_type(),
            profile.release(),
        );
    }
}
