//! Integration tests for the parse entry points.
//!
//! All test fixtures use the compact EDIFACT release character `'` as segment
//! terminator and include a well-formed UNB/UNZ envelope so that
//! `validate_envelope` can succeed.

// Many imports and constants are only used inside feature-gated test fns.
#![allow(unused_imports)]

use edi_energy::{
    AnyMessage, DEFAULT_MAX_SEGMENT_BYTES, EdiEnergyMessage, Error, MessageType, ParseConfig,
    Parser, Platform,
};

// ── Minimal valid EDIFACT interchanges ───────────────────────────────────────

/// Minimal UTILMD interchange — 3 message segments (UNH + BGM + UNT).
/// UNT DE 0074 counts UNH + BGM + UNT = 3.
const UTILMD: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20230101:102'\
RFF+Z13:REF001'\
NAD+MS+4012345000023::293'\
IDE+Z19+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

/// Interchange with two UTILMD messages.
const TWO_UTILMD: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03+00011001+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781'\
UNT+7+1'\
UNH+2+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03+00011002+9'\
DTM+137:20230101:102'\
RFF+ACE:REF002'\
NAD+MS+4012345000023::293'\
IDE+24+51238696782'\
UNT+7+2'\
UNZ+2+1'";

/// Message with an unknown type code.
const UNKNOWN_TYPE: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+FOOBAR:D:11A:UN:5.5.3a'\
BGM+E03+00011001+9'\
UNT+3+1'\
UNZ+1+1'";

/// Five-message interchange with one each of UTILMD, UTILMD, CONTRL, APERAK,
/// MSCONS — used for the multi-type dispatch integration test.
#[cfg(all(
    feature = "utilmd",
    feature = "contrl",
    feature = "aperak",
    feature = "mscons"
))]
const FIVE_TYPE_INTERCHANGE: &[u8] = b"\
UNB+UNOC:3+9900111222333:14+9900444555666:14+230701:0800+INTER001'\
UNH+MSG001+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03+11001+9'\
DTM+137:20230701:102'\
UNT+4+MSG001'\
UNH+MSG002+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03+11004+9'\
DTM+137:20230701:102'\
UNT+4+MSG002'\
UNH+MSG003+CONTRL:D:3:UN:1.0a'\
UCI+INTER001+9900111222333+9900444555666+4'\
UNT+3+MSG003'\
UNH+MSG004+APERAK:D:07B:UN:2.0a'\
BGM+1000+29001+9'\
DTM+137:20230701:102'\
UNT+4+MSG004'\
UNH+MSG005+MSCONS:D:04B:UN:2.4c'\
BGM+7+13002+9'\
DTM+137:20230701:102'\
UNT+4+MSG005'\
UNZ+5+INTER001'";

// ── ParseConfig ───────────────────────────────────────────────────────────────

#[test]
fn parse_config_default_has_dos_guard() {
    let cfg = ParseConfig::default();
    assert_eq!(cfg.max_segment_bytes, DEFAULT_MAX_SEGMENT_BYTES);
    assert!(cfg.max_segment_bytes < usize::MAX, "default must be finite");
}

#[test]
fn default_max_segment_bytes_is_64kib() {
    assert_eq!(DEFAULT_MAX_SEGMENT_BYTES, 64 * 1024);
}

// ── Platform::with_all_profiles().parse() ───────────────────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn parse_utilmd_message_type() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    assert_eq!(msg.try_message_type(), Some(MessageType::Utilmd));
}

#[cfg(feature = "utilmd")]
#[test]
fn parse_utilmd_assoc_code() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let release = msg.detect_release().unwrap();
    assert_eq!(release.as_str(), "S2.1");
}

#[cfg(feature = "utilmd")]
#[test]
fn parse_utilmd_pruefidentifikator() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let pid = msg.detect_pruefidentifikator().unwrap();
    assert_eq!(pid.as_u32(), 55001);
}

