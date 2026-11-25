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

    // Cross-track comparisons carry no BDEW meaning, and `same_track_cmp` is the
    // API that says so.  `Ord` stays total — it has to agree with `PartialOrd`
    // or every sorted container is entitled to misbehave.
    let strom = Release::new("S2.1");
    let dotted = Release::new("5.5.3a");
    assert_eq!(
        strom.same_track_cmp(&dotted),
        None,
        "S2.1 and 5.5.3a are on different tracks — no meaningful order"
    );
    assert_eq!(strom.partial_cmp(&dotted), Some(strom.cmp(&dotted)));
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

#[test]
fn process_context_skips_undated_profiles() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(1900 - 01 - 01));
    assert!(before.active_profile(MessageType::Utilmd).is_none());

    #[cfg(feature = "utilmd")]
    {
        let binding_date = date!(2026 - 10 - 01);
        let ctx = ProcessContext::for_date(binding_date);
        let profile = ctx
            .active_profile(MessageType::Utilmd)
            .expect("dated UTILMD profile");
        assert_eq!(profile.valid_from(), Some(binding_date));
    }
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Mscons)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing MSCONS profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.4c");
    assert_eq!(releases::mscons_fv20260401().as_str(), "2.4c");
}

#[cfg(feature = "mscons")]
#[test]
fn profile_valid_from_fv20261001() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;

    let reg = ReleaseRegistry::global();
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

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_s22_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
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

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_gas_and_strom_tracks_are_independent() {
    use edi_energy::{ProcessContext, ReleaseTrack};
    use time::macros::date;

    let ctx = ProcessContext::for_date(date!(2026 - 04 - 01));

    let strom = ctx
        .active_release_for_track(MessageType::Utilmd, ReleaseTrack::Strom)
        .expect("Strom S2.1 must be active");
    let gas = ctx
        .active_release_for_track(MessageType::Utilmd, ReleaseTrack::Gas)
        .expect("Gas G1.1 must be active");

    assert!(
        strom.as_str().starts_with('S'),
        "Strom track must start with S"
    );
    assert!(gas.as_str().starts_with('G'), "Gas track must start with G");
    assert_ne!(strom.as_str(), gas.as_str());
}

// ── APERAK profile date tests ─────────────────────────────────────────────────

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

    assert_eq!(releases::aperak_fv20251001().as_str(), "2.1i");
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
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

    assert_eq!(releases::aperak_fv20261001().as_str(), "2.2");
}

#[cfg(feature = "aperak")]
#[test]
fn process_context_selects_active_aperak_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    let before_rel = before.active_release(MessageType::Aperak);
    assert!(
        before_rel.is_none() || before_rel.map(edi_energy::Release::as_str) == Some("2.0a"),
        "no fv-dated APERAK profile active before 2025-10-01 (only legacy 2.0a if any)"
    );

    let era_21i = ProcessContext::for_date(date!(2026 - 05 - 01));
    let rel_21i = era_21i
        .active_release(MessageType::Aperak)
        .expect("active APERAK release");
    assert_eq!(rel_21i.as_str(), "2.1i");

    let era_22 = ProcessContext::for_date(date!(2026 - 10 - 01));
    let rel_22 = era_22
        .active_release(MessageType::Aperak)
        .expect("active APERAK release");
    assert_eq!(rel_22.as_str(), "2.2");
}

// ── CONTRL profile date tests ─────────────────────────────────────────────────

// ── IFTSTA profile date tests ─────────────────────────────────────────────────

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
        .unwrap_or_else(|| panic!("missing IFTSTA profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.0g");
    assert_eq!(releases::iftsta_fv20251001().as_str(), "2.0g");
}

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
        .unwrap_or_else(|| panic!("missing IFTSTA profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.1");
    assert_eq!(releases::iftsta_fv20261001().as_str(), "2.1");
}

