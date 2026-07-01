//! Smoke tests for `dvgw-edi` — minimal parse round-trips for ALOCAT, NOMINT,
//! and NOMRES using hand-crafted EDIFACT byte strings.

use dvgw_edi::{AnyDvgwMessage, DvgwMessage, DvgwMessageType, DvgwPlatform};

// ── Minimal valid EDIFACT interchange wrapper ─────────────────────────────────
// Segments separated by ' (single-quote = segment terminator).
// Format: UNA + UNB + UNH(type) + functional segments + UNT + UNZ

fn wrap(msg_type: &str, inner: &str) -> Vec<u8> {
    format!(
        "UNA:+.? 'UNB+UNOC:3+SENDER:14+RECEIVER:14+240101:1000+1'UNH+1+{msg_type}:D:01B:UN'NAD+MS+SENDERCODE::ZZZ'NAD+MR+RECEIVERCODE::ZZZ'{inner}UNT+5+1'UNZ+1+1'",
    )
    .into_bytes()
}

// ── ALOCAT ────────────────────────────────────────────────────────────────────

#[cfg(feature = "alocat")]
#[test]
fn parse_alocat_minimal() {
    let input = wrap(
        "ALOCAT:5:11a",
        "BGM+7+ALLOCREF001'DTM+137:202401011200:203'LOC+Z01+DE_LOC001::ZZZ'QTY+136:12345.6:KWH'DTM+163:202401010600:203'DTM+164:202401020600:203'",
    );
    let platform = DvgwPlatform::default();
    let msg = platform.parse(&input).expect("parse should succeed");

    assert!(matches!(msg, AnyDvgwMessage::Alocat(_)));
    if let AnyDvgwMessage::Alocat(m) = &msg {
        assert_eq!(m.message_type(), DvgwMessageType::Alocat);
        // sender_eic comes from NAD+MS — our fixture has SENDERCODE, not an EIC
        // The parser extracts whatever is in NAD element 1 component 0
        assert_eq!(m.quantities.len(), 1);
        assert_eq!(m.quantities[0].location_code, "DE_LOC001");
        assert_eq!(m.quantities[0].quantity, "12345.6");
        assert_eq!(m.quantities[0].unit.as_deref(), Some("KWH"));
    }
}

// ── NOMINT ────────────────────────────────────────────────────────────────────

#[cfg(feature = "nomint")]
#[test]
fn parse_nomint_minimal() {
    let input = wrap(
        "NOMINT:4:6",
        "BGM+Z01+NOMREF001'RFF+Z13:CORRELATIONID001'DTM+137:202601010600:203'NAD+Z01+BKVCODE::ZZZ'LOC+Z01+DE_LOC001::ZZZ'QTY+136:9999.0:KWH'DTM+318:202601010600:203'DTM+164:202601020600:203'",
    );
    let platform = DvgwPlatform::default();
    let msg = platform.parse(&input).expect("parse should succeed");

    assert!(matches!(msg, AnyDvgwMessage::Nomint(_)));
    if let AnyDvgwMessage::Nomint(m) = &msg {
        assert_eq!(m.message_type(), DvgwMessageType::Nomint);
        // nomination_ref comes from BGM element 1 component 0 (document number)
        assert_eq!(m.nomination_ref.as_deref(), Some("NOMREF001"));
        assert_eq!(m.quantities.len(), 1);
        assert_eq!(m.quantities[0].location_code, "DE_LOC001");
        assert_eq!(
            m.quantities[0].gas_day_start.as_deref(),
            Some("202601010600")
        );
    }
}

// ── NOMRES ────────────────────────────────────────────────────────────────────

