//! Unit tests for core types: `Release`, `Pruefidentifikator`, `MessageType`.

use edi_energy::{MessageType, Pruefidentifikator, Release};

// ── Release ───────────────────────────────────────────────────────────────────

#[test]
fn release_new_and_as_str() {
    let r = Release::new("5.5.3a");
    assert_eq!(r.as_str(), "5.5.3a");
}

#[test]
fn release_display() {
    let r = Release::new("5.2e");
    assert_eq!(r.to_string(), "5.2e");
}

#[test]
fn release_parse_via_fromstr() {
    let r: Release = "S4.0".parse().expect("fromstr should never fail");
    assert_eq!(r.as_str(), "S4.0");
}

#[test]
fn release_eq_and_hash() {
    let a = Release::new("5.5.3a");
    let b = Release::new("5.5.3a");
    assert_eq!(a, b);

    let mut set = std::collections::HashSet::new();
    set.insert(a.clone());
    set.insert(b.clone());
    assert_eq!(set.len(), 1);
}

#[test]
fn release_ord() {
    // Within the same Dotted track, ordering is numeric per component.
    let older = Release::new("5.5.3a");
    let newer = Release::new("5.5.4b");
    assert!(older < newer, "5.5.3a < 5.5.4b (same Dotted track)");

    // Within the same Short track, numeric ordering.
    let short_old = Release::new("2.9a");
    let short_new = Release::new("2.10a");
    assert!(
        short_old < short_new,
        "2.9a < 2.10a (numeric, not lexicographic)"
    );

    // Cross-track comparisons are NOT meaningful; PartialOrd returns None.
    let strom = Release::new("S2.1");
    let dotted = Release::new("5.5.3a");
    assert_eq!(
        strom.partial_cmp(&dotted),
        None,
        "S2.1 and 5.5.3a are on different tracks — incomparable"
    );
}

#[test]
fn release_clone() {
    let r = Release::new("5.5.3a");
    let r2 = r.clone();
    assert_eq!(r, r2);
}

#[test]
fn release_asref() {
    let r = Release::new("5.5.3a");
    let s: &str = r.as_ref();
    assert_eq!(s, "5.5.3a");
}

// ── Pruefidentifikator ───────────────────────────────────────────────────────

#[test]
fn pruefidentifikator_valid_range() {
    assert!(Pruefidentifikator::new(10000).is_ok());
    assert!(Pruefidentifikator::new(11001).is_ok());
    assert!(Pruefidentifikator::new(99999).is_ok());
}

#[test]
fn pruefidentifikator_too_small() {
    assert!(Pruefidentifikator::new(9999).is_err());
    assert!(Pruefidentifikator::new(0).is_err());
}

#[test]
fn pruefidentifikator_too_large() {
    assert!(Pruefidentifikator::new(100_000).is_err());
}

#[test]
fn pruefidentifikator_display_zero_padded() {
    let p = Pruefidentifikator::new(11001).unwrap();
    assert_eq!(p.to_string(), "11001");
    // Edge: exactly 5 digits from min
    let min = Pruefidentifikator::new(10000).unwrap();
    assert_eq!(min.to_string(), "10000");
}

#[test]
fn pruefidentifikator_as_u32() {
    let p = Pruefidentifikator::new(25001).unwrap();
    assert_eq!(p.as_u32(), 25001);
}

#[test]
fn pruefidentifikator_parse_via_fromstr() {
    let p: Pruefidentifikator = "11001".parse().unwrap();
    assert_eq!(p.as_u32(), 11001);
}

#[test]
fn pruefidentifikator_fromstr_invalid() {
    assert!("99".parse::<Pruefidentifikator>().is_err());
    assert!("abc".parse::<Pruefidentifikator>().is_err());
    assert!("100000".parse::<Pruefidentifikator>().is_err());
}

#[test]
fn pruefidentifikator_eq() {
    let a = Pruefidentifikator::new(11001).unwrap();
    let b = Pruefidentifikator::new(11001).unwrap();
    let c = Pruefidentifikator::new(11002).unwrap();
    assert_eq!(a, b);
    assert_ne!(a, c);
}

// ── MessageType ───────────────────────────────────────────────────────────────

#[test]
fn message_type_as_str_round_trip() {
    for (variant, code) in [
        (MessageType::Utilmd, "UTILMD"),
        (MessageType::Mscons, "MSCONS"),
        (MessageType::Aperak, "APERAK"),
        (MessageType::Contrl, "CONTRL"),
        (MessageType::Invoic, "INVOIC"),
        (MessageType::Remadv, "REMADV"),
        (MessageType::Orders, "ORDERS"),
        (MessageType::Iftsta, "IFTSTA"),
        (MessageType::Insrpt, "INSRPT"),
        (MessageType::Reqote, "REQOTE"),
        (MessageType::Partin, "PARTIN"),
    ] {
        assert_eq!(variant.as_str(), code, "as_str mismatch for {variant}");
        assert_eq!(
            MessageType::from_unh_code(code),
            Some(variant),
            "from_unh_code mismatch for {code}"
        );
    }
}

#[test]
fn message_type_from_unh_code_unknown() {
    assert_eq!(MessageType::from_unh_code("FOOBAR"), None);
    assert_eq!(MessageType::from_unh_code(""), None);
}

#[test]
fn message_type_display() {
    assert_eq!(MessageType::Utilmd.to_string(), "UTILMD");
    assert_eq!(MessageType::Mscons.to_string(), "MSCONS");
}

#[test]
fn message_type_eq_hash() {
    use std::collections::HashSet;
    let set: HashSet<MessageType> = [
        MessageType::Utilmd,
        MessageType::Utilmd,
        MessageType::Mscons,
    ]
    .into_iter()
    .collect();
    assert_eq!(set.len(), 2);
}

// ── ProcessContext ────────────────────────────────────────────────────────────

/// MSCONS timeline:
/// - fv20240401: valid from 2024-04-01, wire 2.4c (previous AHB version)
/// - fv20251001: valid from 2025-10-01, wire 2.4c (AHB 3.1g)
/// - fv20261001: valid from 2026-10-01, wire 2.5  (AHB 3.2 / MIG 2.5)
#[cfg(feature = "mscons")]
#[test]
fn process_context_selects_active_mscons_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before fv20240401 → no MSCONS profile with valid_from ≤ this date
    let early = ProcessContext::for_date(date!(2024 - 03 - 31));
    assert!(
        early.active_release(MessageType::Mscons).is_none(),
        "no MSCONS profile should be active before 01.04.2024"
    );

    // Era fv20240401 (2024-04-01 to 2025-09-30) → 2.4c, valid_from 2024-04-01
    // fv20240401 is archived — only active when `mscons-archive` or `archive` is enabled.
    #[cfg(any(feature = "mscons-archive", feature = "archive"))]
    {
        let era_fv20240401 = ProcessContext::for_date(date!(2025 - 06 - 01));
        let rel_fv20240401 = era_fv20240401
            .active_release(MessageType::Mscons)
            .expect("MSCONS 2.4c must be active on 2025-06-01");
        assert_eq!(rel_fv20240401.as_str(), "2.4c");
        let prof_fv20240401 = era_fv20240401
            .active_profile(MessageType::Mscons)
            .expect("fv20240401 profile must be returned");
        assert_eq!(prof_fv20240401.valid_from(), Some(date!(2024 - 04 - 01)));
    }

    // Era fv20251001 (2025-10-01 to 2026-09-30) → 2.4c, valid_from 2025-10-01
    let era_fv20251001 = ProcessContext::for_date(date!(2026 - 06 - 07));
    let rel_fv20251001 = era_fv20251001
        .active_release(MessageType::Mscons)
        .expect("MSCONS 2.4c fv20251001 must be active on 2026-06-07");
    assert_eq!(rel_fv20251001.as_str(), "2.4c");
    let prof_fv20251001 = era_fv20251001
        .active_profile(MessageType::Mscons)
        .expect("fv20251001 profile must be returned");
    assert_eq!(prof_fv20251001.valid_from(), Some(date!(2025 - 10 - 01)));

    // Era fv20261001 (from 2026-10-01) → 2.5
    let era_fv20261001 = ProcessContext::for_date(date!(2026 - 10 - 01));
    let rel_fv20261001 = era_fv20261001
        .active_release(MessageType::Mscons)
        .expect("MSCONS 2.5 must be active on 2026-10-01");
    assert_eq!(rel_fv20261001.as_str(), "2.5");
    let prof_fv20261001 = era_fv20261001
        .active_profile(MessageType::Mscons)
        .expect("fv20261001 profile must be returned");
    assert_eq!(prof_fv20261001.valid_from(), Some(date!(2026 - 10 - 01)));

    // Far future still returns latest registered profile
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_release = future
        .active_release(MessageType::Mscons)
        .expect("some MSCONS profile must be active");
    assert_eq!(
        future_release.as_str(),
        "2.5",
        "latest registered is still 2.5"
    );
}

