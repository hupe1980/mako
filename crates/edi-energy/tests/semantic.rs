//! Integration tests for Layer 5 semantic rules.
//!
//! Each test exercises a specific semantic rule ID and checks that valid
//! messages produce clean reports and invalid messages produce the expected
//! rule ID in their findings.

// Helper functions and the top-level import are only called from feature-gated
// test fns; they carry individual #[allow(dead_code)] / #[allow(unused_imports)]
// annotations below.

// Only used inside feature-gated test fns.
#[allow(unused_imports)]
use edi_energy::{EdiEnergyMessage, Platform};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Assert that `report` contains at least one *error* finding whose `rule_id`
/// starts with `prefix` and that the report is NOT valid.
#[allow(dead_code)] // only called from feature-gated test fns
#[track_caller]
fn assert_has_rule(report: &edi_energy::EdiEnergyReport, prefix: &str) {
    let filtered = report.filter_by_rule_prefix(prefix);
    assert!(
        filtered.has_errors(),
        "expected at least one error finding with rule_id starting with '{prefix}', \
         but errors were: {:#?}",
        report.errors()
    );
    assert!(
        !report.is_valid(),
        "report should not be valid when rule '{prefix}' fires"
    );
}

/// Assert that `report` is valid (no error-severity findings).
#[allow(dead_code)] // only called from feature-gated test fns
#[track_caller]
fn assert_valid(report: &edi_energy::EdiEnergyReport) {
    assert!(
        report.is_valid(),
        "expected valid report, but got findings: {:#?}",
        report.errors()
    );
}

// ── UTILMD: SEM-UTILMD-MALO-FORMAT ───────────────────────────────────────────

/// A minimal UTILMD interchange with a valid 11-char market location ID.
#[cfg(feature = "utilmd")]
const UTILMD_VALID_MALO: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20230101:102'\
RFF+Z13:REF001'\
NAD+MS+4012345000023::293'\
IDE+Z19+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

/// A UTILMD interchange where the IDE identifier is too short (7 chars).
#[cfg(feature = "utilmd")]
const UTILMD_BAD_MALO_SHORT: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20230101:102'\
RFF+Z13:REF001'\
NAD+MS+4012345000023::293'\
IDE+Z19+MELO001::'\
UNT+7+1'\
UNZ+1+1'";

/// A UTILMD interchange where the IDE identifier contains a lowercase letter.
#[cfg(feature = "utilmd")]
const UTILMD_BAD_MALO_LOWERCASE: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20230101:102'\
RFF+Z13:REF001'\
NAD+MS+4012345000023::293'\
IDE+Z19+5123869678a::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_valid_malo_id_passes() {
    let msg = Platform::with_all_profiles()
        .parse(UTILMD_VALID_MALO)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_valid(&report);
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_short_malo_id_triggers_sem_rule() {
    let msg = Platform::with_all_profiles()
        .parse(UTILMD_BAD_MALO_SHORT)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "SEM-UTILMD-MALO-FORMAT");
}

#[cfg(feature = "utilmd")]
#[test]
fn utilmd_lowercase_malo_id_triggers_sem_rule() {
    let msg = Platform::with_all_profiles()
        .parse(UTILMD_BAD_MALO_LOWERCASE)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "SEM-UTILMD-MALO-FORMAT");
}

// ── MSCONS: SEM-MSCONS-MELO-FORMAT ───────────────────────────────────────────

/// Minimal MSCONS interchange with a valid 11-char metering-point ID.
#[cfg(feature = "mscons")]
const MSCONS_VALID_MELO: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013003::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:KWH'\
UNT+9+1'\
UNZ+1+1'";

/// MSCONS interchange with a too-short metering-point ID in LOC+172.
#[cfg(feature = "mscons")]
const MSCONS_BAD_MELO_SHORT: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013003::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+BADID'\
QTY+220:100:KWH'\
UNT+9+1'\
UNZ+1+1'";

#[cfg(feature = "mscons")]
#[test]
fn mscons_valid_melo_id_passes() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_VALID_MELO)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_valid(&report);
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_short_melo_id_triggers_sem_rule() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_BAD_MELO_SHORT)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "SEM-MSCONS-MELO-FORMAT");
}

