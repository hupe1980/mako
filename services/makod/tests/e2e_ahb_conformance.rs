//! AHB-conformant E2E tests.
//!
//! **These tests do NOT bypass AHB validation.**
//!
//! Each test parses a known-valid EDIFACT fixture from
//! `crates/edi-energy/tests/fixtures/*/valid/`, runs full AHB profile
//! validation via [`edi_energy::EdiEnergyMessage::validate_on_date`], and
//! asserts `report.is_valid()` before constructing any workflow command.
//!
//! This is in contrast to `e2e_lieferbeginn.rs` and similar tests which set
//! `validation_passed: true` unconditionally — a bypass pattern that provides
//! no protection against profile-rule regressions.
//!
//! # Covered process families
//!
//! ## Full dispatch tests (parse → validate_on_date → adapt → execute → assert state)
//!
//! | PID | Process | AHB release | Fixture |
//! |---|---|---|---|
//! | 55001 | GPKE Lieferbeginn (LFN → NB) | S2.2 / FV2026-10-01 | `beispiel_55001_lieferbeginn.edi` |
//! | 55002 | GPKE Lieferende (LFN → NB) | S2.2 / FV2026-10-01 | `beispiel_55002_lieferende.edi` |
//! | 44001 | GeLi Gas Lieferbeginn (nLFN → GNB) | G1.1 / FV2025-10-01 | `beispiel_44001_lieferbeginn_gas.edi` |
//! | 44022 | GeLi Gas Stornierung Anfrage (LFN → GNB) | G1.1 / FV2025-10-01 | `beispiel_44022_stornierung_gas.edi` |
//! | 44039 | WiM Gas Kündigung MSB Gas (MSBA → NB) | G1.1 / FV2025-10-01 | `beispiel_44039_kuendigung_msb_gas.edi` |
//! | 44042 | WiM Gas Anmeldung MSB Gas (MSBA → NB) | G1.1 / FV2025-10-01 | `beispiel_44042_anmeldung_msb_gas.edi` |
//! | 44168 | WiM Gas Verpflichtungsanfrage (NB → gMSB) | G1.1 / FV2025-10-01 | `beispiel_44168_verpflichtungsanfrage.edi` |
//! | 31001 | GPKE INVOIC Abschlagsrechnung (MSB → NB) | 2.8e / FV2025-10-01 | `pid_31001.edi` |
//!
//! ## Validation-only tests (parse → validate_on_date → assert is_valid)
//!
//! Response PIDs and other PIDs where the workflow receives these as outbound
//! (not inbound ANFRAGE), or where no dispatch adapter sends them to a fresh process:
//!
//! | PID | Process | AHB release | Fixture |
//! |---|---|---|---|
//! | 44023 | GeLi Gas Stornierung Bestätigung | G1.1 / FV2025-10-01 | `beispiel_44023_bestaetigung_stornierung_gas.edi` |
//! | 44024 | GeLi Gas Stornierung Ablehnung | G1.1 / FV2025-10-01 | `beispiel_44024_ablehnung_stornierung_gas.edi` |
//! | 44040 | WiM Gas Kündigung Bestätigung | G1.1 / FV2025-10-01 | `beispiel_44040_bestaetigung_kuendigung_msb_gas.edi` |
//! | 44041 | WiM Gas Kündigung Ablehnung | G1.1 / FV2025-10-01 | `beispiel_44041_ablehnung_kuendigung_msb_gas.edi` |
//! | 44043 | WiM Gas Anmeldung Bestätigung | G1.1 / FV2025-10-01 | `beispiel_44043_bestaetigung_anmeldung_msb_gas.edi` |
//! | 44044 | WiM Gas Anmeldung Ablehnung | G1.1 / FV2025-10-01 | `beispiel_44044_ablehnung_anmeldung_msb_gas.edi` |
//! | 44051 | WiM Gas Ende MSB / Vorl. Abmeldung (NB → MSBA) | G1.1 / FV2025-10-01 | `beispiel_44051_ende_msb_gas.edi` |
//! | 44052 | WiM Gas Ende Bestätigung | G1.1 / FV2025-10-01 | `beispiel_44052_bestaetigung_ende_msb_gas.edi` |
//! | 44053 | WiM Gas Ende Ablehnung | G1.1 / FV2025-10-01 | `beispiel_44053_ablehnung_ende_msb_gas.edi` |
//! | 44169 | Verpflichtungsanfrage Bestätigung | G1.1 / FV2025-10-01 | `beispiel_44169_bestaetigung_verpflichtungsanfrage.edi` |
//! | 44170 | Verpflichtungsanfrage Ablehnung | G1.1 / FV2025-10-01 | `beispiel_44170_ablehnung_verpflichtungsanfrage.edi` |
//!
//! # Gap note
//!
//! WiM Strom (11001–11003) and MABIS (13003) AHB profiles are absent — blocked
//! by data availability (BDEW XML subscription required). See FINDINGS.md F-004.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    version::{FormatVersion, WorkflowId},
};
use mako_geli_gas::{
    GasSupplierChangeCommand, GeliGasStornierungCommand, GeliGasStornierungState,
    GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow,
};
use mako_gpke::{
    AbrechnungCommand, AbrechnungState, GpkeAbrechnungWorkflow, GpkeSupplierChangeWorkflow,
    SupplierChangeCommand,
};
use mako_wim_gas::{
    WimGasAnmeldungCommand, WimGasAnmeldungState, WimGasAnmeldungWorkflow, WimGasKuendigungCommand,
    WimGasKuendigungState, WimGasKuendigungWorkflow, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageState, WimGasVerpflichtungsanfrageWorkflow,
};
use makod::adapters::{
    geli_gas_registry, geli_gas_stornierung_registry, gpke_abrechnung_registry, gpke_registry,
    wim_gas_anmeldung_registry, wim_gas_kuendigung_registry,
    wim_gas_verpflichtungsanfrage_registry,
};
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

