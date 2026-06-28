//! AHB-conformant E2E tests.
//!
//! **These tests do NOT bypass AHB validation.**
//!
//! Each test parses a known-valid EDIFACT fixture from
//! `crates/edi-energy/tests/fixtures/utilmd/valid/`, runs full AHB profile
//! validation via [`edi_energy::EdiEnergyMessage::validate_on_date`], and
//! asserts `report.is_valid()` before constructing any workflow command.
//!
//! This is in contrast to `e2e_lieferbeginn.rs` and similar tests which set
//! `validation_passed: true` unconditionally — a bypass pattern that provides
//! no protection against profile-rule regressions.
//!
//! # Covered process families
//!
//! | PID | Process | AHB release | Fixture |
//! |---|---|---|---|
//! | 55001 | GPKE Lieferbeginn (LFN → NB) | S2.2 / FV2026-10-01 | `beispiel_55001_lieferbeginn.edi` |
//! | 55002 | GPKE Lieferende (LFN → NB) | S2.2 / FV2026-10-01 | `beispiel_55002_lieferende.edi` |
//!
//! # Gap note
//!
//! WiM Gas PIDs (44022–44053, 44168–44170) and GeLi Gas PIDs do not yet have
//! AHB profiles in the `fv*_gas` profile set. Once `cargo xtask import-xml-ahb`
//! imports them, add coverage here.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    version::{FormatVersion, WorkflowId},
};
use mako_gpke::{GpkeSupplierChangeWorkflow, SupplierChangeCommand};
use makod::adapters::gpke_registry;
use time::macros::date;

// ── Constants ─────────────────────────────────────────────────────────────────

/// GLN of the Lieferant — as in the fixture (sender NAD+MS).
const LFN_ID: &str = "9907317000007";
/// GLN of the Netzbetreiber — as in the fixture (receiver NAD+MR).
const NB_ID: &str = "4012345000023";
/// Marktlokation — from IDE+Z19 in the fixture.
const MALO_ID: &str = "51238696781";
/// BDEW FV matching the fixture's S2.2 release.
const FV_2026: &str = "FV2026-10-01";

/// Date on which S2.2 is the valid release for UTILMD (first day of FV2026-10-01).
const VALIDATION_DATE: time::Date = date!(2026 - 10 - 01);

// ── Fixture bytes (loaded from test fixtures at compile time) ─────────────────

/// AHB-conformant UTILMD S2.2 PID 55001 (Lieferbeginn Anfrage, LFN → NB).
///
/// Source: `crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_55001_lieferbeginn.edi`
/// Release: S2.2 (BDEW UTILMD AHB S2.2, FV2026-10-01)
const UTILMD_55001_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_55001_lieferbeginn.edi"
);

/// AHB-conformant UTILMD S2.2 PID 55002 (Lieferende Anfrage, LFN → NB).
///
/// Source: `crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_55002_lieferende.edi`
/// Release: S2.2 (BDEW UTILMD AHB S2.2, FV2026-10-01)
const UTILMD_55002_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_55002_lieferende.edi"
);

// ── Helper: assert AHB passes ─────────────────────────────────────────────────

/// Parse `wire` bytes and assert that AHB validation passes on `reference_date`.
///
/// Returns `(message, report)` for further use.
///
/// # Panics
///
/// Panics if parsing fails or if AHB validation reports errors.
/// This is the **no-bypass** guarantee: the test truly fails if the fixture
/// violates any profile rule.
fn parse_and_assert_ahb_valid(
    wire: &[u8],
    reference_date: time::Date,
) -> (edi_energy::AnyMessage, edi_energy::EdiEnergyReport) {
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(wire)
        .expect("fixture must parse without error");

    let report = msg
        .validate_on_date(reference_date)
        .expect("AHB validation must not error (only returns Err on internal error)");

    assert!(
        report.is_valid(),
        "AHB validation FAILED for fixture on date {reference_date}: {:?}",
        report.errors(),
    );

    (msg, report)
}

// ── Test: GPKE 55001 Lieferbeginn — real AHB validation passes ────────────────

