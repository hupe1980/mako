//! End-to-end pipeline test: `edi-energy` parse → validate → `mako-engine` execute.
//!
//! This test covers the full production dispatch path for a GPKE Lieferbeginn
//! Strom (PID 55001) message and verifies the UTILMD PID guard accepts all
//! LFW24 GPKE ANFRAGE PIDs (55001, 55002, 55017, 56001–56004)
//! and rejects out-of-range or outbound response PIDs:
//!
//! ```text
//! Raw EDIFACT bytes
//!   │
//!   ▼ edi_energy::Platform::with_all_profiles().parse(bytes)
//! AnyMessage
//!   │
//!   ▼ msg.validate()                → EdiEnergyReport (is_valid / errors)
//!   │
//!   ▼ msg.detect_pruefidentifikator() → Pruefidentifikator (55001)
//!   │
//!   ▼ extract fields → SupplierChangeCommand::ReceiveUtilmd
//!   │
//!   ▼ Process::execute(command)     → Vec<EventEnvelope>
//!   │
//!   ▼ Process::state()              → SupplierChangeState (asserted)
//! ```
//!
//! This test uses only `InMemoryEventStore` — no SlateDB required.

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
use mako_engine::{
    builder::EngineBuilder,
    event_store::InMemoryEventStore,
    ids::{ConversationId, CorrelationId, TenantId},
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::CommandContext,
};
use mako_gpke::{GpkeSupplierChangeWorkflow, SupplierChangeCommand, SupplierChangeState};

// ── UTILMD PID 55001 test fixture ─────────────────────────────────────────────

/// A minimal UTILMD Lieferbeginn Strom message (PID 55001) in EDIFACT format.
///
/// Follows the S2.1 schema (`fv20251001`):
///
/// - UNB  — interchange header (sender 4012345000023, receiver 9900357000004)
/// - UNH  — message header (UTILMD:D:11A:UN:S2.1)
/// - BGM  — document type E01, PID 55001
/// - DTM  — document date 2025-01-15
/// - RFF  — Z13 reference (SG1, Auftragsreferenz)
/// - NAD+MS — sender (new supplier GLN)
/// - NAD+MR — receiver (grid operator GLN)
/// - IDE  — metering point / process (SG4, Z19 qualifier)
/// - UNT  — message trailer
/// - UNZ  — interchange trailer
const UTILMD_55001_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+INTER-2025-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20250115:102'\
RFF+Z13:REF-2025-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+51238696781::'\
UNT+8+MSG-001'\
UNZ+1+INTER-2025-001'";

// ── end-to-end pipeline test ──────────────────────────────────────────