// ── GeLi Gas fixture constants ────────────────────────────────────────────────

/// GLN of the netzunabhängiger Lieferant (nLFN) — NAD+MS in the Gas fixture.
const GAS_NLFN_ID: &str = "4012345000023";
/// GLN of the Gasnetzbetreiber (GNB) — NAD+MR in the Gas fixture.
const GAS_GNB_ID: &str = "9907317000007";
/// Marktlokation from IDE+Z19 in the Gas fixture.
const GAS_MALO_ID: &str = "51238696781";
/// BDEW FV matching the Gas fixture's G1.1 release.
const GAS_FV_2025: &str = "FV2025-10-01";

// ── WiM Gas fixture constants ─────────────────────────────────────────────────

/// GLN of the MSBA (MSB Auftraggeber / Initiator) — NAD+MS in the WiM Gas fixture.
const WIM_GAS_MSBA_ID: &str = "4012345000023";
/// GLN of the NB (Netzbetreiber / Recipient) — NAD+MR in the WiM Gas fixture.
const WIM_GAS_NB_ID: &str = "9907317000007";
/// Vorgangsnummer (11-char [A-Z0-9]{11}) from IDE+24 in the WiM Gas Kündigung fixture.
const WIM_GAS_VORGANG_ID: &str = "WIMGAS00001";

/// Date on which G1.1 is the valid release for UTILMD Gas (first day of FV2025-10-01).
const GAS_VALIDATION_DATE: time::Date = date!(2025 - 10 - 01);

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

/// AHB-conformant UTILMD G G1.1 PID 44001 (Lieferbeginn Gas Anfrage, nLFN → GNB).
///
/// Source: `crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44001_lieferbeginn_gas.edi`
/// Release: G1.1 (BDEW UTILMD AHB Gas G1.1, FV2025-10-01, BK7-24-01-009)
const UTILMD_44001_GAS_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44001_lieferbeginn_gas.edi"
);

/// AHB-conformant UTILMD G G1.1 PID 44039 (Kündigung MSB Gas Anfrage, MSBA → NB).
///
/// Source: `crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44039_kuendigung_msb_gas.edi`
/// Release: G1.1 (BDEW UTILMD AHB Gas G1.1, FV2025-10-01, BK7-24-01-009)
/// Segment structure: BGM+E35 → DTM+137 → NAD+MS → NAD+MR → IDE+24 → STS+7 → LOC+172 → RFF+Z13
/// (RFF appears in SG6 inside SG4, AFTER IDE — not in SG1 before NAD as in GeLi Gas)
const UTILMD_44039_WIM_GAS_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44039_kuendigung_msb_gas.edi"
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

// ── Test: GeLi Gas 44001 Lieferbeginn Gas — real AHB validation passes ────────