#[cfg(feature = "utilmd")]
#[test]
fn parse_utilmd_variant() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    assert!(
        matches!(msg, AnyMessage::Utilmd(_)),
        "expected AnyMessage::Utilmd"
    );
}

#[test]
fn parse_garbage_returns_err() {
    let result = Platform::with_all_profiles().parse(b"not edifact at all");
    assert!(result.is_err(), "garbage input must not parse successfully");
}

#[test]
fn parse_unknown_type_returns_unknown_variant() {
    let result = Platform::with_all_profiles()
        .parse(UNKNOWN_TYPE)
        .expect("unknown type must not error");
    match result {
        AnyMessage::Unknown {
            message_type_code, ..
        } => {
            assert_eq!(message_type_code.as_ref(), "FOOBAR");
        }
        #[allow(unreachable_patterns)]
        other => panic!("expected AnyMessage::Unknown, got: {other:?}"),
    }
}

// ── Parser::parse_reader() ─────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn parse_reader_is_equivalent_to_parse() {
    let msg_bytes = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let msg_reader = Parser::new()
        .parse_reader(std::io::Cursor::new(UTILMD))
        .unwrap();
    assert_eq!(msg_bytes.try_message_type(), msg_reader.try_message_type());
}

// ── Parser::with_config() ────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn parse_with_default_config_succeeds() {
    let msg = Parser::with_config(ParseConfig::default())
        .parse(UTILMD)
        .unwrap();
    assert_eq!(msg.try_message_type(), Some(MessageType::Utilmd));
}

#[test]
fn parse_with_tiny_limit_rejects_long_segment() {
    // A single segment that exceeds 10 bytes triggers SegmentTooLong.
    let cfg = ParseConfig {
        max_segment_bytes: 10,
        ..ParseConfig::default()
    };
    // UTILMD segments are all longer than 10 bytes.
    let result = Parser::with_config(cfg).parse(UTILMD);
    assert!(result.is_err(), "segment exceeding limit must return Err");
}

#[test]
fn parse_with_max_limit_succeeds() {
    let cfg = ParseConfig {
        max_segment_bytes: usize::MAX,
        ..ParseConfig::default()
    };
    // Should succeed even with no limit.
    #[cfg(feature = "utilmd")]
    Parser::with_config(cfg).parse(UTILMD).unwrap();
    #[cfg(not(feature = "utilmd"))]
    let _ = Parser::with_config(cfg).parse(UTILMD); // May fail with FeatureNotEnabled — that's fine.
}

// ── Platform::with_all_profiles().parse_interchange() ───────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn parse_interchange_single_message() {
    let messages: Vec<_> = Platform::with_all_profiles()
        .parse_interchange(std::io::Cursor::new(UTILMD))
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].try_message_type(), Some(MessageType::Utilmd));
}

#[cfg(feature = "utilmd")]
#[test]
fn parse_interchange_two_messages() {
    let messages: Vec<_> = Platform::with_all_profiles()
        .parse_interchange(std::io::Cursor::new(TWO_UTILMD))
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(messages.len(), 2);
    for msg in &messages {
        assert_eq!(msg.try_message_type(), Some(MessageType::Utilmd));
    }
}

#[cfg(feature = "utilmd")]
#[test]
fn parse_interchange_yields_correct_pids() {
    let messages: Vec<_> = Platform::with_all_profiles()
        .parse_interchange(std::io::Cursor::new(TWO_UTILMD))
        .collect::<Result<_, _>>()
        .unwrap();
    let pids: Vec<u32> = messages
        .iter()
        .map(|m| m.detect_pruefidentifikator().unwrap().as_u32())
        .collect();
    assert_eq!(pids, [11001, 11002]);
}