#[cfg(feature = "iftsta")]
#[test]
fn process_context_selects_active_iftsta_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(before.active_release(MessageType::Iftsta).is_none());

    let first_context = ProcessContext::for_date(date!(2026 - 05 - 01));
    let first_release = first_context
        .active_release(MessageType::Iftsta)
        .expect("active IFTSTA release");
    assert_eq!(first_release.as_str(), "2.0g");
    let first_profile = first_context
        .active_profile(MessageType::Iftsta)
        .expect("profile must be returned");
    assert_eq!(first_profile.valid_from(), Some(date!(2025 - 10 - 01)));

    let second_context = ProcessContext::for_date(date!(2026 - 10 - 01));
    let second_release = second_context
        .active_release(MessageType::Iftsta)
        .expect("active IFTSTA release");
    assert_eq!(second_release.as_str(), "2.1");
    let second_profile = second_context
        .active_profile(MessageType::Iftsta)
        .expect("profile must be returned");
    assert_eq!(second_profile.valid_from(), Some(date!(2026 - 10 - 01)));

    let future = ProcessContext::for_date(date!(2030 - 01 - 01));
    let future_profile = future
        .active_profile(MessageType::Iftsta)
        .expect("some IFTSTA profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 10 - 01)));
}

// ── INVOIC profile tests ──────────────────────────────────────────────────────

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
        .unwrap_or_else(|| panic!("missing INVOIC profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.8e");
    assert_eq!(releases::invoic_fv20260401().as_str(), "2.8e");
}

#[cfg(feature = "invoic")]
#[test]
fn invoic_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Invoic)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing INVOIC profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.8e");
    assert_eq!(releases::invoic_fv20261001().as_str(), "2.8e");
}

#[cfg(feature = "invoic")]
#[test]
fn process_context_selects_active_invoic_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2026 - 03 - 31));
    assert!(before.active_release(MessageType::Invoic).is_none());

    let first_context = ProcessContext::for_date(date!(2026 - 07 - 01));
    let first_release = first_context
        .active_release(MessageType::Invoic)
        .expect("active INVOIC release");
    assert_eq!(first_release.as_str(), "2.8e");
    let first_profile = first_context
        .active_profile(MessageType::Invoic)
        .expect("profile must be returned");
    assert_eq!(first_profile.valid_from(), Some(date!(2026 - 04 - 01)));

    let second_context = ProcessContext::for_date(date!(2026 - 10 - 01));
    let second_release = second_context
        .active_release(MessageType::Invoic)
        .expect("active INVOIC release");
    assert_eq!(second_release.as_str(), "2.8e");
    let second_profile = second_context
        .active_profile(MessageType::Invoic)
        .expect("profile must be returned");
    assert_eq!(second_profile.valid_from(), Some(date!(2026 - 10 - 01)));

    let future = ProcessContext::for_date(date!(2030 - 07 - 01));
    let future_profile = future
        .active_profile(MessageType::Invoic)
        .expect("some INVOIC profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 10 - 01)));
}

// ── ORDERS profile tests ──────────────────────────────────────────────────────

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
        .unwrap_or_else(|| panic!("missing ORDERS profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.4b");
    assert_eq!(releases::orders_fv20260401().as_str(), "1.4b");
}

#[cfg(feature = "orders")]
#[test]
fn orders_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Orders)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing ORDERS profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.4c");
    assert_eq!(releases::orders_fv20261001().as_str(), "1.4c");
}