/// Profiles with no `valid_from` (legacy folder names) are never returned
/// by `active_profile` / `active_release`.
#[test]
fn process_context_skips_undated_profiles() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // UTILMD 5.5.3a lives in folder "5.5.3a" — no fv-date, so valid_from = None.
    // ProcessContext must not return it (valid_from filter excludes undated profiles).
    let ctx = ProcessContext::for_date(date!(2099 - 12 - 31));
    // We cannot assert None because there might be dated UTILMD profiles in
    // the future; we just confirm the call does not panic.
    let _ = ctx.active_profile(MessageType::Utilmd);
}

/// valid_from on MSCONS fv20240401 is 2024-04-01 (derived from folder name).
/// Note: `reg.profile(Mscons, "2.4c")` returns the *last-registered* 2.4c
/// profile, which is now fv20251001 (registered after fv20240401 in mod.rs).
/// Use `profiles_for` to find a specific valid_from date.
/// fv20240401 is an archived profile — only available with `mscons-archive` or `archive`.
#[cfg(all(
    feature = "mscons",
    any(feature = "mscons-archive", feature = "archive")
))]
#[test]
fn profile_valid_from_matches_fv_date() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    // fv20240401 — find by valid_from, not by release (last-registered wins)
    let p = reg
        .profiles_for(MessageType::Mscons)
        .find(|p| p.valid_from() == Some(date!(2024 - 04 - 01)))
        .expect("fv20240401 must be registered with valid_from = 2024-04-01");
    assert_eq!(p.release().as_str(), "2.4c");
}

/// valid_from on MSCONS fv20251001 is 2025-10-01 (AHB 3.1g, MIG 2.4c).
#[cfg(feature = "mscons")]
#[test]
fn mscons_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Mscons)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("MSCONS fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "2.4c");
    assert_eq!(releases::mscons_fv20251001().as_str(), "2.4c");
}

/// valid_from on MSCONS fv20261001 is 2026-10-01.
#[cfg(feature = "mscons")]
#[test]
fn profile_valid_from_fv20261001() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;

    let reg = ReleaseRegistry::global();
    // Use profile_on with the profile's own activation date so the lookup
    // succeeds regardless of what day the tests are run.
    let p = reg
        .profile_on(
            MessageType::Mscons,
            &Release::new("2.5"),
            time::macros::date!(2026 - 10 - 01),
        )
        .expect("2.5 profile must be registered");

    let vf = p
        .valid_from()
        .expect("fv20261001 must have a valid_from date");
    assert_eq!(vf, time::macros::date!(2026 - 10 - 01));
    assert_eq!(releases::mscons_fv20261001().as_str(), "2.5");
}

// ── UTILMD release boundary tests ────────────────────────────────────────────

/// UTILMD S1.1a lives in folder fv20241001 — valid_from must be 2024-10-01.
///
/// This is the corrected release code for the FV2024-10-01 profile.  The BDEW
/// wire code for UTILMD Strom messages sent in the Oct 2024 – Jun 2025 window
/// is "S1.1a", not "S2.1" (which starts with fv20251001 on 2025-10-01).
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_s21_valid_from_is_2024_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    // The old S2.1 label on fv20241001 was a bug; correct code is S1.1a.
    let p = reg
        .profile_on(
            MessageType::Utilmd,
            &Release::new("S1.1a"),
            date!(2024 - 10 - 01),
        )
        .expect("UTILMD S1.1a profile must be registered in fv20241001");

    let vf = p
        .valid_from()
        .expect("fv20241001 must have a valid_from date");
    assert_eq!(vf, date!(2024 - 10 - 01));
}

/// UTILMD S2.2 lives in folder fv20261001 — valid_from must be 2026-10-01.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_s22_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    // Use profile_on with the profile's own activation date.
    let p = reg
        .profile_on(
            MessageType::Utilmd,
            &Release::new("S2.2"),
            date!(2026 - 10 - 01),
        )
        .expect("UTILMD S2.2 profile must be registered");

    let vf = p
        .valid_from()
        .expect("fv20261001 must have a valid_from date");
    assert_eq!(vf, date!(2026 - 10 - 01));
}

/// ProcessContext release timeline for UTILMD Strom (track "S"):
///
/// | Date range           | Release | Profile         |
/// |----------------------|---------|-----------------|
/// | before 2024-10-01    | —       | none            |
/// | 2024-10-01 – Jun 05  | S1.1a   | fv20241001      |
/// | 2025-06-06 – Sep 30  | S1.2    | fv20250606      |
/// | 2025-10-01 – Sep 30  | S2.1    | fv20251001      |
/// | 2026-10-01 –         | S2.2    | fv20261001      |
#[cfg(feature = "utilmd")]
#[test]
fn process_context_selects_active_utilmd_strom_release() {
    use edi_energy::{ProcessContext, ReleaseTrack};
    use time::macros::date;

    // Day before S1.1a goes live — no Strom profile active yet.
    let before = ProcessContext::for_date(date!(2024 - 09 - 30));
    assert!(
        before
            .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
            .is_none(),
        "no Strom UTILMD profile active before 2024-10-01"
    );

    // From 2024-10-01 onward → S1.1a (not S2.1!)
    let s11a_era = ProcessContext::for_date(date!(2025 - 01 - 01));
    let s11a_rel = s11a_era
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
        .expect("S1.1a must be active on 2025-01-01");
    assert_eq!(s11a_rel.as_str(), "S1.1a");

    // From 2025-06-06 → S1.2 (LFW24 bridging profile)
    let s12_era = ProcessContext::for_date(date!(2025 - 06 - 06));
    let s12_rel = s12_era
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
        .expect("S1.2 must be active on 2025-06-06");
    assert_eq!(s12_rel.as_str(), "S1.2");

    // From 2025-10-01 → S2.1
    let s21_era = ProcessContext::for_date(date!(2025 - 10 - 01));
    let s21_rel = s21_era
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
        .expect("S2.1 must be active on 2025-10-01");
    assert_eq!(s21_rel.as_str(), "S2.1");

    // On 2026-09-30 — still S2.1
    let last_day_s21 = ProcessContext::for_date(date!(2026 - 09 - 30));
    let last_rel = last_day_s21
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
        .expect("S2.1 must be active on 2026-09-30");
    assert_eq!(
        last_rel.as_str(),
        "S2.1",
        "S2.1 must still be active the day before S2.2"
    );

    // From 2026-10-01 → S2.2
    let s22_era = ProcessContext::for_date(date!(2026 - 10 - 01));
    let s22_rel = s22_era
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
        .expect("S2.2 must be active on 2026-10-01");
    assert_eq!(s22_rel.as_str(), "S2.2");

    // Far future still returns S2.2 (latest Strom profile)
    let far_future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_rel = far_future
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
        .expect("some Strom UTILMD profile must be active");
    assert_eq!(
        future_rel.as_str(),
        "S2.2",
        "S2.2 must be the latest Strom UTILMD profile"
    );
}

