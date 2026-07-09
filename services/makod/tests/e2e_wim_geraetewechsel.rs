//! End-to-end test: nMSB → NB WiM Gerätewechsel (PID 55042).
//!
//! A mock NB (Netzbetreiber) receives a UTILMD S2.1 PID 55042 from the incoming
//! Messstellenbetreiber (nMSB) and dispatches a positive or negative APERAK.
//!
//! # Protocol trace
//!
//! ```text
//!   nMSB ERP (wire fixture)                    NB ERP (MockNb)
//!   ──────────────────────────────────────────────────────────
//!                        ──── UTILMD 55042 ───►
//!                                               receive_utilmd(wire)
//!                                                 → wim_registry adapter
//!                                                 → state: ValidationPassed
//!                                               dispatch_aperak(positive=true)
//!                                                 → DispatchAperak
//!                                                 → state: AperakSent
//!                                               complete_device_change()
//!                                                 → Complete
//!                                                 → state: Completed
//!   ──────────────────────────────────────────────────────────
//! ```
//!
//! # APERAK outbox
//!
//! `DispatchAperak` enqueues exactly one `"Aperak"` [`OutboxMessage`] addressed
//! to the nMSB (the original UTILMD sender).  The payload carries the PID,
//! MeLo, `positive` flag, and `orig_message_ref` so the delivery worker can
//! render and transmit the wire-format APERAK without re-reading the event store.
//!
//! # Regulatory context
//!
//! - **PID 55042**: Anmeldung Messstellenbetrieb (nMSB → NB, WiM AHB S2.1)
//! - **APERAK Frist**: **5 Werktage** (BNetzA BK6-18-032, WiM)
//! - **Saturday counts as a Werktag**; Sunday and federal public holidays
//!   do not.  This is distinct from GPKE (24 wall-clock hours) and GeLi Gas
//!   (10 Werktage).
//! - The NB state machine:
//!   `New → Initiated → ValidationPassed → AperakSent → Completed` (positive)
//!   `New → Initiated → ValidationPassed → Rejected` (negative APERAK)
//!   `New → Initiated → Rejected` (validation failure)
//!
//! AHB validation is bypassed for the inbound `ReceiveUtilmd` because the
//! hand-crafted fixture does not satisfy all S2.1 profile rules.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    outbox::OutboxMessage,
    process::Process,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::{FormatVersion, WorkflowId},
};
use mako_wim::{DeviceChangeCommand, DeviceChangeState, WimDeviceChangeWorkflow};
use makod::adapters::wim_registry;

// ── Constants ──────────────────────────────────────────────────────────────────

const NMSB_ID: &str = "4012345000023"; // incoming Messstellenbetreiber (nMSB)
const NB_ID: &str = "9900357000004"; // Netzbetreiber (receiver)
const MELO_ID: &str = "DE0001000001234567890000000000001"; // Messlokations-ID
const DEVICE_ID: &str = "ZHR-12345678"; // Zählernummer / Geräte-ID
const FV: &str = "FV2025-10-01";

// ── UTILMD S2.1 PID 55042 wire fixture ────────────────────────────────────────
//
// Minimal EDIFACT UTILMD S2.1 Anmeldung Messstellenbetrieb (PID 55042).
// Direction: nMSB (sender NAD+MS) → NB (receiver NAD+MR).
//
// WiM uses Messlokationen (MeLo) rather than Marktlokationen (MaLo) as the
// IDE object.  The LOC+172 segment carries the Zählernummer (device ID).
//
// Source: WiM AHB (BDEW), FV2025-10-01, UTILMD S2.1.
const UTILMD_55042_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+WIM-2025-001'\
UNH+MSG-WIM-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055042::+9'\
DTM+137:20250115:102'\
RFF+Z13:WIM-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+DE0001000001234567890000000000001::'\
LOC+172+ZHR-12345678::'\
UNT+9+MSG-WIM-001'\
UNZ+1+WIM-2025-001'";

// ── Mock NB ERP backend ────────────────────────────────────────────────────────

