//! End-to-end test: NB → MSB WiM Stammdaten (PID 17132).
//!
//! A mock MSB (Messstellenbetreiber) receives an ORDERS 17132 (Anfrage zur
//! Übermittlung von Stammdaten Strom) from the NB (Netzbetreiber) and either
//! transmits master data or rejects the request.
//!
//! # Protocol trace (positive path)
//!
//! ```text
//!   NB ERP (wire fixture)                        MSB ERP (MockMsb)
//!   ──────────────────────────────────────────────────────────────────
//!                        ──── ORDERS 17132 ───►
//!                                                receive_anforderung(wire)
//!                                                  → adapter: ReceiveAnforderung
//!                                                  → state: ValidationPassed
//!                                                transmit_stammdaten(17102)
//!                                                  → TransmitStammdaten
//!                                                  → state: Uebermittelt
//!   ──────────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 17132**: Anfrage zur Übermittlung von Stammdaten Strom (ORDERS, NB → MSB)
//! - **Response Frist**: **5 Werktage** (BNetzA BK6-18-032)
//! - **Saturday counts as a Werktag**; Sunday and public holidays do not.
//! - MSB state machine:
//!   `New → AnforderungReceived → ValidationPassed → Uebermittelt` (positive)
//!   `New → AnforderungReceived → ValidationPassed → Abgelehnt` (rejection)

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::{MessageRef, Pruefidentifikator},
    version::{FormatVersion, WorkflowId},
};
use mako_wim::{
    STAMMDATEN_DEADLINE_LABEL, StammdatenCommand, StammdatenState, WimStammdatenWorkflow,
};
use makod::adapters::wim_stammdaten_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const NB_ID: &str = "4012345000023"; // Netzbetreiber (sender of ORDERS 17132)
const MSB_ID: &str = "9900357000004"; // MSB (receiver)
const MELO_ID: &str = "E0000000000000000001"; // Messlokation EIC code
const FV: &str = "FV2025-10-01";

// ── ORDERS 17132 wire fixture ─────────────────────────────────────────────────
//
// Minimal EDIFACT ORDERS — Anfrage zur Übermittlung von Stammdaten Strom (PID 17132).
// Direction: NB (NAD+MS) → MSB (NAD+MR).
// BGM code Z55 + PID in element 2; UNH release 1.4c per BDEW WiM AHB.
const ORDERS_17132_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+WIM-SD-001'\
UNH+MSG-001+ORDERS:D:09B:UN:1.4c'\
BGM+Z55+00017132+9'\
DTM+137:20250115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+E0000000000000000001::'\
UNT+7+MSG-001'\
UNZ+1+WIM-SD-001'";

// ── Mock MSB ERP backend ──────────────────────────────────────────────────────

/// Simulates the **MSB ERP** receiving a WiM Stammdaten ORDERS 17132.
struct MockMsb {
    process: Process<WimStammdatenWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockMsb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(MSB_ID),
                WorkflowId::new("wim-stammdaten", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive ORDERS 17132 wire bytes, adapt, and execute.
    ///
    /// AHB validation is bypassed — the minimal fixture does not satisfy all
    /// profile rules; AHB conformance is tested separately in `edi-energy` tests.
    async fn receive_anforderung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("MSB: parse ORDERS wire bytes");

        let unh_ref = raw.message_ref().to_owned();
        assert!(!unh_ref.is_empty(), "UNH message_ref must be non-empty");

        let cmd = wim_stammdaten_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("MSB: adapt ORDERS 17132 to StammdatenCommand");

        let cmd = match cmd {
            StammdatenCommand::ReceiveAnforderung {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(pid.as_u32(), 17132, "adapter must extract PID 17132");
                assert_eq!(sender.as_str(), NB_ID, "sender must be NB GLN");
                assert_eq!(receiver.as_str(), MSB_ID, "receiver must be MSB GLN");
                assert_eq!(melo_id.as_str(), MELO_ID, "MeLo must match IDE+Z19");
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref",
                );
                StammdatenCommand::ReceiveAnforderung {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    document_date,
                    message_ref,
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected StammdatenCommand::ReceiveAnforderung"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("MSB: execute ReceiveAnforderung 17132");
    }

    /// ERP action: transmit master data as ORDERS response (PID 17102).
    async fn transmit_stammdaten(&self) {
        self.process
            .execute(StammdatenCommand::TransmitStammdaten {
                response_pid: Pruefidentifikator::new(17102).expect("17102 is a valid PID"),
                response_ref: MessageRef::new("WIM-SD-RESP-001"),
                standorteigenschaften: None,
                zaehlwerke: vec![],
            })
            .await
            .expect("MSB: execute TransmitStammdaten");
    }

    /// ERP action: reject the Stammdaten request.
    async fn reject_anforderung(&self, reason: &str) {
        self.process
            .execute(StammdatenCommand::RejectAnforderung {
                reason: reason.to_owned(),
            })
            .await
            .expect("MSB: execute RejectAnforderung");
    }

    async fn state(&self) -> StammdatenState {
        self.process.state().await.expect("must load state")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn wim_stammdaten_positive_transmit() {
    let msb = MockMsb::new();

    msb.receive_anforderung(ORDERS_17132_BYTES).await;

    let state = msb.state().await;
    assert!(
        matches!(state, StammdatenState::ValidationPassed(_)),
        "after ReceiveAnforderung, state must be ValidationPassed; got: {state:?}",
    );

    msb.transmit_stammdaten().await;

    let state = msb.state().await;
    assert!(
        matches!(state, StammdatenState::Uebermittelt { .. }),
        "after TransmitStammdaten, state must be Uebermittelt; got: {state:?}",
    );
}

#[tokio::test]
async fn wim_stammdaten_rejection() {
    let msb = MockMsb::new();

    msb.receive_anforderung(ORDERS_17132_BYTES).await;
    msb.reject_anforderung("Stammdaten nicht verfügbar").await;

    let state = msb.state().await;
    assert!(
        matches!(state, StammdatenState::Abgelehnt { .. }),
        "after RejectAnforderung, state must be Abgelehnt; got: {state:?}",
    );
}

#[tokio::test]
async fn wim_stammdaten_validation_failure_rejects() {
    let msb = MockMsb::new();

    let raw = msb.platform.parse(ORDERS_17132_BYTES).expect("parse");
    let cmd = wim_stammdaten_registry()
        .dispatch(&raw as &dyn Any, &msb.fv)
        .expect("adapt");

    // Override to simulate validation failure.
    let cmd = match cmd {
        StammdatenCommand::ReceiveAnforderung {
            pid,
            sender,
            receiver,
            melo_id,
            document_date,
            message_ref,
            ..
        } => StammdatenCommand::ReceiveAnforderung {
            pid,
            sender,
            receiver,
            melo_id,
            document_date,
            message_ref,
            validation_passed: false,
            validation_errors: vec!["missing required RFF".to_owned()],
        },
        _ => panic!("expected ReceiveAnforderung"),
    };

    msb.process.execute(cmd).await.expect("execute");

    let state = msb.state().await;
    assert!(
        matches!(state, StammdatenState::Abgelehnt { .. }),
        "validation failure must reject; got: {state:?}",
    );
}

#[test]
fn wim_stammdaten_deadline_label_is_canonical() {
    assert_eq!(
        STAMMDATEN_DEADLINE_LABEL, "wim-stammdaten-deadline",
        "deadline label must match the canonical form expected by deadline_dispatch.rs",
    );
}