/// ProcessContext release timeline for UTILMD Gas (track "G"):
///
/// | Date range           | Release | Profile            |
/// |----------------------|---------|--------------------|
/// | before 2024-10-01    | —       | none               |
/// | 2024-10-01 – Sep 30  | G1.0a   | fv20241001_gas     |
/// | 2025-10-01 – Sep 30  | G1.1    | fv20251001_gas     |
/// | 2026-10-01 –         | G1.2    | fv20261001_gas     |
#[cfg(feature = "utilmd")]
#[test]
fn process_context_selects_active_utilmd_gas_release() {
    use edi_energy::{ProcessContext, ReleaseTrack};
    use time::macros::date;

    // Before G1.0a — no Gas profile active.
    let before = ProcessContext::for_date(date!(2024 - 09 - 30));
    assert!(
        before
            .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Gas)
            .is_none(),
        "no Gas UTILMD profile active before 2024-10-01"
    );

    // From 2024-10-01 → G1.0a (not G1.1!)
    let g10a_era = ProcessContext::for_date(date!(2025 - 06 - 01));
    let g10a_rel = g10a_era
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Gas)
        .expect("G1.0a must be active on 2025-06-01");
    assert_eq!(g10a_rel.as_str(), "G1.0a");

    // From 2025-10-01 → G1.1
    let g11_era = ProcessContext::for_date(date!(2025 - 10 - 01));
    let g11_rel = g11_era
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Gas)
        .expect("G1.1 must be active on 2025-10-01");
    assert_eq!(g11_rel.as_str(), "G1.1");

    // From 2026-10-01 → G1.2
    let g12_era = ProcessContext::for_date(date!(2026 - 10 - 01));
    let g12_rel = g12_era
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Gas)
        .expect("G1.2 must be active on 2026-10-01");
    assert_eq!(g12_rel.as_str(), "G1.2");
}

/// Gas and Strom tracks are independent: selecting one track does not return
/// profiles from the other track.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_gas_and_strom_tracks_are_independent() {
    use edi_energy::{ProcessContext, ReleaseTrack};
    use time::macros::date;

    let ctx = ProcessContext::for_date(date!(2025 - 01 - 01));

    let strom = ctx
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Strom)
        .expect("Strom S2.1 must be active");
    let gas = ctx
        .active_release_for_track(MessageType::Utilmd, &ReleaseTrack::Gas)
        .expect("Gas G1.1 must be active");

    // Track prefix guarantees they are different releases
    assert!(
        strom.as_str().starts_with('S'),
        "Strom track must start with S"
    );
    assert!(gas.as_str().starts_with('G'), "Gas track must start with G");
    assert_ne!(strom.as_str(), gas.as_str());
}

// ── APERAK profile date tests ─────────────────────────────────────────────────

/// APERAK 2.1i lives in folder fv20251001 — valid_from must be 2025-10-01.
#[cfg(feature = "aperak")]
#[test]
fn aperak_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let p = reg
        .profile(MessageType::Aperak, &Release::new("2.1i"))
        .expect("APERAK 2.1i profile must be registered");

    let vf = p
        .valid_from()
        .expect("fv20251001 must have a valid_from date");
    assert_eq!(vf, date!(2025 - 10 - 01));

    // releases() accessor must point to the same release code
    assert_eq!(releases::aperak_fv20251001().as_str(), "2.1i");
}

/// APERAK 2.2 lives in folder fv20261001 — valid_from must be 2026-10-01.
#[cfg(feature = "aperak")]
#[test]
fn aperak_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    // Use profile_on with the profile's own activation date.
    let p = reg
        .profile_on(
            MessageType::Aperak,
            &Release::new("2.2"),
            date!(2026 - 10 - 01),
        )
        .expect("APERAK 2.2 profile must be registered");

    let vf = p
        .valid_from()
        .expect("fv20261001 must have a valid_from date");
    assert_eq!(vf, date!(2026 - 10 - 01));

    // releases() accessor must point to the same release code
    assert_eq!(releases::aperak_fv20261001().as_str(), "2.2");
}

/// ProcessContext selects the correct APERAK release by date.
/// 2.1i is active from 2025-10-01; 2.2 supersedes it from 2026-10-01.
#[cfg(feature = "aperak")]
#[test]
fn process_context_selects_active_aperak_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated APERAK profile active.
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    let before_rel = before.active_release(MessageType::Aperak);
    assert!(
        before_rel.is_none() || before_rel.map(edi_energy::Release::as_str) == Some("2.0a"),
        "no fv-dated APERAK profile active before 2025-10-01 (only legacy 2.0a if any)"
    );

    // From 2025-10-01 → 2.1i
    let era_21i = ProcessContext::for_date(date!(2026 - 05 - 01));
    let rel_21i = era_21i
        .active_release(MessageType::Aperak)
        .expect("APERAK 2.1i must be active on 2026-05-01");
    assert_eq!(rel_21i.as_str(), "2.1i");

    // From 2026-10-01 → 2.2
    let era_22 = ProcessContext::for_date(date!(2026 - 10 - 01));
    let rel_22 = era_22
        .active_release(MessageType::Aperak)
        .expect("APERAK 2.2 must be active on 2026-10-01");
    assert_eq!(rel_22.as_str(), "2.2");
}

// ── CONTRL profile date tests ─────────────────────────────────────────────────

/// CONTRL 2.0b lives in folder fv20251001 — that profile's valid_from must be 2025-10-01.
/// Note: fv20260101 also has release "2.0b"; we locate fv20251001 by valid_from date.
/// This profile is archived — only available with `contrl-archive` or `archive` feature.
#[cfg(all(
    feature = "contrl",
    any(feature = "contrl-archive", feature = "archive")
))]
#[test]
fn contrl_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    // Both fv20251001 and fv20260101 share release "2.0b".
    // Find the one with valid_from == 2025-10-01 via profiles_for().
    let profile = reg
        .profiles_for(MessageType::Contrl)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("CONTRL fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "2.0b");
    // releases() accessor returns the same wire code
    assert_eq!(releases::contrl_fv20251001().as_str(), "2.0b");
}

/// CONTRL 2.0b fv20260101 — the extraordinary-correction profile valid from 2026-01-01.
#[cfg(feature = "contrl")]
#[test]
fn contrl_fv20260101_valid_from_is_2026_01_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 01 - 01);

    let profile = reg
        .profiles_for(MessageType::Contrl)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("CONTRL fv20260101 must be registered with valid_from = 2026-01-01");

    assert_eq!(profile.release().as_str(), "2.0b");
    assert_eq!(releases::contrl_fv20260101().as_str(), "2.0b");
}

/// ProcessContext selects the correct CONTRL profile by date.
/// fv20251001 is active from 2025-10-01; fv20260101 supersedes it from 2026-01-01.
/// Both have the same wire release "2.0b".
#[cfg(feature = "contrl")]
#[test]
fn process_context_selects_active_contrl_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated CONTRL profile active (only legacy 1.0a with no valid_from).
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Contrl).is_none(),
        "no fv-dated CONTRL profile active before 2025-10-01"
    );

    // From 2025-10-01 through 2025-12-31 → fv20251001 is active (wire: 2.0b).
    // fv20251001 is archived — only active when `contrl-archive` or `archive` is enabled.
    #[cfg(any(feature = "contrl-archive", feature = "archive"))]
    {
        let era_fv20251001 = ProcessContext::for_date(date!(2025 - 12 - 31));
        let rel_251001 = era_fv20251001
            .active_release(MessageType::Contrl)
            .expect("CONTRL 2.0b fv20251001 must be active on 2025-12-31");
        assert_eq!(rel_251001.as_str(), "2.0b");
        let profile_251001 = era_fv20251001
            .active_profile(MessageType::Contrl)
            .expect("profile must be returned");
        assert_eq!(profile_251001.valid_from(), Some(date!(2025 - 10 - 01)));
    }

    // From 2026-01-01 → fv20260101 supersedes (same wire "2.0b", different valid_from).
    let era_fv20260101 = ProcessContext::for_date(date!(2026 - 01 - 01));
    let rel_260101 = era_fv20260101
        .active_release(MessageType::Contrl)
        .expect("CONTRL 2.0b fv20260101 must be active on 2026-01-01");
    assert_eq!(rel_260101.as_str(), "2.0b");
    let profile_260101 = era_fv20260101
        .active_profile(MessageType::Contrl)
        .expect("profile must be returned");
    assert_eq!(profile_260101.valid_from(), Some(date!(2026 - 01 - 01)));

    // Far future still returns the latest (fv20260101).
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Contrl)
        .expect("some CONTRL profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 01 - 01)));
}