/// Simulates the **Netzbetreiber's ERP** receiving and processing a WiM
/// Gerätewechsel Anmeldung.
///
/// Owns a single `WimDeviceChangeWorkflow` process backed by an in-memory store.
struct MockNb {
    process: Process<WimDeviceChangeWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("wim-device-change", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive nMSB's UTILMD 55042 wire bytes, adapt, and
    /// execute.
    ///
    /// AHB validation is forced to `true` — the minimal fixture does not satisfy
    /// all S2.1 profile rules; AHB conformance is tested separately.
    ///
    /// Asserts:
    /// - The adapter correctly extracts PID 55042, sender (nMSB), receiver (NB),
    ///   MeLo ID, and UNH message reference.
    /// - The UNH message reference is non-trivial (not empty, not `"1"`).
    async fn receive_utilmd(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse nMSB UTILMD 55042 wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must not be empty; got: {unh_ref:?}",
        );

        let cmd = wim_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt UTILMD 55042 to DeviceChangeCommand");

        let cmd = match cmd {
            DeviceChangeCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(
                    pid.as_u32(),
                    55042,
                    "adapter must extract PID 55042 from wire"
                );
                assert_eq!(
                    sender.as_str(),
                    NMSB_ID,
                    "adapter must extract sender GLN (nMSB) from NAD+MS"
                );
                assert_eq!(
                    receiver.as_str(),
                    NB_ID,
                    "adapter must extract receiver GLN (NB) from NAD+MR"
                );
                assert_eq!(
                    melo_id.as_str(),
                    MELO_ID,
                    "adapter must extract MeLo from IDE+Z19"
                );
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref for APERAK orig_message_ref",
                );
                // Override validation_passed to bypass AHB profile check.
                DeviceChangeCommand::ReceiveUtilmd {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    device_id: DeviceId::new(DEVICE_ID),
                    document_date,
                    message_ref,
                    validation_passed: true,
                    validation_errors: vec![],
                    received_at: time::OffsetDateTime::now_utc(),
                }
            }
            _ => panic!("expected DeviceChangeCommand::ReceiveUtilmd"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveUtilmd 55042");
    }

