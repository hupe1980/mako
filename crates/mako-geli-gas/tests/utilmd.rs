//! End-to-end pipeline test: `edi-energy` parse → validate → `mako-engine` execute.
//!
//! This test covers the full production dispatch path for a GeLi Gas
//! Lieferbeginn (PID 44001) message and verifies the cross-crate extraction
//! boundary end-to-end. Any breaking change to the `UtilmdMessage` API
//! (e.g. renaming a method used in the GeLi Gas adapter) will surface here.
//!
//! ```text
//! Raw EDIFACT bytes
//!   │
//!   ▼ edi_energy::Platform::with_all_profiles().parse(bytes)
//! AnyMessage
//!   │
//!   ▼ msg.validate()                → EdiEnergyReport (is_valid / errors)
//!   │
//!   ▼ msg.detect_pruefidentifikator() → Pruefidentifikator (44001)
//!   │
//!   ▼ extract fields → GasSupplierChangeCommand::ReceiveUtilmd
//!   │
//!   ▼ Process::execute(command)     → Vec<EventEnvelope>
//!   │
//!   ▼ Process::state()              → GasSupplierChangeState (asserted)
//! ```
//!
//! This test uses only `InMemoryEventStore` — no SlateDB required.

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
use mako_engine::{
    builder::EngineBuilder,
    event_store::InMemoryEventStore,
    ids::TenantId,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_geli_gas::{
    GasSupplierChangeCommand, GasSupplierChangeState, GeliGasSupplierChangeWorkflow,
};

// ── UTILMD G PID 44001 test fixture ──────────────────────────────────────────

/// A minimal UTILMD Lieferbeginn Gas message (PID 44001) in EDIFACT format.
///
/// Follows the G1.1 schema (`fv20251001_gas`):
///
/// - UNB  — interchange header (sender 4012345000023, receiver 9900357000004)
/// - UNH  — message header (UTILMD:D:11A:UN:G1.1)
/// - BGM  — document type E01, PID 44001
/// - DTM  — document date 2025-01-15
/// - RFF  — Z13 reference (Auftragsreferenz)
/// - NAD+MS — new gas supplier (LF, sender)
/// - NAD+MR — gas grid operator (NB, receiver)
/// - IDE  — MaLo identifier (Z19 qualifier)
/// - UNT  — message trailer
/// - UNZ  — interchange trailer
///
/// # Regulatory context
///
/// GeLi Gas APERAK Frist: **10 Werktage** (BNetzA BK7).
/// Saturday counts as a Werktag; Sunday and federal public holidays do not.
const UTILMD_44001_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+GAS-2025-001'\
UNH+MSG-001+UTILMD:D:11A:UN:G1.1'\
BGM+E01:::+00044001::+9'\
DTM+137:20250115:102'\
RFF+Z13:GAS-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+52695662085::'\
UNT+8+MSG-001'\
UNZ+1+GAS-2025-001'";

// ── E2E pipeline test ─────────────────────────────────────────────────────────

/// Full pipeline: parse EDIFACT G → validate → extract fields → execute →
/// reconstruct state and assert typed result.
///
/// Exercises the cross-crate `edi-energy → mako-geli-gas` boundary end-to-end.
/// Any breaking change to the `UtilmdMessage` API (e.g. renaming `sender()`,
/// `transactions()`, `dtm()`) will surface here before reaching production.
#[tokio::test]
async fn end_to_end_lieferbeginn_gas_pipeline() {
    // ── Step 1: Parse raw EDIFACT bytes ──────────────────────────────────────
    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(UTILMD_44001_BYTES)
        .expect("UTILMD G PID 44001 parse must succeed");

    // ── Step 2: Validate against BDEW profile rules ──────────────────────────
    let report = msg.validate().expect("validation call must not error");
    let validation_passed = report.is_valid();
    let validation_errors: Vec<String> =
        report.errors().iter().map(|e| e.message.clone()).collect();

    // ── Step 3: Detect PID and extract domain fields ──────────────────────────
    let pid = Pruefidentifikator::new(
        msg.detect_pruefidentifikator()
            .expect("PID detection must succeed for a BGM+E01 GeLi Gas message")
            .as_u32(),
    )
    .expect("PID in range");
    assert_eq!(
        pid.as_u32(),
        44001,
        "PID must be 44001 (GeLi Gas Lieferbeginn)"
    );

    let (sender, receiver, malo_id, document_date, message_ref) =
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
            let malo_id = MaLo::new(
                utilmd
                    .transactions()
                    .first()
                    .and_then(|tx| tx.ide.object_id.as_deref())
                    .unwrap_or("MALO-UNKNOWN"),
            );
            (sender, receiver, malo_id, document_date, message_ref)
        } else {
            panic!("expected AnyMessage::Utilmd variant");
        };

    // ── Step 4: Build command ─────────────────────────────────────────────────
    let command = GasSupplierChangeCommand::ReceiveUtilmd {
        pid,
        sender,
        receiver,
        malo_id,
        document_date,
        process_date: String::new(),
        message_ref,
        validation_passed,
        validation_errors,
        received_at: time::OffsetDateTime::now_utc(),
        bilanzierungsmethode: None,
        fallgruppe: None,
    };

    // ── Step 5: Execute against in-memory engine ──────────────────────────────
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let process = ctx.spawn::<GeliGasSupplierChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("geli-gas-supplier-change", "FV2025-10-01"),
    );

    let envelopes = process
        .execute(command)
        .await
        .expect("execute must succeed for a valid PID-44001 command");

    // ── Step 6: Verify events ─────────────────────────────────────────────────
    assert!(!envelopes.is_empty(), "at least one event must be emitted");
    assert_eq!(
        envelopes[0].event_type.as_ref(),
        "GasSupplierChangeInitiated",
        "first event must be GasSupplierChangeInitiated"
    );

    // ── Step 7: Reconstruct state and assert typed fields ─────────────────────
    let state: GasSupplierChangeState = process
        .state()
        .await
        .expect("state reconstruction must succeed");

    if validation_passed {
        match state {
            GasSupplierChangeState::ValidationPassed(data) => {
                assert_eq!(data.malo_id, MaLo::new("52695662085"));
                assert_eq!(data.pruefidentifikator.as_u32(), 44001);
            }
            other => panic!("state must be ValidationPassed after valid message; got {other:?}"),
        }
    } else {
        assert!(
            matches!(state, GasSupplierChangeState::Rejected { .. }),
            "state must be Rejected after invalid message; got {state:?}",
        );
    }
}