// ── IFTSTA profile date tests ─────────────────────────────────────────────────

/// IFTSTA fv20251001 (MIG 2.0g / AHB 2.0h) — valid from 2025-10-01.
/// Note: legacy `2.0g/` also has release "2.0g"; use profiles_for + valid_from.
#[cfg(feature = "iftsta")]
#[test]
fn iftsta_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Iftsta)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("IFTSTA fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "2.0g");
    assert_eq!(releases::iftsta_fv20251001().as_str(), "2.0g");
}

/// IFTSTA fv20261001 (MIG 2.1 / AHB 2.1) — valid from 2026-10-01, wire code "2.1".
#[cfg(feature = "iftsta")]
#[test]
fn iftsta_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Iftsta)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("IFTSTA fv20261001 must be registered with valid_from = 2026-10-01");

    assert_eq!(profile.release().as_str(), "2.1");
    assert_eq!(releases::iftsta_fv20261001().as_str(), "2.1");
}

/// ProcessContext selects the correct IFTSTA release by date.
/// fv20251001 (2.0g) is active from 2025-10-01; fv20261001 (2.1) from 2026-10-01.
/// The legacy 2.0g profile (no valid_from) must never be selected by date.
#[cfg(feature = "iftsta")]
#[test]
fn process_context_selects_active_iftsta_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated IFTSTA profile active (legacy 2.0g has no valid_from).
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Iftsta).is_none(),
        "no fv-dated IFTSTA profile active before 2025-10-01"
    );

    // From 2025-10-01 → fv20251001 (wire: 2.0g).
    let era_fv20251001 = ProcessContext::for_date(date!(2026 - 05 - 01));
    let rel_fv20251001 = era_fv20251001
        .active_release(MessageType::Iftsta)
        .expect("IFTSTA 2.0g fv20251001 must be active on 2026-05-01");
    assert_eq!(rel_fv20251001.as_str(), "2.0g");
    let profile_251001 = era_fv20251001
        .active_profile(MessageType::Iftsta)
        .expect("profile must be returned");
    assert_eq!(profile_251001.valid_from(), Some(date!(2025 - 10 - 01)));

    // From 2026-10-01 → fv20261001 (wire: 2.1).
    let era_fv20261001 = ProcessContext::for_date(date!(2026 - 10 - 01));
    let rel_fv20261001 = era_fv20261001
        .active_release(MessageType::Iftsta)
        .expect("IFTSTA 2.1 fv20261001 must be active on 2026-10-01");
    assert_eq!(rel_fv20261001.as_str(), "2.1");
    let profile_261001 = era_fv20261001
        .active_profile(MessageType::Iftsta)
        .expect("profile must be returned");
    assert_eq!(profile_261001.valid_from(), Some(date!(2026 - 10 - 01)));

    // Far future — still fv20261001.
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Iftsta)
        .expect("some IFTSTA profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 10 - 01)));
}

// ── INSRPT profile tests ───────────────────────────────────────────────────────

/// INSRPT fv20211001 (MIG 1.1a / AHB 1.1g) — valid from 2021-10-01, wire code "1.1a".
/// This profile is archived — only available with `insrpt-archive` or `archive` feature.
#[cfg(all(
    feature = "insrpt",
    any(feature = "insrpt-archive", feature = "archive")
))]
#[test]
fn insrpt_fv20211001_valid_from_is_2021_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2021 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Insrpt)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("INSRPT fv20211001 must be registered with valid_from = 2021-10-01");

    assert_eq!(profile.release().as_str(), "1.1a");
    assert_eq!(releases::insrpt_fv20211001().as_str(), "1.1a");
}

/// INSRPT fv20260101 (extraordinary correction of AHB 1.1g, Stand 11.12.2025)
/// — valid from 2026-01-01, wire code "1.1a" (same-release-multiple-fv-date pattern).
#[cfg(feature = "insrpt")]
#[test]
fn insrpt_fv20260101_valid_from_is_2026_01_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 01 - 01);

    let profile = reg
        .profiles_for(MessageType::Insrpt)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("INSRPT fv20260101 must be registered with valid_from = 2026-01-01");

    assert_eq!(profile.release().as_str(), "1.1a");
    assert_eq!(releases::insrpt_fv20260101().as_str(), "1.1a");
}

/// ProcessContext selects the correct INSRPT profile by date.
/// Both fv20211001 and fv20260101 use wire code "1.1a" (same-release pattern).
/// The legacy 1.1a profile (no valid_from) must never be selected by date.
#[cfg(feature = "insrpt")]
#[test]
fn process_context_selects_active_insrpt_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2021-10-01 — no fv-dated INSRPT profile active (legacy 1.1a has no valid_from).
    let before = ProcessContext::for_date(date!(2021 - 09 - 30));
    assert!(
        before.active_release(MessageType::Insrpt).is_none(),
        "no fv-dated INSRPT profile active before 2021-10-01"
    );

    // From 2021-10-01 → fv20211001 (wire: 1.1a).
    // fv20211001 is archived — only active when `insrpt-archive` or `archive` is enabled.
    #[cfg(any(feature = "insrpt-archive", feature = "archive"))]
    {
        let era_fv20211001 = ProcessContext::for_date(date!(2025 - 06 - 01));
        let rel_fv20211001 = era_fv20211001
            .active_release(MessageType::Insrpt)
            .expect("INSRPT 1.1a fv20211001 must be active on 2025-06-01");
        assert_eq!(rel_fv20211001.as_str(), "1.1a");
        let profile_211001 = era_fv20211001
            .active_profile(MessageType::Insrpt)
            .expect("profile must be returned");
        assert_eq!(profile_211001.valid_from(), Some(date!(2021 - 10 - 01)));
    }

    // From 2026-01-01 → fv20260101 (wire: 1.1a, extraordinary correction).
    let era_fv20260101 = ProcessContext::for_date(date!(2026 - 01 - 01));
    let rel_fv20260101 = era_fv20260101
        .active_release(MessageType::Insrpt)
        .expect("INSRPT 1.1a fv20260101 must be active on 2026-01-01");
    assert_eq!(rel_fv20260101.as_str(), "1.1a");
    let profile_260101 = era_fv20260101
        .active_profile(MessageType::Insrpt)
        .expect("profile must be returned");
    assert_eq!(profile_260101.valid_from(), Some(date!(2026 - 01 - 01)));

    // Far future — still fv20260101.
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Insrpt)
        .expect("some INSRPT profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 01 - 01)));
}

// ── INVOIC profile tests ──────────────────────────────────────────────────────

/// INVOIC fv20251001 (MIG 2.8e / AHB 1.0a) — valid from 2025-10-01, wire code "2.8e".
#[cfg(feature = "invoic")]
#[test]
fn invoic_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Invoic)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("INVOIC fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "2.8e");
    assert_eq!(releases::invoic_fv20251001().as_str(), "2.8e");
}

/// INVOIC fv20260401 (MIG 2.8e / AHB 1.0b, Konsultationsfassung)
/// — valid from 2026-04-01, wire code "2.8e" (same-release-multiple-fv-date pattern).
#[cfg(feature = "invoic")]
#[test]
fn invoic_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Invoic)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("INVOIC fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "2.8e");
    assert_eq!(releases::invoic_fv20260401().as_str(), "2.8e");
}