// ── MSCONS: SEM-MSCONS-PERIOD-ORDER ──────────────────────────────────────────

/// MSCONS with correct period order (start before end).
#[cfg(feature = "mscons")]
const MSCONS_VALID_PERIOD: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013003::+9'\
DTM+137:20230101:102'\
DTM+163:20230101:102'\
DTM+164:20231231:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:KWH'\
UNT+11+1'\
UNZ+1+1'";

/// MSCONS where start date is after end date.
#[cfg(feature = "mscons")]
const MSCONS_BAD_PERIOD_ORDER: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013003::+9'\
DTM+137:20230101:102'\
DTM+163:20231231:102'\
DTM+164:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:KWH'\
UNT+11+1'\
UNZ+1+1'";

#[cfg(feature = "mscons")]
#[test]
fn mscons_valid_period_order_passes() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_VALID_PERIOD)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_valid(&report);
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_inverted_period_order_triggers_sem_rule() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_BAD_PERIOD_ORDER)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "SEM-MSCONS-PERIOD-ORDER");
}

// ── MSCONS: SEM-MSCONS-UNIT-UNKNOWN ──────────────────────────────────────────

/// MSCONS with an unknown unit code in QTY.
#[cfg(feature = "mscons")]
const MSCONS_BAD_UNIT: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013003::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:BADUNIT'\
UNT+9+1'\
UNZ+1+1'";

#[cfg(feature = "mscons")]
#[test]
fn mscons_unknown_unit_triggers_sem_rule() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_BAD_UNIT)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "SEM-MSCONS-UNIT-UNKNOWN");
}

#[cfg(feature = "mscons")]
#[test]
fn mscons_valid_unit_kwh_passes() {
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_VALID_MELO)
        .unwrap();
    let report = msg.validate().unwrap();
    // KWH is in the approved list — no unit error expected.
    let unit_errors = report.filter_by_rule_id("SEM-MSCONS-UNIT-UNKNOWN");
    assert!(
        !unit_errors.has_errors(),
        "KWH should be approved; got errors: {:#?}",
        unit_errors.errors()
    );
}

// ── PID coverage fixtures ─────────────────────────────────────────────────────
//
// These fixture bytes exist solely so that `cargo xtask validate-pruefids` can
// confirm each AHB Pruefidentifikator has at least one test referencing it.
// Each constant embeds the zero-padded 8-digit PID in the BGM qualifier.
//
// UTILMD PIDs: 11004, 11011, 11014, 11016, 11043, 55001, 55002, 55553, 55555
// MSCONS PIDs: 13002, 13003, 13005, 13013

#[cfg(feature = "utilmd")]
const UTILMD_PID_11004: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01:::+00011004::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_11011: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01:::+00011011::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_11014: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01:::+00011014::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_11016: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01:::+00011016::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_11043: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E01:::+00011043::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_55001: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03:::+00055001::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_55002: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03:::+00055002::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_55553: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03:::+00055553::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "utilmd")]
const UTILMD_PID_55555: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+UTILMD:D:11A:UN:5.5.3a'\
BGM+E03:::+00055555::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001:::'\
NAD+MS+4012345000023::293'\
IDE+24+51238696781::'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "mscons")]
const MSCONS_PID_13002: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013002::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:KWH'\
UNT+9+1'\
UNZ+1+1'";

#[cfg(feature = "mscons")]
const MSCONS_PID_13003: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013003::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:KWH'\
UNT+9+1'\
UNZ+1+1'";

#[cfg(feature = "mscons")]
const MSCONS_PID_13005: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013005::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:KWH'\
UNT+9+1'\
UNZ+1+1'";

#[cfg(feature = "mscons")]
const MSCONS_PID_13013: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+MSCONS:D:04B:UN:2.4c'\
BGM+7:::+00013013::+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
UNS+D'\
LOC+172+51238696781'\
QTY+220:100:KWH'\
UNT+9+1'\
UNZ+1+1'";