#[tokio::test]
async fn ahb_44001_lieferbeginn_gas_validates_and_dispatches() {
    // Step 1: Parse and assert AHB valid — no bypass!
    // G1.1 UTILMD Gas profiles are present under fv20251001_gas/ahb.json.
    // PID 44001 has 11 AHB segment rules: BGM(E01), DTM(137), NAD(MS+MR),
    // IDE(Z19), RFF(Z13) — all mandatory. The fixture satisfies all of them.
    let (msg, report) = parse_and_assert_ahb_valid(UTILMD_44001_GAS_VALID, GAS_VALIDATION_DATE);

    // Step 2: Verify key fields extracted from the AHB-conformant fixture.
    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 44001, "fixture must encode PID 44001");

    let release = msg.detect_release().expect("release must be detectable");
    assert_eq!(release.as_str(), "G1.1", "fixture must encode G1.1 release");

    // Step 3: Adapt to domain command using geli_gas_registry.
    let fv = FormatVersion::new(GAS_FV_2025);
    let adapter_cmd = geli_gas_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .expect("geli_gas_registry must adapt PID 44001 UTILMD Gas to GasSupplierChangeCommand");

    // Step 4: Assert command fields match fixture content.
    let GasSupplierChangeCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        message_ref: cmd_ref,
        ..
    } = adapter_cmd
    else {
        panic!("expected GasSupplierChangeCommand::ReceiveUtilmd");
    };

    assert_eq!(cmd_pid.as_u32(), 44001);
    assert_eq!(
        cmd_sender.as_str(),
        GAS_NLFN_ID,
        "sender GLN must match NAD+MS (nLFN) in fixture"
    );
    assert_eq!(
        cmd_receiver.as_str(),
        GAS_GNB_ID,
        "receiver GLN must match NAD+MR (GNB) in fixture"
    );
    assert_eq!(
        cmd_malo.as_str(),
        GAS_MALO_ID,
        "MaLo must match IDE+Z19 in fixture"
    );
    assert_eq!(
        cmd_ref.as_str(),
        "00001",
        "message_ref must match UNH ref in fixture"
    );

    // Step 5: Build execute command with authoritative AHB result.
    let exec_cmd = GasSupplierChangeCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        document_date: String::new(),
        message_ref: cmd_ref,
        validation_passed: report.is_valid(), // ← authoritative AHB result, no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };

    // Step 6: Execute the command against an in-memory GNB process.
    // The GNB receives the nLFN's Lieferbeginn Anfrage.
    let gnb_process: Process<GeliGasSupplierChangeWorkflow, InMemoryEventStore> = Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(GAS_GNB_ID),
        WorkflowId::new("geli-gas-supplier-change", GAS_FV_2025),
    );

    gnb_process
        .execute(exec_cmd)
        .await
        .expect("GNB process must accept AHB-validated 44001 without error");

    let state: mako_geli_gas::GasSupplierChangeState = gnb_process
        .state()
        .await
        .expect("must be able to load state");

    assert!(
        matches!(
            state,
            mako_geli_gas::GasSupplierChangeState::ValidationPassed(_)
        ),
        "after AHB-validated 44001 ReceiveUtilmd, state must be ValidationPassed; got: {state:?}",
    );
}

// ── Test: WiM Gas 44039 Kündigung MSB Gas — real AHB validation passes ────────

#[tokio::test]
async fn ahb_44039_kuendigung_msb_gas_validates_and_dispatches() {
    // Step 1: Parse and assert AHB valid — no bypass!
    // G1.1 UTILMD Gas AHB profiles now include WiM Gas PIDs 44039–44053, 44168–44170
    // (added as Phase 1 profiles derived from UTILMD AHB Gas 1.1, BK7-24-01-009).
    // PID 44039 has mandatory rules: BGM(E35), DTM(137), NAD(MS+MR), IDE(24),
    // STS, LOC(172), RFF(Z13). RFF appears in SG6 (inside SG4), not SG1.
    let (msg, report) = parse_and_assert_ahb_valid(UTILMD_44039_WIM_GAS_VALID, GAS_VALIDATION_DATE);

    // Step 2: Verify key fields from the AHB-conformant fixture.
    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 44039, "fixture must encode PID 44039");

    let release = msg.detect_release().expect("release must be detectable");
    assert_eq!(release.as_str(), "G1.1", "fixture must encode G1.1 release");

    // Step 3: Adapt to domain command using wim_gas_kuendigung_registry.
    let fv = FormatVersion::new(GAS_FV_2025);
    let adapter_cmd = wim_gas_kuendigung_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .expect("wim_gas_kuendigung_registry must adapt PID 44039 UTILMD Gas to WimGasKuendigungCommand");

    // Step 4: Assert command fields match fixture content.
    let WimGasKuendigungCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        message_ref: cmd_ref,
        ..
    } = adapter_cmd
    else {
        panic!("expected WimGasKuendigungCommand::ReceiveUtilmd");
    };

    assert_eq!(cmd_pid.as_u32(), 44039);
    assert_eq!(
        cmd_sender.as_str(),
        WIM_GAS_MSBA_ID,
        "sender GLN must match NAD+MS (MSBA) in fixture"
    );
    assert_eq!(
        cmd_receiver.as_str(),
        WIM_GAS_NB_ID,
        "receiver GLN must match NAD+MR (NB) in fixture"
    );
    assert_eq!(
        cmd_malo.as_str(),
        WIM_GAS_VORGANG_ID,
        "malo_id must match IDE+24 Vorgangsnummer in fixture"
    );
    assert_eq!(
        cmd_ref.as_str(),
        "00001",
        "message_ref must match UNH ref in fixture"
    );

    // Step 5: Build execute command with authoritative AHB result (no bypass).
    let exec_cmd = WimGasKuendigungCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        document_date: String::new(),
        message_ref: cmd_ref,
        validation_passed: report.is_valid(), // ← authoritative AHB result, no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };

    // Step 6: Execute the command against an in-memory NB process.
    // The NB receives the MSBA's Kündigung MSB Gas Anfrage.
    let nb_process: Process<WimGasKuendigungWorkflow, InMemoryEventStore> = Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(WIM_GAS_NB_ID),
        WorkflowId::new("wim-gas-kuendigung", GAS_FV_2025),
    );

    nb_process
        .execute(exec_cmd)
        .await
        .expect("NB process must accept AHB-validated 44039 without error");

    let state: WimGasKuendigungState = nb_process
        .state()
        .await
        .expect("must be able to load state");

    assert!(
        matches!(state, WimGasKuendigungState::ValidationPassed(_)),
        "after AHB-validated 44039 ReceiveUtilmd, state must be ValidationPassed; got: {state:?}",
    );
}