#[cfg(feature = "orders")]
#[test]
fn process_context_selects_active_orders_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2026 - 03 - 31));
    assert!(before.active_release(MessageType::Orders).is_none());

    let first_context = ProcessContext::for_date(date!(2026 - 07 - 01));
    let first_release = first_context
        .active_release(MessageType::Orders)
        .expect("active ORDERS release");
    assert_eq!(first_release.as_str(), "1.4b");
    let first_profile = first_context
        .active_profile(MessageType::Orders)
        .expect("profile must be returned");
    assert_eq!(first_profile.valid_from(), Some(date!(2026 - 04 - 01)));

    let second_context = ProcessContext::for_date(date!(2026 - 10 - 01));
    let second_release = second_context
        .active_release(MessageType::Orders)
        .expect("active ORDERS release");
    assert_eq!(second_release.as_str(), "1.4c");
    let second_profile = second_context
        .active_profile(MessageType::Orders)
        .expect("profile must be returned");
    assert_eq!(second_profile.valid_from(), Some(date!(2026 - 10 - 01)));

    let future = ProcessContext::for_date(date!(2030 - 07 - 01));
    let future_profile = future
        .active_profile(MessageType::Orders)
        .expect("some ORDERS profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 10 - 01)));
    assert_eq!(future_profile.release().as_str(), "1.4c");
}

// ── PARTIN profile tests ──────────────────────────────────────────────────────

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
        .unwrap_or_else(|| panic!("missing PARTIN profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.0f");
    assert_eq!(releases::partin_fv20260401().as_str(), "1.0f");
}

#[cfg(feature = "partin")]
#[test]
fn partin_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Partin)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing PARTIN profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.1");
    assert_eq!(releases::partin_fv20261001().as_str(), "1.1");
}

#[cfg(feature = "partin")]
#[test]
fn process_context_selects_active_partin_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2026 - 03 - 31));
    assert!(before.active_release(MessageType::Partin).is_none());

    let first_context = ProcessContext::for_date(date!(2026 - 07 - 01));
    let first_release = first_context
        .active_release(MessageType::Partin)
        .expect("active PARTIN release");
    assert_eq!(first_release.as_str(), "1.0f");
    let first_profile = first_context
        .active_profile(MessageType::Partin)
        .expect("profile must be returned");
    assert_eq!(first_profile.valid_from(), Some(date!(2026 - 04 - 01)));

    let second_context = ProcessContext::for_date(date!(2026 - 10 - 01));
    let second_release = second_context
        .active_release(MessageType::Partin)
        .expect("active PARTIN release");
    assert_eq!(second_release.as_str(), "1.1");
    let second_profile = second_context
        .active_profile(MessageType::Partin)
        .expect("profile must be returned");
    assert_eq!(second_profile.valid_from(), Some(date!(2026 - 10 - 01)));

    let future = ProcessContext::for_date(date!(2030 - 07 - 01));
    let future_profile = future
        .active_profile(MessageType::Partin)
        .expect("some PARTIN profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 10 - 01)));
    assert_eq!(future_profile.release().as_str(), "1.1");
}

// ── REQOTE profile tests ──────────────────────────────────────────────────────

#[cfg(feature = "reqote")]
#[test]
fn reqote_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Reqote)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing REQOTE profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.3c");
    assert_eq!(releases::reqote_fv20251001().as_str(), "1.3c");
}

#[cfg(feature = "reqote")]
#[test]
fn reqote_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Reqote)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing REQOTE profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.3c");
    assert_eq!(releases::reqote_fv20261001().as_str(), "1.3c");
}

#[cfg(feature = "reqote")]
#[test]
fn process_context_selects_active_reqote_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(before.active_release(MessageType::Reqote).is_none());

    let first_context = ProcessContext::for_date(date!(2025 - 12 - 01));
    let first_release = first_context
        .active_release(MessageType::Reqote)
        .expect("active REQOTE release");
    assert_eq!(first_release.as_str(), "1.3c");
    let first_profile = first_context
        .active_profile(MessageType::Reqote)
        .expect("profile must be returned");
    assert_eq!(first_profile.valid_from(), Some(date!(2025 - 10 - 01)));

    let second_context = ProcessContext::for_date(date!(2026 - 10 - 01));
    let second_release = second_context
        .active_release(MessageType::Reqote)
        .expect("active REQOTE release");
    assert_eq!(second_release.as_str(), "1.3c");
    let second_profile = second_context
        .active_profile(MessageType::Reqote)
        .expect("profile must be returned");
    assert_eq!(second_profile.valid_from(), Some(date!(2026 - 10 - 01)));

    let future = ProcessContext::for_date(date!(2030 - 07 - 01));
    let future_profile = future
        .active_profile(MessageType::Reqote)
        .expect("some REQOTE profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 10 - 01)));
    assert_eq!(future_profile.release().as_str(), "1.3c");
}

