//! End-to-end test: LFN Gas ↔ GNB Lieferbeginn Gas (PID 44001).
//!
//! A mock GNB (Gasnetzbetreiber) receives a UTILMD G 44001 from the LFN
//! (Gaslieferant) and dispatches a positive or negative APERAK response.
//!
//! # Protocol trace
//!
//! ```text
//!   LFN Gas ERP (wire fixture)                 GNB ERP (MockGnb)
//!   ──────────────────────────────────────────────────────────────
//!                        ──── UTILMD G 44001 ─►
//!                                               receive_utilmd(wire)
//!                                                 → adapter: ReceiveUtilmd
//!                                                 → state: ValidationPassed
//!                                               dispatch_aperak(positive=true)
//!                                                 → DispatchAperak
//!                                                 → state: AperakSent
//!                                               activate()
//!                                                 → Activate
//!                                                 → state: Active
//!   ──────────────────────────────────────────────────────────────
//! ```
//!
//! # APERAK outbox
//!
//! `DispatchAperak` enqueues exactly one `"Aperak"` [`OutboxMessage`] addressed
//! to the LFN Gas (the original UTILMD G sender).  The payload carries the PID,
//! MaLo, `positive` flag, and `orig_message_ref` so the delivery worker can
//! render and transmit the wire-format APERAK without re-reading the event store.
//!
//! # Regulatory context
//!
//! - **PID 44001**: Anfrage Lieferbeginn Gas (LFN → GNB, GeLi Gas AHB G1.1)
//! - **APERAK Frist**: **10 Werktage** (BNetzA BK7 GeLi Gas)
//! - **Saturday counts as a Werktag**; Sunday and federal public holidays
//!   do not.  This is distinct from GPKE (24 wall-clock hours) and WiM
//!   (5 Werktage).
//! - The GNB state machine:
//!   `New → Initiated → ValidationPassed → AperakSent → Active` (positive)
//!   `New → Initiated → ValidationPassed → Rejected` (negative APERAK)

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    error::EngineError,
    event_store::InMemoryEventStore,
    ids::TenantId,
    outbox::OutboxMessage,
    process::Process,
    version::{FormatVersion, WorkflowId},
};
use mako_geli_gas::{
    GasSupplierChangeCommand, GasSupplierChangeState, GeliGasSupplierChangeWorkflow,
};
use makod::adapters::geli_gas_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const LFN_GAS_ID: &str = "4012345000023"; // Gaslieferant (sender of UTILMD G)
const GNB_ID: &str = "9900357000004"; // Gasnetzbetreiber (receiver)
const MALO_GAS_ID: &str = "52695662085"; // Marktlokations-ID (Gas)
const FV: &str = "FV2025-10-01";

// ── UTILMD G 44001 wire fixture ────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD G1.1 Anfrage Lieferbeginn Gas (PID 44001).
// Direction: LFN Gas (sender NAD+MS) → GNB (receiver NAD+MR).
//
// Source: GeLi Gas AHB G1.1 (BNetzA BK7), FV2025-10-01.
// This is the same fixture used in `crates/mako-geli-gas/tests/utilmd.rs`.
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