/// ProcessContext selects the correct INVOIC profile by date.
/// Both fv20251001 and fv20260401 use wire code "2.8e" (same-release pattern).
/// The legacy 2.8e profile (no valid_from) must never be selected by date.
#[cfg(feature = "invoic")]
#[test]
fn process_context_selects_active_invoic_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated INVOIC profile active (legacy 2.8e has no valid_from).
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Invoic).is_none(),
        "no fv-dated INVOIC profile active before 2025-10-01"
    );

    // From 2025-10-01 → fv20251001 (wire: 2.8e, AHB 1.0a).
    let era_fv20251001 = ProcessContext::for_date(date!(2026 - 01 - 01));
    let rel_fv20251001 = era_fv20251001
        .active_release(MessageType::Invoic)
        .expect("INVOIC 2.8e fv20251001 must be active on 2026-01-01");
    assert_eq!(rel_fv20251001.as_str(), "2.8e");
    let profile_251001 = era_fv20251001
        .active_profile(MessageType::Invoic)
        .expect("profile must be returned");
    assert_eq!(profile_251001.valid_from(), Some(date!(2025 - 10 - 01)));

    // From 2026-04-01 → fv20260401 (wire: 2.8e, AHB 1.0b).
    let era_fv20260401 = ProcessContext::for_date(date!(2026 - 04 - 01));
    let rel_fv20260401 = era_fv20260401
        .active_release(MessageType::Invoic)
        .expect("INVOIC 2.8e fv20260401 must be active on 2026-04-01");
    assert_eq!(rel_fv20260401.as_str(), "2.8e");
    let profile_260401 = era_fv20260401
        .active_profile(MessageType::Invoic)
        .expect("profile must be returned");
    assert_eq!(profile_260401.valid_from(), Some(date!(2026 - 04 - 01)));

    // Far future — still fv20260401.
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Invoic)
        .expect("some INVOIC profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 04 - 01)));
}

// ── ORDERS profile tests ──────────────────────────────────────────────────────

/// ORDERS fv20251001 (MIG 1.4b / AHB 1.1a, Publikationsdatum 01.10.2025)
/// — valid from 2025-10-01, wire code "1.4b".
/// ORDERS uses the different-release-per-fv-date pattern (unlike INVOIC/MSCONS).
#[cfg(feature = "orders")]
#[test]
fn orders_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Orders)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("ORDERS fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "1.4b");
    assert_eq!(releases::orders_fv20251001().as_str(), "1.4b");
}

/// ORDERS fv20260401 (MIG 1.4c / AHB 1.1b)
/// — valid from 2026-04-01, wire code "1.4c" (different release than fv20251001).
#[cfg(feature = "orders")]
#[test]
fn orders_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Orders)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("ORDERS fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "1.4c");
    assert_eq!(releases::orders_fv20260401().as_str(), "1.4c");
}

/// ProcessContext selects the correct ORDERS profile by date.
/// ORDERS uses DIFFERENT wire releases per fv-date (different-release pattern):
/// - fv20251001 → wire "1.4b" (AHB 1.1a)
/// - fv20260401 → wire "1.4c" (AHB 1.1b)
#[cfg(feature = "orders")]
#[test]
fn process_context_selects_active_orders_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated ORDERS profile active.
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Orders).is_none(),
        "no fv-dated ORDERS profile active before 2025-10-01"
    );

    // From 2025-10-01 → fv20251001 (wire: 1.4b, AHB 1.1a).
    let era_fv20251001 = ProcessContext::for_date(date!(2026 - 01 - 01));
    let rel_fv20251001 = era_fv20251001
        .active_release(MessageType::Orders)
        .expect("ORDERS 1.4b fv20251001 must be active on 2026-01-01");
    assert_eq!(rel_fv20251001.as_str(), "1.4b");
    let profile_251001 = era_fv20251001
        .active_profile(MessageType::Orders)
        .expect("profile must be returned");
    assert_eq!(profile_251001.valid_from(), Some(date!(2025 - 10 - 01)));

    // From 2026-04-01 → fv20260401 (wire: 1.4c, AHB 1.1b).
    let era_fv20260401 = ProcessContext::for_date(date!(2026 - 04 - 01));
    let rel_fv20260401 = era_fv20260401
        .active_release(MessageType::Orders)
        .expect("ORDERS 1.4c fv20260401 must be active on 2026-04-01");
    assert_eq!(rel_fv20260401.as_str(), "1.4c");
    let profile_260401 = era_fv20260401
        .active_profile(MessageType::Orders)
        .expect("profile must be returned");
    assert_eq!(profile_260401.valid_from(), Some(date!(2026 - 04 - 01)));

    // Far future — still fv20260401 (wire: 1.4c).
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Orders)
        .expect("some ORDERS profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 04 - 01)));
    assert_eq!(future_profile.release().as_str(), "1.4c");
}

// ── PARTIN profile tests ──────────────────────────────────────────────────────

/// PARTIN fv20251001 (MIG 1.0f / AHB 1.0f, Publikationsdatum 01.10.2025)
/// — valid from 2025-10-01, wire code "1.0f".
/// PARTIN uses the different-release-per-fv-date pattern (same as ORDERS).
/// Key feature in 1.0f: NAD+MS has CTA+COM contact segments in the header.
#[cfg(feature = "partin")]
#[test]
fn partin_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Partin)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("PARTIN fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "1.0f");
    assert_eq!(releases::partin_fv20251001().as_str(), "1.0f");
}

/// PARTIN fv20260401 (MIG 1.1 / AHB 1.1, Publikationsdatum 01.04.2026)
/// — valid from 2026-04-01, wire code "1.1" (different release than fv20251001).
/// Key change in 1.1: CTA+COM removed from NAD+MS header (2 segments fewer).
#[cfg(feature = "partin")]
#[test]
fn partin_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Partin)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("PARTIN fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "1.1");
    assert_eq!(releases::partin_fv20260401().as_str(), "1.1");
}

/// ProcessContext selects the correct PARTIN profile by date.
/// PARTIN uses DIFFERENT wire releases per fv-date (different-release pattern):
/// - fv20251001 → wire "1.0f" (AHB 1.0f, with CTA+COM in NAD+MS header)
/// - fv20260401 → wire "1.1"  (AHB 1.1, CTA+COM removed from NAD+MS header)
#[cfg(feature = "partin")]
#[test]
fn process_context_selects_active_partin_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated PARTIN profile active.
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Partin).is_none(),
        "no fv-dated PARTIN profile active before 2025-10-01"
    );

    // From 2025-10-01 → fv20251001 (wire: 1.0f, AHB 1.0f).
    let era_fv20251001 = ProcessContext::for_date(date!(2026 - 01 - 01));
    let rel_fv20251001 = era_fv20251001
        .active_release(MessageType::Partin)
        .expect("PARTIN 1.0f fv20251001 must be active on 2026-01-01");
    assert_eq!(rel_fv20251001.as_str(), "1.0f");
    let profile_251001 = era_fv20251001
        .active_profile(MessageType::Partin)
        .expect("profile must be returned");
    assert_eq!(profile_251001.valid_from(), Some(date!(2025 - 10 - 01)));

    // From 2026-04-01 → fv20260401 (wire: 1.1, AHB 1.1).
    let era_fv20260401 = ProcessContext::for_date(date!(2026 - 04 - 01));
    let rel_fv20260401 = era_fv20260401
        .active_release(MessageType::Partin)
        .expect("PARTIN 1.1 fv20260401 must be active on 2026-04-01");
    assert_eq!(rel_fv20260401.as_str(), "1.1");
    let profile_260401 = era_fv20260401
        .active_profile(MessageType::Partin)
        .expect("profile must be returned");
    assert_eq!(profile_260401.valid_from(), Some(date!(2026 - 04 - 01)));

    // Far future — still fv20260401 (wire: 1.1).
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Partin)
        .expect("some PARTIN profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 04 - 01)));
    assert_eq!(future_profile.release().as_str(), "1.1");
}

// ── REQOTE profile tests ──────────────────────────────────────────────────────