// ── REMADV profile tests ──────────────────────────────────────────────────────

#[cfg(feature = "remadv")]
#[test]
fn remadv_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Remadv)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing REMADV profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.9e");
    assert_eq!(releases::remadv_fv20260401().as_str(), "2.9e");
}

#[cfg(feature = "remadv")]
#[test]
fn process_context_selects_active_remadv_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2026 - 03 - 31));
    assert!(before.active_release(MessageType::Remadv).is_none());

    let first_context = ProcessContext::for_date(date!(2026 - 07 - 01));
    let rel = first_context
        .active_release(MessageType::Remadv)
        .expect("active REMADV release");
    assert_eq!(rel.as_str(), "2.9e");
    let profile = first_context
        .active_profile(MessageType::Remadv)
        .expect("profile must be returned");
    assert_eq!(profile.valid_from(), Some(date!(2026 - 04 - 01)));

    let future = ProcessContext::for_date(date!(2030 - 07 - 01));
    let future_profile = future
        .active_profile(MessageType::Remadv)
        .expect("some REMADV profile must be active");
    assert_eq!(future_profile.valid_from(), Some(date!(2026 - 04 - 01)));
    assert_eq!(future_profile.release().as_str(), "2.9e");
}

// ── ORDCHG profile tests ──────────────────────────────────────────────────────

#[cfg(feature = "ordchg")]
#[test]
fn ordchg_fv20250401_valid_from_is_2025_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Ordchg)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing ORDCHG profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.1");
    assert_eq!(releases::ordchg_fv20250401().as_str(), "1.1");
}

// ── ORDRSP profile tests ──────────────────────────────────────────────────────

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
        .unwrap_or_else(|| panic!("missing ORDRSP profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.4b");
    assert_eq!(releases::ordrsp_fv20260401().as_str(), "1.4b");
}

#[cfg(feature = "ordrsp")]
#[test]
fn ordrsp_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Ordrsp)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing ORDRSP profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.4c");
    assert_eq!(releases::ordrsp_fv20261001().as_str(), "1.4c");
}

#[cfg(feature = "ordrsp")]
#[test]
fn process_context_selects_active_ordrsp_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2026 - 03 - 31));
    assert!(before.active_release(MessageType::Ordrsp).is_none());

    for &d in &[
        date!(2026 - 04 - 01),
        date!(2026 - 07 - 01),
        date!(2026 - 09 - 30),
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
            Some(date!(2026 - 04 - 01))
        );
    }

    for &d in &[
        date!(2026 - 10 - 01),
        date!(2027 - 01 - 01),
        date!(2030 - 07 - 01),
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
            Some(date!(2026 - 10 - 01))
        );
    }
}

// ── QUOTES profile types ──────────────────────────────────────────────────────

#[cfg(feature = "quotes")]
#[test]
fn quotes_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Quotes)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing QUOTES profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.3b");
    assert_eq!(releases::quotes_fv20251001().as_str(), "1.3b");
}

#[cfg(feature = "quotes")]
#[test]
fn quotes_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Quotes)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing QUOTES profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.3c");
    assert_eq!(releases::quotes_fv20261001().as_str(), "1.3c");
}

