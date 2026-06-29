//! End-to-end test: nMSB → NB WiM Stornierung (PID 39002).
//!
//! A mock NB (Netzbetreiber / aMSB) receives an ORDCHG 39002 (Stornierung
//! Sperr-/Entsperrauftrag) from the requesting party and dispatches a positive
//! or negative ORDRSP response.
//!
//! # Protocol trace
//!
//! ```text
//!   nMSB ERP (wire fixture)                     NB/aMSB ERP (MockNb)
//!   ───────────────────────────────────────────────────────────────────
//!                        ──── ORDCHG 39002 ───►
//!                                                receive_ordchg(wire)
//!                                                  → adapter: ReceiveOrdchg
//!                                                  → state: ValidationPassed
//!                                                accept()
//!                                                  → Accept
//!                                                  → state: Bestaetigt
//!   ───────────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 39002**: Stornierung der Bestellung (ORDCHG, nMSB → NB)
//! - **ORDRSP Frist**: **5 Werktage** (BNetzA BK6-18-032)
//! - **Saturday counts as a Werktag**; Sunday and public holidays do not.
//! - NB state machine:
//!   `New → StornierungReceived → ValidationPassed → Bestaetigt` (positive)
//!   `New → StornierungReceived → ValidationPassed → Abgelehnt` (negative)

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::MessageRef,
    version::{FormatVersion, WorkflowId},
};
use mako_wim::{
    STORNIERUNG_DEADLINE_LABEL, StornierungCommand, StornierungState, WimStornierungWorkflow,
};
use makod::adapters::wim_stornierung_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const NMSB_ID: &str = "9900357000004"; // nMSB (sender of ORDCHG)
const NB_ID: &str = "4012345000023"; // NB/aMSB (receiver)
const MELO_ID: &str = "E0000000000000000001"; // Messlokation EIC code
const FV: &str = "FV2025-10-01";

// ── ORDCHG 39002 wire fixture ─────────────────────────────────────────────────
//
// Minimal EDIFACT ORDCHG — Stornierung der Bestellung (PID 39002).
// Direction: nMSB (sender NAD+MS) → NB/aMSB (receiver NAD+MR).
// BGM code Z51 + PID in element 2; UNH release 1.1 per BDEW WiM AHB.
const ORDCHG_39002_BYTES: &[u8] = b"\
UNB+UNOC:3+9900357000004:14+4012345000023:14+250115:0800+WIM-STOR-001'\
UNH+MSG-001+ORDCHG:D:20B:UN:1.1'\
BGM+Z51+00039002'\
DTM+137:20250115:102'\
RFF+Z13:WIM-ORDER-001'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::293'\
IDE+Z19+E0000000000000000001::'\
UNT+8+MSG-001'\
UNZ+1+WIM-STOR-001'";

// ── Mock NB/aMSB ERP backend ──────────────────────────────────────────────────

/// Simulates the **NB/aMSB ERP** receiving a WiM Stornierung ORDCHG.
struct MockNb {
    process: Process<WimStornierungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("wim-stornierung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive ORDCHG 39002 wire bytes, adapt, and execute.
    ///
    /// AHB validation is bypassed — the minimal fixture does not satisfy all
    /// profile rules; AHB conformance is tested separately in `edi-energy` tests.
    async fn receive_ordchg(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse ORDCHG wire bytes");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = wim_stornierung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt ORDCHG 39002 to StornierungCommand");

        let cmd = match cmd {
            StornierungCommand::ReceiveOrdchg {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                cancelled_ref,
                ..
            } => {
                assert_eq!(pid.as_u32(), 39002, "adapter must extract PID 39002");
                assert_eq!(sender.as_str(), NMSB_ID, "sender must be nMSB GLN");
                assert_eq!(receiver.as_str(), NB_ID, "receiver must be NB GLN");
                assert_eq!(melo_id.as_str(), MELO_ID, "MeLo must match IDE+Z19");
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref",
                );
                assert_eq!(
                    cancelled_ref.as_ref().map(MessageRef::as_str),
                    Some("WIM-ORDER-001"),
                    "cancelled_ref must be extracted from RFF+Z13",
                );
                StornierungCommand::ReceiveOrdchg {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    document_date,
                    message_ref,
                    cancelled_ref,
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected StornierungCommand::ReceiveOrdchg"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveOrdchg 39002");
    }

    /// ERP action: accept the Stornierung (dispatch ORDRSP 19013).
    async fn accept(&self) {
        self.process
            .execute(StornierungCommand::Accept {
                response_ref: MessageRef::new("WIM-STOR-RESP-001"),
            })
            .await
            .expect("NB: execute Accept");
    }

    /// ERP action: reject the Stornierung (dispatch ORDRSP 19014).
    async fn reject(&self, reason: &str) {
        self.process
            .execute(StornierungCommand::Reject {
                reason: reason.to_owned(),
                response_ref: MessageRef::new("WIM-STOR-RESP-001"),
            })
            .await
            .expect("NB: execute Reject");
    }

    async fn state(&self) -> StornierungState {
        self.process.state().await.expect("must load state")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn wim_stornierung_positive_acceptance() {
    let nb = MockNb::new();

    nb.receive_ordchg(ORDCHG_39002_BYTES).await;

    let state = nb.state().await;
    assert!(
        matches!(state, StornierungState::ValidationPassed(_)),
        "after ReceiveOrdchg, state must be ValidationPassed; got: {state:?}",
    );

    nb.accept().await;

    let state = nb.state().await;
    assert!(
        matches!(state, StornierungState::Bestaetigt(_)),
        "after Accept, state must be Bestaetigt; got: {state:?}",
    );
}

#[tokio::test]
async fn wim_stornierung_negative_rejection() {
    let nb = MockNb::new();

    nb.receive_ordchg(ORDCHG_39002_BYTES).await;
    nb.reject("Auftrag nicht stornierbar").await;

    let state = nb.state().await;
    assert!(
        matches!(state, StornierungState::Abgelehnt { .. }),
        "after Reject, state must be Abgelehnt; got: {state:?}",
    );
}

#[tokio::test]
async fn wim_stornierung_validation_failure_rejects() {
    let nb = MockNb::new();

    let raw = nb.platform.parse(ORDCHG_39002_BYTES).expect("parse");
    let cmd = wim_stornierung_registry()
        .dispatch(&raw as &dyn Any, &nb.fv)
        .expect("adapt");

    // Override to simulate validation failure.
    let cmd = match cmd {
        StornierungCommand::ReceiveOrdchg {
            pid,
            sender,
            receiver,
            melo_id,
            document_date,
            message_ref,
            cancelled_ref,
            ..
        } => StornierungCommand::ReceiveOrdchg {
            pid,
            sender,
            receiver,
            melo_id,
            document_date,
            message_ref,
            cancelled_ref,
            validation_passed: false,
            validation_errors: vec!["missing required DTM".to_owned()],
        },
        _ => panic!("expected ReceiveOrdchg"),
    };

    nb.process.execute(cmd).await.expect("execute");

    let state = nb.state().await;
    assert!(
        matches!(state, StornierungState::Abgelehnt { .. }),
        "validation failure must reject; got: {state:?}",
    );
}

#[test]
fn wim_stornierung_deadline_label_is_canonical() {
    assert_eq!(
        STORNIERUNG_DEADLINE_LABEL, "wim-stornierung-deadline",
        "deadline label must match the canonical form expected by deadline_dispatch.rs",
    );
}