// ── INVOIC fixture bytes ──────────────────────────────────────────────────────

/// AHB-conformant INVOIC 2.8e PID 31001 (Abschlagsrechnung, MSB → NB).
///
/// Source: `crates/edi-energy/tests/fixtures/invoic/valid/pid_31001.edi`
/// Release: 2.8e (BDEW INVOIC AHB 2.8e / FV2025-10-01, BK6-22-024)
const INVOIC_31001_VALID: &[u8] =
    include_bytes!("../../../crates/edi-energy/tests/fixtures/invoic/valid/pid_31001.edi");

/// GLN of the MSB sender in the INVOIC 31001 fixture (NAD+MS).
const INVOIC_MSB_ID: &str = "4012345000023";
/// GLN of the NB recipient in the INVOIC 31001 fixture (NAD+MR / UNB receiver).
const INVOIC_NB_ID: &str = "9900357000004";
/// BDEW FV for INVOIC 2.8e (starts 2025-10-01).
const INVOIC_FV_2025: &str = "FV2025-10-01";
/// Validation date: first day of FV2025-10-01.
const INVOIC_VALIDATION_DATE: time::Date = date!(2025 - 10 - 01);

// ── Test: GPKE 31001 INVOIC Abschlagsrechnung — real AHB validation passes ───

/// End-to-end AHB conformance test for GPKE INVOIC PID 31001 (Abschlagsrechnung).
///
/// Confirms that:
/// 1. The INVOIC 2.8e fixture parses cleanly under `Platform::with_all_profiles()`.
/// 2. `validate_on_date(2025-10-01)` returns `is_valid()=true` — no AHB bypass.
/// 3. The INVOIC fields are correctly adapted to `AbrechnungCommand::ReceiveInvoic`.
/// 4. The GpkeAbrechnungWorkflow transitions to `AbrechnungState::ValidationPassed`
///    after accepting the command, proving the full dispatch chain is functional.
#[tokio::test]
async fn ahb_31001_invoic_validates_and_dispatches() {
    // Step 1: Parse and assert AHB valid — no bypass!
    // INVOIC AHB 2.8e profile is present under fv20251001/ahb.json.
    // PID 31001 (Abschlagsrechnung) has rules for BGM, DTM, NAD(MS), LOC, CUX,
    // PYT, LIN, QTY, MOA, PRI, TAX, ALC — all mandatory.
    let (msg, report) = parse_and_assert_ahb_valid(INVOIC_31001_VALID, INVOIC_VALIDATION_DATE);

    // Step 2: Verify key fields from the AHB-conformant fixture.
    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 31001, "fixture must encode PID 31001");

    let release = msg.detect_release().expect("release must be detectable");
    assert_eq!(release.as_str(), "2.8e", "fixture must encode 2.8e release");

    // Step 3: Adapt to domain command using gpke_abrechnung_registry.
    let fv = FormatVersion::new(INVOIC_FV_2025);
    let adapter_cmd = gpke_abrechnung_registry()
        .dispatch(&msg as &dyn std::any::Any, &fv)
        .expect("gpke_abrechnung_registry must adapt PID 31001 INVOIC to AbrechnungCommand");

    // Step 4: Assert command fields match fixture content.
    let AbrechnungCommand::ReceiveInvoic {
        pid: cmd_pid,
        sender: cmd_sender,
        recipient: cmd_recipient,
        invoice_ref: cmd_ref,
        validation_passed: cmd_valid,
        ..
    } = adapter_cmd
    else {
        panic!("expected AbrechnungCommand::ReceiveInvoic");
    };

    assert_eq!(cmd_pid.as_u32(), 31001);
    assert_eq!(
        cmd_sender.as_str(),
        INVOIC_MSB_ID,
        "sender GLN must match NAD+MS in fixture"
    );
    assert_eq!(
        cmd_recipient.as_str(),
        INVOIC_NB_ID,
        "recipient GLN must match UNB receiver in fixture"
    );
    assert!(
        cmd_ref.as_str().contains("31001"),
        "invoice_ref must contain PID 31001 (from BGM+380+00031001)"
    );

    // Step 5: Build the execute command with authoritative AHB result (no bypass).
    let exec_cmd = AbrechnungCommand::ReceiveInvoic {
        pid: cmd_pid,
        sender: cmd_sender,
        recipient: cmd_recipient,
        invoice_ref: cmd_ref,
        document_date: String::from("20250101"),
        validation_passed: report.is_valid(), // authoritative AHB result — no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };
    assert!(
        cmd_valid,
        "adapter-computed validation_passed must be true for a conformant INVOIC 31001 fixture"
    );

    // Step 6: Execute the command against an in-memory NB process.
    let nb_process: Process<GpkeAbrechnungWorkflow, InMemoryEventStore> = Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(INVOIC_NB_ID),
        WorkflowId::new("gpke-abrechnung", INVOIC_FV_2025),
    );

    nb_process
        .execute(exec_cmd)
        .await
        .expect("NB process must accept AHB-validated 31001 INVOIC without error");

    let state: AbrechnungState = nb_process
        .state()
        .await
        .expect("must be able to load state");

    assert!(
        matches!(state, AbrechnungState::ValidationPassed(_)),
        "after AHB-validated 31001 ReceiveInvoic, state must be ValidationPassed; got: {state:?}",
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// GeLi Gas Stornierung (PIDs 44022–44024)
// ══════════════════════════════════════════════════════════════════════════════

// ── Fixture constants ─────────────────────────────────────────────────────────

/// AHB-conformant UTILMD G G1.1 PID 44022 (Stornierung Anfrage, LFN → GNB).
const UTILMD_44022_STORNIERUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44022_stornierung_gas.edi"
);
/// AHB-conformant UTILMD G G1.1 PID 44023 (Bestätigung Stornierung, GNB → LFN).
const UTILMD_44023_STORNIERUNG_BESTAETIGUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44023_bestaetigung_stornierung_gas.edi"
);
/// AHB-conformant UTILMD G G1.1 PID 44024 (Ablehnung Stornierung, GNB → LFN).
const UTILMD_44024_STORNIERUNG_ABLEHNUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44024_ablehnung_stornierung_gas.edi"
);

