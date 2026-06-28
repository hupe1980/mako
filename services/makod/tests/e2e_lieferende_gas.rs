//! End-to-end test: LFN Gas ↔ GNB Lieferende Gas (PID 44002).
//!
//! A mock GNB (Gasnetzbetreiber) receives a UTILMD G 44002 from the LFN
//! (Gaslieferant) and dispatches a positive or negative APERAK response.
//!
//! # Protocol trace
//!
//! ```text
//!   LFN Gas ERP (wire fixture)                 GNB ERP (MockGnb)
//!   ──────────────────────────────────────────────────────────────
//!                        ──── UTILMD G 44002 ─►
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
//! to the LFN Gas (the original UTILMD G sender).
//!
//! # Regulatory context
//!
//! - **PID 44002**: Anfrage Lieferende Gas (LFN → GNB, GeLi Gas AHB G1.1)
//! - **APERAK Frist**: **10 Werktage** (BNetzA BK7 GeLi Gas 3.0, BK7-24-01-009)
//! - **Saturday counts as a Werktag**; Sunday and federal public holidays do not.
//!   This is distinct from GPKE (24 wall-clock hours) and WiM (5 Werktage).
//! - The GNB state machine:
//!   `New → Initiated → ValidationPassed → AperakSent → Active` (positive)
//!   `New → Initiated → ValidationPassed → Rejected` (negative APERAK)

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
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

// ── UTILMD G 44002 wire fixture ───────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD G1.1 Anfrage Lieferende Gas (PID 44002).
// Direction: LFN Gas (sender NAD+MS) → GNB (receiver NAD+MR).
//
// Source: GeLi Gas AHB G1.1 (BNetzA BK7), FV2025-10-01.
const UTILMD_44002_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0900+GAS-2025-002'\
UNH+MSG-002+UTILMD:D:11A:UN:G1.1'\
BGM+E03:::+00044002::+9'\
DTM+137:20250115:102'\
RFF+Z13:GAS-REF-002'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+52695662085::'\
UNT+8+MSG-002'\
UNZ+1+GAS-2025-002'";

// ── Mock GNB ERP backend ──────────────────────────────────────────────────────