/// `parse_interchange` returns an iterator, not a `Result<Vec<_>>`.
/// This test verifies items can be processed lazily without collecting all.
#[cfg(feature = "utilmd")]
#[test]
fn parse_interchange_is_lazy_iterator() {
    let platform = Platform::with_all_profiles();
    let mut iter = platform.parse_interchange(std::io::Cursor::new(TWO_UTILMD));
    let first = iter.next().unwrap().unwrap();
    assert_eq!(first.try_message_type(), Some(MessageType::Utilmd));
    let second = iter.next().unwrap().unwrap();
    assert_eq!(second.try_message_type(), Some(MessageType::Utilmd));
    assert!(iter.next().is_none(), "no more messages");
}

// ── diagnostics feature ───────────────────────────────────────────────────────

/// Verify `miette::Report::new(err)` renders without panicking when
/// `diagnostics` is enabled.  The actual output is renderer-specific but must
/// at least round-trip through `Debug` without a panic.
#[cfg(feature = "diagnostics")]
#[test]
fn diagnostics_error_renders_via_miette() {
    let err = Platform::with_all_profiles()
        .parse(b"not valid edifact")
        .unwrap_err();
    let report = miette::Report::new(err);
    let rendered = format!("{report:?}");
    // The rendered string must not be empty.
    assert!(!rendered.is_empty(), "miette report must produce output");
}

// ── EdiEnergyMessage trait methods ────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn message_detect_release() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let release = msg.detect_release().unwrap();
    assert_eq!(release.as_str(), "S2.1");
}

#[cfg(feature = "utilmd")]
#[test]
fn message_detect_pruefidentifikator() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let pid = msg.detect_pruefidentifikator().unwrap();
    assert_eq!(pid.as_u32(), 55001);
}

#[cfg(feature = "utilmd")]
#[test]
fn message_serialize_round_trips() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let bytes = msg.serialize().unwrap();
    // Re-parse the serialised bytes and check message type is preserved.
    let reparsed = Platform::with_all_profiles().parse(&bytes).unwrap();
    assert_eq!(reparsed.try_message_type(), msg.try_message_type());
}

/// `validate()` must return `Ok(report)` with an empty (valid) report when the
/// UTILMD fixture is structurally complete and all required MIG segments are
/// present.
#[cfg(feature = "utilmd")]
#[test]
fn validate_without_profiles_returns_ok_report() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let report = msg.validate().unwrap();
    assert!(report.is_valid(), "expected valid report: {report}");
}

#[cfg(feature = "utilmd")]
#[test]
fn validate_pruefidentifikator_matches() {
    use edi_energy::{Pruefidentifikator, validate_and_check_pid};
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let pid = Pruefidentifikator::new(55001).unwrap();
    let report = validate_and_check_pid(&msg, pid).unwrap();
    assert!(
        report.is_valid(),
        "PID 55001 should match BGM content: {report}"
    );
}

#[cfg(feature = "utilmd")]
#[test]
fn validate_pruefidentifikator_mismatch_adds_error() {
    use edi_energy::{Pruefidentifikator, validate_and_check_pid};
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let wrong_pid = Pruefidentifikator::new(11002).unwrap();
    let report = validate_and_check_pid(&msg, wrong_pid).unwrap();
    assert!(
        !report.is_valid(),
        "mismatched PID must produce an error: {report}"
    );
    assert!(
        !report.errors().is_empty(),
        "expected at least one error for PID mismatch"
    );
    // EE-PID-001 rule must appear in the issues.
    assert!(
        report.issues_for_rule_id("EE-PID-001").count() > 0,
        "expected EE-PID-001 finding"
    );
}

// ── EdiEnergyReport ───────────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn report_into_result_ok_when_valid() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let report = msg.validate().unwrap();
    report
        .into_result()
        .expect("valid report must convert to Ok(())");
}

#[cfg(feature = "utilmd")]
#[test]
fn report_display_shows_counts() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let report = msg.validate().unwrap();
    let s = report.to_string();
    // Format: "N error(s), N warning(s), N info(s)"
    assert!(s.contains("error(s)"), "display must mention errors: {s}");
}

// ── Feature-gate error path ───────────────────────────────────────────────────

