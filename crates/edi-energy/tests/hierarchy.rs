//! Integration tests for typed segment-group hierarchies.
//!
//! Verifies that the parser correctly builds nested group structures for:
//! - CONTRL: SG1 (UCM) → SG2 (UCS) → SG3 (UCD)
//! - MSCONS: SG5 (NAD) → SG6 (LOC) → SG9 (LIN) → SG10 (QTY+DTM+STS)
//! - APERAK: SG2 (ERC+FTX+RFF) error groups
//! - UTILMD: SG1 (RFF+DTM) and SG4 (IDE+DTM+LOC+RFF) transaction groups

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
))]
use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};

// ── CONTRL hierarchy ──────────────────────────────────────────────────────────

/// CONTRL that rejects one message (SG1), identifies one bad segment (SG2),
/// and one bad data element within that segment (SG3).
///
/// UCS element 0 is C011 (segment position : service segment tag) — composite.
/// UCD element 1 is C085 (element position : component position) — composite.
#[cfg(feature = "contrl")]
const CONTRL_HIERARCHY: &[u8] = b"\
UNH+MSG001+CONTRL:D:3:UN:1.0a'\
UCI+INTERCHANGE01+SENDER001+RECEIVER001+7'\
UCM+MSG999+UTILMD:D:11A:UN:5.5.3a+7+12'\
UCS+5:BGM+5'\
UCD+12+2:1'\
UNT+5+MSG001'";

#[cfg(feature = "contrl")]
#[test]
fn contrl_message_response_parsed() {
    let msg = Platform::with_all_profiles()
        .parse(CONTRL_HIERARCHY)
        .unwrap();
    let AnyMessage::Contrl(c) = msg else {
        panic!("expected CONTRL")
    };

    // UCI must be set and indicate rejection (code 7).
    let uci = c.uci().expect("UCI must be present");
    assert_eq!(uci.interchange_ref, "INTERCHANGE01");
    assert_eq!(
        uci.action_code.as_deref(),
        Some("7"),
        "interchange rejected"
    );

    // Exactly one SG1 group.
    assert_eq!(c.message_responses().len(), 1, "one UCM group expected");
    let resp = &c.message_responses()[0];
    assert_eq!(resp.ucm.message_ref, "MSG999");
    assert_eq!(resp.ucm.message_type, "UTILMD");
    assert_eq!(resp.ucm.action_code, "7", "message rejected");
    assert_eq!(resp.ucm.syntax_error.as_deref(), Some("12"));

    // One SG2 (UCS) segment error.
    assert_eq!(resp.segment_errors.len(), 1, "one UCS group expected");
    let seg_err = &resp.segment_errors[0];
    // UCS+5:BGM+5' — C011: component 0 = position "5", component 1 = tag "BGM".
    assert_eq!(seg_err.ucs.segment_position, "5");
    assert_eq!(seg_err.ucs.segment_tag.as_deref(), Some("BGM"));
    assert_eq!(seg_err.ucs.error_code.as_deref(), Some("5"));

    // One SG3 (UCD) element error.
    assert_eq!(seg_err.element_errors.len(), 1, "one UCD expected");
    let elem_err = &seg_err.element_errors[0];
    assert_eq!(elem_err.ucd.error_code, "12");
    // UCD+12+2:1' — C085: component 0 = element position "2", component 1 = component position "1".
    assert_eq!(elem_err.ucd.element_position.as_deref(), Some("2"));
    assert_eq!(elem_err.ucd.component_position.as_deref(), Some("1"));
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_no_ucm_gives_empty_responses() {
    // A simple positive acknowledgement has no UCM groups.
    let input = b"\
UNH+MSG001+CONTRL:D:3:UN:1.0a'\
UCI+ICR001+SENDER001+RECEIVER001+4'\
UNT+2+MSG001'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Contrl(c) = msg else {
        panic!("expected CONTRL")
    };
    assert_eq!(
        c.message_responses().len(),
        0,
        "positive ACK has no UCM groups"
    );
    assert_eq!(c.uci().unwrap().action_code.as_deref(), Some("4"));
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_multiple_ucm_groups() {
    // CONTRL rejecting two messages in one interchange.
    let input = b"\
UNH+MSG001+CONTRL:D:3:UN:1.0a'\
UCI+INTER01+SENDER001+RECEIVER001+7'\
UCM+MSG001+UTILMD:D:11A:UN:5.5.3a+7+12'\
UCM+MSG002+MSCONS:D:04B:UN:2.4c+4'\
UNT+4+MSG001'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Contrl(c) = msg else {
        panic!("expected CONTRL")
    };
    assert_eq!(c.message_responses().len(), 2);
    assert_eq!(c.message_responses()[0].ucm.message_type, "UTILMD");
    assert_eq!(c.message_responses()[1].ucm.message_type, "MSCONS");
    assert_eq!(
        c.message_responses()[1].ucm.action_code,
        "4",
        "second MSG accepted"
    );
}