/// A command with a non-GeLi-Gas PID must be rejected by the workflow.
#[tokio::test]
async fn wrong_pid_returns_workflow_error() {
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();

    let process = ctx.spawn::<GeliGasSupplierChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("geli-gas-supplier-change", "FV2025-10-01"),
    );

    let result = process
        .execute(GasSupplierChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(55001).unwrap(), // GPKE Strom PID — wrong for GeLi Gas
            sender: "4012345000023".into(),
            receiver: "9900357000004".into(),
            malo_id: "52695662085".into(),
            document_date: "20250115".to_owned(),
            process_date: String::new(),
            message_ref: "GAS-REF-001".into(),
            validation_passed: true,
            validation_errors: vec![],
            received_at: time::OffsetDateTime::now_utc(),
            bilanzierungsmethode: None,
            fallgruppe: None,
        })
        .await;

    assert!(
        result.is_err(),
        "wrong PID must produce WorkflowError::Rejected"
    );
}

// ── Negative AHB conformance tests ───────────────────────────────────────────

/// A UTILMD G PID 44001 with an invalid BGM qualifier (`E99` instead of `E01`)
/// must be rejected by the AHB rule pack with rule `AHB-44001-BGM-1001-Q`.
///
/// This guards against the `Some(_unknown)` dispatch branch silently returning
/// `is_valid = true` for any PID absent from the AHB profile dispatch table,
/// and against profile regressions that remove or relax the BGM qualifier check.
#[test]
fn negative_ahb_wrong_bgm_qualifier_pid_44001() {
    let invalid_bytes: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+GAS-NEG-001'\
UNH+MSG-001+UTILMD:D:11A:UN:G1.1'\
BGM+E99:::+00044001::+9'\
DTM+137:20250115:102'\
RFF+Z13:GAS-NEG-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+52695662085::'\
UNT+8+MSG-001'\
UNZ+1+GAS-NEG-001'";

    let platform = Platform::with_all_profiles();
    let msg = platform
        .parse(invalid_bytes)
        .expect("parse must succeed even for an AHB-invalid message");

    let ref_date = time::Date::from_calendar_date(2026, time::Month::January, 15).unwrap();
    let report = msg
        .validate_on_date(ref_date)
        .expect("validate_on_date must not error");

    assert!(
        !report.is_valid(),
        "wrong BGM qualifier E99 must cause AHB validation failure for PID 44001"
    );
    assert!(
        report.errors().iter().any(|e| e
            .rule_id
            .as_deref()
            .is_some_and(|id| id == "AHB-44001-BGM-1001-Q")),
        "error list must contain rule AHB-44001-BGM-1001-Q; got: {:?}",
        report
            .errors()
            .iter()
            .map(|e| e.rule_id.as_deref())
            .collect::<Vec<_>>(),
    );
}
