//! End-to-end test: NB → LFN/MSB Anweisung Sperrung (PID 55555).
//!
//! The NB (Netzbetreiber) initiates a disconnection order; the receiving
//! party (MockLfn, acting as Lieferant or Messstellenbetreiber) processes
//! the UTILMD 55555 and confirms or denies execution.
//!
//! # Protocol trace
//!
//! ```text
//!   NB ERP (wire fixture)                      LFN/MSB ERP (MockLfn)
//!   ──────────────────────────────────────────────────────────────────
//!                        ──── UTILMD 55555 ────►
//!                                               receive_sperrung(wire)
//!                                                 → adapter: ReceiveSperrung
//!                                                 → state: ValidationPassed
//!                                               confirm_execution(true)
//!                                                 → BestaetigueSperrung
//!                                                 → state: Ausgefuehrt
//!   ──────────────────────────────────────────────────────────────────
//! ```
//!
//! The NB side does **not** have a `mako-engine` workflow for sending the
//! Sperrung order — the NB's internal system generates the EDIFACT UTILMD
//! 55555 directly.  This test exercises only the receiving (LFN/MSB) side of
//! the protocol.
//!
//! # Regulatory context
//!
//! - **PID 55555**: Anweisung Sperrung (NB → LFN/MSB, Strom)
//! - **Deadline**: 24 wall-clock hours for execution confirmation
//!   (BNetzA BK6-22-024).
//! - The workflow state machine:
//!   `New → AnweisungErhalten → ValidationPassed → Ausgefuehrt` (success)
//!   `New → AnweisungErhalten → ValidationPassed → Rejected` (cannot execute)
//!
//! AHB validation is bypassed for the inbound `ReceiveSperrung` because the
//! hand-crafted fixture does not satisfy all S2.1 profile rules.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::MessageRef,
    version::{FormatVersion, WorkflowId},
};
use mako_gpke::{GpkeSperrungWorkflow, SperrungCommand, SperrungState};
use makod::adapters::gpke_sperrung_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const NB_ID: &str = "9900357000004"; // Netzbetreiber (Sperrung initiator)
const LFN_ID: &str = "4012345000023"; // Lieferant / Messstellenbetreiber
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── UTILMD 55555 wire fixture ──────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD S2.1 Anweisung Sperrung (PID 55555).
// Direction: NB (sender NAD+MS) → LFN/MSB (receiver NAD+MR).
//
// This fixture is equivalent to what the NB's internal EDI system would
// produce. The UNH message reference ("MSG-SPERR-001") is intentionally
// non-trivial so the adapter's reference-preservation logic is exercised.
const UTILMD_55555_BYTES: &[u8] = b"\
UNB+UNOC:3+9900357000004:14+4012345000023:14+250115:0800+SPERR-2025-001'\
UNH+MSG-SPERR-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055555::+9'\
DTM+137:20250115:102'\
RFF+Z13:SPERR-REF-001'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::293'\
IDE+Z19+51238696781::'\
UNT+8+MSG-SPERR-001'\
UNZ+1+SPERR-2025-001'";

// ── Mock LFN/MSB ERP backend ───────────────────────────────────────────────────

/// Simulates the **Lieferant/MSB ERP** receiving and acting on a Sperrung order.
///
/// Owns a single `GpkeSperrungWorkflow` process backed by an in-memory store.
struct MockLfn {
    process: Process<GpkeSperrungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockLfn {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LFN_ID),
                WorkflowId::new("gpke-sperrung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive NB's UTILMD 55555 wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true` — the fixture does not satisfy all
    /// S2.1 profile rules; AHB conformance is tested separately.
    ///
    /// Asserts that the adapter preserves the UNH message reference so that
    /// subsequent APERAK `orig_message_ref` can echo it back to the NB.
    async fn receive_sperrung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LFN: parse NB UTILMD 55555 wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty() && unh_ref != "1",
            "UNH message_ref must be non-trivial; got: {unh_ref:?}",
        );

        let cmd = gpke_sperrung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LFN: adapt UTILMD 55555 to SperrungCommand");