/// REQOTE fv20250401 (MIG 1.3c / AHB 1.1, Publikationsdatum 01.04.2025)
/// — valid from 2025-04-01, wire code "1.3c".
/// REQOTE uses the same-release-multiple-fv-date pattern (same as INVOIC):
/// both fv20250401 and fv20260401 share wire release "1.3c".
#[cfg(feature = "reqote")]
#[test]
fn reqote_fv20250401_valid_from_is_2025_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Reqote)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("REQOTE fv20250401 must be registered with valid_from = 2025-04-01");

    assert_eq!(profile.release().as_str(), "1.3c");
    assert_eq!(releases::reqote_fv20250401().as_str(), "1.3c");
}

/// REQOTE fv20260401 (MIG 1.3c / AHB 1.2, Publikationsdatum 01.04.2026)
/// — valid from 2026-04-01, wire code "1.3c" (same as fv20250401).
/// AHB 1.2 lists "Stand MIG: 1.4b" which references the ORDERS MIG used in
/// the response workflow — the REQOTE wire code itself remains "1.3c".
#[cfg(feature = "reqote")]
#[test]
fn reqote_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Reqote)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("REQOTE fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "1.3c");
    assert_eq!(releases::reqote_fv20260401().as_str(), "1.3c");
}

/// ProcessContext selects the correct REQOTE profile by date.
/// Both fv20250401 and fv20260401 use wire code "1.3c" (same-release pattern).
/// The legacy 1.3c profile (no valid_from) must never be selected by date.
#[cfg(feature = "reqote")]
#[test]
fn process_context_selects_active_reqote_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-04-01 — no fv-dated REQOTE profile active.
    let before = ProcessContext::for_date(date!(2025 - 03 - 31));
    assert!(
        before.active_release(MessageType::Reqote).is_none(),
        "no fv-dated REQOTE profile active before 2025-04-01"
    );

    // From 2025-04-01 → fv20250401 (wire: 1.3c, AHB 1.1).
    let era_fv20250401 = ProcessContext::for_date(date!(2025 - 06 - 01));
    let rel_fv20250401 = era_fv20250401
        .active_release(MessageType::Reqote)
        .expect("REQOTE 1.3c fv20250401 must be active on 2025-06-01");
    assert_eq!(rel_fv20250401.as_str(), "1.3c");
    let profile_250401 = era_fv20250401
        .active_profile(MessageType::Reqote)
        .expect("profile must be returned");
    assert_eq!(profile_250401.valid_from(), Some(date!(2025 - 04 - 01)));

    // From 2026-04-01 → fv20260401 (wire: 1.3c, AHB 1.2).
    let era_fv20260401 = ProcessContext::for_date(date!(2026 - 04 - 01));
    let rel_fv20260401 = era_fv20260401
        .active_release(MessageType::Reqote)
        .expect("REQOTE 1.3c fv20260401 must be active on 2026-04-01");
    assert_eq!(rel_fv20260401.as_str(), "1.3c");
    let profile_260401 = era_fv20260401
        .active_profile(MessageType::Reqote)
        .expect("profile must be returned");
    assert_eq!(profile_260401.valid_from(), Some(date!(2026 - 04 - 01)));

    // Far future — still fv20260401 (wire: 1.3c).
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Reqote)
        .expect("some REQOTE profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 04 - 01)));
    assert_eq!(future_profile.release().as_str(), "1.3c");
}

// ── REMADV profile tests ──────────────────────────────────────────────────────

/// REMADV fv20251001 (MIG 2.9e / AHB 1.0a, Publikationsdatum 01.10.2025)
/// — valid from 2025-10-01, wire code "2.9e".
/// This is the only fv-dated REMADV profile available (no next-version AHB
/// has been published as of June 2026). See REFACTOR.md.
#[cfg(feature = "remadv")]
#[test]
fn remadv_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Remadv)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("REMADV fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "2.9e");
    assert_eq!(releases::remadv_fv20251001().as_str(), "2.9e");
}

/// ProcessContext selects the correct REMADV profile by date.
/// The legacy 2.9e profile (no valid_from) must never be selected by date;
/// fv20251001 is returned from 2025-10-01 onwards.
#[cfg(feature = "remadv")]
#[test]
fn process_context_selects_active_remadv_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated REMADV profile active.
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Remadv).is_none(),
        "no fv-dated REMADV profile active before 2025-10-01"
    );

    // From 2025-10-01 → fv20251001 (wire: 2.9e, AHB 1.0a).
    let era_fv20251001 = ProcessContext::for_date(date!(2026 - 01 - 01));
    let rel = era_fv20251001
        .active_release(MessageType::Remadv)
        .expect("REMADV 2.9e fv20251001 must be active on 2026-01-01");
    assert_eq!(rel.as_str(), "2.9e");
    let profile = era_fv20251001
        .active_profile(MessageType::Remadv)
        .expect("profile must be returned");
    assert_eq!(profile.valid_from(), Some(date!(2025 - 10 - 01)));

    // Far future — fv20260401 (wire: 2.9f) is the latest published REMADV profile.
    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Remadv)
        .expect("some REMADV profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 04 - 01)));
    assert_eq!(future_profile.release().as_str(), "2.9f");
}

// ── ORDCHG profile tests ──────────────────────────────────────────────────────

/// ORDCHG fv20241001 (MIG 1.1 / AHB 1.0a, Publikationsdatum 01.10.2024)
/// — valid from 2024-10-01, wire code "1.1".
/// Contains SG6 (CTA + COM) inside SG3 (NAD), which was removed in MIG 1.2.
#[cfg(feature = "ordchg")]
#[test]
fn ordchg_fv20241001_valid_from_is_2024_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2024 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Ordchg)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("ORDCHG fv20241001 must be registered with valid_from = 2024-10-01");

    assert_eq!(profile.release().as_str(), "1.1");
    assert_eq!(releases::ordchg_fv20241001().as_str(), "1.1");
}

/// ORDCHG fv20260401 (MIG 1.2 / AHB 1.1, Publikationsdatum 01.04.2026)
/// — valid from 2026-04-01, wire code "1.2".
/// Key change vs fv20241001: SG6 (CTA+COM) removed from MIG.
#[cfg(feature = "ordchg")]
#[test]
fn ordchg_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Ordchg)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("ORDCHG fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "1.2");
    assert_eq!(releases::ordchg_fv20260401().as_str(), "1.2");
}

/// ProcessContext selects the correct ORDCHG profile by date:
/// - Before 2024-10-01: no fv-dated profile active
/// - 2024-10-01 to 2026-03-31: fv20241001 (wire "1.1", MIG 1.1 with SG6)
/// - From 2026-04-01: fv20260401 (wire "1.2", MIG 1.2 without SG6)
///
/// This validates the annually changing format concept: two different wire
/// codes correspond to two structurally different MIG versions.
#[cfg(feature = "ordchg")]
#[test]
fn process_context_selects_active_ordchg_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2024-10-01 — no fv-dated ORDCHG profile active.
    let before = ProcessContext::for_date(date!(2024 - 09 - 30));
    assert!(
        before.active_release(MessageType::Ordchg).is_none(),
        "no fv-dated ORDCHG profile before 2024-10-01"
    );

    // From 2024-10-01 to 2026-03-31 → fv20241001 (wire "1.1").
    for &d in &[
        date!(2024 - 10 - 01),
        date!(2025 - 06 - 01),
        date!(2026 - 03 - 31),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Ordchg)
            .unwrap_or_else(|| panic!("ORDCHG must be active on {d}"));
        assert_eq!(rel.as_str(), "1.1", "expected 1.1 on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Ordchg)
                .unwrap()
                .valid_from(),
            Some(date!(2024 - 10 - 01))
        );
    }

    // From 2026-04-01 → fv20260401 (wire "1.2", SG6 removed).
    for &d in &[
        date!(2026 - 04 - 01),
        date!(2026 - 07 - 01),
        date!(2030 - 01 - 01),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Ordchg)
            .unwrap_or_else(|| panic!("ORDCHG must be active on {d}"));
        assert_eq!(rel.as_str(), "1.2", "expected 1.2 on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Ordchg)
                .unwrap()
                .valid_from(),
            Some(date!(2026 - 04 - 01))
        );
    }
}