/// GLN of the LFN sender in the GeLi Gas Stornierung 44022 fixture.
const STORNO_LFN_ID: &str = "4012345000023";
/// GLN of the GNB receiver in the GeLi Gas Stornierung 44022 fixture.
const STORNO_GNB_ID: &str = "9907317000007";
/// Vorgangsnummer (IDE+24) in the GeLi Gas Stornierung 44022 fixture.
const STORNO_VORGANG_ID: &str = "STORNO0000A";

// ── Test: GeLi Gas 44022 Stornierung Anfrage — real AHB validation passes ─────

/// End-to-end AHB conformance test for GeLi Gas Stornierung Anfrage (PID 44022).
///
/// Confirms:
/// 1. UTILMD G G1.1 fixture parses and passes AHB validation via `validate_on_date`.
/// 2. Adapter extracts correct fields (sender, receiver, vorgang_id).
/// 3. `GeliGasStornierungWorkflow` transitions to `ValidationPassed` after dispatch.
#[tokio::test]
async fn ahb_44022_geli_gas_stornierung_validates_and_dispatches() {
    let (msg, report) =
        parse_and_assert_ahb_valid(UTILMD_44022_STORNIERUNG_VALID, GAS_VALIDATION_DATE);

    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 44022, "fixture must encode PID 44022");
    let release = msg.detect_release().expect("release must be detectable");
    assert_eq!(release.as_str(), "G1.1", "fixture must encode G1.1 release");

    let fv = FormatVersion::new(GAS_FV_2025);
    let adapter_cmd = geli_gas_stornierung_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .expect("geli_gas_stornierung_registry must adapt PID 44022 to GeliGasStornierungCommand");

    let GeliGasStornierungCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        vorgang_id: cmd_vorgang,
        message_ref: cmd_ref,
        ..
    } = adapter_cmd
    else {
        panic!("expected GeliGasStornierungCommand::ReceiveUtilmd");
    };

    assert_eq!(cmd_pid.as_u32(), 44022);
    assert_eq!(
        cmd_sender.as_str(),
        STORNO_LFN_ID,
        "sender GLN must match NAD+MS in fixture"
    );
    assert_eq!(
        cmd_receiver.as_str(),
        STORNO_GNB_ID,
        "receiver GLN must match NAD+MR in fixture"
    );
    assert_eq!(
        cmd_vorgang.as_str(),
        STORNO_VORGANG_ID,
        "vorgang_id must match IDE+24 in fixture"
    );
    assert_eq!(
        cmd_ref.as_str(),
        "00001",
        "message_ref must match UNH ref in fixture"
    );

    let exec_cmd = GeliGasStornierungCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        vorgang_id: cmd_vorgang,
        document_date: String::new(),
        message_ref: cmd_ref,
        validation_passed: report.is_valid(), // ← authoritative AHB result, no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };

    // GNB (Gasnetzbetreiber) owns the GeLi Gas Stornierung process.
    let gnb_process: Process<GeliGasStornierungWorkflow, InMemoryEventStore> = Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(STORNO_GNB_ID),
        WorkflowId::new("geli-gas-stornierung", GAS_FV_2025),
    );

    gnb_process
        .execute(exec_cmd)
        .await
        .expect("GNB process must accept AHB-validated 44022 without error");

    let state: GeliGasStornierungState = gnb_process.state().await.expect("must load state");
    assert!(
        matches!(state, GeliGasStornierungState::ValidationPassed(_)),
        "after AHB-validated 44022, state must be ValidationPassed; got: {state:?}",
    );
}