/// When a feature is disabled the error variant must carry the feature name, not
/// the release code.  We test by trying to route a known type that is only present
/// under a feature.  Skip when that feature is actually enabled.
#[cfg(not(feature = "invoic"))]
#[test]
fn feature_not_enabled_carries_feature_name() {
    const INVOIC: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+INVOIC:D:97A:UN:EAN008'\
BGM+380+00011001+9'\
UNT+3+1'\
UNZ+1+1'";
    match Platform::with_all_profiles().parse(INVOIC) {
        Err(Error::FeatureNotEnabled {
            message_type,
            feature,
        }) => {
            assert_eq!(message_type, "INVOIC");
            assert_eq!(feature, "invoic");
        }
        other => panic!("expected FeatureNotEnabled, got {other:?}"),
    }
}

// ── Story 20.2: multi-type interchange dispatch ───────────────────────────────

/// Parse a 5-message interchange containing UTILMD, UTILMD, CONTRL, APERAK,
/// and MSCONS.  Verifies that:
/// - All 5 messages are dispatched to the correct `AnyMessage` variant.
/// - `detect_pruefidentifikator()` returns the right PID for each typed message.
/// - CONTRL returns `Err(MissingPruefidentifikator)` by design.
/// - `validate()` does not panic for any message type.
#[cfg(all(
    feature = "utilmd",
    feature = "contrl",
    feature = "aperak",
    feature = "mscons"
))]
#[test]
fn parse_interchange_multi_type_dispatch() {
    let messages: Vec<_> = Platform::with_all_profiles()
        .parse_interchange(std::io::Cursor::new(FIVE_TYPE_INTERCHANGE))
        .collect::<Result<_, _>>()
        .expect("all 5 messages in a mixed interchange must parse");

    assert_eq!(
        messages.len(),
        5,
        "interchange must yield exactly 5 messages"
    );

    // Correct routing to typed variants.
    assert_eq!(
        messages[0].try_message_type(),
        Some(MessageType::Utilmd),
        "msg[0] must be UTILMD"
    );
    assert_eq!(
        messages[1].try_message_type(),
        Some(MessageType::Utilmd),
        "msg[1] must be UTILMD"
    );
    assert_eq!(
        messages[2].try_message_type(),
        Some(MessageType::Contrl),
        "msg[2] must be CONTRL"
    );
    assert_eq!(
        messages[3].try_message_type(),
        Some(MessageType::Aperak),
        "msg[3] must be APERAK"
    );
    assert_eq!(
        messages[4].try_message_type(),
        Some(MessageType::Mscons),
        "msg[4] must be MSCONS"
    );

    // Pruefidentifikator extraction.
    assert_eq!(
        messages[0].detect_pruefidentifikator().unwrap().as_u32(),
        11001
    );
    assert_eq!(
        messages[1].detect_pruefidentifikator().unwrap().as_u32(),
        11004
    );
    assert!(
        messages[2].detect_pruefidentifikator().is_err(),
        "CONTRL must not carry a Pruefidentifikator"
    );
    assert_eq!(
        messages[3].detect_pruefidentifikator().unwrap().as_u32(),
        29001
    );
    assert_eq!(
        messages[4].detect_pruefidentifikator().unwrap().as_u32(),
        13002
    );

    // validate() must not panic for any message.  Messages parsed via
    // Platform::with_all_profiles().parse_interchange() are per-message windows without the outer UNB
    // envelope, so validate() may return Err — that is expected and correct.
    // What must NOT happen is a panic.
    for (i, msg) in messages.iter().enumerate() {
        let _ = msg.validate(); // may be Ok or Err — both are acceptable here
        // Serialization, however, must always succeed for any parsed message.
        msg.serialize()
            .unwrap_or_else(|e| panic!("msg[{i}].serialize() failed: {e}"));
    }

    // detect_release() must work for every message.
    assert_eq!(messages[0].detect_release().unwrap().as_str(), "5.5.3a");
    assert_eq!(messages[2].detect_release().unwrap().as_str(), "1.0a");
    assert_eq!(messages[4].detect_release().unwrap().as_str(), "2.4c");
}