/// Simulates the **Gasnetzbetreiber's ERP** receiving and processing a GeLi Gas
/// Lieferende request.
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

    /// ERP notification: receive LFN Gas UTILMD G 44002 wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true` — the hand-crafted fixture does not
    /// satisfy all G1.1 profile rules; AHB conformance is tested separately in
    /// `crates/edi-energy/tests/`.
    async fn receive_utilmd(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("GNB: parse LFN Gas UTILMD G wire");

        let unh_ref = raw.message_ref().to_owned();

        let cmd = geli_gas_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("GNB: adapt UTILMD G 44002 to GasSupplierChangeCommand");

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
                assert_eq!(pid.as_u32(), 44002, "adapter must extract PID 44002");
                assert_eq!(sender.as_str(), LFN_GAS_ID, "sender GLN must match NAD+MS");
                assert_eq!(receiver.as_str(), GNB_ID, "receiver GLN must match NAD+MR");
                assert_eq!(malo_id.as_str(), MALO_GAS_ID, "MaLo must match IDE+Z19");
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "message_ref must preserve UNH ref"
                );
                GasSupplierChangeCommand::ReceiveUtilmd {
                    pid,
                    sender,
                    receiver,
                    malo_id,
                    document_date,
                    message_ref,
                    validation_passed: true,
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected GasSupplierChangeCommand::ReceiveUtilmd"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("GNB: execute ReceiveUtilmd 44002");
    }

    /// ERP action: dispatch APERAK (positive or negative) for the received Lieferende.
    async fn dispatch_aperak(&self, positive: bool, reason: Option<&str>) -> Vec<OutboxMessage> {
        let (_, outbox) = self
            .process
            .execute_and_collect(GasSupplierChangeCommand::DispatchAperak {
                positive,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("GNB: execute DispatchAperak");
        outbox
    }

    /// ERP action: activate supply relationship (after positive APERAK).
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

// ── Tests ─────────────────────────────────────────────────────────────────────

/// GeLi Gas Lieferende — positive APERAK path (PID 44002 → AperakSent → Active).
///
/// The GNB receives a 44002 Anfrage Lieferende, validates it, sends a positive
/// APERAK (44005 Bestätigung), and transitions to Active.
///
/// BNetzA BK7 GeLi Gas: APERAK must be sent within **10 Werktage**.
/// Saturday counts as a Werktag; Sunday and federal holidays do not.
#[tokio::test]
async fn e2e_lieferende_gas_positive_aperak() {
    let gnb = MockGnb::new();

    gnb.receive_utilmd(UTILMD_44002_BYTES).await;
    assert!(
        matches!(
            gnb.state().await,
            GasSupplierChangeState::ValidationPassed(_)
        ),
        "GNB must be ValidationPassed after ReceiveUtilmd 44002"
    );

    let aperak_outbox = gnb.dispatch_aperak(true, None).await;
    assert_eq!(
        aperak_outbox.len(),
        1,
        "positive DispatchAperak must enqueue exactly one Aperak outbox entry"
    );
    let aperak = &aperak_outbox[0];
    assert_eq!(aperak.message_type.as_ref(), "Aperak");
    assert_eq!(
        aperak.recipient.as_ref(),
        LFN_GAS_ID,
        "APERAK must be addressed to LFN Gas sender"
    );
    let payload = aperak
        .payload
        .as_object()
        .expect("Aperak payload must be a JSON object");
    assert!(
        payload["positive"].as_bool().unwrap(),
        "positive flag must be true for acceptance"
    );
    assert_eq!(
        payload["pid"].as_u64().unwrap(),
        44002_u64,
        "outbox payload must carry PID 44002"
    );
    assert_eq!(payload["malo"].as_str().unwrap(), MALO_GAS_ID);

    assert!(
        matches!(gnb.state().await, GasSupplierChangeState::AperakSent(_)),
        "GNB must be AperakSent after positive DispatchAperak"
    );

    gnb.activate().await;
    let final_state = gnb.state().await;
    assert!(
        matches!(final_state, GasSupplierChangeState::Active(_)),
        "GNB must be Active after Activate; got: {final_state:?}"
    );
    if let GasSupplierChangeState::Active(data) = final_state {
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            44002,
            "persisted data must carry PID 44002"
        );
        assert_eq!(data.new_supplier.as_str(), LFN_GAS_ID);
        assert_eq!(data.gas_operator.as_str(), GNB_ID);
        assert_eq!(data.malo_id.as_str(), MALO_GAS_ID);
    }
}

/// GeLi Gas Lieferende — negative APERAK path (PID 44002 → Rejected).
///
/// The GNB rejects the Lieferende request (e.g. Marktlokation unknown).
/// A negative APERAK outbox entry is enqueued and the workflow reaches Rejected.
#[tokio::test]
async fn e2e_lieferende_gas_negative_aperak() {
    let gnb = MockGnb::new();

    gnb.receive_utilmd(UTILMD_44002_BYTES).await;
    assert!(
        matches!(
            gnb.state().await,
            GasSupplierChangeState::ValidationPassed(_)
        ),
        "GNB must be ValidationPassed after ReceiveUtilmd 44002"
    );

    let aperak_outbox = gnb
        .dispatch_aperak(
            false,
            Some("Marktlokation nicht im Netzgebiet des GNB registriert"),
        )
        .await;
    assert_eq!(
        aperak_outbox.len(),
        1,
        "negative DispatchAperak must enqueue one Aperak outbox entry"
    );
    let aperak = &aperak_outbox[0];
    assert_eq!(aperak.message_type.as_ref(), "Aperak");
    assert_eq!(aperak.recipient.as_ref(), LFN_GAS_ID);
    let payload = aperak
        .payload
        .as_object()
        .expect("Aperak payload must be a JSON object");
    assert!(
        !payload["positive"].as_bool().unwrap(),
        "positive flag must be false for rejection"
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

/// GeLi Gas Lieferende — AHB validation failure (PID 44002 → immediate Rejected).
///
/// If the received UTILMD G 44002 fails AHB validation, the workflow transitions
/// to `Rejected` immediately after `ReceiveUtilmd`, without a `DispatchAperak`.
#[tokio::test]
async fn e2e_lieferende_gas_ahb_validation_failure() {
    let gnb = MockGnb::new();

    gnb.process
        .execute(GasSupplierChangeCommand::ReceiveUtilmd {
            pid: mako_engine::types::Pruefidentifikator::new(44002).unwrap(),
            sender: mako_engine::types::MarktpartnerCode::new(LFN_GAS_ID),
            receiver: mako_engine::types::MarktpartnerCode::new(GNB_ID),
            malo_id: mako_engine::types::MaLo::new(MALO_GAS_ID),
            document_date: "2025-01-15".to_owned(),
            message_ref: mako_engine::types::MessageRef::new("MSG-GAS-003"),
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
        "invalid UTILMD G 44002 must reach Rejected without DispatchAperak; got: {final_state:?}"
    );
}