// ── Validation-only: GeLi Gas 44023 Bestätigung Stornierung ──────────────────

#[test]
fn ahb_44023_geli_gas_stornierung_bestaetigung_validates() {
    let (msg, _report) = parse_and_assert_ahb_valid(
        UTILMD_44023_STORNIERUNG_BESTAETIGUNG_VALID,
        GAS_VALIDATION_DATE,
    );
    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 44023, "fixture must encode PID 44023");
}

// ── Validation-only: GeLi Gas 44024 Ablehnung Stornierung ────────────────────

#[test]
fn ahb_44024_geli_gas_stornierung_ablehnung_validates() {
    let (msg, _report) = parse_and_assert_ahb_valid(
        UTILMD_44024_STORNIERUNG_ABLEHNUNG_VALID,
        GAS_VALIDATION_DATE,
    );
    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 44024, "fixture must encode PID 44024");
}

// ══════════════════════════════════════════════════════════════════════════════
// WiM Gas Kündigung responses (PIDs 44040–44041)
// ══════════════════════════════════════════════════════════════════════════════

const UTILMD_44040_KUENDIGUNG_BESTAETIGUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44040_bestaetigung_kuendigung_msb_gas.edi"
);
const UTILMD_44041_KUENDIGUNG_ABLEHNUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44041_ablehnung_kuendigung_msb_gas.edi"
);

#[test]
fn ahb_44040_wim_gas_kuendigung_bestaetigung_validates() {
    let (msg, _) = parse_and_assert_ahb_valid(
        UTILMD_44040_KUENDIGUNG_BESTAETIGUNG_VALID,
        GAS_VALIDATION_DATE,
    );
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44040);
}

#[test]
fn ahb_44041_wim_gas_kuendigung_ablehnung_validates() {
    let (msg, _) =
        parse_and_assert_ahb_valid(UTILMD_44041_KUENDIGUNG_ABLEHNUNG_VALID, GAS_VALIDATION_DATE);
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44041);
}

// ══════════════════════════════════════════════════════════════════════════════
// WiM Gas Anmeldung MSB Gas (PID 44042 + responses 44043–44044)
// ══════════════════════════════════════════════════════════════════════════════

const UTILMD_44042_ANMELDUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44042_anmeldung_msb_gas.edi"
);
const UTILMD_44043_ANMELDUNG_BESTAETIGUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44043_bestaetigung_anmeldung_msb_gas.edi"
);
const UTILMD_44044_ANMELDUNG_ABLEHNUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44044_ablehnung_anmeldung_msb_gas.edi"
);

/// GLN of the MSBA sender in the WiM Gas Anmeldung 44042 fixture.
const ANMELDUNG_MSBA_ID: &str = "4012345000023";
/// GLN of the NB receiver in the WiM Gas Anmeldung 44042 fixture.
const ANMELDUNG_NB_ID: &str = "9907317000007";
/// Vorgangsnummer (IDE+24) in the WiM Gas Anmeldung 44042 fixture.
const ANMELDUNG_VORGANG_ID: &str = "WIMGAS00002";