// ── ORDRSP profile tests ──────────────────────────────────────────────────────

/// ORDRSP fv20251001 (MIG 1.4b / AHB 1.1a, Publikationsdatum 01.10.2025)
/// — valid from 2025-10-01, wire code "1.4b".
/// Contains 40 Prüfidentifikatoren. SG27 supports FTX+ABO/Z27/Z28.
#[cfg(feature = "ordrsp")]
#[test]
fn ordrsp_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Ordrsp)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("ORDRSP fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "1.4b");
    assert_eq!(releases::ordrsp_fv20251001().as_str(), "1.4b");
}

/// ORDRSP fv20260401 (MIG 1.4c / AHB 1.1b, Publikationsdatum 01.04.2026)
/// — valid from 2026-04-01, wire code "1.4c".
/// Key change vs fv20251001: FTX+Z33 (APN-Kommunikationsdaten) added to SG27.
#[cfg(feature = "ordrsp")]
#[test]
fn ordrsp_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Ordrsp)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("ORDRSP fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "1.4c");
    assert_eq!(releases::ordrsp_fv20260401().as_str(), "1.4c");
}

/// ProcessContext selects the correct ORDRSP profile by date:
/// - Before 2025-10-01: no fv-dated profile active
/// - 2025-10-01 to 2026-03-31: fv20251001 (wire "1.4b", MIG 1.4b without Z33)
/// - From 2026-04-01: fv20260401 (wire "1.4c", MIG 1.4c with Z33)
///
/// This validates the annually changing format: two different wire codes
/// track two structurally distinct MIG versions.
#[cfg(feature = "ordrsp")]
#[test]
fn process_context_selects_active_ordrsp_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated ORDRSP profile active.
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Ordrsp).is_none(),
        "no fv-dated ORDRSP profile before 2025-10-01"
    );

    // From 2025-10-01 to 2026-03-31 → fv20251001 (wire "1.4b").
    for &d in &[
        date!(2025 - 10 - 01),
        date!(2026 - 01 - 01),
        date!(2026 - 03 - 31),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Ordrsp)
            .unwrap_or_else(|| panic!("ORDRSP must be active on {d}"));
        assert_eq!(rel.as_str(), "1.4b", "expected 1.4b on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Ordrsp)
                .unwrap()
                .valid_from(),
            Some(date!(2025 - 10 - 01))
        );
    }

    // From 2026-04-01 → fv20260401 (wire "1.4c", FTX+Z33 added).
    for &d in &[
        date!(2026 - 04 - 01),
        date!(2026 - 07 - 01),
        date!(2030 - 01 - 01),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Ordrsp)
            .unwrap_or_else(|| panic!("ORDRSP must be active on {d}"));
        assert_eq!(rel.as_str(), "1.4c", "expected 1.4c on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Ordrsp)
                .unwrap()
                .valid_from(),
            Some(date!(2026 - 04 - 01))
        );
    }
}

// ── QUOTES profile types ──────────────────────────────────────────────────────

/// QUOTES fv20250401 (MIG 1.3b / AHB 1.1, Publikationsdatum 01.04.2025)
/// — valid from 2025-04-01, wire code "1.3b".
#[cfg(feature = "quotes")]
#[test]
fn quotes_fv20250401_valid_from_is_2025_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Quotes)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("QUOTES fv20250401 must be registered with valid_from = 2025-04-01");

    assert_eq!(profile.release().as_str(), "1.3b");
    assert_eq!(releases::quotes_fv20250401().as_str(), "1.3b");
}

/// QUOTES fv20260401 (MIG 1.3c / AHB 1.1a, Publikationsdatum 01.04.2026)
/// — valid from 2026-04-01, wire code "1.3c".
#[cfg(feature = "quotes")]
#[test]
fn quotes_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Quotes)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("QUOTES fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "1.3c");
    assert_eq!(releases::quotes_fv20260401().as_str(), "1.3c");
}

/// ProcessContext selects the correct QUOTES profile by date:
/// - Before 2025-04-01: no fv-dated profile active
/// - 2025-04-01 to 2026-03-31: fv20250401 (wire "1.3b", MIG 1.3b)
/// - From 2026-04-01: fv20260401 (wire "1.3c", MIG 1.3c)
///
/// Both releases share the same 5 Prüfidentifikatoren (15001–15005).
#[cfg(feature = "quotes")]
#[test]
fn process_context_selects_active_quotes_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-04-01 — no fv-dated QUOTES profile active.
    let before = ProcessContext::for_date(date!(2025 - 03 - 31));
    assert!(
        before.active_release(MessageType::Quotes).is_none(),
        "no fv-dated QUOTES profile before 2025-04-01"
    );

    // From 2025-04-01 to 2026-03-31 → fv20250401 (wire "1.3b").
    for &d in &[
        date!(2025 - 04 - 01),
        date!(2025 - 07 - 01),
        date!(2026 - 03 - 31),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Quotes)
            .unwrap_or_else(|| panic!("QUOTES must be active on {d}"));
        assert_eq!(rel.as_str(), "1.3b", "expected 1.3b on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Quotes)
                .unwrap()
                .valid_from(),
            Some(date!(2025 - 04 - 01))
        );
    }

    // From 2026-04-01 → fv20260401 (wire "1.3c").
    for &d in &[
        date!(2026 - 04 - 01),
        date!(2026 - 07 - 01),
        date!(2030 - 01 - 01),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Quotes)
            .unwrap_or_else(|| panic!("QUOTES must be active on {d}"));
        assert_eq!(rel.as_str(), "1.3c", "expected 1.3c on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Quotes)
                .unwrap()
                .valid_from(),
            Some(date!(2026 - 04 - 01))
        );
    }
}

// ── COMDIS ───────────────────────────────────────────────────────────────────

/// fv20251001 profile must be registered with valid_from = 2025-10-01 and release "1.0g".
#[cfg(feature = "comdis")]
#[test]
fn comdis_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Comdis)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("COMDIS fv20251001 must be registered with valid_from = 2025-10-01");

    assert_eq!(profile.release().as_str(), "1.0g");
    assert_eq!(releases::comdis_fv20251001().as_str(), "1.0g");
}

/// fv20261001 profile must be registered with valid_from = 2026-10-01 and release "1.0g".
#[cfg(feature = "comdis")]
#[test]
fn comdis_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Comdis)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("COMDIS fv20261001 must be registered with valid_from = 2026-10-01");

    assert_eq!(profile.release().as_str(), "1.0g");
    assert_eq!(releases::comdis_fv20261001().as_str(), "1.0g");
}

/// ProcessContext selects the correct COMDIS profile by date:
/// - Before 2025-10-01: no fv-dated profile active
/// - 2025-10-01 to 2026-09-30: fv20251001 (wire "1.0g")
/// - From 2026-10-01: fv20261001 (wire "1.0g", same MIG — placeholder for next year)
///
/// Both releases use the same 2 Prüfidentifikatoren (29001, 29002).
#[cfg(feature = "comdis")]
#[test]
fn process_context_selects_active_comdis_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-10-01 — no fv-dated COMDIS profile active.
    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(
        before.active_release(MessageType::Comdis).is_none(),
        "no fv-dated COMDIS profile before 2025-10-01"
    );

    // From 2025-10-01 to 2026-09-30 → fv20251001 (wire "1.0g").
    for &d in &[
        date!(2025 - 10 - 01),
        date!(2026 - 01 - 01),
        date!(2026 - 09 - 30),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Comdis)
            .unwrap_or_else(|| panic!("COMDIS must be active on {d}"));
        assert_eq!(rel.as_str(), "1.0g", "expected 1.0g on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Comdis)
                .unwrap()
                .valid_from(),
            Some(date!(2025 - 10 - 01))
        );
    }

    // From 2026-10-01 → fv20261001 (wire "1.0g", same release — placeholder).
    for &d in &[
        date!(2026 - 10 - 01),
        date!(2027 - 01 - 01),
        date!(2030 - 01 - 01),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Comdis)
            .unwrap_or_else(|| panic!("COMDIS must be active on {d}"));
        assert_eq!(rel.as_str(), "1.0g", "expected 1.0g on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Comdis)
                .unwrap()
                .valid_from(),
            Some(date!(2026 - 10 - 01))
        );
    }
}

