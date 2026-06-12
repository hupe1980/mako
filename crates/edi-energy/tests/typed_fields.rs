//! Integration tests for typed message fields (Stories 14.1, 15.1, 16.1, 17.1).
//!
//! Verifies that:
//! - `bgm`, `dtm`, `sender`, `receiver`, `uci`, `ref_acw` are correctly
//!   pre-extracted from raw segments in each message type.
//! - Each message type implements `EdifactDeserialize` (parse from segments).
//! - Each message type implements `EdifactSerialize` (round-trip via `serialize()`).

#![allow(unused_imports, dead_code)]

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
use edi_energy::{AnyMessage, EdiEnergyMessage, EdifactDeserialize, parse};

// ── UTILMD typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
const UTILMD_TYPED: &[u8] = b"\
UNH+MSG001+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01+11001+9'\
DTM+137:20230615:102'\
DTM+163:20230701:102'\
NAD+MS+9900987654321::293'\
NAD+MR+9900123456789::293'\
UNT+6+MSG001'";

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_bgm_field_extracted() {
    let msg = parse(UTILMD_TYPED).unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };
    let bgm = u.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "E01");
    assert_eq!(bgm.document_id.as_deref(), Some("11001"));
    assert_eq!(bgm.function.as_deref(), Some("9"));
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_dtm_fields_extracted() {
    let msg = parse(UTILMD_TYPED).unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };
    assert_eq!(u.dtm().len(), 2, "should extract 2 DTM segments");
    assert_eq!(u.dtm()[0].qualifier, "137");
    assert_eq!(u.dtm()[0].value.as_deref(), Some("20230615"));
    assert_eq!(u.dtm()[0].format.as_deref(), Some("102"));
    assert_eq!(u.dtm()[1].qualifier, "163");
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_sender_receiver_extracted() {
    let msg = parse(UTILMD_TYPED).unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };
    let sender = u.sender().expect("sender (NAD+MS) must be present");
    assert_eq!(sender.qualifier, "MS");
    assert_eq!(sender.party_id.as_deref(), Some("9900987654321"));
    let receiver = u.receiver().expect("receiver (NAD+MR) must be present");
    assert_eq!(receiver.qualifier, "MR");
    assert_eq!(receiver.party_id.as_deref(), Some("9900123456789"));
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_round_trip_serialize() {
    let msg = parse(UTILMD_TYPED).unwrap();
    let AnyMessage::Utilmd(u) = msg else {
        panic!("expected UTILMD")
    };
    let bytes = u.serialize().expect("serialize must succeed");
    let msg2 = parse(&bytes).unwrap();
    let AnyMessage::Utilmd(u2) = msg2 else {
        panic!("re-parse must be UTILMD")
    };
    assert_eq!(u.bgm(), u2.bgm(), "BGM should round-trip");
    assert_eq!(u.sender(), u2.sender(), "sender should round-trip");
    assert_eq!(u.receiver(), u2.receiver(), "receiver should round-trip");
    assert_eq!(u.dtm().len(), u2.dtm().len(), "DTM count should round-trip");
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_deserialize_from_segments() {
    // EdifactDeserialize impl: parse from raw segments slice.
    use edifact_rs::from_bytes;
    let segs: Vec<_> = from_bytes(UTILMD_TYPED).collect::<Result<_, _>>().unwrap();
    let u = edi_energy::messages::utilmd::UtilmdMessage::edifact_deserialize(&segs)
        .expect("EdifactDeserialize must succeed");
    assert_eq!(u.message_ref(), "MSG001");
    assert_eq!(u.assoc_code(), "5.5.3a");
    assert_eq!(u.bgm().unwrap().document_id.as_deref(), Some("11001"));
}

// ── MSCONS typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "mscons")]
const MSCONS_TYPED: &[u8] = b"\
UNH+MSG002+MSCONS:D:04B:UN:2.4c'\
BGM+7+13003+9'\
DTM+137:20230701:102'\
NAD+MS+9900111222333::293'\
NAD+MR+9900444555666::293'\
UNT+5+MSG002'";

#[cfg(feature = "mscons")]
#[test]
fn mscons_typed_fields() {
    let msg = parse(MSCONS_TYPED).unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "7");
    assert_eq!(bgm.document_id.as_deref(), Some("13003"));
    assert_eq!(m.dtm().len(), 1);
    assert_eq!(
        m.sender().unwrap().party_id.as_deref(),
        Some("9900111222333")
    );
    assert_eq!(
        m.receiver().unwrap().party_id.as_deref(),
        Some("9900444555666")
    );
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_round_trip_serialize() {
    let msg = parse(MSCONS_TYPED).unwrap();
    let AnyMessage::Mscons(m) = msg else {
        panic!("expected MSCONS")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = parse(&bytes).unwrap();
    let AnyMessage::Mscons(m2) = msg2 else {
        panic!("re-parse must be MSCONS")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
}

// ── APERAK typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "aperak")]
const APERAK_TYPED: &[u8] = b"\
UNH+MSG003+APERAK:D:07B:UN:2.0a'\
BGM+1000++9'\
DTM+137:20230801:102'\
NAD+MS+9900777888999::293'\
NAD+MR+9900333444555::293'\
RFF+ACW:REF-APER-001'\
UNT+7+MSG003'";

#[cfg(feature = "aperak")]
#[test]
fn aperak_typed_fields_and_ref_acw() {
    let msg = parse(APERAK_TYPED).unwrap();
    let AnyMessage::Aperak(a) = msg else {
        panic!("expected APERAK")
    };
    let bgm = a.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "1000");
    let ref_acw = a.ref_acw().expect("RFF+ACW must be present");
    assert_eq!(ref_acw.qualifier, "ACW");
    assert_eq!(ref_acw.reference.as_deref(), Some("REF-APER-001"));
    assert_eq!(
        a.sender().unwrap().party_id.as_deref(),
        Some("9900777888999")
    );
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_round_trip_serialize() {
    let msg = parse(APERAK_TYPED).unwrap();
    let AnyMessage::Aperak(a) = msg else {
        panic!("expected APERAK")
    };
    let bytes = a.serialize().expect("serialize must succeed");
    let msg2 = parse(&bytes).unwrap();
    let AnyMessage::Aperak(a2) = msg2 else {
        panic!("re-parse must be APERAK")
    };
    assert_eq!(a.bgm(), a2.bgm(), "BGM should round-trip");
    assert_eq!(a.ref_acw(), a2.ref_acw(), "RFF+ACW should round-trip");
}

// ── CONTRL typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "contrl")]
const CONTRL_TYPED: &[u8] = b"\
UNH+MSG004+CONTRL:D:3:UN:1.0a'\
UCI+ICR001+SENDER001+RECEIVER001+4'\
UNT+3+MSG004'";

#[cfg(feature = "contrl")]
#[test]
fn contrl_uci_field_extracted() {
    let msg = parse(CONTRL_TYPED).unwrap();
    let AnyMessage::Contrl(c) = msg else {
        panic!("expected CONTRL")
    };
    let uci = c.uci().expect("UCI must be present");
    assert_eq!(uci.interchange_ref, "ICR001");
    assert_eq!(uci.sender.as_deref(), Some("SENDER001"));
    assert_eq!(uci.recipient.as_deref(), Some("RECEIVER001"));
    assert_eq!(uci.action_code.as_deref(), Some("4"));
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_round_trip_serialize() {
    let msg = parse(CONTRL_TYPED).unwrap();
    let AnyMessage::Contrl(c) = msg else {
        panic!("expected CONTRL")
    };
    let bytes = c.serialize().expect("serialize must succeed");
    let msg2 = parse(&bytes).unwrap();
    let AnyMessage::Contrl(c2) = msg2 else {
        panic!("re-parse must be CONTRL")
    };
    assert_eq!(c.uci(), c2.uci(), "UCI should round-trip");
}

// ── Builder APIs ──────────────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_builder_constructs_valid_message() {
    use edi_energy::{Pruefidentifikator, Release, builders::UtilmdBuilder};

    let release = Release::new("5.5.3a");
    let pid = Pruefidentifikator::new(11001).unwrap();
    let msg = UtilmdBuilder::new(release)
        .pruefidentifikator(pid)
        .sender("9900987654321")
        .receiver("9900123456789")
        .message_ref("BLDTEST")
        .document_date("20230901")
        .build()
        .expect("UtilmdBuilder::build must succeed");

    assert_eq!(msg.message_ref(), "BLDTEST");
    assert_eq!(msg.assoc_code(), "5.5.3a");
    let bgm = msg.bgm().expect("BGM must be set");
    assert_eq!(bgm.document_id.as_deref(), Some("11001"));
    let sender = msg.sender().expect("sender must be set");
    assert_eq!(sender.party_id.as_deref(), Some("9900987654321"));
    let receiver = msg.receiver().expect("receiver must be set");
    assert_eq!(receiver.party_id.as_deref(), Some("9900123456789"));
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_builder_constructs_valid_message() {
    use edi_energy::{Pruefidentifikator, Release, builders::MsconsBuilder};

    let release = Release::new("2.4c");
    let pid = Pruefidentifikator::new(13003).unwrap();
    let msg = MsconsBuilder::new(release)
        .pruefidentifikator(pid)
        .sender("9900111222333")
        .receiver("9900444555666")
        .document_date("20230901")
        .build()
        .expect("MsconsBuilder::build must succeed");

    let bgm = msg.bgm().expect("BGM must be set");
    assert_eq!(bgm.document_id.as_deref(), Some("13003"));
    assert_eq!(
        msg.sender().unwrap().party_id.as_deref(),
        Some("9900111222333")
    );
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_builder_constructs_valid_message() {
    use edi_energy::{Release, builders::AperakBuilder};

    let release = Release::new("2.0a");
    let msg = AperakBuilder::new(release)
        .sender("9900777888999")
        .receiver("9900333444555")
        .acw_ref("REF-BUILD-001")
        .error_code("Z43")
        .error_text("Ungueltige Marktlokation")
        .document_date("20230901")
        .build()
        .expect("AperakBuilder::build must succeed");

    let ref_acw = msg.ref_acw().expect("RFF+ACW must be set by builder");
    assert_eq!(ref_acw.reference.as_deref(), Some("REF-BUILD-001"));
    assert_eq!(
        msg.sender().unwrap().party_id.as_deref(),
        Some("9900777888999")
    );
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_builder_accept() {
    use edi_energy::{Release, builders::ContrlBuilder};

    let msg = ContrlBuilder::new(Release::new("1.0a"))
        .interchange_ref("INTER-2024-001")
        .sender("9900111222333")
        .receiver("9900444555666")
        .accept()
        .build()
        .expect("ContrlBuilder::build must succeed");

    let uci = msg.uci().expect("UCI must be present");
    assert_eq!(uci.interchange_ref, "INTER-2024-001");
    assert_eq!(uci.sender.as_deref(), Some("9900111222333"));
    assert_eq!(uci.action_code.as_deref(), Some("4"), "accept = code 4");
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_builder_reject() {
    use edi_energy::{Release, builders::ContrlBuilder};

    let msg = ContrlBuilder::new(Release::new("1.0a"))
        .interchange_ref("INTER-2024-002")
        .sender("9900111222333")
        .receiver("9900444555666")
        .reject()
        .build()
        .expect("ContrlBuilder::reject build must succeed");

    let uci = msg.uci().expect("UCI must be present");
    assert_eq!(uci.action_code.as_deref(), Some("8"), "reject = code 8");
    assert_eq!(msg.message_responses().len(), 0);
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_builder_metering_point_sub_builder() {
    use edi_energy::{Pruefidentifikator, Release, builders::MsconsBuilder};

    let msg = MsconsBuilder::new(Release::new("2.4c"))
        .pruefidentifikator(Pruefidentifikator::new(13003).unwrap())
        .sender("9900111222333")
        .receiver("9900444555666")
        .document_date("20230901")
        .metering_point("DE0001234567890")
        .location_id("12345678901")
        .quantity("220", "1000.500", "KWH")
        .done()
        .build()
        .expect("MsconsBuilder with metering_point must succeed");

    assert_eq!(
        msg.delivery_points().len(),
        1,
        "one delivery point from sub-builder"
    );
    let dp = &msg.delivery_points()[0];
    assert_eq!(dp.nad.party_id.as_deref(), Some("DE0001234567890"));
    assert_eq!(dp.time_series.len(), 1);
    let ts = &dp.time_series[0];
    assert_eq!(ts.loc.qualifier, "172");
    assert_eq!(ts.loc.location_id.as_deref(), Some("12345678901"));
    assert_eq!(ts.items.len(), 1);
    let qty = &ts.items[0].quantities[0];
    assert_eq!(qty.qty.qualifier, "220");
    assert_eq!(qty.qty.value.as_deref(), Some("1000.500"));
    assert_eq!(qty.qty.unit.as_deref(), Some("KWH"));
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_builder_serialize_roundtrip() {
    use edi_energy::{Pruefidentifikator, Release, builders::UtilmdBuilder};

    let bytes = UtilmdBuilder::new(Release::new("5.5.3a"))
        .pruefidentifikator(Pruefidentifikator::new(11001).unwrap())
        .sender("9900987654321")
        .receiver("9900123456789")
        .document_date("20230901")
        .serialize()
        .expect("UtilmdBuilder::serialize must succeed");

    // Must be non-empty and start with EDIFACT segment
    assert!(!bytes.is_empty());
    let text = std::str::from_utf8(&bytes).expect("output must be UTF-8");
    assert!(
        text.contains("UTILMD"),
        "serialized bytes must contain message type"
    );
    assert!(
        text.contains("11001"),
        "serialized bytes must contain Pruefidentifikator"
    );

    // Round-trip: must parse back successfully
    let msg = parse(&bytes).expect("serialized bytes must parse");
    let AnyMessage::Utilmd(u) = msg else {
        panic!("must re-parse as UTILMD")
    };
    assert_eq!(u.bgm().unwrap().document_id.as_deref(), Some("11001"));
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_builder_serialize_roundtrip() {
    use edi_energy::{Release, builders::ContrlBuilder};

    let bytes = ContrlBuilder::new(Release::new("1.0a"))
        .interchange_ref("INTER-SER-001")
        .sender("9900111222333")
        .accept()
        .serialize()
        .expect("ContrlBuilder::serialize must succeed");

    assert!(!bytes.is_empty());
    let msg = parse(&bytes).expect("serialized CONTRL must parse");
    let AnyMessage::Contrl(c) = msg else {
        panic!("must re-parse as CONTRL")
    };
    assert_eq!(c.uci().unwrap().interchange_ref, "INTER-SER-001");
}

// ── Segment convenience methods ─────────────────────────────────────────────

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
#[test]
fn dtm_is_document_date_qualifier_137() {
    use edi_energy::messages::segments::Dtm;
    let dtm = Dtm {
        qualifier: "137".into(),
        value: Some("20230701".into()),
        format: Some("102".into()),
    };
    assert!(dtm.is_document_date());
    assert!(!dtm.is_period_start());
    assert!(!dtm.is_period_end());
    assert_eq!(dtm.value_str(), Some("20230701"));
}

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
#[test]
fn dtm_period_qualifiers() {
    use edi_energy::messages::segments::Dtm;
    let start = Dtm {
        qualifier: "163".into(),
        value: Some("202307010000".into()),
        format: Some("203".into()),
    };
    let end = Dtm {
        qualifier: "164".into(),
        value: Some("202307312300".into()),
        format: Some("203".into()),
    };
    assert!(start.is_period_start());
    assert!(!start.is_period_end());
    assert!(end.is_period_end());
    assert!(!end.is_period_start());
}

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
#[test]
fn qty_value_f64_parses_dot_and_comma() {
    use edi_energy::messages::segments::Qty;
    let q_dot = Qty {
        qualifier: "220".into(),
        value: Some("1234.56".into()),
        unit: Some("KWH".into()),
    };
    let q_comma = Qty {
        qualifier: "220".into(),
        value: Some("1234,56".into()),
        unit: Some("KWH".into()),
    };
    let q_none = Qty {
        qualifier: "220".into(),
        value: None,
        unit: None,
    };
    assert!((q_dot.value_f64().unwrap() - 1234.56_f64).abs() < 1e-6);
    assert!((q_comma.value_f64().unwrap() - 1234.56_f64).abs() < 1e-6);
    assert_eq!(q_none.value_f64(), None);
}

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
#[test]
fn qty_is_metered() {
    use edi_energy::messages::segments::Qty;
    let metered = Qty {
        qualifier: "220".into(),
        value: None,
        unit: None,
    };
    let not_metered = Qty {
        qualifier: "211".into(),
        value: None,
        unit: None,
    };
    assert!(metered.is_metered());
    assert!(!not_metered.is_metered());
}

#[cfg(feature = "utilmd")]
#[test]
fn bgm_pruefidentifikator_from_document_id() {
    use edi_energy::messages::segments::Bgm;
    let bgm_valid = Bgm {
        document_code: "E01".into(),
        document_id: Some("11001".into()),
        function: None,
    };
    let bgm_invalid = Bgm {
        document_code: "E01".into(),
        document_id: Some("123".into()),
        function: None,
    };
    let bgm_none = Bgm {
        document_code: "E01".into(),
        document_id: None,
        function: None,
    };
    assert!(bgm_valid.pruefidentifikator().is_some());
    assert_eq!(bgm_valid.pruefidentifikator().unwrap().as_u32(), 11001);
    assert!(
        bgm_invalid.pruefidentifikator().is_none(),
        "out-of-range code should return None"
    );
    assert!(bgm_none.pruefidentifikator().is_none());
}

// ── INVOIC typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "invoic")]
const INVOIC_TYPED: &[u8] = b"\
UNH+INV001+INVOIC:D:07A:UN:2.8e'\
BGM+380+00031001+9'\
DTM+137:20240101:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+INV001'";

#[cfg(feature = "invoic")]
#[test]
fn invoic_typed_fields() {
    let msg = edi_energy::parse(INVOIC_TYPED).unwrap();
    let edi_energy::AnyMessage::Invoic(m) = msg else {
        panic!("expected INVOIC")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "380");
    assert_eq!(bgm.document_id.as_deref(), Some("00031001"));
    assert_eq!(m.dtm().len(), 1);
    assert_eq!(
        m.sender().unwrap().party_id.as_deref(),
        Some("4012345000023")
    );
    assert_eq!(
        m.receiver().unwrap().party_id.as_deref(),
        Some("9900357000004")
    );
}

#[cfg(feature = "invoic")]
#[test]
fn invoic_round_trip_serialize() {
    let msg = edi_energy::parse(INVOIC_TYPED).unwrap();
    let edi_energy::AnyMessage::Invoic(m) = msg else {
        panic!("expected INVOIC")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = edi_energy::parse(&bytes).unwrap();
    let edi_energy::AnyMessage::Invoic(m2) = msg2 else {
        panic!("re-parse must be INVOIC")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
    assert_eq!(m.receiver(), m2.receiver(), "receiver should round-trip");
}

// ── REMADV typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "remadv")]
const REMADV_TYPED: &[u8] = b"\
UNH+REM001+REMADV:D:07A:UN:2.9e'\
BGM+481+00033001+9'\
DTM+137:20240201:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+REM001'";

#[cfg(feature = "remadv")]
#[test]
fn remadv_typed_fields() {
    let msg = edi_energy::parse(REMADV_TYPED).unwrap();
    let edi_energy::AnyMessage::Remadv(m) = msg else {
        panic!("expected REMADV")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "481");
    assert_eq!(bgm.document_id.as_deref(), Some("00033001"));
    assert_eq!(m.dtm().len(), 1);
    assert_eq!(
        m.sender().unwrap().party_id.as_deref(),
        Some("4012345000023")
    );
}

#[cfg(feature = "remadv")]
#[test]
fn remadv_round_trip_serialize() {
    let msg = edi_energy::parse(REMADV_TYPED).unwrap();
    let edi_energy::AnyMessage::Remadv(m) = msg else {
        panic!("expected REMADV")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = edi_energy::parse(&bytes).unwrap();
    let edi_energy::AnyMessage::Remadv(m2) = msg2 else {
        panic!("re-parse must be REMADV")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
}

// ── ORDERS typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "orders")]
const ORDERS_TYPED: &[u8] = b"\
UNH+ORD001+ORDERS:D:07A:UN:1.4b'\
BGM+105+00036001+9'\
DTM+137:20240301:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+ORD001'";

#[cfg(feature = "orders")]
#[test]
fn orders_typed_fields() {
    let msg = edi_energy::parse(ORDERS_TYPED).unwrap();
    let edi_energy::AnyMessage::Orders(m) = msg else {
        panic!("expected ORDERS")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "105");
    assert_eq!(bgm.document_id.as_deref(), Some("00036001"));
    assert_eq!(m.dtm().len(), 1);
    assert_eq!(
        m.sender().unwrap().party_id.as_deref(),
        Some("4012345000023")
    );
}

#[cfg(feature = "orders")]
#[test]
fn orders_round_trip_serialize() {
    let msg = edi_energy::parse(ORDERS_TYPED).unwrap();
    let edi_energy::AnyMessage::Orders(m) = msg else {
        panic!("expected ORDERS")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = edi_energy::parse(&bytes).unwrap();
    let edi_energy::AnyMessage::Orders(m2) = msg2 else {
        panic!("re-parse must be ORDERS")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
}

// ── IFTSTA typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "iftsta")]
const IFTSTA_TYPED: &[u8] = b"\
UNH+IFT001+IFTSTA:D:95B:UN:2.0g'\
BGM+77+00044001+9'\
DTM+137:20240401:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+IFT001'";

#[cfg(feature = "iftsta")]
#[test]
fn iftsta_typed_fields() {
    let msg = edi_energy::parse(IFTSTA_TYPED).unwrap();
    let edi_energy::AnyMessage::Iftsta(m) = msg else {
        panic!("expected IFTSTA")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "77");
    assert_eq!(bgm.document_id.as_deref(), Some("00044001"));
    assert_eq!(m.dtm().len(), 1);
    assert_eq!(
        m.sender().unwrap().party_id.as_deref(),
        Some("4012345000023")
    );
}

#[cfg(feature = "iftsta")]
#[test]
fn iftsta_round_trip_serialize() {
    let msg = edi_energy::parse(IFTSTA_TYPED).unwrap();
    let edi_energy::AnyMessage::Iftsta(m) = msg else {
        panic!("expected IFTSTA")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = edi_energy::parse(&bytes).unwrap();
    let edi_energy::AnyMessage::Iftsta(m2) = msg2 else {
        panic!("re-parse must be IFTSTA")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
}

// ── INSRPT typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "insrpt")]
const INSRPT_TYPED: &[u8] = b"\
UNH+INS001+INSRPT:D:96A:UN:1.1a'\
BGM+17+00023001+9'\
DTM+137:20240501:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+INS001'";

#[cfg(feature = "insrpt")]
#[test]
fn insrpt_typed_fields() {
    let msg = edi_energy::parse(INSRPT_TYPED).unwrap();
    let edi_energy::AnyMessage::Insrpt(m) = msg else {
        panic!("expected INSRPT")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "17");
    assert_eq!(bgm.document_id.as_deref(), Some("00023001"));
    assert_eq!(m.dtm().len(), 1);
}

#[cfg(feature = "insrpt")]
#[test]
fn insrpt_round_trip_serialize() {
    let msg = edi_energy::parse(INSRPT_TYPED).unwrap();
    let edi_energy::AnyMessage::Insrpt(m) = msg else {
        panic!("expected INSRPT")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = edi_energy::parse(&bytes).unwrap();
    let edi_energy::AnyMessage::Insrpt(m2) = msg2 else {
        panic!("re-parse must be INSRPT")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
}

// ── REQOTE typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "reqote")]
const REQOTE_TYPED: &[u8] = b"\
UNH+REQ001+REQOTE:D:07A:UN:1.3c'\
BGM+68+00035004+9'\
DTM+137:20240601:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+REQ001'";

#[cfg(feature = "reqote")]
#[test]
fn reqote_typed_fields() {
    let msg = edi_energy::parse(REQOTE_TYPED).unwrap();
    let edi_energy::AnyMessage::Reqote(m) = msg else {
        panic!("expected REQOTE")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "68");
    assert_eq!(bgm.document_id.as_deref(), Some("00035004"));
    assert_eq!(m.dtm().len(), 1);
}

#[cfg(feature = "reqote")]
#[test]
fn reqote_round_trip_serialize() {
    let msg = edi_energy::parse(REQOTE_TYPED).unwrap();
    let edi_energy::AnyMessage::Reqote(m) = msg else {
        panic!("expected REQOTE")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = edi_energy::parse(&bytes).unwrap();
    let edi_energy::AnyMessage::Reqote(m2) = msg2 else {
        panic!("re-parse must be REQOTE")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
}

// ── PARTIN typed fields ───────────────────────────────────────────────────────

#[cfg(feature = "partin")]
const PARTIN_TYPED: &[u8] = b"\
UNH+PAR001+PARTIN:D:96A:UN:1.0f'\
BGM+35+00037007+9'\
DTM+137:20240701:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+PAR001'";

#[cfg(feature = "partin")]
#[test]
fn partin_typed_fields() {
    let msg = edi_energy::parse(PARTIN_TYPED).unwrap();
    let edi_energy::AnyMessage::Partin(m) = msg else {
        panic!("expected PARTIN")
    };
    let bgm = m.bgm().expect("BGM must be present");
    assert_eq!(bgm.document_code, "35");
    assert_eq!(bgm.document_id.as_deref(), Some("00037007"));
    assert_eq!(m.dtm().len(), 1);
}

#[cfg(feature = "partin")]
#[test]
fn partin_round_trip_serialize() {
    let msg = edi_energy::parse(PARTIN_TYPED).unwrap();
    let edi_energy::AnyMessage::Partin(m) = msg else {
        panic!("expected PARTIN")
    };
    let bytes = m.serialize().expect("serialize must succeed");
    let msg2 = edi_energy::parse(&bytes).unwrap();
    let edi_energy::AnyMessage::Partin(m2) = msg2 else {
        panic!("re-parse must be PARTIN")
    };
    assert_eq!(m.bgm(), m2.bgm(), "BGM should round-trip");
    assert_eq!(m.sender(), m2.sender(), "sender should round-trip");
}