#[tokio::test]
async fn ahb_55001_lieferbeginn_validates_and_dispatches() {
    // Step 1: Parse and assert AHB valid — no bypass!
    // validate_on_date returns is_valid()=true only if all profile rules pass.
    let (msg, report) = parse_and_assert_ahb_valid(UTILMD_55001_VALID, VALIDATION_DATE);
    // report.is_valid() is asserted inside parse_and_assert_ahb_valid; use it as the
    // authoritative validation_passed value to pass to the command.

    // Step 2: Verify key fields extracted from the AHB-conformant fixture.
    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 55001, "fixture must encode PID 55001");

    let release = msg.detect_release().expect("release must be detectable");
    assert_eq!(release.as_str(), "S2.2", "fixture must encode S2.2 release");

    // Step 3: Adapt to domain command to extract fields; the adapter uses today's date
    // for validate() internally so validation_passed may be false for future-dated fixtures.
    // We override it with the authoritative report.is_valid() from validate_on_date().
    let fv = FormatVersion::new(FV_2026);
    let adapter_cmd = gpke_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .expect("gpke_registry must adapt PID 55001 UTILMD to SupplierChangeCommand");

    // Step 4: Assert command fields match fixture content.
    let SupplierChangeCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        location_id: cmd_location,
        document_date: cmd_doc_date,
        message_ref: cmd_ref,
        ..
    } = adapter_cmd
    else {
        panic!("expected SupplierChangeCommand::ReceiveUtilmd");
    };

    assert_eq!(cmd_pid.as_u32(), 55001);
    assert_eq!(
        cmd_sender.as_str(),
        LFN_ID,
        "sender GLN must match NAD+MS in fixture"
    );
    assert_eq!(
        cmd_receiver.as_str(),
        NB_ID,
        "receiver GLN must match NAD+MR in fixture"
    );
    assert_eq!(
        cmd_location.as_str(),
        MALO_ID,
        "MaLo must match IDE+Z19 in fixture"
    );
    assert_eq!(
        cmd_ref.as_str(),
        "00001",
        "message_ref must match UNH ref in fixture"
    );

    // Step 5: Build the execute command with AHB validation result from validate_on_date.
    // This is the production-correct behaviour: validation_passed reflects a full AHB check.
    let exec_cmd = SupplierChangeCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        location_id: cmd_location,
        document_date: cmd_doc_date,
        process_date: String::new(), // not present in minimal fixture
        message_ref: cmd_ref,
        validation_passed: report.is_valid(), // ← authoritative AHB result, no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };

    // Step 6: Execute the command against an in-memory NB process.
    let nb_process: Process<GpkeSupplierChangeWorkflow, InMemoryEventStore> = Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(NB_ID),
        WorkflowId::new("gpke-supplier-change", FV_2026),
    );

    nb_process
        .execute(exec_cmd)
        .await
        .expect("NB process must accept AHB-validated 55001 without error");

    let state: mako_gpke::SupplierChangeState = nb_process
        .state()
        .await
        .expect("must be able to load state");

    assert!(
        matches!(state, mako_gpke::SupplierChangeState::ValidationPassed(_)),
        "after AHB-validated 55001 ReceiveUtilmd, state must be ValidationPassed; got: {state:?}",
    );
}

// ── Test: GPKE 55002 Lieferende — real AHB validation passes ──────────────────

#[tokio::test]
async fn ahb_55002_lieferende_validates_and_dispatches() {
    // Step 1: Parse and assert AHB valid — no bypass!
    let (msg, report) = parse_and_assert_ahb_valid(UTILMD_55002_VALID, VALIDATION_DATE);

    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 55002, "fixture must encode PID 55002");

    // Step 2: Adapt to extract fields.
    let fv = FormatVersion::new(FV_2026);
    let adapter_cmd = gpke_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .expect("gpke_registry must adapt PID 55002 UTILMD to SupplierChangeCommand");

    let SupplierChangeCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        location_id: cmd_location,
        document_date: cmd_doc_date,
        message_ref: cmd_ref,
        ..
    } = adapter_cmd
    else {
        panic!("expected SupplierChangeCommand::ReceiveUtilmd");
    };

    assert_eq!(cmd_pid.as_u32(), 55002);

    // Step 3: Build execute command with authoritative AHB result.
    let exec_cmd = SupplierChangeCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        location_id: cmd_location,
        document_date: cmd_doc_date,
        process_date: String::new(),
        message_ref: cmd_ref,
        validation_passed: report.is_valid(), // ← authoritative AHB result, no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };

    let nb_process: Process<GpkeSupplierChangeWorkflow, InMemoryEventStore> = Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(NB_ID),
        WorkflowId::new("gpke-supplier-change", FV_2026),
    );

    nb_process
        .execute(exec_cmd)
        .await
        .expect("NB process must accept AHB-validated 55002 without error");

    let state: mako_gpke::SupplierChangeState = nb_process
        .state()
        .await
        .expect("must be able to load state");

    assert!(
        matches!(state, mako_gpke::SupplierChangeState::ValidationPassed(_)),
        "after AHB-validated 55002 ReceiveUtilmd, state must be ValidationPassed; got: {state:?}",
    );
}

// ── Test: AHB-invalid message is correctly rejected ───────────────────────────

#[test]
fn ahb_invalid_utilmd_fails_validation() {
    // A minimal UTILMD that is syntactically valid but violates profile rules
    // (missing required segments like DTM, RFF, etc. per AHB S2.1).
    const INVALID_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
UNT+3+1'\
UNZ+1+1'";

    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(INVALID_BYTES)
        .expect("syntactically valid EDIFACT must parse");

    let report = msg
        .validate_on_date(date!(2025 - 10 - 01))
        .expect("validate_on_date must not error");

    assert!(
        !report.is_valid(),
        "AHB-invalid message must fail validation; got: {:?} errors",
        report.errors().len(),
    );
    assert!(
        !report.errors().is_empty(),
        "AHB validation errors must be non-empty for invalid fixture",
    );
}

// ── Test: validate_on_date returns valid=true for S2.2 fixture on FV2026-10-01 ─

#[test]
fn ahb_s2_2_is_valid_on_fv2026_10_01() {
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(UTILMD_55001_VALID)
        .expect("55001 fixture must parse");

    // Must be valid on 2026-10-01 (first day S2.2 is current).
    let report_valid = msg
        .validate_on_date(date!(2026 - 10 - 01))
        .expect("validate_on_date must not error");
    assert!(
        report_valid.is_valid(),
        "S2.2 fixture must validate on 2026-10-01; errors: {:?}",
        report_valid.errors(),
    );

    // Must still be valid during grace window (e.g. 2026-10-07 = valid_until + 7d).
    let report_grace = msg
        .validate_on_date(date!(2026 - 10 - 07))
        .expect("validate_on_date must not error");
    assert!(
        report_grace.is_valid(),
        "S2.2 fixture must validate within grace window on 2026-10-07",
    );
}