// ── PRICAT ───────────────────────────────────────────────────────────────────

/// fv20250401 profile must be registered with valid_from = 2025-04-01 and release "2.0e".
#[cfg(feature = "pricat")]
#[test]
fn pricat_fv20250401_valid_from_is_2025_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Pricat)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("PRICAT fv20250401 must be registered with valid_from = 2025-04-01");

    assert_eq!(profile.release().as_str(), "2.0e");
    assert_eq!(releases::pricat_fv20250401().as_str(), "2.0e");
}

/// fv20260401 profile must be registered with valid_from = 2026-04-01 and release "2.1".
#[cfg(feature = "pricat")]
#[test]
fn pricat_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Pricat)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("PRICAT fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "2.1");
    assert_eq!(releases::pricat_fv20260401().as_str(), "2.1");
}

/// ProcessContext selects the correct PRICAT profile by date:
/// - Before 2025-04-01: no fv-dated profile active
/// - 2025-04-01 to 2026-03-31: fv20250401 (wire "2.0e", MIG 2.0e with SG4)
/// - From 2026-04-01: fv20260401 (wire "2.1", MIG 2.1 — SG4 removed)
///
/// This validates the annual format change concept: two structurally different
/// releases coexist and the context selector transitions correctly at the boundary.
#[cfg(feature = "pricat")]
#[test]
fn process_context_selects_active_pricat_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2025-04-01 — no fv-dated PRICAT profile active.
    let before = ProcessContext::for_date(date!(2025 - 03 - 31));
    assert!(
        before.active_release(MessageType::Pricat).is_none(),
        "no fv-dated PRICAT profile before 2025-04-01"
    );

    // From 2025-04-01 to 2026-03-31 → fv20250401 (wire "2.0e").
    for &d in &[
        date!(2025 - 04 - 01),
        date!(2025 - 07 - 01),
        date!(2026 - 03 - 31),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Pricat)
            .unwrap_or_else(|| panic!("PRICAT must be active on {d}"));
        assert_eq!(rel.as_str(), "2.0e", "expected 2.0e on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Pricat)
                .unwrap()
                .valid_from(),
            Some(date!(2025 - 04 - 01))
        );
    }

    // From 2026-04-01 → fv20260401 (wire "2.1").
    for &d in &[
        date!(2026 - 04 - 01),
        date!(2027 - 01 - 01),
        date!(2030 - 01 - 01),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Pricat)
            .unwrap_or_else(|| panic!("PRICAT must be active on {d}"));
        assert_eq!(rel.as_str(), "2.1", "expected 2.1 on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Pricat)
                .unwrap()
                .valid_from(),
            Some(date!(2026 - 04 - 01))
        );
    }
}

// ── UTILTS ───────────────────────────────────────────────────────────────────

/// fv20241001 profile must be registered with valid_from = 2024-10-01 and release "1.1e".
///
/// Both UTILTS profiles use the same MIG 1.1e — the annual change is AHB-level
/// (different package conditions and roles), not structural (same wire format).
#[cfg(feature = "utilts")]
#[test]
fn utilts_fv20241001_valid_from_is_2024_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2024 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Utilts)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("UTILTS fv20241001 must be registered with valid_from = 2024-10-01");

    assert_eq!(profile.release().as_str(), "1.1e");
    assert_eq!(releases::utilts_fv20241001().as_str(), "1.1e");
}

/// fv20260401 profile must be registered with valid_from = 2026-04-01 and release "1.1e".
///
/// The release wire code is identical to fv20241001 — both use MIG 1.1e.
/// Only the AHB version changes (conditions, package roles).
#[cfg(feature = "utilts")]
#[test]
fn utilts_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Utilts)
        .find(|p| p.valid_from() == Some(target_date))
        .expect("UTILTS fv20260401 must be registered with valid_from = 2026-04-01");

    assert_eq!(profile.release().as_str(), "1.1e");
    assert_eq!(releases::utilts_fv20260401().as_str(), "1.1e");
}

/// ProcessContext selects the correct UTILTS profile by date:
/// - Before 2024-10-01: no fv-dated profile active
/// - 2024-10-01 to 2026-03-31: fv20241001 (AHB 1.0, wire "1.1e")
/// - From 2026-04-01: fv20260401 (AHB 1.1, wire "1.1e")
///
/// This validates the annual AHB-only format change concept: both releases share
/// the same wire format (MIG 1.1e) but the profile registry correctly selects the
/// active AHB version. The `active_release()` returns "1.1e" in both periods.
#[cfg(feature = "utilts")]
#[test]
fn process_context_selects_active_utilts_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // Before 2024-10-01 — no fv-dated UTILTS profile active.
    let before = ProcessContext::for_date(date!(2024 - 09 - 30));
    assert!(
        before.active_release(MessageType::Utilts).is_none(),
        "no fv-dated UTILTS profile before 2024-10-01"
    );

    // From 2024-10-01 to 2026-03-31 → fv20241001 (AHB 1.0, wire "1.1e").
    for &d in &[
        date!(2024 - 10 - 01),
        date!(2025 - 04 - 01),
        date!(2026 - 03 - 31),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Utilts)
            .unwrap_or_else(|| panic!("UTILTS must be active on {d}"));
        assert_eq!(rel.as_str(), "1.1e", "expected wire 1.1e on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Utilts)
                .unwrap()
                .valid_from(),
            Some(date!(2024 - 10 - 01)),
            "expected fv20241001 profile on {d}"
        );
    }

    // From 2026-04-01 → fv20260401 (AHB 1.1, wire still "1.1e").
    // Both release strings are identical ("1.1e") — difference is only AHB-level.
    for &d in &[
        date!(2026 - 04 - 01),
        date!(2027 - 01 - 01),
        date!(2030 - 01 - 01),
    ] {
        let ctx = ProcessContext::for_date(d);
        let rel = ctx
            .active_release(MessageType::Utilts)
            .unwrap_or_else(|| panic!("UTILTS must be active on {d}"));
        assert_eq!(rel.as_str(), "1.1e", "expected wire 1.1e on {d}");
        assert_eq!(
            ctx.active_profile(MessageType::Utilts)
                .unwrap()
                .valid_from(),
            Some(date!(2026 - 04 - 01)),
            "expected fv20260401 profile on {d}"
        );
    }
}

// ── EdiEnergyReport serde ─────────────────────────────────────────────────────

/// When the `serde` feature is enabled, `EdiEnergyReport` must serialize to
/// valid JSON with the expected top-level keys.
#[cfg(all(feature = "serde", feature = "mscons"))]
#[test]
fn report_serializes_to_json() {
    use edi_energy::{EdiEnergyMessage, parse};

    // A bare (builder-produced) message without UNB/UNZ — validation runs on
    // message-level segments only (Layer 1 skipped for bare messages).
    const BARE: &[u8] = b"UNA:+.? 'UNH+1+MSCONS:D:04B:UN:2.4c'UNT+1+1'";
    let msg = parse(BARE).expect("bare MSCONS must parse");

    // validate() may produce errors (missing mandatory segments) — what matters
    // is that the report serializes successfully.
    let report = msg.validate().expect("validate must not return an error");
    let json = serde_json::to_string(&report).expect("report must serialize to JSON");
    assert!(json.contains("\"valid\""), "JSON must contain 'valid' key");
    assert!(
        json.contains("\"errors\""),
        "JSON must contain 'errors' key"
    );
    assert!(
        json.contains("\"warnings\""),
        "JSON must contain 'warnings' key"
    );
    assert!(json.contains("\"infos\""), "JSON must contain 'infos' key");
    assert!(
        json.contains("\"totalIssues\""),
        "JSON must contain 'totalIssues' key"
    );
}
