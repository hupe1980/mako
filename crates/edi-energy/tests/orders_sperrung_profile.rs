//! Regression guards for the ORDERS Sperrung/Entsperrung columns.
//!
//! Two defects the import used to have, pinned against the shipped profiles:
//!
//! 1. PIDs 17008/17116/17117 were lost — only the first column of each
//!    multi-column table survived.
//! 2. `IMD` was read as mandatory for 17115 (Sperrauftrag) instead of 17117
//!    (Entsperrauftrag).

// The ORDERS profiles are embedded only with the `orders` feature.
#![cfg(feature = "orders")]

use edi_energy::{MessageType, Profile, ReleaseRegistry};

/// The ORDERS profile with wire release `release`.
fn orders(release: &str) -> &'static Profile {
    ReleaseRegistry::global()
        .profiles_for(MessageType::Orders)
        .find(|p| p.release().as_str() == release)
        .unwrap_or_else(|| panic!("ORDERS {release} is shipped"))
}

/// The AHB status of the `tag` place the `pid` column lists, if any.
fn segment_status(profile: &Profile, pid: u32, tag: &str) -> Option<Vec<String>> {
    let af = profile.anwendungsfall(pid)?;
    profile
        .structure
        .layouts
        .iter()
        .filter(|l| l.tag == tag)
        .find_map(|l| af.segment_status(&l.nr).map(<[String]>::to_vec))
}

/// 17116/17117 are routed by `mako-geli-gas::sperrung_nb::SPERRUNG_PIDS`, so an
/// ORDERS profile that lost them would leave those workflows without rules.
#[test]
fn sperrung_pids_have_rules_in_the_current_release() {
    for release in ["1.4b", "1.4c"] {
        let profile = orders(release);
        for pid in [17007, 17008, 17115, 17116, 17117] {
            let af = profile
                .anwendungsfall(pid)
                .unwrap_or_else(|| panic!("PID {pid} must be a column of ORDERS {release}"));
            assert!(
                !af.rows.is_empty(),
                "{release}: PID {pid} must carry AHB rules"
            );
        }
    }
}

/// `IMD` is required for the **Entsperrauftrag** (17117) only.
///
/// The AHB carries `IMD Muss` solely in the 17117 column — its `Z53`/`Z54`
/// codes say whether the Entsperrung is an Auftrag or a Wiederinbetriebnahme.
#[test]
fn imd_is_mandatory_only_for_the_entsperrauftrag() {
    for release in ["1.4b", "1.4c"] {
        let profile = orders(release);
        assert_eq!(
            segment_status(profile, 17117, "IMD").as_deref(),
            Some(&["Muss".to_owned()][..]),
            "{release}: 17117 Entsperrauftrag requires IMD"
        );
        assert_eq!(
            segment_status(profile, 17115, "IMD"),
            None,
            "{release}: 17115 Sperrauftrag does not carry IMD"
        );
    }
}