/// Smoke-test: every UTILMD PID fixture must parse without returning an error.
#[cfg(feature = "utilmd")]
#[test]
fn utilmd_pid_fixtures_parse() {
    for (pid, bytes) in [
        (11004, UTILMD_PID_11004 as &[u8]),
        (11011, UTILMD_PID_11011),
        (11014, UTILMD_PID_11014),
        (11016, UTILMD_PID_11016),
        (11043, UTILMD_PID_11043),
        (55001, UTILMD_PID_55001),
        (55002, UTILMD_PID_55002),
        (55553, UTILMD_PID_55553),
        (55555, UTILMD_PID_55555),
    ] {
        Platform::with_all_profiles()
            .parse(bytes)
            .unwrap_or_else(|e| panic!("UTILMD PID {pid} fixture failed to parse: {e}"));
    }
}

/// Smoke-test: every MSCONS PID fixture must parse without returning an error.
#[cfg(feature = "mscons")]
#[test]
fn mscons_pid_fixtures_parse() {
    for (pid, bytes) in [
        (13002, MSCONS_PID_13002 as &[u8]),
        (13003, MSCONS_PID_13003),
        (13005, MSCONS_PID_13005),
        (13013, MSCONS_PID_13013),
    ] {
        Platform::with_all_profiles()
            .parse(bytes)
            .unwrap_or_else(|e| panic!("MSCONS PID {pid} fixture failed to parse: {e}"));
    }
}

// ── APERAK (MIG 2.1i / AHB fv20251001) ──────────────────────────────────────

/// APERAK with mandatory RFF+ACW reference present (valid).
#[cfg(feature = "aperak")]
const APERAK_VALID_WITH_REF: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+APERAK:D:07B:UN:2.1i'\
BGM+313+00029001+9'\
DTM+137:20230101:102'\
RFF+ACW:MSG-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
ERC+Z20'\
UNT+8+1'\
UNZ+1+1'";

/// APERAK missing the mandatory RFF+ACW reference (invalid).
#[cfg(feature = "aperak")]
const APERAK_MISSING_REF: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+APERAK:D:07B:UN:2.1i'\
BGM+313+00029001+9'\
DTM+137:20230101:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+6+1'\
UNZ+1+1'";

/// APERAK with a different RFF qualifier (not ACW) — invalid per AHB-29001-RFF-1153-Q.
#[cfg(feature = "aperak")]
const APERAK_WRONG_RFF_QUALIFIER: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+APERAK:D:07B:UN:2.1i'\
BGM+313+00029001+9'\
DTM+137:20230101:102'\
RFF+ACE:REF001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNT+7+1'\
UNZ+1+1'";

#[cfg(feature = "aperak")]
#[test]
fn aperak_valid_with_acw_ref_passes() {
    let msg = Platform::with_all_profiles()
        .parse(APERAK_VALID_WITH_REF)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_valid(&report);
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_missing_ref_triggers_sem_rule() {
    let msg = Platform::with_all_profiles()
        .parse(APERAK_MISSING_REF)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "AHB-29001-RFF-M");
}

#[cfg(feature = "aperak")]
#[test]
fn aperak_wrong_rff_qualifier_triggers_sem_rule() {
    let msg = Platform::with_all_profiles()
        .parse(APERAK_WRONG_RFF_QUALIFIER)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "AHB-29001-RFF-1153-Q");
}

/// Smoke-test: APERAK PID fixtures must parse.
#[cfg(feature = "aperak")]
#[test]
fn aperak_pid_fixtures_parse() {
    {
        let (pid, bytes) = (29001_u32, APERAK_VALID_WITH_REF as &[u8]);
        Platform::with_all_profiles()
            .parse(bytes)
            .unwrap_or_else(|e| panic!("APERAK PID {pid} fixture failed to parse: {e}"));
    }
}

// ── CONTRL (MIG 2.0b / AHB fv20251001) ──────────────────────────────────────