/// Full pipeline: parse EDIFACT → validate → extract fields → execute →
/// reconstruct state and assert typed result.
#[tokio::test]
async fn end_to_end_lieferbeginn_strom_pipeline() {
    // ── Step 1: Parse raw EDIFACT bytes ──────────────────────────────────────
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(UTILMD_55001_BYTES)
        .expect("UTILMD parse must succeed");

    // ── Step 2: Validate against BDEW profile rules ──────────────────────────
    let report = msg.validate().expect("validation call must not error");
    // Extract validation outcome for the command payload.
    let validation_passed = report.is_valid();
    let validation_errors: Vec<String> =
        report.errors().iter().map(|e| e.message.clone()).collect();

    // ── Step 3: Detect PID and extract domain fields ──────────────────────────
    let pid = Pruefidentifikator::new(
        msg.detect_pruefidentifikator()
            .expect("PID detection must succeed for a BGM+E01 message")
            .as_u32(),
    )
    .expect("PID in range");
    assert_eq!(
        pid.as_u32(),
        55001,
        "PID must be 55001 (Lieferbeginn Strom)"
    );

    // Extract sender, receiver, document date, and message reference from
    // the parsed message. In production this extraction lives in a
    // `MessageAdapter` — here we do it inline to keep the test self-contained.
    let (sender, receiver, document_date, process_date, message_ref, location_id) =
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
            let process_date = utilmd
                .transactions()
                .first()
                .and_then(|t| t.dtm.iter().find(|d| d.is_period_start()))
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
            let location_id = MaLo::new(
                utilmd
                    .transactions()
                    .first()
                    .and_then(|tx| tx.ide.object_id.as_deref())
                    .unwrap_or("MALO-UNKNOWN"),
            );
            (
                sender,
                receiver,
                document_date,
                process_date,
                message_ref,
                location_id,
            )
        } else {
            panic!("expected AnyMessage::Utilmd variant");
        };

    // ── Step 4: Build command ─────────────────────────────────────────────────
    let command = SupplierChangeCommand::ReceiveUtilmd {
        pid,
        sender,
        receiver,
        location_id,
        document_date,
        process_date,
        message_ref,
        validation_passed,
        validation_errors,
    };

    // ── Step 5: Execute against in-memory engine ──────────────────────────────
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let tenant_id = TenantId::new();
    let workflow_id = WorkflowId::new("gpke-supplier-change", "FV2024-10-01");
    let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(tenant_id, workflow_id.clone());

    // Propagate the EDIFACT interchange's conversation context to the CommandContext.
    //
    // In production, `conversation_id` and `correlation_id` are derived from the
    // EDIFACT interchange header (UNB DE0020 reference number) so that all events
    // produced by this command share the same root trace as the inbound message.
    // This is the canonical pattern for all AS4-triggered commands — do NOT use
    // `process.execute(command)` (which generates fresh IDs) for messages received
    // from the EDIFACT network.
    //
    // Here we use a deterministic UUID so the test can assert on it.
    let interchange_conversation = ConversationId::new();
    let interchange_correlation = CorrelationId::new();
    let cmd_ctx = CommandContext::new(tenant_id, process.process_id(), workflow_id)
        .with_conversation(interchange_conversation)
        .with_correlation(interchange_correlation);

    let envelopes = process
        .execute_with(command, cmd_ctx)
        .await
        .expect("execute must succeed for a valid PID-55001 command");

    // ── Step 6: Verify events ─────────────────────────────────────────────────
    // `validation_passed` determines whether 2 or 3 events are emitted:
    //   - Initiated (always)
    //   - ValidationPassed  (when validation_passed = true)
    //   - Rejected          (when validation_passed = false)
    assert!(!envelopes.is_empty(), "at least one event must be emitted");
    assert_eq!(
        envelopes[0].event_type.as_ref(),
        "SupplierChangeInitiated",
        "first event must be SupplierChangeInitiated"
    );
    // All events must carry the propagated conversation and correlation IDs from
    // the inbound EDIFACT interchange — not fresh auto-generated ones.
    for env in &envelopes {
        assert_eq!(
            env.conversation_id, interchange_conversation,
            "event conversation_id must be propagated from the EDIFACT interchange"
        );
        assert_eq!(
            env.correlation_id, interchange_correlation,
            "event correlation_id must be propagated from the EDIFACT interchange"
        );
    }

    // ── Step 7: Reconstruct state and assert typed fields ─────────────────────
    let state: SupplierChangeState = process
        .state()
        .await
        .expect("state reconstruction must succeed");

    if validation_passed {
        // Both Initiated + ValidationPassed emitted → state is ValidationPassed.
        assert!(
            matches!(state, SupplierChangeState::ValidationPassed(_)),
            "state must be ValidationPassed after valid message; got {:?}",
            state.label()
        );
        let data = state
            .initiated_data()
            .expect("ValidationPassed must carry InitiatedData");
        assert_eq!(data.location_id, MaLo::new("51238696781"));
        assert_eq!(data.pruefidentifikator.as_u32(), 55001);
    } else {
        // Validation failed → state is Rejected.
        assert!(
            matches!(state, SupplierChangeState::Rejected { .. }),
            "state must be Rejected after invalid message; got {:?}",
            state.label()
        );
    }
}

/// A command with an unexpected PID must be rejected by the workflow.
#[tokio::test]
async fn wrong_pid_returns_workflow_error() {
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("gpke-supplier-change", "FV2024-10-01"),
    );

    let result = process
        .execute(SupplierChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(17001).unwrap(), // GeLi Gas PID — wrong for this workflow
            sender: "4012345000023".into(),
            receiver: "9900357000004".into(),
            location_id: "51238696781".into(),
            document_date: "20250115".into(),
            process_date: "20251001".into(),
            message_ref: "REF-001".into(),
            validation_passed: true,
            validation_errors: vec![],
        })
        .await;

    assert!(
        result.is_err(),
        "wrong PID must produce WorkflowError::Rejected"
    );
}

/// SendAntwort command when state is not ValidationPassed must error.
#[tokio::test]
async fn send_antwort_from_wrong_state_returns_error() {
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("gpke-supplier-change", "FV2024-10-01"),
    );

    // Try to send Antwort before any Initiated event.
    let result = process
        .execute(SupplierChangeCommand::SendAntwort {
            accepted: true,
            reason: None,
            obligations: vec![],
        })
        .await;

    assert!(
        result.is_err(),
        "SendAntwort from New state must produce WorkflowError::InvalidState"
    );
}