/// End-to-end AHB conformance test for WiM Gas Anmeldung MSB Gas Anfrage (PID 44042).
///
/// Confirms:
/// 1. UTILMD G G1.1 PID 44042 fixture parses and passes AHB validation.
/// 2. `wim_gas_anmeldung_registry()` correctly extracts sender, receiver, and malo_id.
/// 3. `WimGasAnmeldungWorkflow` transitions to `ValidationPassed` after dispatch.
///
/// PID 44042 is the MSBA → NB Anmeldung Anfrage; the NB receives it and owns the
/// WiM Gas Anmeldung process (PIDs 44042–44053, BK7-24-01-009).
#[tokio::test]
async fn ahb_44042_wim_gas_anmeldung_validates_and_dispatches() {
    let (msg, report) =
        parse_and_assert_ahb_valid(UTILMD_44042_ANMELDUNG_VALID, GAS_VALIDATION_DATE);

    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 44042, "fixture must encode PID 44042");
    let release = msg.detect_release().expect("release must be detectable");
    assert_eq!(release.as_str(), "G1.1", "fixture must encode G1.1 release");

    let fv = FormatVersion::new(GAS_FV_2025);
    let adapter_cmd = wim_gas_anmeldung_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .expect("wim_gas_anmeldung_registry must adapt PID 44042 to WimGasAnmeldungCommand");

    let WimGasAnmeldungCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        message_ref: cmd_ref,
        ..
    } = adapter_cmd
    else {
        panic!("expected WimGasAnmeldungCommand::ReceiveUtilmd");
    };

    assert_eq!(cmd_pid.as_u32(), 44042);
    assert_eq!(
        cmd_sender.as_str(),
        ANMELDUNG_MSBA_ID,
        "sender GLN must match NAD+MS (MSBA) in fixture"
    );
    assert_eq!(
        cmd_receiver.as_str(),
        ANMELDUNG_NB_ID,
        "receiver GLN must match NAD+MR (NB) in fixture"
    );
    assert_eq!(
        cmd_malo.as_str(),
        ANMELDUNG_VORGANG_ID,
        "malo_id must match IDE+24 Vorgangsnummer in fixture"
    );
    assert_eq!(
        cmd_ref.as_str(),
        "00001",
        "message_ref must match UNH ref in fixture"
    );

    let exec_cmd = WimGasAnmeldungCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        document_date: String::new(),
        message_ref: cmd_ref,
        validation_passed: report.is_valid(), // ← authoritative AHB result, no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };

    // NB (Netzbetreiber) owns the WiM Gas Anmeldung process; receives MSBA's 44042.
    let nb_process: Process<WimGasAnmeldungWorkflow, InMemoryEventStore> = Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(ANMELDUNG_NB_ID),
        WorkflowId::new("wim-gas-anmeldung", GAS_FV_2025),
    );

    nb_process
        .execute(exec_cmd)
        .await
        .expect("NB process must accept AHB-validated 44042 without error");

    let state: WimGasAnmeldungState = nb_process.state().await.expect("must load state");
    assert!(
        matches!(state, WimGasAnmeldungState::ValidationPassed(_)),
        "after AHB-validated 44042 ReceiveUtilmd, state must be ValidationPassed; got: {state:?}",
    );
}

#[test]
fn ahb_44043_wim_gas_anmeldung_bestaetigung_validates() {
    let (msg, _) = parse_and_assert_ahb_valid(
        UTILMD_44043_ANMELDUNG_BESTAETIGUNG_VALID,
        GAS_VALIDATION_DATE,
    );
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44043);
}

#[test]
fn ahb_44044_wim_gas_anmeldung_ablehnung_validates() {
    let (msg, _) =
        parse_and_assert_ahb_valid(UTILMD_44044_ANMELDUNG_ABLEHNUNG_VALID, GAS_VALIDATION_DATE);
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44044);
}

// ══════════════════════════════════════════════════════════════════════════════
// WiM Gas Ende MSB / Vorläufige Abmeldung (PIDs 44051–44053)
// ══════════════════════════════════════════════════════════════════════════════

const UTILMD_44051_ENDE_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44051_ende_msb_gas.edi"
);
const UTILMD_44052_ENDE_BESTAETIGUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44052_bestaetigung_ende_msb_gas.edi"
);
const UTILMD_44053_ENDE_ABLEHNUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44053_ablehnung_ende_msb_gas.edi"
);

/// PID 44051 is Ende MSB Gas / Vorläufige Abmeldung (NB → MSBA direction).
/// AHB profile: G1.1 (fv20251001_gas). Handled by WimGasAnmeldungWorkflow.
#[test]
fn ahb_44051_wim_gas_ende_validates() {
    let (msg, _) = parse_and_assert_ahb_valid(UTILMD_44051_ENDE_VALID, GAS_VALIDATION_DATE);
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44051);
}

#[test]
fn ahb_44052_wim_gas_ende_bestaetigung_validates() {
    let (msg, _) =
        parse_and_assert_ahb_valid(UTILMD_44052_ENDE_BESTAETIGUNG_VALID, GAS_VALIDATION_DATE);
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44052);
}

#[test]
fn ahb_44053_wim_gas_ende_ablehnung_validates() {
    let (msg, _) =
        parse_and_assert_ahb_valid(UTILMD_44053_ENDE_ABLEHNUNG_VALID, GAS_VALIDATION_DATE);
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44053);
}

// ══════════════════════════════════════════════════════════════════════════════
// WiM Gas Verpflichtungsanfrage (PID 44168 + responses 44169–44170)
// ══════════════════════════════════════════════════════════════════════════════

const UTILMD_44168_VERPFLICHTUNGSANFRAGE_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44168_verpflichtungsanfrage.edi"
);
const UTILMD_44169_VERPFLICHTUNGSANFRAGE_BESTAETIGUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44169_bestaetigung_verpflichtungsanfrage.edi"
);
const UTILMD_44170_VERPFLICHTUNGSANFRAGE_ABLEHNUNG_VALID: &[u8] = include_bytes!(
    "../../../crates/edi-energy/tests/fixtures/utilmd/valid/beispiel_44170_ablehnung_verpflichtungsanfrage.edi"
);

/// GLN of the NB sender in the Verpflichtungsanfrage 44168 fixture (NB → gMSB).
const VERPFL_NB_ID: &str = "9907317000007";
/// GLN of the gMSB receiver in the Verpflichtungsanfrage 44168 fixture.
const VERPFL_GMSB_ID: &str = "4012345000023";
/// Vorgangsnummer (IDE+24) in the Verpflichtungsanfrage 44168 fixture.
const VERPFL_VORGANG_ID: &str = "WIMGAS00004";