// ── Mock GNB ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Gasnetzbetreiber's ERP** receiving and processing a GeLi Gas
/// Lieferbeginn request.
///
/// Owns a single `GeliGasSupplierChangeWorkflow` process backed by an in-memory
/// store.
struct MockGnb {
    process: Process<GeliGasSupplierChangeWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockGnb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(GNB_ID),
                WorkflowId::new("geli-gas-supplier-change", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive LFN Gas's UTILMD G 44001 wire bytes, adapt,
    /// and execute.
    ///
    /// AHB validation is forced to `true` — the fixture does not satisfy all
    /// G1.1 profile rules; AHB conformance is tested separately in
    /// `crates/edi-energy/tests/`.
    ///
    /// Asserts that the adapter correctly extracts PID, sender, receiver, MaLo
    /// ID, and message reference from the wire bytes.
    async fn receive_utilmd(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("GNB: parse LFN Gas UTILMD G wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = geli_gas_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("GNB: adapt UTILMD G 44001 to GasSupplierChangeCommand");

        let cmd = match cmd {
            GasSupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(
                    pid.as_u32(),
                    44001,
                    "adapter must extract PID 44001 from wire"
                );
                assert_eq!(
                    sender.as_str(),
                    LFN_GAS_ID,
                    "adapter must extract sender GLN (LFN Gas) from NAD+MS"
                );
                assert_eq!(
                    receiver.as_str(),
                    GNB_ID,
                    "adapter must extract receiver GLN (GNB) from NAD+MR"
                );
                assert_eq!(
                    malo_id.as_str(),
                    MALO_GAS_ID,
                    "adapter must extract MaLo from IDE+Z19"
                );
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref for APERAK orig_message_ref",
                );
                GasSupplierChangeCommand::ReceiveUtilmd {
                    pid,
                    sender,
                    receiver,
                    malo_id,
                    document_date,
                    process_date: String::new(),
                    message_ref,
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected GasSupplierChangeCommand::ReceiveUtilmd"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("GNB: execute ReceiveUtilmd 44001");
    }

    /// ERP action: dispatch positive or negative Antwort.
    ///
    /// Returns the `OutboxMessage` entries queued atomically with the
    /// `AntwortGesendet` event so callers can assert on the outbox payload.
    ///
    /// `positive = true`  → Antwort accepted → state `AntwortGesendet`
    /// `positive = false` → Antwort rejected → state `Rejected`
    async fn dispatch_aperak(&self, positive: bool, reason: Option<&str>) -> Vec<OutboxMessage> {
        let (_, outbox) = self
            .process
            .execute_and_collect(GasSupplierChangeCommand::SendAntwort {
                accepted: positive,
                reason: reason.map(str::to_owned),
                obligations: vec![],
            })
            .await
            .expect("GNB: execute SendAntwort");
        outbox
    }

    /// ERP action: activate the supply relationship (after positive Antwort).
    ///
    /// Transitions state from `AntwortGesendet` to `Active`.
    async fn activate(&self) {
        self.process
            .execute(GasSupplierChangeCommand::Activate)
            .await
            .expect("GNB: execute Activate");
    }

    async fn state(&self) -> GasSupplierChangeState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// GeLi Gas Lieferbeginn — positive Antwort path (PID 44001 → AntwortGesendet → Active).
///
/// GNB receives the UTILMD G 44001 from the LFN, dispatches a positive APERAK,
/// then activates the supply relationship.
///
/// BNetzA BK7 GeLi Gas: APERAK must be sent within **10 Werktage**.
/// Saturday counts as a Werktag; Sunday and federal holidays do not.
#[tokio::test]
async fn e2e_lieferbeginn_gas_positive_aperak() {
    let gnb = MockGnb::new();

    // ── GNB ERP: receive UTILMD G 44001 ───────────────────────────────────────
    gnb.receive_utilmd(UTILMD_44001_BYTES).await;
    let state_after_receive = gnb.state().await;
    assert!(
        matches!(
            state_after_receive,
            GasSupplierChangeState::ValidationPassed(_)
        ),
        "GNB must be ValidationPassed after ReceiveUtilmd 44001; got: {state_after_receive:?}"
    );

    // ── GNB ERP: dispatch positive APERAK (within 10 Werktage per BK7) ───────
    let aperak_outbox = gnb.dispatch_aperak(true, None).await;
    // ── Assert Antwort outbox entry ───────────────────────────────────────────
    assert_eq!(
        aperak_outbox.len(),
        1,
        "positive SendAntwort must enqueue exactly one UtilmdAntwort outbox entry"
    );
    let aperak = &aperak_outbox[0];
    assert_eq!(aperak.message_type.as_ref(), "UtilmdAntwort");
    assert_eq!(
        aperak.recipient.as_ref(),
        LFN_GAS_ID,
        "UtilmdAntwort must be addressed to the LFN Gas sender"
    );
    let payload = aperak
        .payload
        .as_object()
        .expect("UtilmdAntwort payload must be a JSON object");
    assert_eq!(payload["anfrage_pid"].as_u64().unwrap(), 44001);
    assert_eq!(payload["malo_id"].as_str().unwrap(), MALO_GAS_ID);
    assert!(
        payload["accepted"].as_bool().unwrap(),
        "accepted flag must be true"
    );
    assert_eq!(
        payload["orig_message_ref"].as_str().unwrap(),
        "MSG-001",
        "outbox must reference the original UTILMD G message"
    );
    let state_after_aperak = gnb.state().await;
    assert!(
        matches!(
            state_after_aperak,
            GasSupplierChangeState::AntwortGesendet { .. }
        ),
        "GNB must be AntwortGesendet after positive SendAntwort; got: {state_after_aperak:?}"
    );

    // ── GNB ERP: activate supply relationship ─────────────────────────────────
    gnb.activate().await;

    let final_state = gnb.state().await;
    assert!(
        matches!(final_state, GasSupplierChangeState::Active(_)),
        "GNB must be Active after Activate; got: {final_state:?}"
    );
    if let GasSupplierChangeState::Active(data) = final_state {
        assert_eq!(data.malo_id.as_str(), MALO_GAS_ID);
        assert_eq!(data.sender.as_str(), LFN_GAS_ID);
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            44001,
            "persisted data must carry PID 44001"
        );
    }
}

/// GeLi Gas Lieferbeginn — negative APERAK path (PID 44001 → Rejected).
///
/// GNB receives the UTILMD G 44001 from the LFN but the data validation fails
/// (e.g. the Marktlokation is unknown at the GNB).  The GNB dispatches a
/// negative APERAK and the workflow transitions to `Rejected`.
///
/// BNetzA BK7 GeLi Gas: negative APERAK must also be sent within 10 Werktage.
#[tokio::test]
async fn e2e_lieferbeginn_gas_negative_aperak() {
    let gnb = MockGnb::new();

    // ── GNB ERP: receive UTILMD G 44001 ───────────────────────────────────────
    gnb.receive_utilmd(UTILMD_44001_BYTES).await;
    assert!(
        matches!(
            gnb.state().await,
            GasSupplierChangeState::ValidationPassed(_)
        ),
        "GNB must be ValidationPassed after ReceiveUtilmd 44001"
    );

    // ── GNB ERP: dispatch negative APERAK (Marktlokation unbekannt) ───────────
    let aperak_outbox = gnb
        .dispatch_aperak(
            false,
            Some("Marktlokation nicht im Netzgebiet des GNB registriert"),
        )
        .await;
    assert_eq!(
        aperak_outbox.len(),
        1,
        "negative SendAntwort must also enqueue one UtilmdAntwort outbox entry"
    );
    let aperak = &aperak_outbox[0];
    assert_eq!(aperak.message_type.as_ref(), "UtilmdAntwort");
    assert_eq!(aperak.recipient.as_ref(), LFN_GAS_ID);
    let payload = aperak
        .payload
        .as_object()
        .expect("UtilmdAntwort payload must be a JSON object");
    assert!(
        !payload["accepted"].as_bool().unwrap(),
        "accepted flag must be false for rejection"
    );
    assert!(
        payload["reason"]
            .as_str()
            .unwrap()
            .contains("Marktlokation"),
        "outbox payload must include rejection reason"
    );
    let final_state = gnb.state().await;
    assert!(
        matches!(final_state, GasSupplierChangeState::Rejected { .. }),
        "GNB must be Rejected after negative DispatchAperak; got: {final_state:?}"
    );
}

/// GeLi Gas Lieferbeginn — validation failure (AHB check fails, PID 44001).
///
/// If the received UTILMD G 44001 fails AHB validation, the workflow
/// transitions to `Rejected` immediately after `ReceiveUtilmd` — no
/// `DispatchAperak` step is required.
///
/// This simulates a malformed EDIFACT message (e.g. missing mandatory segments)
/// reaching the GNB adapter in production.
#[tokio::test]
async fn e2e_lieferbeginn_gas_ahb_validation_failure() {
    let gnb = MockGnb::new();

    // Construct the ReceiveUtilmd command directly with validation_passed=false.
    gnb.process
        .execute(GasSupplierChangeCommand::ReceiveUtilmd {
            pid: mako_engine::types::Pruefidentifikator::new(44001).unwrap(),
            sender: mako_engine::types::MarktpartnerCode::new(LFN_GAS_ID),
            receiver: mako_engine::types::MarktpartnerCode::new(GNB_ID),
            malo_id: mako_engine::types::MaLo::new(MALO_GAS_ID),
            document_date: "2025-01-15".to_owned(),
            process_date: String::new(),
            message_ref: mako_engine::types::MessageRef::new("MSG-GAS-002"),
            validation_passed: false,
            validation_errors: vec![
                "UTILMD G segment IDE missing mandatory Z18 Marktlokation reference".to_owned(),
            ],
        })
        .await
        .expect("ReceiveUtilmd with invalid message must not panic");

    let final_state = gnb.state().await;
    assert!(
        matches!(final_state, GasSupplierChangeState::Rejected { .. }),
        "invalid UTILMD G must reach Rejected without DispatchAperak; got: {final_state:?}"
    );
}

/// GeLi Gas Lieferbeginn — duplicate message rejected (PID 44001 idempotency guard).
///
/// The engine must reject a second `ReceiveUtilmd` with an `InvalidState` error
/// once the workflow has already left `New`, preserving the original state.
/// This guards against double-delivery at the AS4 / ingest boundary.
#[tokio::test]
async fn e2e_lieferbeginn_gas_duplicate_message_rejected() {
    let gnb = MockGnb::new();

    // ── First delivery (accepted) ─────────────────────────────────────────────
    gnb.receive_utilmd(UTILMD_44001_BYTES).await;
    let state_after_first = gnb.state().await;
    assert!(
        matches!(
            state_after_first,
            GasSupplierChangeState::ValidationPassed(_)
        ),
        "first ReceiveUtilmd must leave state ValidationPassed; got: {state_after_first:?}"
    );

    // ── Second delivery (duplicate — must be rejected) ────────────────────────
    let err = gnb
        .process
        .execute(GasSupplierChangeCommand::ReceiveUtilmd {
            pid: mako_engine::types::Pruefidentifikator::new(44001).unwrap(),
            sender: mako_engine::types::MarktpartnerCode::new(LFN_GAS_ID),
            receiver: mako_engine::types::MarktpartnerCode::new(GNB_ID),
            malo_id: mako_engine::types::MaLo::new(MALO_GAS_ID),
            document_date: "2025-01-15".to_owned(),
            process_date: String::new(),
            message_ref: mako_engine::types::MessageRef::new("MSG-GAS-DUP"),
            validation_passed: true,
            validation_errors: vec![],
        })
        .await
        .expect_err("second ReceiveUtilmd must be rejected");

    assert!(
        err.is_workflow_error(),
        "duplicate ReceiveUtilmd must return EngineError::Workflow; got: {err:?}"
    );
    if let EngineError::Workflow(wf_err) = &err {
        assert!(
            wf_err.is_invalid_state(),
            "WorkflowError for duplicate must be InvalidState; got: {wf_err:?}"
        );
    }

    // State must be unchanged after the rejected duplicate.
    assert!(
        matches!(
            gnb.state().await,
            GasSupplierChangeState::ValidationPassed(_)
        ),
        "state must still be ValidationPassed after rejected duplicate"
    );
}