#[cfg(feature = "quotes")]
#[test]
fn process_context_selects_active_quotes_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(before.active_release(MessageType::Quotes).is_none());

    for &d in &[
        date!(2025 - 10 - 01),
        date!(2026 - 01 - 01),
        date!(2026 - 09 - 30),
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
            Some(date!(2025 - 10 - 01))
        );
    }

    for &d in &[
        date!(2026 - 10 - 01),
        date!(2027 - 01 - 01),
        date!(2030 - 07 - 01),
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
            Some(date!(2026 - 10 - 01))
        );
    }
}

// ── COMDIS ───────────────────────────────────────────────────────────────────

#[cfg(feature = "comdis")]
#[test]
fn comdis_fv20260401_valid_from_is_2026_04_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 04 - 01);

    let profile = reg
        .profiles_for(MessageType::Comdis)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing COMDIS profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "1.0g");
    assert_eq!(releases::comdis_fv20260401().as_str(), "1.0g");
}

#[cfg(feature = "comdis")]
#[test]
fn process_context_selects_active_comdis_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2026 - 03 - 31));
    assert!(before.active_release(MessageType::Comdis).is_none());

    for &d in &[
        date!(2026 - 04 - 01),
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
            Some(date!(2026 - 04 - 01))
        );
    }
}

// ── PRICAT ───────────────────────────────────────────────────────────────────

#[cfg(feature = "pricat")]
#[test]
fn pricat_fv20251001_valid_from_is_2025_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2025 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Pricat)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing PRICAT profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.0e");
    assert_eq!(releases::pricat_fv20251001().as_str(), "2.0e");
}

#[cfg(feature = "pricat")]
#[test]
fn pricat_fv20261001_valid_from_is_2026_10_01() {
    use edi_energy::registry::ReleaseRegistry;
    use edi_energy::releases;
    use time::macros::date;

    let reg = ReleaseRegistry::global();
    let target_date = date!(2026 - 10 - 01);

    let profile = reg
        .profiles_for(MessageType::Pricat)
        .find(|p| p.valid_from() == Some(target_date))
        .unwrap_or_else(|| panic!("missing PRICAT profile for {target_date}"));

    assert_eq!(profile.release().as_str(), "2.1");
    assert_eq!(releases::pricat_fv20261001().as_str(), "2.1");
}

#[cfg(feature = "pricat")]
#[test]
fn process_context_selects_active_pricat_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    let before = ProcessContext::for_date(date!(2025 - 09 - 30));
    assert!(before.active_release(MessageType::Pricat).is_none());

    for &d in &[
        date!(2025 - 10 - 01),
        date!(2026 - 01 - 01),
        date!(2026 - 09 - 30),
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
            Some(date!(2025 - 10 - 01))
        );
    }

    for &d in &[
        date!(2026 - 10 - 01),
        date!(2027 - 07 - 01),
        date!(2030 - 07 - 01),
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
            Some(date!(2026 - 10 - 01))
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

/// Every rule ID this crate emits must classify to an origin.
///
/// The classifier is a prefix table, so a rule ID that matches nothing reports
/// `rule_origin: None` and disappears from any dashboard that groups by layer —
/// silently, because `None` is also the honest answer for a caller's own rule.
#[test]
fn every_rule_id_the_crate_emits_classifies() {
    use edi_energy::RuleOrigin;

    for (rule_id, expected) in [
        ("MIG-DTM-001", RuleOrigin::Mig),
        ("AHB-13001-STS-I0", RuleOrigin::Ahb),
        ("AHB-SKIP-NO-PID", RuleOrigin::Ahb),
        ("SEM-UTILMD-001", RuleOrigin::Semantic),
        ("PARSE-001", RuleOrigin::Parse),
        ("DIR-001", RuleOrigin::Directory),
        ("UNKNOWN-MSG-TYPE", RuleOrigin::Unvalidated),
    ] {
        assert_eq!(
            edi_energy::RuleOrigin::classify(rule_id),
            Some(expected),
            "{rule_id} must classify to {expected}"
        );
    }
    assert_eq!(edi_energy::RuleOrigin::classify("EE-PID-001"), None);
}