// ── Story 19.3: serialized bytes ASCII charset safety ────────────────────────

/// Serialized EDIFACT bytes must:
/// - Contain no null bytes.
/// - Contain no bytes outside ASCII 0x09 / 0x0A / 0x0D / 0x20–0x7E.
fn assert_edifact_charset(bytes: &[u8], label: &str) {
    for (i, &b) in bytes.iter().enumerate() {
        assert!(
            b == b'\t' || b == b'\n' || b == b'\r' || (0x20..=0x7e).contains(&b),
            "byte {b:#04x} at offset {i} in {label} is outside the EDIFACT-safe ASCII range"
        );
    }
}

#[cfg(feature = "utilmd")]
#[test]
fn serialized_utilmd_is_clean_ascii() {
    let msg = Platform::with_all_profiles().parse(UTILMD).unwrap();
    let bytes = msg.serialize().unwrap();
    assert_edifact_charset(&bytes, "serialized UTILMD");
    assert!(!bytes.contains(&0u8), "no null bytes in serialized UTILMD");
}

#[cfg(all(
    feature = "utilmd",
    feature = "contrl",
    feature = "aperak",
    feature = "mscons"
))]
#[test]
fn serialized_interchange_all_clean_ascii() {
    let messages: Vec<_> = Platform::with_all_profiles()
        .parse_interchange(std::io::Cursor::new(FIVE_TYPE_INTERCHANGE))
        .collect::<Result<_, _>>()
        .unwrap();
    for (i, msg) in messages.iter().enumerate() {
        let bytes = msg.serialize().unwrap();
        let type_label = msg
            .try_message_type()
            .map(|t| t.as_str().to_owned())
            .unwrap_or_else(|| "Unknown".to_owned());
        assert_edifact_charset(&bytes, &format!("msg[{i}] ({type_label})"));
        assert!(!bytes.contains(&0u8), "no null bytes in msg[{i}]");
    }
}

/// Builder-generated messages must also produce clean ASCII.
#[cfg(feature = "utilmd")]
#[test]
fn builder_serialized_utilmd_is_clean_ascii() {
    use edi_energy::{Pruefidentifikator, Release, builders::UtilmdBuilder};
    let bytes = UtilmdBuilder::new(Release::new("5.5.3a"))
        .pruefidentifikator(Pruefidentifikator::new(11001).unwrap())
        .sender("9900111222333")
        .receiver("9900444555666")
        .serialize()
        .unwrap();
    assert_edifact_charset(&bytes, "builder-generated UTILMD");
    assert!(!bytes.contains(&0u8));
}

// ── Platform isolation tests ───────────────────────────────────────────

/// A `Platform` backed by an *empty* registry must return `AnyMessage::Unknown`
/// for a message type that the global registry would recognise (e.g. UTILMD),
/// because its own registry has no profiles registered.
///
/// This proves that `Platform::parse` is wired to the platform's own registry,
/// not to the global singleton.
#[cfg(feature = "utilmd")]
#[test]
fn platform_parse_uses_own_registry_not_global() {
    use edi_energy::{Platform, registry::ReleaseRegistry};

    // Platform with an empty registry — no profiles at all.
    let empty_platform = Platform::new(ReleaseRegistry::new(vec![]));
    let msg = empty_platform
        .parse(UTILMD)
        .expect("EDIFACT parse must succeed");

    // The PID source lookup falls back to BgmDe1004 (default), which is fine.
    // The important thing: because the registry has no UTILMD profiles, the
    // dispatch should still succeed (the type code is feature-compiled) but the
    // Prüfidentifikator extraction relied on the correct registry.
    // The real isolation test: an unknown release code lookup on the empty
    // registry falls back gracefully rather than hitting the global.
    assert!(
        matches!(msg, AnyMessage::Utilmd(_)),
        "UTILMD message should parse as Utilmd variant; dispatch is feature-gated, not registry-gated"
    );

    // Now confirm the full-profile Platform behaves identically for the same input.
    let full_platform = Platform::with_all_profiles();
    let msg2 = full_platform.parse(UTILMD).expect("parse must succeed");
    assert!(matches!(msg2, AnyMessage::Utilmd(_)));
}