/// End-to-end AHB conformance test for WiM Gas Verpflichtungsanfrage (PID 44168).
///
/// Confirms:
/// 1. UTILMD G G1.1 PID 44168 fixture parses and passes AHB validation.
/// 2. `wim_gas_verpflichtungsanfrage_registry()` correctly extracts sender, receiver, malo_id.
/// 3. `WimGasVerpflichtungsanfrageWorkflow` transitions to `ValidationPassed` after dispatch.
///
/// PID 44168 is NB → gMSB direction; the gMSB owns the Verpflichtungsanfrage process
/// (PIDs 44168–44170, BK7-24-01-009).
#[tokio::test]
async fn ahb_44168_wim_gas_verpflichtungsanfrage_validates_and_dispatches() {
    let (msg, report) = parse_and_assert_ahb_valid(
        UTILMD_44168_VERPFLICHTUNGSANFRAGE_VALID,
        GAS_VALIDATION_DATE,
    );

    let pid = msg
        .detect_pruefidentifikator()
        .expect("PID must be detectable");
    assert_eq!(pid.as_u32(), 44168, "fixture must encode PID 44168");
    let release = msg.detect_release().expect("release must be detectable");
    assert_eq!(release.as_str(), "G1.1", "fixture must encode G1.1 release");

    let fv = FormatVersion::new(GAS_FV_2025);
    let adapter_cmd = wim_gas_verpflichtungsanfrage_registry()
        .dispatch(&msg as &dyn Any, &fv)
        .expect("wim_gas_verpflichtungsanfrage_registry must adapt PID 44168 to WimGasVerpflichtungsanfrageCommand");

    let WimGasVerpflichtungsanfrageCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        message_ref: cmd_ref,
        ..
    } = adapter_cmd
    else {
        panic!("expected WimGasVerpflichtungsanfrageCommand::ReceiveUtilmd");
    };

    assert_eq!(cmd_pid.as_u32(), 44168);
    assert_eq!(
        cmd_sender.as_str(),
        VERPFL_NB_ID,
        "sender GLN must match NAD+MS (NB) in fixture"
    );
    assert_eq!(
        cmd_receiver.as_str(),
        VERPFL_GMSB_ID,
        "receiver GLN must match NAD+MR (gMSB) in fixture"
    );
    assert_eq!(
        cmd_malo.as_str(),
        VERPFL_VORGANG_ID,
        "malo_id must match IDE+24 Vorgangsnummer in fixture"
    );
    assert_eq!(
        cmd_ref.as_str(),
        "00001",
        "message_ref must match UNH ref in fixture"
    );

    let exec_cmd = WimGasVerpflichtungsanfrageCommand::ReceiveUtilmd {
        pid: cmd_pid,
        sender: cmd_sender,
        receiver: cmd_receiver,
        malo_id: cmd_malo,
        document_date: String::new(),
        message_ref: cmd_ref,
        validation_passed: report.is_valid(), // ← authoritative AHB result, no bypass
        validation_errors: report.errors().iter().map(|e| format!("{e}")).collect(),
    };

    // The gMSB (grundzuständiger MSB) owns the Verpflichtungsanfrage process.
    let gmsb_process: Process<WimGasVerpflichtungsanfrageWorkflow, InMemoryEventStore> =
        Process::new(
            InMemoryEventStore::new(),
            TenantId::from_party_id(VERPFL_GMSB_ID),
            WorkflowId::new("wim-gas-verpflichtungsanfrage", GAS_FV_2025),
        );

    gmsb_process
        .execute(exec_cmd)
        .await
        .expect("gMSB process must accept AHB-validated 44168 without error");

    let state: WimGasVerpflichtungsanfrageState =
        gmsb_process.state().await.expect("must load state");
    assert!(
        matches!(state, WimGasVerpflichtungsanfrageState::ValidationPassed(_)),
        "after AHB-validated 44168 ReceiveUtilmd, state must be ValidationPassed; got: {state:?}",
    );
}

#[test]
fn ahb_44169_wim_gas_verpflichtungsanfrage_bestaetigung_validates() {
    let (msg, _) = parse_and_assert_ahb_valid(
        UTILMD_44169_VERPFLICHTUNGSANFRAGE_BESTAETIGUNG_VALID,
        GAS_VALIDATION_DATE,
    );
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44169);
}

#[test]
fn ahb_44170_wim_gas_verpflichtungsanfrage_ablehnung_validates() {
    let (msg, _) = parse_and_assert_ahb_valid(
        UTILMD_44170_VERPFLICHTUNGSANFRAGE_ABLEHNUNG_VALID,
        GAS_VALIDATION_DATE,
    );
    assert_eq!(msg.detect_pruefidentifikator().unwrap().as_u32(), 44170);
}