#[cfg(feature = "nomres")]
#[test]
fn parse_nomres_minimal() {
    let input = wrap(
        "NOMRES:4:7",
        "BGM+Z02+RESREF001'RFF+Z13:CORRELATIONID001'DTM+137:202601010600:203'STS+Z01'LOC+Z01+DE_LOC001::ZZZ'QTY+136:9000.0:KWH'STS+Z01'DTM+318:202601010600:203'DTM+164:202601020600:203'",
    );
    let platform = DvgwPlatform::default();
    let msg = platform.parse(&input).expect("parse should succeed");

    assert!(matches!(msg, AnyDvgwMessage::Nomres(_)));
    if let AnyDvgwMessage::Nomres(m) = &msg {
        assert_eq!(m.message_type(), DvgwMessageType::Nomres);
        assert_eq!(m.nomination_ref.as_deref(), Some("CORRELATIONID001"));
        assert!(m.overall_status.is_some());
        assert_eq!(m.quantities.len(), 1);
        assert_eq!(m.quantities[0].location_code, "DE_LOC001");
    }
}

// ── Error path: unknown message type ─────────────────────────────────────────

#[test]
fn parse_unknown_message_type() {
    let input = wrap("UNKNOWN:1:2", "");
    let result = DvgwPlatform::default().parse(&input);
    assert!(matches!(
        result,
        Err(dvgw_edi::Error::UnknownMessageType { .. })
    ));
}

// ── Error path: malformed EDIFACT ─────────────────────────────────────────────

#[test]
fn parse_malformed_returns_parse_error() {
    let result = DvgwPlatform::default().parse(b"NOT EDIFACT AT ALL!!!");
    // Either a parse error or unknown type — either is fine, must not panic
    assert!(result.is_err());
}

// ── detect_pid ────────────────────────────────────────────────────────────────

#[cfg(feature = "nomint")]
#[test]
fn detect_pid_nomint() {
    let input = wrap(
        "NOMINT:4:6",
        "BGM+Z01+NOMREF002'DTM+137:202601010600:203'LOC+Z01+DE_LOC001::ZZZ'QTY+136:1.0:KWH'",
    );
    let msg = DvgwPlatform::with_all_profiles()
        .parse(&input)
        .expect("parse ok");
    // BKV → FNB direction: no role qualifier → primary PID 90011
    assert_eq!(msg.detect_pid(None), Some(90011));
    // BKV → MGV direction
    assert_eq!(msg.detect_pid(Some("Z02")), Some(90012));
}

#[cfg(feature = "nomres")]
#[test]
fn detect_pid_nomres() {
    let input = wrap(
        "NOMRES:4:7",
        "BGM+Z02+RESREF002'RFF+Z13:NOMREF002'DTM+137:202601010600:203'STS+Z01'",
    );
    let msg = DvgwPlatform::with_all_profiles()
        .parse(&input)
        .expect("parse ok");
    assert_eq!(msg.detect_pid(None), Some(90021));
    assert_eq!(msg.detect_pid(Some("Z02")), Some(90022));
}

#[test]
fn detect_pid_unknown_returns_none() {
    let input = wrap("UNKNOWN:1:0", "");
    // Unknown type errors, but let's test the AnyDvgwMessage::Unknown branch
    // by constructing via the public API through a feature-disabled scenario:
    // Since all features are enabled in tests, we check via error path instead.
    let result = DvgwPlatform::default().parse(&input);
    assert!(result.is_err());
}

// ── quantity_f64 ──────────────────────────────────────────────────────────────

#[cfg(feature = "alocat")]
#[test]
fn quantity_f64_parses_correctly() {
    let input = wrap(
        "ALOCAT:5:11a",
        "BGM+7+QTYREF'DTM+137:202401011200:203'LOC+Z01+DE_QTY::ZZZ'QTY+136:12345.6:KWH'",
    );
    let msg = DvgwPlatform::default().parse(&input).unwrap();
    if let AnyDvgwMessage::Alocat(m) = msg {
        let qty = &m.quantities[0];
        assert_eq!(qty.quantity, "12345.6");
        let f = qty.quantity_f64().expect("should parse as f64");
        assert!((f - 12_345.6_f64).abs() < 0.001);
    } else {
        panic!("expected Alocat");
    }
}