// ── MSCONS hierarchy ──────────────────────────────────────────────────────────

/// MSCONS with one delivery point, one time series (LOC), two line items (LIN),
/// and two quantities (QTY) each with a status (STS).
#[cfg(feature = "mscons")]
const MSCONS_HIERARCHY: &[u8] = b"\
UNH+MSG002+MSCONS:D:04B:UN:2.4c'\
BGM+7+21001+9'\
DTM+137:20230701:102'\
NAD+MS+9900111222333::293'\
NAD+MR+9900444555666::293'\
UNS+D'\
NAD+DP+DE0001234567890::293'\
LOC+172+12345678901'\
DTM+163:20230101:102'\
DTM+164:20230131:102'\
LIN+1'\
PIA+5+1-1:1.29.0+Z12'\
QTY+220:1000.500:KWH'\
DTM+163:20230101:102'\
STS+7+Z03::293'\
QTY+220:500.000:KWH'\
LIN+2'\
QTY+220:250.000:KWH'\
STS+7+Z04::293'\
UNT+18+MSG002'";

#[cfg(feature = "mscons")]
#[test]
fn mscons_delivery_point_parsed() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_HIERARCHY)
        .unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };

    assert_eq!(m.delivery_points().len(), 1);
    let dp = &m.delivery_points()[0];
    assert_eq!(
        dp.nad.party_id.as_deref(),
        Some("DE0001234567890"),
        "metering point NAD+DP"
    );
    assert_eq!(dp.nad.qualifier, "DP");
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_time_series_within_delivery_point() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_HIERARCHY)
        .unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };

    let dp = &m.delivery_points()[0];
    assert_eq!(dp.time_series.len(), 1, "one LOC group");
    let ts = &dp.time_series[0];
    assert_eq!(ts.loc.qualifier, "172");
    assert_eq!(ts.loc.location_id.as_deref(), Some("12345678901"));
    // DTM 163 and 164 collected.
    assert_eq!(ts.dtm.len(), 2);
    assert_eq!(ts.dtm[0].qualifier, "163");
    assert_eq!(ts.dtm[1].qualifier, "164");
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_line_items_within_time_series() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_HIERARCHY)
        .unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };

    let ts = &m.delivery_points()[0].time_series[0];
    assert_eq!(ts.items.len(), 2, "two LIN groups");

    let item0 = &ts.items[0];
    assert_eq!(item0.lin.line_number.as_deref(), Some("1"));
    // PIA+5+1-1:1.29.0+Z12' — element 1 is C212 composite.
    // OBIS codes use ':' as part of their notation, which is also the EDIFACT
    // component separator.  The parser splits on ':' so component 0 = "1-1"
    // and component 1 = "1.29.0".  'Z12' falls in element 2 (not mapped here).
    let pia = item0.pia.as_ref().expect("PIA must be present on item 0");
    assert_eq!(pia.item_number.as_deref(), Some("1-1"), "C212 component 0");
    assert_eq!(pia.item_type.as_deref(), Some("1.29.0"), "C212 component 1");

    let item1 = &ts.items[1];
    assert_eq!(item1.lin.line_number.as_deref(), Some("2"));
    assert!(item1.pia.is_none(), "second LIN has no PIA");
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_quantities_with_status() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_HIERARCHY)
        .unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };

    let item0 = &m.delivery_points()[0].time_series[0].items[0];
    // Two QTY segments under first LIN.
    assert_eq!(item0.quantities.len(), 2);

    let qty0 = &item0.quantities[0];
    assert_eq!(qty0.qty.qualifier, "220");
    assert_eq!(qty0.qty.value.as_deref(), Some("1000.500"));
    assert_eq!(qty0.qty.unit.as_deref(), Some("KWH"));
    // DTM attached to first QTY.
    assert_eq!(qty0.dtm.len(), 1);
    assert_eq!(qty0.dtm[0].qualifier, "163");
    // STS attached to first QTY.
    assert_eq!(qty0.status.len(), 1);
    assert_eq!(qty0.status[0].status_code.as_deref(), Some("Z03"));

    let item1 = &m.delivery_points()[0].time_series[0].items[1];
    let qty_sts = &item1.quantities[0].status;
    assert_eq!(qty_sts.len(), 1);
    assert_eq!(qty_sts[0].status_code.as_deref(), Some("Z04"));
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_no_detail_section_gives_empty_delivery_points() {
    // A valid but header-only MSCONS (no UNS segment).
    let input = b"\
UNH+MSG002+MSCONS:D:04B:UN:2.4c'\
BGM+7+21001+9'\
DTM+137:20230701:102'\
NAD+MS+9900111222333::293'\
NAD+MR+9900444555666::293'\
UNT+5+MSG002'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };
    assert_eq!(
        m.delivery_points().len(),
        0,
        "no UNS+D → no delivery points"
    );
}

