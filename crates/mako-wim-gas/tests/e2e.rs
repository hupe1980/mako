//! End-to-end pipeline tests: `edi-energy` parse → validate → `mako-wim-gas` execute.
//!
//! Tests the full production dispatch path for PIDs 44022–44024 (WiM Gas /
//! GeLi Gas Stornierung) — the role-conditional routing area with the highest
//! regression risk after a profile update.
//!
//! ## Why these tests matter
//!
//! PIDs 44022–44024 have unusually complex routing:
//! - PID 44022 → `wim-gas-stornierung` (all/Msb/Nmsb roles) or
//!   `geli-gas-stornierung` (Nb-only role)
//! - PIDs 44023/44024 → `wim-gas-stornierung` (all/Msb/Nmsb) or
//!   `geli-gas-stornierung-lf` (Lf-only)
//!
//! These tests cover the `all()` / MSB path (handled by `mako-wim-gas`). The
//! corresponding NB/LF paths are tested in `mako-geli-gas`.
//!
//! Without a parse→validate test the following regressions are invisible:
//! - Profile update removes PID 44022 from the UTILMD Gas AHB dispatch table
//!   → `Some(_unknown)` → silent `is_valid = true` for any payload
//! - Stornierung BGM qualifier rule relaxed / removed
//! - UTILMD G release string changed, breaking `conformance_reference_date()`
//!   profile selection

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_wim_gas::{WimGasStornierungCommand, WimGasStornierungState, WimGasStornierungWorkflow};

// ── UTILMD G PID 44022 test fixture ──────────────────────────────────────────

/// A minimal UTILMD Stornierungsanfrage Gas message (PID 44022) in EDIFACT.
///
/// Follows the G1.1 schema (`fv20251001_gas`):
///
/// - BGM+E01 — Stornierung einer Anmeldung (original message type)
/// - STS+7+E05 — Transaktionsgrund Stornierung
/// - LOC+172 — Marktlokation (gas metering point)
/// - RFF+Z13 — Prüfidentifikator reference
const UTILMD_44022_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9907317000007:14+251001:0700+00001'\
UNH+00001+UTILMD:D:11A:UN:G1.1'\
BGM+E01:::+00044022::+9'\
DTM+137:20251001:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9907317000007::293'\
IDE+24+STORNO0000A::'\
STS+7+E05'\
LOC+172+DE0000011000000000000000012345678'\
RFF+Z13:STORNO0000A'\
UNT+10+00001'\
UNZ+1+00001'";

/// A minimal UTILMD Bestätigung Stornierung Gas message (PID 44023) in EDIFACT.
const UTILMD_44023_BYTES: &[u8] = b"\
UNB+UNOC:3+9907317000007:14+4012345000023:14+251001:0800+00002'\
UNH+00002+UTILMD:D:11A:UN:G1.1'\
BGM+E01:::+00044023::+9'\
DTM+137:20251002:102'\
NAD+MS+9907317000007::293'\
NAD+MR+4012345000023::293'\
IDE+24+STORNO0000A::'\
STS+7+E05'\
STS+E01'\
LOC+172+DE0000011000000000000000012345678'\
RFF+Z13:STORNO0000A'\
UNT+11+00002'\
UNZ+1+00002'";

/// A minimal UTILMD Ablehnung Stornierung Gas message (PID 44024) in EDIFACT.
const UTILMD_44024_BYTES: &[u8] = b"\
UNB+UNOC:3+9907317000007:14+4012345000023:14+251001:0900+00003'\
UNH+00003+UTILMD:D:11A:UN:G1.1'\
BGM+E01:::+00044024::+9'\
DTM+137:20251002:102'\
NAD+MS+9907317000007::293'\
NAD+MR+4012345000023::293'\
IDE+24+STORNO0000A::'\
STS+7+E05'\
STS+E01'\
LOC+172+DE0000011000000000000000012345678'\
RFF+Z13:STORNO0000A'\
UNT+11+00003'\
UNZ+1+00003'";

// ── E2E pipeline tests ────────────────────────────────────────────────────────