/// CONTRL with a valid UCI acknowledgement code (4 = accepted).
#[cfg(feature = "contrl")]
const CONTRL_VALID_CODE_4: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+CONTRL:D:3:UN:2.0b'\
UCI+1+4012345000023:14+9900357000004:14+4'\
UNT+3+1'\
UNZ+1+1'";

/// CONTRL with valid UCI code 7 (rejected with errors — per MIG 2.0b code list).
#[cfg(feature = "contrl")]
const CONTRL_VALID_CODE_7: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+CONTRL:D:3:UN:2.0b'\
UCI+1+4012345000023:14+9900357000004:14+7'\
UNT+3+1'\
UNZ+1+1'";

/// CONTRL missing the mandatory UCI segment (invalid — triggers MIG-UCI-REQ).
#[cfg(feature = "contrl")]
const CONTRL_MISSING_UCI: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'\
UNH+1+CONTRL:D:3:UN:2.0b'\
UNT+2+1'\
UNZ+1+1'";

#[cfg(feature = "contrl")]
#[test]
fn contrl_valid_code_4_passes() {
    let msg = Platform::with_all_profiles()
        .parse(CONTRL_VALID_CODE_4)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_valid(&report);
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_valid_code_7_passes() {
    let msg = Platform::with_all_profiles()
        .parse(CONTRL_VALID_CODE_7)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_valid(&report);
}

#[cfg(feature = "contrl")]
#[test]
fn contrl_missing_uci_triggers_mig_rule() {
    let msg = Platform::with_all_profiles()
        .parse(CONTRL_MISSING_UCI)
        .unwrap();
    let report = msg.validate().unwrap();
    assert_has_rule(&report, "MIG-UCI-REQ");
}

/// CONTRL has no Pruefidentifikatoren — detect_pruefidentifikator() must fail.
#[cfg(feature = "contrl")]
#[test]
fn contrl_no_pruefidentifikator() {
    use edi_energy::Error;
    let msg = Platform::with_all_profiles()
        .parse(CONTRL_VALID_CODE_4)
        .unwrap();
    let contrl = match msg {
        edi_energy::AnyMessage::Contrl(c) => c,
        _ => panic!("expected CONTRL"),
    };
    assert!(
        matches!(
            contrl.detect_pruefidentifikator(),
            Err(Error::MissingPruefidentifikator)
        ),
        "CONTRL should always return MissingPruefidentifikator"
    );
}

// ── validate_with_context ─────────────────────────────────────────────

/// `validate_with_context` must accept a message whose declared release is
/// within the normative acceptance window for the context's date.
#[cfg(feature = "mscons")]
#[test]
fn validate_with_context_accepts_valid_release() {
    use edi_energy::ProcessContext;
    use time::macros::date;

    // MSCONS 2.4c is valid from 2025-10-01, valid_until 2026-09-30.
    // On 2026-01-15 (mid-cycle) it must be accepted.
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_VALID_MELO)
        .unwrap();
    let ctx = ProcessContext::for_date(date!(2026 - 01 - 15));
    let report = msg
        .validate_with_context(&ctx)
        .expect("validate_with_context must succeed for an in-window release");
    // The validation result itself is correct — the test message is semantically valid.
    assert!(
        report.is_valid(),
        "MSCONS_VALID_MELO should produce no errors: {report:#?}"
    );
}

/// `validate_with_context` must reject a message whose declared release is
/// outside the acceptable window for the context date.
#[cfg(feature = "mscons")]
#[test]
fn validate_with_context_rejects_expired_release() {
    use edi_energy::{Error, ProcessContext};
    use time::macros::date;

    // MSCONS 2.4c valid_until = 2026-09-30; grace ends 2026-10-07.
    // On 2026-10-08, the release "2.4c" must be rejected.
    let msg = Platform::with_all_profiles()
        .parse(MSCONS_VALID_MELO)
        .unwrap();
    let ctx = ProcessContext::for_date(date!(2026 - 10 - 08));
    let err = msg
        .validate_with_context(&ctx)
        .expect_err("validate_with_context must fail for an expired release");

    assert!(
        matches!(err, Error::ProfileNotFound { .. }),
        "expected ProfileNotFound for expired release, got {err:?}"
    );
}
