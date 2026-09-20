//! The NAD DE 3055 agency follows the MP-ID, not the builder.
//!
//! DE 3055 names the **codevergebende Stelle** — Allgemeine Festlegungen V6.1d
//! Kap. 6.1: `9` GS1, `293` BDEW, `332` DVGW. Which one applies is a property of
//! the identifier: BDEW issues 13-digit `99…` codes for Strom, DVGW `98…` codes
//! for Gas (AWH Identifikatoren V1.2 Kap. 2.2, Bildungsvorschrift).
//!
//! UTILMD AHB Gas G1.1/G1.2 admits only `9` and `332` on every party NAD. A Gas
//! message stamped `293` therefore names a code list its own Anwendungsfall does
//! not define — and contradicts the DVGW `502` the same interchange already
//! declares in UNB DE 0007.

#![cfg(feature = "utilmd")]

use edi_energy::builders::{UtilmdBuilder, unb_qualifier};
use edi_energy::{AgencyCode, Pruefidentifikator, Release};

/// DVGW-issued Gas codes (`98…`); BDEW-issued Strom codes (`99…`).
const GNB_GAS: &str = "9870123456789";
const LF_GAS: &str = "9871234567897";
const NB_STROM: &str = "9900357000004";
const LF_STROM: &str = "9900555000005";

fn utilmd(sender: &str, receiver: &str, pid: u32, release: &str) -> String {
    let bytes = UtilmdBuilder::new(Release::new(release))
        .pruefidentifikator(Pruefidentifikator::new(pid).expect("valid PID"))
        .sender(sender)
        .receiver(receiver)
        .message_ref("MSG1")
        .document_date("202608040900")
        .transaction("VG1")
        .marktlokation("51238696781")
        .done()
        .serialize()
        .expect("serializes");
    String::from_utf8(bytes).expect("UNOC output is valid UTF-8 for ASCII payloads")
}

/// A Gas party NAD carries DVGW `332`, never BDEW `293`.
#[test]
fn a_gas_utilmd_names_the_dvgw_code_list() {
    let wire = utilmd(GNB_GAS, LF_GAS, 44_002, "G1.1");
    assert!(
        wire.contains(&format!("NAD+MS+{GNB_GAS}::332")),
        "Gas sender must carry DVGW 332, got: {wire}"
    );
    assert!(
        wire.contains(&format!("NAD+MR+{LF_GAS}::332")),
        "Gas receiver must carry DVGW 332, got: {wire}"
    );
    assert!(
        !wire.contains("::293"),
        "UTILMD AHB Gas does not define 293 on a party NAD: {wire}"
    );
}

#[test]
fn a_strom_utilmd_still_names_the_bdew_code_list() {
    let wire = utilmd(NB_STROM, LF_STROM, 55_002, "S2.1");
    assert!(wire.contains(&format!("NAD+MS+{NB_STROM}::293")), "{wire}");
    assert!(wire.contains(&format!("NAD+MR+{LF_STROM}::293")), "{wire}");
}

/// An explicit override still wins — for a party whose registered code list
/// differs from what its number implies.
#[test]
fn an_explicit_agency_overrides_the_derivation() {
    let bytes = UtilmdBuilder::new(Release::new("S2.1"))
        .pruefidentifikator(Pruefidentifikator::new(55_002).expect("valid PID"))
        .sender(NB_STROM)
        .receiver(LF_STROM)
        .sender_agency(AgencyCode::Gs1)
        .message_ref("MSG1")
        .transaction("VG1")
        .marktlokation("51238696781")
        .done()
        .serialize()
        .expect("serializes");
    let wire = String::from_utf8(bytes).expect("ASCII payload");
    assert!(wire.contains(&format!("NAD+MS+{NB_STROM}::9'")), "{wire}");
}

/// UNB DE 0007 and NAD DE 3055 name the same issuing office. Deriving both from
/// the MP-ID is what keeps them from disagreeing.
#[test]
fn the_envelope_and_the_nad_agree_on_the_issuing_office() {
    for (mp_id, unb, nad) in [
        (NB_STROM, "500", "293"),
        (GNB_GAS, "502", "332"),
        ("4012345000023", "14", "9"),
    ] {
        assert_eq!(unb_qualifier(mp_id), unb, "UNB DE 0007 for {mp_id}");
        assert_eq!(
            AgencyCode::for_mp_id(mp_id).as_str(),
            nad,
            "NAD DE 3055 for {mp_id}"
        );
    }
}