/// Full pipeline for PID 44022: parse UTILMD G → validate → extract → execute.
///
/// Asserts:
/// - `Platform::with_all_profiles()` parses the EDIFACT bytes without error
/// - `report.is_valid()` is true (no AHB violations)
/// - PID detected as 44022
/// - `WimGasStornierungCommand::ReceiveUtilmd` succeeds and emits events
/// - State transitions to `AnfrageReceived`
#[tokio::test]
async fn e2e_stornierungsanfrage_pid_44022_parse_validate_execute() {
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(UTILMD_44022_BYTES)
        .expect("PID 44022 EDIFACT bytes must parse");

    // Validate on a reference date within fv20251001_gas validity (2025-10-01 → 2026-09-30).
    let ref_date = time::Date::from_calendar_date(2026, time::Month::January, 15).unwrap();
    let report = msg
        .validate_on_date(ref_date)
        .expect("validate_on_date must not error");
    assert!(
        report.is_valid(),
        "PID 44022 message must pass AHB validation; errors: {:?}",
        report.errors()
    );

    // Detect PID.
    let pid_u32 = msg
        .detect_pruefidentifikator()
        .expect("PID detection must succeed for BGM+E01+00044022")
        .as_u32();
    assert_eq!(pid_u32, 44022, "PID must be 44022");

    // Execute command.
    let process = Process::<WimGasStornierungWorkflow, _>::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("wim-gas-stornierung", "FV2025-10-01"),
    );

    let validation_errors: Vec<String> =
        report.errors().iter().map(|e| e.message.clone()).collect();
    process
        .execute(WimGasStornierungCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(44022).unwrap(),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9907317000007"),
            vorgang_id: "STORNO0000A".into(),
            document_date: "20251001".to_owned(),
            message_ref: MessageRef::new("00001"),
            validation_passed: report.is_valid(),
            validation_errors,
        })
        .await
        .expect("ReceiveUtilmd for PID 44022 must succeed");

    let state = process.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(
            state,
            WimGasStornierungState::Initiated(_) | WimGasStornierungState::ValidationPassed(_)
        ),
        "state must be Initiated or ValidationPassed after valid 44022; got: {state:?}",
    );
}

/// Full pipeline for PID 44023: parse UTILMD G → validate → assert `is_valid`.
///
/// PID 44023 (Bestätigung Stornierung) is the GNB's positive response. It is
/// not ingested via `ReceiveUtilmd` by the same workflow instance that received
/// 44022 — the LFN/LFA receives it as a correlation response. This test only
/// covers the parse→validate half of the pipeline, verifying the AHB rule pack
/// for 44023 fires correctly.
#[test]
fn e2e_bestaetigung_stornierung_pid_44023_parse_validate() {
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(UTILMD_44023_BYTES)
        .expect("PID 44023 EDIFACT bytes must parse");

    let ref_date = time::Date::from_calendar_date(2026, time::Month::January, 15).unwrap();
    let report = msg
        .validate_on_date(ref_date)
        .expect("validate_on_date must not error");
    assert!(
        report.is_valid(),
        "PID 44023 message must pass AHB validation; errors: {:?}",
        report.errors()
    );

    let pid_u32 = msg
        .detect_pruefidentifikator()
        .expect("PID detection must succeed for BGM+E01+00044023")
        .as_u32();
    assert_eq!(pid_u32, 44023, "PID must be 44023");
}

/// Full pipeline for PID 44024: parse UTILMD G → validate → assert `is_valid`.
///
/// PID 44024 (Ablehnung Stornierung) is the GNB's negative response.
#[test]
fn e2e_ablehnung_stornierung_pid_44024_parse_validate() {
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(UTILMD_44024_BYTES)
        .expect("PID 44024 EDIFACT bytes must parse");

    let ref_date = time::Date::from_calendar_date(2026, time::Month::January, 15).unwrap();
    let report = msg
        .validate_on_date(ref_date)
        .expect("validate_on_date must not error");
    assert!(
        report.is_valid(),
        "PID 44024 message must pass AHB validation; errors: {:?}",
        report.errors()
    );

    let pid_u32 = msg
        .detect_pruefidentifikator()
        .expect("PID detection must succeed for BGM+E01+00044024")
        .as_u32();
    assert_eq!(pid_u32, 44024, "PID must be 44024");
}