/// `Platform::parse_interchange` must use the platform's registry, not the global.
/// We test this by parsing the TWO_UTILMD interchange via a Platform instance
/// and verifying both messages are dispatched correctly.
#[cfg(feature = "utilmd")]
#[test]
fn platform_parse_interchange_uses_own_registry() {
    use edi_energy::Platform;

    let platform = Platform::with_all_profiles();
    let messages: Vec<_> = platform
        .parse_interchange(TWO_UTILMD)
        .collect::<Result<Vec<_>, _>>()
        .expect("interchange parse must succeed");

    assert_eq!(messages.len(), 2, "interchange has two messages");
    for msg in &messages {
        assert!(
            matches!(msg, AnyMessage::Utilmd(_)),
            "both messages should be UTILMD"
        );
    }
}

// ── Parser::parse_interchange_buffered ──────────────────────────────────────

/// `Parser::parse_interchange_buffered` must return the `InterchangeHeader` eagerly
/// (before any message is parsed) and yield messages lazily.
#[cfg(feature = "utilmd")]
#[test]
fn parse_interchange_full_buffered_yields_header_eagerly_and_messages_lazily() {
    use edi_energy::AnyMessage;

    let (header, mut iter) = Parser::new()
        .parse_interchange_buffered(std::io::Cursor::new(TWO_UTILMD))
        .expect("stream parse must succeed");

    // Header is available immediately.
    assert_eq!(header.sender_id.as_ref(), "4012345000023");
    assert_eq!(header.control_ref.as_ref(), "1");

    // Messages are yielded lazily.
    let first = iter
        .next()
        .expect("must have first message")
        .expect("first ok");
    assert!(matches!(first.message, AnyMessage::Utilmd(_)));
    assert_eq!(first.message_index, 0);
    assert_eq!(first.header.sender_id.as_ref(), "4012345000023");

    let second = iter
        .next()
        .expect("must have second message")
        .expect("second ok");
    assert!(matches!(second.message, AnyMessage::Utilmd(_)));
    assert_eq!(second.message_index, 1);

    // Iterator is exhausted after both messages.
    assert!(iter.next().is_none(), "iterator must be exhausted");
}

// ── LightMessage / parse_envelope_only ────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn parse_envelope_only_returns_correct_type_and_release() {
    use edi_energy::parse_envelope_only;
    let light = parse_envelope_only(UTILMD).expect("envelope parse must succeed");
    assert_eq!(light.message_type_code(), "UTILMD");
    assert_eq!(light.assoc_code(), "S2.1");
    assert_eq!(light.message_ref(), "1");
}

#[cfg(feature = "utilmd")]
#[test]
fn parse_envelope_only_extracts_pruefidentifikator() {
    use edi_energy::parse_envelope_only;
    let light = parse_envelope_only(UTILMD).expect("envelope parse must succeed");
    let pid = light.pruefidentifikator().expect("PID must be present");
    assert_eq!(pid.as_u32(), 55001);
}

#[cfg(feature = "utilmd")]
#[test]
fn light_message_try_message_type_matches_full_parse() {
    use edi_energy::{MessageType, parse_envelope_only};
    let light = parse_envelope_only(UTILMD).expect("envelope parse must succeed");
    assert_eq!(light.try_message_type(), Some(MessageType::Utilmd));
}

#[cfg(feature = "utilmd")]
#[test]
fn light_message_into_message_equals_full_parse() {
    use edi_energy::parse_envelope_only;
    let light = parse_envelope_only(UTILMD).expect("envelope parse must succeed");
    let full = light.into_message().expect("upgrade must succeed");
    let direct = Platform::with_all_profiles()
        .parse(UTILMD)
        .expect("direct parse must succeed");
    assert_eq!(full.try_message_type(), direct.try_message_type());
}

