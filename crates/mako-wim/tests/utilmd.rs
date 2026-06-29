//! End-to-end pipeline test: `edi-energy` parse → validate → `mako-engine` execute.
//!
//! This test covers the full production dispatch path for a WiM Messstellenbetrieb
//! (PID 55042 — Anmeldung MSB) message and verifies that the cross-crate extraction/
//! adaptation boundary is exercised end-to-end. An incompatibility between `edi-energy`
//! (e.g. renaming a method on `UtilmdMessage`) and the WiM adapter code would
//! be caught by this test before reaching production.
//!
//! ```text
//! Raw EDIFACT bytes
//!   │
//!   ▼ edi_energy::Platform::with_all_profiles().parse(bytes)
//! AnyMessage
//!   │
//!   ▼ msg.validate()                → EdiEnergyReport (is_valid / errors)
//!   │
//!   ▼ msg.detect_pruefidentifikator() → Pruefidentifikator (55042)
//!   │
//!   ▼ extract fields → DeviceChangeCommand::ReceiveUtilmd
//!   │
//!   ▼ Process::execute(command)     → Vec<EventEnvelope>
//!   │
//!   ▼ Process::state()              → DeviceChangeState (asserted)
//! ```
//!
//! This test uses only `InMemoryEventStore` — no SlateDB required.

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
use mako_engine::{
    builder::EngineBuilder,
    event_store::InMemoryEventStore,
    ids::TenantId,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_wim::{DeviceChangeCommand, DeviceChangeState, WimDeviceChangeWorkflow};

// ── UTILMD W PID 55042 test fixture ──────────────────────────────────────────

/// A minimal WiM UTILMD Anmeldung MSB (PID 55042) in EDIFACT format.
///
/// Follows the S2.1 schema (`fv20251001`):
///
/// - UNB  — interchange header (sender 4012345000023, receiver 9900357000004)
/// - UNH  — message header (UTILMD:D:11A:UN:S2.1)
/// - BGM  — document type E01, PID 55042
/// - DTM  — document date 2025-01-15
/// - RFF  — Z13 reference (Auftragsreferenz)
/// - NAD+MS — new MSB (MSBN, sender)
/// - NAD+MR — grid operator (NB, receiver)
/// - IDE  — messlokation (Z19 qualifier)
/// - LOC  — device ID (LOC+172)
/// - UNT  — message trailer
/// - UNZ  — interchange trailer
const UTILMD_55042_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+WIM-2025-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055042::+9'\
DTM+137:20250115:102'\
RFF+Z13:WIM-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+DE0001000001234567890000000000001::'\
LOC+172+ZHR-12345678::'\
UNT+9+MSG-001'\
UNZ+1+WIM-2025-001'";

// ── E2E pipeline test ─────────────────────────────────────────────────────────

/// Full pipeline: parse EDIFACT → validate → extract fields → execute →
/// reconstruct state and assert typed result.
///
/// Exercises the cross-crate `edi-energy → mako-wim` boundary end-to-end.
/// Any breaking change to the `UtilmdMessage` API (e.g. renaming `sender()`,
/// `transactions()`, `dtm()`) will surface here before reaching production.
#[tokio::test]
async fn end_to_end_geraetewechsel_pipeline() {
    // ── Step 1: Parse raw EDIFACT bytes ──────────────────────────────────────
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(UTILMD_55042_BYTES)
        .expect("UTILMD W PID 55042 parse must succeed");

    // ── Step 2: Validate against BDEW profile rules ──────────────────────────
    let report = msg.validate().expect("validation call must not error");
    let validation_passed = report.is_valid();
    let validation_errors: Vec<String> =
        report.errors().iter().map(|e| e.message.clone()).collect();

    // ── Step 3: Detect PID and extract domain fields ──────────────────────────
    let pid = Pruefidentifikator::new(
        msg.detect_pruefidentifikator()
            .expect("PID detection must succeed for a BGM+E01 WiM message")
            .as_u32(),
    )
    .expect("PID in range");
    assert_eq!(pid.as_u32(), 55042, "PID must be 55042 (Anmeldung MSB)");

    let (sender, receiver, melo_id, document_date, message_ref) =
        if let AnyMessage::Utilmd(utilmd) = &msg {
            let sender = MarktpartnerCode::new(
                utilmd
                    .sender()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or("UNKNOWN"),
            );
            let receiver = MarktpartnerCode::new(
                utilmd
                    .receiver()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or("UNKNOWN"),
            );
            let document_date = utilmd
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("19700101")
                .to_owned();
            let message_ref = MessageRef::new(
                utilmd
                    .references()
                    .iter()
                    .next()
                    .and_then(|r| r.rff.reference.as_deref())
                    .unwrap_or("REF-UNKNOWN"),
            );
            // MeLo from IDE Z19 transaction
            let melo_id = MeLo::new(
                utilmd
                    .transactions()
                    .first()
                    .and_then(|tx| tx.ide.object_id.as_deref())
                    .unwrap_or("MELO-UNKNOWN"),
            );
            (sender, receiver, melo_id, document_date, message_ref)
        } else {
            panic!("expected AnyMessage::Utilmd variant");
        };

    // ── Step 4: Build command ─────────────────────────────────────────────────
    //
    // In production the device_id is extracted from LOC+172 in the transaction.
    // The UtilmdMessage API does not yet expose LOC segments directly, so we
    // fall back to a well-known value from the fixture for this test.
    let device_id = DeviceId::new("ZHR-12345678");
    let command = DeviceChangeCommand::ReceiveUtilmd {
        pid,
        sender,
        receiver,
        melo_id,
        device_id,
        document_date,
        message_ref,
        validation_passed,
        validation_errors,
    };

    // ── Step 5: Execute against in-memory engine ──────────────────────────────
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let process = ctx.spawn::<WimDeviceChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
    );

    let envelopes = process
        .execute(command)
        .await
        .expect("execute must succeed for a valid PID-55042 command");

    // ── Step 6: Verify events ─────────────────────────────────────────────────
    assert!(!envelopes.is_empty(), "at least one event must be emitted");
    assert_eq!(
        envelopes[0].event_type.as_ref(),
        "WimDeviceChangeInitiated",
        "first event must be WimDeviceChangeInitiated"
    );

    // ── Step 7: Reconstruct state and assert typed fields ─────────────────────
    let state: DeviceChangeState = process
        .state()
        .await
        .expect("state reconstruction must succeed");

    if validation_passed {
        match state {
            DeviceChangeState::ValidationPassed(data) => {
                assert_eq!(data.melo_id, MeLo::new("DE0001000001234567890000000000001"));
                assert_eq!(data.pruefidentifikator.as_u32(), 55042);
            }
            other => panic!("state must be ValidationPassed after valid message; got {other:?}"),
        }
    } else {
        assert!(
            matches!(state, DeviceChangeState::Rejected { .. }),
            "state must be Rejected after invalid message; got {state:?}",
        );
    }
}

/// A command with a non-WiM PID must be rejected by the workflow.
#[tokio::test]
async fn wrong_pid_returns_workflow_error() {
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let process = ctx.spawn::<WimDeviceChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
    );

    let result = process
        .execute(DeviceChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(55001).unwrap(), // GPKE PID — wrong for WiM
            sender: "4012345000023".into(),
            receiver: "9900357000004".into(),
            melo_id: "DE0001000001234567890000000000001".into(),
            device_id: DeviceId::new("ZHR-12345678"),
            document_date: "20250115".to_owned(),
            message_ref: "WIM-REF-001".into(),
            validation_passed: true,
            validation_errors: vec![],
        })
        .await;

    assert!(
        result.is_err(),
        "wrong PID must produce WorkflowError::Rejected"
    );
}