/// Every checked-in fixture stamps the DE 3055 its own MP-ID implies.
///
/// `xtask`'s fixture generator restates the derivation rather than importing it —
/// `xtask` does not depend on `edi-energy` — so nothing stops the two from
/// drifting. This checks the artefact instead of the copy: whatever the
/// generator believes, what landed on disk has to agree with
/// [`AgencyCode::for_mp_id`].
///
/// The `demos/` fixtures are in scope. They are the copy an evaluator reads
/// first and the one a market partner is handed as "this is what you send us",
/// and nothing else holds them to the derivation: `demo_fixtures.rs` validates
/// them against the AHB, which says nothing about which office issued the code.
///
/// Fixtures that deliberately carry a wrong agency belong under `invalid/` with
/// an `.expected.json` naming the rule that must fire.
#[test]
fn every_checked_in_fixture_stamps_the_agency_its_mp_id_implies() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut checked = 0usize;
    let mut wrong: Vec<String> = Vec::new();

    let demos = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../demos")
        .canonicalize()
        .expect("demos/ is part of the workspace");
    let mut dirs = vec![root.clone(), demos];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "edi") {
                continue;
            }
            let raw = String::from_utf8_lossy(&std::fs::read(&path).expect("fixture is readable"))
                .into_owned();
            for seg in raw.split('\'').map(str::trim) {
                let Some(rest) = seg.strip_prefix("NAD+") else {
                    continue;
                };
                // NAD+<3035>+<mp-id>::<3055>
                let mut parts = rest.split('+');
                let _qualifier = parts.next();
                let Some(c082) = parts.next() else { continue };
                let mut comps = c082.split(':');
                let (Some(mp_id), Some(_), Some(agency)) =
                    (comps.next(), comps.next(), comps.next())
                else {
                    continue;
                };
                checked += 1;
                let expected = AgencyCode::for_mp_id(mp_id).as_str();
                if agency != expected {
                    wrong.push(format!(
                        "{}: NAD {mp_id} stamped {agency}, but for_mp_id says {expected}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
    }

    assert!(
        checked > 100,
        "expected the generated corpus, saw {checked} NADs"
    );
    assert!(
        wrong.is_empty(),
        "{} generated NAD(s) name the wrong code list — fix `agency_for` and \
         regenerate the fixtures it feeds:\n  {}",
        wrong.len(),
        wrong
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// Every checked-in fixture stamps the DE 0007 its own MP-ID implies.
///
/// The envelope half of the same rule, and the half that had nothing holding
/// it: `unb_qualifier` is asserted on four literals in its own unit test, and
/// the corpus was never read. DE 0007 and DE 3055 name the same issuing office
/// on **different code lists** — `500`/`293` BDEW, `502`/`332` DVGW, `14`/`9`
/// GS1 — so a fixture can carry the right NAD and still declare in its envelope
/// that GS1 issued a BDEW-Codenummer.
///
/// That is not cosmetic. A fixture is what a reader copies and what the parser
/// is exercised on, so a corpus stamping `14` everywhere means mako emits
/// `9900357000004:500` and cannot point at a single inbound example of the shape
/// it sends — and the demo that reads the fixture shows the same party
/// identified two different ways in one exchange.
#[test]
fn every_checked_in_fixture_stamps_the_unb_qualifier_its_mp_id_implies() {
    let mut dirs = vec![
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../demos")
            .canonicalize()
            .expect("demos/ is part of the workspace"),
    ];
    let mut checked = 0usize;
    let mut wrong: Vec<String> = Vec::new();

    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "edi") {
                continue;
            }
            let raw = String::from_utf8_lossy(&std::fs::read(&path).expect("fixture is readable"))
                .into_owned();
            let Some(unb) = raw.split('\'').next().map(str::trim) else {
                continue;
            };
            // UNB+<S001>+<S002 sender:qualifier>+<S003 receiver:qualifier>+…
            let mut fields = unb.split('+');
            let (Some(_tag), Some(_s001), Some(s002), Some(s003)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            for (role, field) in [("sender", s002), ("receiver", s003)] {
                let mut comps = field.split(':');
                let (Some(mp_id), Some(qualifier)) = (comps.next(), comps.next()) else {
                    continue;
                };
                checked += 1;
                let expected = unb_qualifier(mp_id);
                if qualifier != expected {
                    wrong.push(format!(
                        "{}: UNB {role} {mp_id} stamped {qualifier}, but unb_qualifier says \
                         {expected}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
    }

    assert!(
        checked > 150,
        "expected both parties of every interchange fixture, saw {checked}"
    );
    assert!(
        wrong.is_empty(),
        "{} fixture interchange(s) declare an issuing office their MP-ID does not imply \
         (Allgemeine Festlegungen V6.1d: DE 0007 is 500 BDEW / 502 DVGW / 14 GS1):\n  {}",
        wrong.len(),
        wrong
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