// ── APERAK hierarchy ──────────────────────────────────────────────────────────

/// APERAK with two SG2 error groups (ERC + FTX + RFF).
#[cfg(feature = "aperak")]
const APERAK_HIERARCHY: &[u8] = b"\
UNH+MSG003+APERAK:D:07B:UN:2.0a'\
BGM+1000++9'\
DTM+137:20230801:102'\
NAD+MS+9900777888999::293'\
NAD+MR+9900333444555::293'\
RFF+ACW:REF-APER-001'\
ERC+Z01'\
FTX+AAI+++Fehlerhafter Pruefidentifikator'\
RFF+TN:ORIG-001'\
ERC+Z02'\
FTX+AAI+++Ungueltige Marktlokation'\
UNT+11+MSG003'";

#[cfg(feature = "aperak")]
#[test]
fn aperak_errors_parsed() {
    let msg = Platform::with_all_profiles()
        .parse(APERAK_HIERARCHY)
        .unwrap();
    let AnyMessage::Aperak(a) = msg else {
        panic!("expected APERAK")
    };

    assert_eq!(a.errors().len(), 2, "two ERC groups expected");
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_first_error_group_fields() {
    let msg = Platform::with_all_profiles()
        .parse(APERAK_HIERARCHY)
        .unwrap();
    let AnyMessage::Aperak(a) = msg else {
        panic!("expected APERAK")
    };

    let err0 = &a.errors()[0];
    assert_eq!(err0.erc.error_code, "Z01");
    assert_eq!(err0.ftx.len(), 1);
    assert_eq!(
        err0.ftx[0].text.as_deref(),
        Some("Fehlerhafter Pruefidentifikator")
    );
    // RFF+TN groups the reference for this error.
    assert_eq!(err0.references.len(), 1);
    assert_eq!(err0.references[0].qualifier, "TN");
    assert_eq!(err0.references[0].reference.as_deref(), Some("ORIG-001"));
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_second_error_group_no_reference() {
    let msg = Platform::with_all_profiles()
        .parse(APERAK_HIERARCHY)
        .unwrap();
    let AnyMessage::Aperak(a) = msg else {
        panic!("expected APERAK")
    };

    let err1 = &a.errors()[1];
    assert_eq!(err1.erc.error_code, "Z02");
    assert_eq!(
        err1.ftx[0].text.as_deref(),
        Some("Ungueltige Marktlokation")
    );
    assert_eq!(err1.references.len(), 0, "no RFF after second ERC");
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_ref_acw_still_extracted_with_errors() {
    let msg = Platform::with_all_profiles()
        .parse(APERAK_HIERARCHY)
        .unwrap();
    let AnyMessage::Aperak(a) = msg else {
        panic!("expected APERAK")
    };

    let ref_acw = a.ref_acw().expect("RFF+ACW must be present");
    assert_eq!(ref_acw.reference.as_deref(), Some("REF-APER-001"));
}

// ── UTILMD hierarchy ──────────────────────────────────────────────────────────

/// UTILMD with one header reference (RFF+TN / SG1) and one transaction (IDE / SG4).
#[cfg(feature = "utilmd")]
const UTILMD_HIERARCHY: &[u8] = b"\
UNH+MSG004+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01+11001+9'\
DTM+137:20230615:102'\
NAD+MS+9900987654321::293'\
NAD+MR+9900123456789::293'\
RFF+TN:TREF001'\
DTM+163:20230701:102'\
IDE+Z01+DE0001234567890'\
DTM+92:20230701:102'\
LOC+172+12345678901'\
RFF+Z13:11001'\
UNT+11+MSG004'";

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_header_reference_parsed() {
    let msg = Platform::with_all_profiles()
        .parse(UTILMD_HIERARCHY)
        .unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };

    assert_eq!(u.references().len(), 1, "one RFF+TN header reference");
    let r = &u.references()[0];
    assert_eq!(r.rff.qualifier, "TN");
    assert_eq!(r.rff.reference.as_deref(), Some("TREF001"));
    // DTM 163 follows the header RFF.
    assert_eq!(r.dtm.len(), 1);
    assert_eq!(r.dtm[0].qualifier, "163");
    assert_eq!(r.dtm[0].value.as_deref(), Some("20230701"));
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_transaction_parsed() {
    let msg = Platform::with_all_profiles()
        .parse(UTILMD_HIERARCHY)
        .unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };

    assert_eq!(u.transactions().len(), 1, "one IDE transaction group");
    let tx = &u.transactions()[0];
    assert_eq!(tx.ide.qualifier, "Z01");
    assert_eq!(tx.ide.object_id.as_deref(), Some("DE0001234567890"));
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_transaction_dtm_and_loc() {
    let msg = Platform::with_all_profiles()
        .parse(UTILMD_HIERARCHY)
        .unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };

    let tx = &u.transactions()[0];
    assert_eq!(tx.dtm.len(), 1, "DTM 92 within transaction");
    assert_eq!(tx.dtm[0].qualifier, "92");
    let loc = tx.loc.as_ref().expect("LOC within transaction");
    assert_eq!(loc.qualifier, "172");
    assert_eq!(loc.location_id.as_deref(), Some("12345678901"));
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_transaction_references() {
    let msg = Platform::with_all_profiles()
        .parse(UTILMD_HIERARCHY)
        .unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };

    let tx = &u.transactions()[0];
    assert_eq!(tx.references.len(), 1);
    assert_eq!(tx.references[0].qualifier, "Z13");
    assert_eq!(tx.references[0].reference.as_deref(), Some("11001"));
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_multiple_transactions() {
    let input = b"\
UNH+MSG004+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01+11001+9'\
DTM+137:20230615:102'\
NAD+MS+9900987654321::293'\
NAD+MR+9900123456789::293'\
IDE+Z01+DE0001234567890'\
IDE+Z01+DE0009876543210'\
UNT+8+MSG004'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };
    assert_eq!(u.transactions().len(), 2, "two IDE transactions");
    assert_eq!(
        u.transactions()[0].ide.object_id.as_deref(),
        Some("DE0001234567890")
    );
    assert_eq!(
        u.transactions()[1].ide.object_id.as_deref(),
        Some("DE0009876543210")
    );
}

// ── Cross-cutting: Pruefidentifikator detection ───────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_detect_pruefidentifikator_from_bgm() {
    // BGM element 1 = document_id = "11001" → Pruefidentifikator 11001.
    let input = b"\
UNH+MSG001+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01+11001+9'\
NAD+MS+9900987654321::293'\
NAD+MR+9900123456789::293'\
UNT+4+MSG001'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };
    let pid = u
        .detect_pruefidentifikator()
        .expect("must detect Pruefidentifikator");
    assert_eq!(pid.as_u32(), 11001);
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_detect_pruefidentifikator_from_bgm() {
    let input = b"\
UNH+MSG002+MSCONS:D:04B:UN:2.4c'\
BGM+7+21001+9'\
NAD+MS+9900111222333::293'\
NAD+MR+9900444555666::293'\
UNT+4+MSG002'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };
    let pid = m
        .detect_pruefidentifikator()
        .expect("must detect Pruefidentifikator");
    assert_eq!(pid.as_u32(), 21001);
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_detect_pruefidentifikator_from_bgm() {
    let input = b"\
UNH+MSG003+APERAK:D:07B:UN:2.0a'\
BGM+1000++9'\
NAD+MS+9900777888999::293'\
NAD+MR+9900333444555::293'\
UNT+4+MSG003'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Aperak(a) = msg else {
        panic!("expected APERAK")
    };
    // BGM element 1 is empty for APERAK (no document_id) → no PID.
    let result = a.detect_pruefidentifikator();
    // APERAK BGM+1000 — no PID in document_id field.
    assert!(
        result.is_err(),
        "APERAK without document_id has no Pruefidentifikator"
    );
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_detect_pruefidentifikator_always_errors() {
    let input = b"\
UNH+MSG004+CONTRL:D:3:UN:1.0a'\
UCI+ICR001+SENDER001+RECEIVER001+4'\
UNT+2+MSG004'";

    let msg = Platform::with_all_profiles().parse(input).unwrap();
    let AnyMessage::Contrl(c) = msg else {
        panic!("expected CONTRL")
    };
    assert!(
        c.detect_pruefidentifikator().is_err(),
        "CONTRL never has a Pruefidentifikator"
    );
}
