//! A Konsultationsfassung as a candidate: loaded at runtime beside the binding
//! profiles, never selectable for ingest, and compared with them by the
//! verdict it gives on a message — only a message that is refused under the
//! candidate and was not under the binding profile is reported.
//!
//! The candidate is built here from the committed binding profile with one
//! condition tightened, so no copy of a profile is committed for the test.

use edi_energy::candidate::{Candidate, Comparison, interchange_messages};
use edi_energy::profile::{Parteirollen, SkeletonParties};
use edi_energy::{MessageType, Platform, Release, ReleaseRegistry};
use serde_json::{Value, json};

const MIG: &str = include_str!("../../profiles/utilmd/fv20261001/mig.json");
const AHB: &str = include_str!("../../profiles/utilmd/fv20261001/ahb.json");

fn manifest(changed: &[u32]) -> String {
    json!({
        "message_type": "UTILMD",
        "fv": "fv20270401",
        "status": "Konsultationsfassung",
        "against": "fv20261001",
        "sources": [
            { "file": "UTILMD_AHB_Strom_Konsultation.pdf", "sha256": "a".repeat(64) },
        ],
        "changed_pids": changed,
        "added_pids": [],
        "removed_pids": [],
    })
    .to_string()
}

/// The binding AHB with one condition on PID 55001 tightened: the first data
/// element whose column admits two or more codes loses the first of them.
/// Returns the AHB and the `(nr, de, code)` it removed.
fn tightened() -> (String, (String, String, String)) {
    let mut ahb: Value = serde_json::from_str(AHB).expect("the binding AHB parses");
    let af = ahb["anwendungsfaelle"]
        .as_array_mut()
        .expect("columns")
        .iter_mut()
        .find(|a| a["pid"] == 55001)
        .expect("55001 is in the binding AHB");
    let rule = af["elements"]
        .as_array_mut()
        .expect("element rules")
        .iter_mut()
        .find(|e| {
            e["operands"]
                .as_array()
                .is_some_and(|o| o.iter().filter(|o| o["code"].is_string()).count() >= 2)
        })
        .expect("a 55001 data element with a choice of codes");
    let nr = rule["nr"].as_str().unwrap_or_default().to_owned();
    let de = rule["de"].as_str().unwrap_or_default().to_owned();
    let operands = rule["operands"].as_array_mut().expect("operands");
    let at = operands
        .iter()
        .position(|o| o["code"].is_string())
        .expect("a code");
    let code = operands.remove(at)["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    (ahb.to_string(), (nr, de, code))
}

fn binding() -> &'static edi_energy::Profile {
    ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .copied()
        .find(|p| {
            p.message_type() == MessageType::Utilmd
                && p.valid_from()
                    == time::Date::from_calendar_date(2026, time::Month::October, 1).ok()
                && p.mig.track.as_deref() != Some("Gas")
        })
        .expect("UTILMD fv20261001 is embedded")
}

fn interchange(profile: &edi_energy::Profile, pid: u32) -> Vec<u8> {
    let af = profile.anwendungsfall(pid).expect("the column");
    profile
        .skeleton_interchange(af, &SkeletonParties::default())
        .expect("the skeleton writes")
}

fn compare_all(candidate: &Candidate, corpus: &[Vec<u8>]) -> Vec<(usize, Comparison)> {
    let mut out = Vec::new();
    for (i, bytes) in corpus.iter().enumerate() {
        for message in interchange_messages(bytes).expect("the interchange parses") {
            out.push((
                i,
                candidate.compare(binding(), &message, &Parteirollen::unbekannt()),
            ));
        }
    }
    out
}