    /// ERP action: dispatch positive or negative APERAK.
    ///
    /// Returns the `OutboxMessage` entries queued atomically with the
    /// `AperakDispatched` event so callers can assert on the outbox payload.
    ///
    /// `positive = true`  → APERAK accepted → state `AperakSent`
    /// `positive = false` → APERAK rejected → state `Rejected`
    async fn dispatch_aperak(&self, positive: bool, reason: Option<&str>) -> Vec<OutboxMessage> {
        let (_, outbox) = self
            .process
            .execute_and_collect(DeviceChangeCommand::DispatchAperak {
                positive,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("NB: execute DispatchAperak");
        outbox
    }

    /// ERP action: record physical completion of the meter device change.
    ///
    /// This command is issued after the nMSB confirms the device has been
    /// physically swapped.  Transitions state from `AperakSent` to `Completed`.
    async fn complete_device_change(&self) {
        self.process
            .execute(DeviceChangeCommand::Complete {
                device_id: DeviceId::new(DEVICE_ID),
            })
            .await
            .expect("NB: execute Complete");
    }

    async fn state(&self) -> DeviceChangeState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// WiM Gerätewechsel — positive APERAK path (PID 55042 → AperakSent → Completed).
///
/// NB receives the UTILMD 55042, dispatches a positive APERAK within 5 Werktage
/// (BNetzA BK6-18-032), then records physical completion of the device change.
///
/// Per WiM AHB: Saturday counts as a Werktag; Sunday and federal public holidays
/// do not.  This is distinct from GPKE (24 wall-clock hours) and GeLi Gas
/// (10 Werktage).
#[tokio::test]
async fn e2e_wim_geraetewechsel_positive_aperak() {
    let nb = MockNb::new();

    // ── NB ERP: receive UTILMD 55042 ──────────────────────────────────────────
    nb.receive_utilmd(UTILMD_55042_BYTES).await;
    let state_after_receive = nb.state().await;
    assert!(
        matches!(state_after_receive, DeviceChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveUtilmd 55042; got: {state_after_receive:?}"
    );

    // ── NB ERP: dispatch positive APERAK (within 5 Werktage per BK6-18-032) ──
    let aperak_outbox = nb.dispatch_aperak(true, None).await;
    // ── Assert APERAK outbox entry ─────────────────────────────────────────────
    assert_eq!(
        aperak_outbox.len(),
        1,
        "positive DispatchAperak must enqueue exactly one APERAK outbox entry"
    );
    let aperak = &aperak_outbox[0];
    assert_eq!(aperak.message_type.as_ref(), "APERAK");
    assert_eq!(
        aperak.recipient.as_ref(),
        NMSB_ID,
        "Aperak must be addressed to the nMSB sender"
    );
    let payload = aperak
        .payload
        .as_object()
        .expect("Aperak payload must be a JSON object");
    assert_eq!(payload["pid"].as_u64().unwrap(), 55042);
    assert_eq!(payload["melo"].as_str().unwrap(), MELO_ID);
    assert!(
        payload["positive"].as_bool().unwrap(),
        "positive flag must be true"
    );
    assert_eq!(
        payload["orig_message_ref"].as_str().unwrap(),
        "MSG-WIM-001",
        "outbox must reference the original UTILMD message"
    );
    let state_after_aperak = nb.state().await;
    assert!(
        matches!(state_after_aperak, DeviceChangeState::AperakSent(_)),
        "NB must be AperakSent after positive DispatchAperak; got: {state_after_aperak:?}"
    );

    // ── NB ERP: record device change completion ────────────────────────────────
    nb.complete_device_change().await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, DeviceChangeState::Completed(_)),
        "NB must be Completed after device change; got: {final_state:?}"
    );
    if let DeviceChangeState::Completed(data) = final_state {
        assert_eq!(data.melo_id.as_str(), MELO_ID);
        assert_eq!(data.incoming_msb.as_str(), NMSB_ID);
        assert_eq!(data.grid_operator.as_str(), NB_ID);
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55042,
            "persisted data must carry PID 55042"
        );
    }
}

/// WiM Gerätewechsel — negative APERAK path (PID 55042 → Rejected).
///
/// NB receives the UTILMD 55042 but rejects the Anmeldung (e.g. the Messlokation
/// is unknown at the NB, or the nMSB is not authorized for this grid area).
///
/// Per WiM AHB: the negative APERAK must also be dispatched within 5 Werktage.
#[tokio::test]
async fn e2e_wim_geraetewechsel_negative_aperak() {
    let nb = MockNb::new();

    // ── NB ERP: receive UTILMD 55042 ──────────────────────────────────────────
    nb.receive_utilmd(UTILMD_55042_BYTES).await;
    assert!(
        matches!(nb.state().await, DeviceChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveUtilmd 55042"
    );

    // ── NB ERP: dispatch negative APERAK (nMSB not authorized) ───────────────
    let aperak_outbox = nb
        .dispatch_aperak(
            false,
            Some("nMSB nicht für diese Netzzuständigkeit registriert"),
        )
        .await;
    assert_eq!(
        aperak_outbox.len(),
        1,
        "negative DispatchAperak must also enqueue one APERAK outbox entry"
    );
    let aperak = &aperak_outbox[0];
    assert_eq!(aperak.message_type.as_ref(), "APERAK");
    assert_eq!(aperak.recipient.as_ref(), NMSB_ID);
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
            .contains("Netzzuständigkeit"),
        "outbox payload must include rejection reason"
    );

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, DeviceChangeState::Rejected { .. }),
        "NB must be Rejected after negative DispatchAperak; got: {final_state:?}"
    );
}

/// WiM Gerätewechsel — AHB validation failure (PID 55042, malformed UTILMD).
///
/// If the received UTILMD 55042 fails AHB validation (e.g. missing mandatory
/// segments), the workflow must immediately transition to `Rejected` without
/// requiring a `DispatchAperak` step.
///
/// This is the built-in validation failure path in `ReceiveUtilmd`: when
/// `validation_passed = false` the workflow emits `Initiated` followed by
/// `Rejected` in a single batch.
#[tokio::test]
async fn e2e_wim_geraetewechsel_ahb_validation_failure() {
    let nb = MockNb::new();

    // Construct the ReceiveUtilmd command directly with validation_passed=false
    // to simulate a malformed UTILMD 55042 that failed AHB profile checks.
    nb.process
        .execute(DeviceChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(55042).unwrap(),
            sender: MarktpartnerCode::new(NMSB_ID),
            receiver: MarktpartnerCode::new(NB_ID),
            melo_id: MeLo::new(MELO_ID),
            device_id: DeviceId::new(DEVICE_ID),
            document_date: "2025-01-15".to_owned(),
            message_ref: MessageRef::new("MSG-WIM-002"),
            validation_passed: false,
            validation_errors: vec![
                "UTILMD WiM segment RFF missing mandatory Z13 Auftragsreferenz".to_owned(),
            ],
            received_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .expect("ReceiveUtilmd with invalid message must not panic");

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, DeviceChangeState::Rejected { .. }),
        "invalid UTILMD 55042 must reach Rejected without DispatchAperak; got: {final_state:?}"
    );
}
