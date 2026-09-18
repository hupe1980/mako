//! Every Anwendungsfall of every embedded profile has a skeleton that its own
//! Prüfschablone accepts.
//!
//! The skeleton is generated from the profile, so this is not a test of BDEW
//! conformance; it is the consistency check between the three things the
//! profile pipeline produces — the extracted MIG, the extracted AHB column and
//! the validator's reading of both. A contradiction (a data element the AHB
//! column lists but the MIG marks `N`, a group the column requires whose
//! trigger it does not list, an element the MIG requires that the column
//! omits) surfaces here as a finding on a skeleton, per Anwendungsfall.

use edi_energy::profile::SkeletonParties;
use edi_energy::{Platform, Pruefidentifikator};

#[test]
fn every_anwendungsfall_has_a_conformant_skeleton() {
    let platform = Platform::with_all_profiles();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let mut excused: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for profile in platform.registry().all_profiles() {
        for af in profile.anwendungsfaelle() {
            let segs = profile.skeleton(af, &SkeletonParties::default());
            let pid = af.pid.and_then(|p| Pruefidentifikator::new(p).ok());
            let issues = profile.validate(&segs, pid);
            let errors: Vec<_> = issues
                .iter()
                .filter(|i| i.severity == edifact_rs::ValidationSeverity::Error)
                .collect();
            checked += 1;
            let errors: Vec<_> = errors
                .into_iter()
                .filter(|e| match e.rule_id() {
                    Some(r) if PER_INSTANCE_BLIND_SPOT.contains(&r) => {
                        excused.insert(r.to_owned());
                        false
                    }
                    _ => true,
                })
                .collect();
            if !errors.is_empty() {
                let wire = String::from_utf8_lossy(
                    &edifact_rs::segments_to_bytes(&segs).unwrap_or_default(),
                )
                .replace('\'', "'\n  ");
                failures.push(format!(
                    "{} {} {}: {} error(s)\n  {}\n{}",
                    profile.message_type(),
                    profile.release(),
                    af.pid.map_or_else(|| af.name.clone(), |p| p.to_string()),
                    errors.len(),
                    wire.trim_end(),
                    errors
                        .iter()
                        .map(|e| format!("    [{}] {}", e.rule_id().unwrap_or("-"), e.message))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} skeletons do not validate against their own Prüfschablone:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    // The other direction: an excuse for something that no longer happens is
    // an excuse nobody reads, and it hides the next regression at that place.
    // Only where the profiles that would fire it are shipped — every id here
    // is UTILMD, and `--no-default-features` visits no Anwendungsfall at all.
    #[cfg(feature = "utilmd")]
    for id in PER_INSTANCE_BLIND_SPOT {
        assert!(
            excused.contains(*id),
            "{id} is excused in PER_INSTANCE_BLIND_SPOT but the skeleton no \
             longer fails on it — delete the row"
        );
    }
}

/// The one thing the generator cannot place, and why.
///
/// [`Profile::skeleton`] converges by `Nr`: a place the validator reports
/// missing is forced in, everywhere that place occurs. A Voraussetzung is
/// evaluated per **group instance**, and the two do not meet. UTILMD 55235
/// emits two `SG8 SEQ+Z22` — one whose `CAV` is `ZA5` and one whose `CAV` is
/// `ZF1` — and `[344]` („in dieser SG8 das SG10 CCI+++ZB4 CAV+ZF1 vorhanden")
/// holds in the second alone, so the `SG10` it gates is owed there and
/// forbidden in the first. Forcing it by `Nr` cannot say „in that one", so it
/// is never emitted.
///
/// Keying the fixpoint by `(Nr, instance)` is the fix and is its own item in
/// `concepts/ROADMAP.md`. Until then this list is the ratchet: it may only
/// shrink, and a rule id that stops firing has to leave it.
const PER_INSTANCE_BLIND_SPOT: &[&str] = &[
    "AHB-55235-SG10-00376-MISSING",
    "AHB-55235-SG10-00416-MISSING",
];

/// `Profile::complete` keeps what a sender states and fills the rest of the
/// column: a 55001 seed that names only the Vorgang, the Marktlokation and
/// the Bilanzkreis comes back conformant with those values intact. Completion
/// only adds — a place the column does not permit (`LOC+Z17` on a
/// Lieferbeginn) stays where the sender put it, and validation says so.
#[cfg(feature = "utilmd")]
#[test]
fn a_seed_is_completed_to_its_column_and_keeps_its_values() {
    use edi_energy::profile::SkeletonParties;

    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(edi_energy::MessageType::Utilmd)
        .find(|p| p.release().as_str() == "S2.1")
        .expect("UTILMD S2.1 is shipped");
    let af = profile.anwendungsfall(55001).expect("55001");
    // `DTM+92` is 31.01.2026 23:00 UTC because [UB1] puts a Beginn-Zeitpunkt on
    // the day boundary in gesetzlicher deutscher Zeit: 01.02.2026 00:00 MEZ.
    let seed = b"UNH+MSG-7+UTILMD:D:11A:UN:S2.1'BGM+E01+DOC-7'DTM+137:202601150800?+00:303'\
NAD+MS+4012345000023::9'NAD+MR+9900357000004::293'IDE+24+VG-ABC'\
DTM+92:202601312300?+00:303'LOC+Z16+51238696781'LOC+Z17+DE00056266802AO6G56M11SN51G21M24S'\
RFF+Z13:55001'SEQ+Z79+1'PIA+5+9991000002082:Z11'CCI+Z66'CAV+ZV4:::11XBK-STD-----9'UNT+15+MSG-7'";
    let seed: Vec<edifact_rs::OwnedSegment> = edifact_rs::from_bytes(seed)
        .map(|s| s.map(edifact_rs::Segment::into_owned))
        .collect::<Result<_, _>>()
        .expect("the seed parses");
    let parties = SkeletonParties {
        sender: "4012345000023".into(),
        receiver: "9900357000004".into(),
    };
    let done = profile.complete(&seed, af, &parties);
    let wire = String::from_utf8(edifact_rs::segments_to_bytes(&done).unwrap()).unwrap();
    let issues = profile.validate(&done, edi_energy::Pruefidentifikator::new(55001).ok());
    let ids: Vec<&str> = issues.iter().filter_map(|i| i.rule_id()).collect();
    assert_eq!(
        ids,
        ["AHB-55001-00055-LOC-NOT-PERMITTED"],
        "only what the sender chose is left to report: {wire}\n{issues:#?}"
    );
    for kept in [
        "IDE+24+VG-ABC'",
        "LOC+Z16+51238696781'",
        "CAV+ZV4:::11XBK-STD-----9'",
        "BGM+E01+DOC-7'",
        "UNH+MSG-7+",
    ] {
        assert!(
            wire.contains(kept),
            "{kept} must survive completion: {wire}"
        );
    }
    assert!(
        wire.contains("LOC+Z17+DE00056266802AO6G56M11SN51G21M24S'"),
        "kept as stated: {wire}"
    );
    assert!(
        wire.contains("NAD+Z09+"),
        "the column's Kunde is filled in: {wire}"
    );
    assert!(
        wire.contains("STS+7++E01"),
        "the Transaktionsgrund is filled in: {wire}"
    );
}