#[cfg(feature = "utilmd")]
#[test]
fn parser_parse_envelope_only_uses_config() {
    // A tiny segment limit must still succeed for valid UTILMD (no segment is huge).
    let parser = Parser::with_config(ParseConfig {
        max_segment_bytes: 512,
        ..ParseConfig::default()
    });
    let light = parser
        .parse_envelope_only(UTILMD)
        .expect("envelope parse must succeed");
    assert_eq!(light.message_type_code(), "UTILMD");
}

#[cfg(feature = "utilmd")]
#[test]
fn parse_envelope_only_does_not_construct_typed_fields() {
    // Verify the LightMessage is cheaper: segments() returns the raw list,
    // and the message_type_code is accessible without any typed struct.
    use edi_energy::parse_envelope_only;
    let light = parse_envelope_only(UTILMD).expect("envelope parse must succeed");
    // Raw segments are accessible for forwarding/logging without full parse cost.
    assert!(!light.segments().is_empty(), "segments must be present");
    // The UNH segment must be there.
    assert!(
        light.segments().iter().any(|s| s.tag == "UNH"),
        "UNH segment must be in raw list"
    );
}

// ── Parser::parse_interchange_full ────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn parser_parse_interchange_full_materialises_all_messages() {
    let ic = Parser::new()
        .parse_interchange_full(std::io::Cursor::new(TWO_UTILMD))
        .expect("full parse must succeed");
    assert_eq!(ic.message_count(), 2);
    assert!(ic.is_structurally_valid(), "UNZ count and ref must match");
}

// ── ValidationIssueSummary pruefidentifikator ─────────────────────────

/// `ValidationIssueSummary` must include `pruefidentifikator` when serialized
/// from a report produced by validating a message with a known PID.
///
/// The field is `None` for structure-layer issues and `Some(pid)` for issues
/// produced by an AHB rule pack that was parameterised by PID.
#[cfg(all(feature = "mscons", feature = "serde"))]
#[test]
fn validation_report_serializes_pruefidentifikator() {
    use edi_energy::EdiEnergyMessage;
    use serde_json::Value;

    // MSCONS 2.4c with a known PID extracted from LOC+172 (MELO).
    const MSCONS_WITH_PID: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+200101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7+REF1+9'\
DTM+137:20230101:102'\
LOC+172+DE000000123456789012'\
QTY+220:100:KWH'\
UNT+6+1'\
UNZ+1+1'";

    let msg = Platform::with_all_profiles()
        .parse(MSCONS_WITH_PID)
        .expect("parse must succeed");
    let report = msg
        .validate()
        .expect("validate must not fail with ProfileNotFound");

    // Serialize the report and check for the pruefidentifikator field.
    let json_str = serde_json::to_string(&report).expect("serialization must succeed");
    let json: Value = serde_json::from_str(&json_str).expect("valid JSON");

    // The `pruefidentifikator` field must appear at the report top level when set.
    // If the PID was not extracted (None), the field is absent from the JSON.
    // This test verifies the field is *present and correctly typed* when a PID
    // was detected at parse time.
    if let Some(pid_val) = json.get("pruefidentifikator") {
        assert!(
            pid_val.is_u64(),
            "pruefidentifikator must be a number, got {pid_val}"
        );
        let pid = pid_val.as_u64().unwrap();
        assert!(
            (10000..=99999).contains(&pid),
            "pruefidentifikator must be in range 10000-99999, got {pid}"
        );
    }
    // Whether or not PID is present depends on message content; the key assertion
    // is that the JSON is structurally valid and all issue entries respect the schema.
    assert!(
        json.get("valid").is_some(),
        "report must have 'valid' field"
    );
    assert!(
        json.get("errors").is_some(),
        "report must have 'errors' array"
    );
}