        // Validate adapter field extraction, then override validation_passed
        // to bypass AHB profile rules for this minimal fixture.
        let cmd = match cmd {
            SperrungCommand::ReceiveSperrung {
                pid,
                sender,
                location_id,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(
                    pid.as_u32(),
                    55555,
                    "adapter must extract PID 55555 from wire"
                );
                assert_eq!(
                    sender.as_str(),
                    NB_ID,
                    "adapter must extract sender GLN (NB) from NAD+MS"
                );
                assert_eq!(
                    location_id.as_str(),
                    MALO_ID,
                    "adapter must extract MaLo from IDE+Z19"
                );
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref",
                );
                SperrungCommand::ReceiveSperrung {
                    pid,
                    sender,
                    location_id,
                    document_date,
                    message_ref,
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected SperrungCommand::ReceiveSperrung"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("LFN: execute ReceiveSperrung");
    }

    /// ERP action: confirm or deny the execution of the Sperrung.
    ///
    /// `durchgefuehrt = true`  → execution succeeded → state `Ausgefuehrt`.
    /// `durchgefuehrt = false` → execution failed (e.g. meter access denied)
    ///                           → state `Rejected`.
    async fn confirm_execution(&self, durchgefuehrt: bool, reason: Option<&str>) {
        self.process
            .execute(SperrungCommand::BestaetigueSperrung {
                durchgefuehrt,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("LFN: execute BestaetigueSperrung");
    }

    async fn state(&self) -> SperrungState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Anweisung Sperrung — execution success path (PID 55555 → Ausgefuehrt).
///
/// NB sends UTILMD 55555; LFN/MSB receives the order, confirms successful
/// execution (e.g. smart meter instructed, meter physically disconnected).
///
/// Per BNetzA BK6-22-024 the execution confirmation must be delivered within
/// 24 wall-clock hours.
#[tokio::test]
async fn e2e_sperrung_execution_success() {
    let lfn = MockLfn::new();

    // ── LFN/MSB ERP: receive Sperrung order ───────────────────────────────────
    lfn.receive_sperrung(UTILMD_55555_BYTES).await;
    assert!(
        matches!(lfn.state().await, SperrungState::ValidationPassed(_)),
        "LFN must be ValidationPassed after ReceiveSperrung"
    );

    // ── LFN/MSB ERP: confirm successful execution ─────────────────────────────
    lfn.confirm_execution(true, None).await;

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, SperrungState::Ausgefuehrt(_)),
        "LFN must be Ausgefuehrt after successful BestaetigueSperrung; got: {final_state:?}"
    );
    if let SperrungState::Ausgefuehrt(data) = final_state {
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.sender.as_str(), NB_ID);
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55555,
            "persisted data must carry PID 55555"
        );
    }
}

/// Anweisung Sperrung — execution failure path (PID 55555 → Rejected).
///
/// NB sends UTILMD 55555; LFN/MSB receives the order but cannot execute the
/// disconnection (e.g. physical access to meter denied, property blocked).
///
/// The workflow transitions to `Rejected` with a reason string that the NB
/// can surface to its operator for manual follow-up.
#[tokio::test]
async fn e2e_sperrung_execution_failure() {
    let lfn = MockLfn::new();

    // ── LFN/MSB ERP: receive Sperrung order ───────────────────────────────────
    lfn.receive_sperrung(UTILMD_55555_BYTES).await;
    assert!(
        matches!(lfn.state().await, SperrungState::ValidationPassed(_)),
        "LFN must be ValidationPassed after ReceiveSperrung"
    );

    // ── LFN/MSB ERP: deny execution (meter access blocked) ───────────────────
    lfn.confirm_execution(false, Some("Zähler nicht zugänglich — Zugang verweigert"))
        .await;

    let final_state = lfn.state().await;
    match &final_state {
        SperrungState::Rejected { reason } => {
            assert!(
                reason.contains("Zähler nicht zugänglich"),
                "rejection reason must carry the operator message; got: {reason:?}"
            );
        }
        _ => panic!("LFN must be Rejected after failed BestaetigueSperrung; got: {final_state:?}"),
    }
}

/// Anweisung Sperrung — validation failure (PID 55555, malformed UTILMD).
///
/// If the received UTILMD 55555 fails AHB validation (e.g. missing mandatory
/// segments), the workflow must immediately transition to `Rejected` without
/// requiring a `BestaetigueSperrung` step.
///
/// This is the built-in rejection path in `ReceiveSperrung`: the command emits
/// both `AnweisungErhalten` and `Rejected` events in a single batch when
/// `validation_passed = false`.
#[tokio::test]
async fn e2e_sperrung_validation_failure() {
    let lfn = MockLfn::new();

    // Construct the ReceiveSperrung command directly with validation_passed=false
    // to simulate a malformed UTILMD 55555 that failed AHB profile checks.
    lfn.process
        .execute(SperrungCommand::ReceiveSperrung {
            pid: mako_engine::types::Pruefidentifikator::new(55555).unwrap(),
            sender: mako_engine::types::MarktpartnerCode::new(NB_ID),
            location_id: mako_engine::types::MaLo::new(MALO_ID),
            document_date: "2025-01-15".to_owned(),
            message_ref: MessageRef::new("MSG-SPERR-002"),
            validation_passed: false,
            validation_errors: vec!["SG4 IDE segment missing mandatory Z19 qualifier".to_owned()],
        })
        .await
        .expect("ReceiveSperrung with invalid message must not panic");

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, SperrungState::Rejected { .. }),
        "invalid Sperrung must reach Rejected without BestaetigueSperrung; got: {final_state:?}"
    );
}
