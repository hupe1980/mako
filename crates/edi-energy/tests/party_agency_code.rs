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

/// The defect this file exists to close: every Gas UTILMD used to carry `293`.
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