/// With one condition on 55001 tightened, exactly the recorded 55001
/// message violating it is listed, with that rule; the 55001 message that
/// meets it and the 55002 message are not.
#[test]
fn a_tightened_condition_lists_exactly_the_messages_it_refuses() {
    let (ahb, (nr, de, code)) = tightened();
    let candidate =
        Candidate::from_json(&manifest(&[55001]), MIG, &ahb).expect("the candidate loads");

    let violating = interchange(binding(), 55001);
    let meeting = interchange(candidate.profile(), 55001);
    let other_pid = interchange(binding(), 55002);
    let results = compare_all(&candidate, &[violating, meeting, other_pid]);

    let worse: Vec<_> = results
        .iter()
        .filter_map(|(i, c)| match c {
            Comparison::Worse(refusals) => Some((*i, refusals)),
            _ => None,
        })
        .collect();
    assert_eq!(
        worse.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        [0],
        "only the message using the removed code {code} ({nr} DE {de}) is listed: {results:?}"
    );
    let refusal = &worse[0].1[0];
    let rule = refusal.rule_id.as_deref().unwrap_or_default();
    assert!(
        rule.contains(&nr),
        "the refusal names the tightened place {nr}: {refusal:?}"
    );
    assert!(refusal.segment_position.is_some(), "{refusal:?}");
    assert!(
        matches!(
            results[2].1,
            Comparison::NotRevalidated { pid: Some(55002) }
        ),
        "a PID the candidate does not change is stated, not re-validated: {:?}",
        results[2].1
    );
}

/// A candidate identical to the binding release reports nothing.
#[test]
fn a_candidate_identical_to_the_binding_release_reports_nothing() {
    let candidate =
        Candidate::from_json(&manifest(&[55001, 55002]), MIG, AHB).expect("the candidate loads");
    let corpus = [interchange(binding(), 55001), interchange(binding(), 55002)];
    for (i, c) in compare_all(&candidate, &corpus) {
        assert!(matches!(c, Comparison::Unchanged), "message {i}: {c:?}");
    }
}

/// A PID the candidate drops is listed apart, never as a refusal.
#[test]
fn a_removed_pid_is_listed_apart() {
    let mut ahb: Value = serde_json::from_str(AHB).expect("parses");
    ahb["anwendungsfaelle"]
        .as_array_mut()
        .expect("columns")
        .retain(|a| a["pid"] != 55001);
    let candidate =
        Candidate::from_json(&manifest(&[]), MIG, &ahb.to_string()).expect("the candidate loads");
    let results = compare_all(&candidate, &[interchange(binding(), 55001)]);
    assert!(
        matches!(results[0].1, Comparison::PidRemoved { pid: 55001 }),
        "{:?}",
        results[0].1
    );
}

/// A candidate is never selected for ingest: loading one registers nothing,
/// and the registry answers its wire release from the binding profiles alone.
#[test]
fn a_candidate_is_never_selected_for_ingest() {
    let before = ReleaseRegistry::global().all_profiles().len();
    let mut mig: Value = serde_json::from_str(MIG).expect("parses");
    mig["release"] = json!("S9.9");
    let mut ahb: Value = serde_json::from_str(AHB).expect("parses");
    ahb["release"] = json!("S9.9");
    let candidate = Candidate::from_json(&manifest(&[]), &mig.to_string(), &ahb.to_string())
        .expect("the candidate loads");
    assert!(candidate.manifest().is_candidate());
    assert_eq!(ReleaseRegistry::global().all_profiles().len(), before);
    let platform = Platform::with_all_profiles();
    for registry in [ReleaseRegistry::global(), platform.registry()] {
        assert!(
            registry
                .profile(MessageType::Utilmd, &Release::new("S9.9"))
                .is_err(),
            "the candidate's wire release is unknown to ingest"
        );
    }
}

/// A Konsultationsfassung is never binding: a manifest naming another status
/// does not load.
#[test]
fn only_a_konsultationsfassung_loads_as_a_candidate() {
    let mut m: Value = serde_json::from_str(&manifest(&[])).expect("parses");
    m["status"] = json!("binding");
    assert!(Candidate::from_json(&m.to_string(), MIG, AHB).is_err());
    let mut m: Value = serde_json::from_str(&manifest(&[])).expect("parses");
    m["sources"][0]["sha256"] = json!("not-a-digest");
    assert!(Candidate::from_json(&m.to_string(), MIG, AHB).is_err());
}
