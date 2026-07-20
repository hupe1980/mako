//! End-to-end test: nMSB → NB WiM Geräteübernahme Phase 1 (PID 17001).
//!
//! A mock NB/aMSB receives an ORDERS 17001 (Anfrage Geräteübernahmeangebot)
//! from the incoming Messstellenbetreiber (nMSB) and dispatches a positive or
//! negative ORDRSP.
//!
//! # Protocol trace (Phase 1 positive path)
//!
//! ```text
//!   nMSB ERP (wire fixture)                    NB/aMSB ERP (MockNb)
//!   ─────────────────────────────────────────────────────────────────
//!                        ──── ORDERS 17001 ───►
//!                                               receive_anfrage(wire)
//!                                                 → adapter: ReceiveAnfrage
//!                                                 → state: ValidationPassed
//!                                               dispatch_anfrage_ordrsp(+)
//!                                                 → DispatchAnfrageOrdrsp
//!                                                 → state: AngebotGesendet
//!   ─────────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 17001**: Anfrage Geräteübernahmeangebot (ORDERS, nMSB → NB/aMSB)
//! - **ORDRSP Frist**: **5 Werktage** (BNetzA BK6-18-032)
//! - **Saturdays, Sundays and public holidays are not Werktage.**
//! - NB/aMSB state machine (Phase 1):
//!   `New → AnfrageReceived → ValidationPassed → AngebotGesendet` (positive)
//!   `New → AnfrageReceived → ValidationPassed → Abgelehnt` (negative)

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
    GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL, GeraeteubernahmeCommand, GeraeteubernahmeState,
    WimGeraeteubernahmeWorkflow,
};
use makod::adapters::wim_geraeteubernahme_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const NMSB_ID: &str = "9900357000004"; // nMSB (sender of ORDERS)
const NB_ID: &str = "4012345000023"; // NB/aMSB (receiver)
const MELO_ID: &str = "E0000000000000000001"; // Messlokation EIC code
const DEVICE_ID: &str = "DEV-001"; // Physical device identifier (from RFF)
const FV: &str = "FV2025-10-01";

// ── ORDERS 17001 wire fixture ─────────────────────────────────────────────────
//
// Minimal EDIFACT ORDERS — Anfrage Geräteübernahmeangebot (PID 17001).
// Direction: nMSB (NAD+MS) → NB/aMSB (NAD+MR).
// BGM code Z55 + PID in element 2; UNH release 1.4b per BDEW WiM AHB.
const ORDERS_17001_BYTES: &[u8] = b"\
UNB+UNOC:3+9900357000004:14+4012345000023:14+250115:0800+WIM-GT-001'\
UNH+MSG-001+ORDERS:D:09B:UN:1.4b'\
BGM+Z55+00017001+9'\
DTM+137:20250115:102'\
RFF+Z13:DEV-001'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::293'\
IDE+Z19+E0000000000000000001::'\
UNT+8+MSG-001'\
UNZ+1+WIM-GT-001'";

// ── Mock NB/aMSB ERP backend ──────────────────────────────────────────────────

/// Simulates the **NB/aMSB ERP** receiving a WiM Geräteübernahme ORDERS 17001.
struct MockNb {
    process: Process<WimGeraeteubernahmeWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("wim-geraeteubernahme", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive ORDERS 17001 wire bytes, adapt, and execute.
    ///
    /// AHB validation is bypassed — the minimal fixture does not satisfy all
    /// profile rules; AHB conformance is tested separately in `edi-energy` tests.
    async fn receive_anfrage(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse ORDERS wire bytes");

        let unh_ref = raw.message_ref().to_owned();
        assert!(!unh_ref.is_empty(), "UNH message_ref must be non-empty");

        let cmd = wim_geraeteubernahme_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt ORDERS 17001 to GeraeteubernahmeCommand");

        let cmd = match cmd {
            GeraeteubernahmeCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                melo_id,
                device_id,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(pid.as_u32(), 17001, "adapter must extract PID 17001");
                assert_eq!(sender.as_str(), NMSB_ID, "sender must be nMSB GLN");
                assert_eq!(receiver.as_str(), NB_ID, "receiver must be NB GLN");
                assert_eq!(melo_id.as_str(), MELO_ID, "MeLo must match IDE+Z19");
                assert_eq!(device_id.as_str(), DEVICE_ID, "DeviceId must match RFF");
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref",
                );
                GeraeteubernahmeCommand::ReceiveAnfrage {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    device_id,
                    document_date,
                    message_ref,
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected GeraeteubernahmeCommand::ReceiveAnfrage"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveAnfrage 17001");
    }

    /// ERP action: dispatch positive ORDRSP 17003 (Angebot accepted).
    async fn accept_anfrage(&self) {
        self.process
            .execute(GeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive: true,
                response_ref: MessageRef::new("WIM-GT-RESP-001"),
                reason: None,
            })
            .await
            .expect("NB: execute DispatchAnfrageOrdrsp positive");
    }

    /// ERP action: dispatch negative ORDRSP 17004 (Angebot rejected).
    async fn reject_anfrage(&self, reason: &str) {
        self.process
            .execute(GeraeteubernahmeCommand::DispatchAnfrageOrdrsp {
                positive: false,
                response_ref: MessageRef::new("WIM-GT-RESP-001"),
                reason: Some(reason.to_owned()),
            })
            .await
            .expect("NB: execute DispatchAnfrageOrdrsp negative");
    }

    async fn state(&self) -> GeraeteubernahmeState {
        self.process.state().await.expect("must load state")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn wim_geraeteubernahme_anfrage_positive_ordrsp() {
    let nb = MockNb::new();

    nb.receive_anfrage(ORDERS_17001_BYTES).await;

    let state = nb.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::ValidationPassed(_)),
        "after ReceiveAnfrage, state must be ValidationPassed; got: {state:?}",
    );

    nb.accept_anfrage().await;

    let state = nb.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::AngebotGesendet(_)),
        "after positive ORDRSP, state must be AngebotGesendet; got: {state:?}",
    );
}

#[tokio::test]
async fn wim_geraeteubernahme_anfrage_negative_ordrsp() {
    let nb = MockNb::new();

    nb.receive_anfrage(ORDERS_17001_BYTES).await;
    nb.reject_anfrage("Gerät nicht verfügbar").await;

    let state = nb.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::Abgelehnt { .. }),
        "after negative ORDRSP, state must be Abgelehnt; got: {state:?}",
    );
}

#[tokio::test]
async fn wim_geraeteubernahme_validation_failure_rejects() {
    let nb = MockNb::new();

    let raw = nb.platform.parse(ORDERS_17001_BYTES).expect("parse");
    let cmd = wim_geraeteubernahme_registry()
        .dispatch(&raw as &dyn Any, &nb.fv)
        .expect("adapt");

    // Override to simulate validation failure.
    let cmd = match cmd {
        GeraeteubernahmeCommand::ReceiveAnfrage {
            pid,
            sender,
            receiver,
            melo_id,
            device_id,
            document_date,
            message_ref,
            ..
        } => GeraeteubernahmeCommand::ReceiveAnfrage {
            pid,
            sender,
            receiver,
            melo_id,
            device_id,
            document_date,
            message_ref,
            validation_passed: false,
            validation_errors: vec!["missing required IDE".to_owned()],
        },
        _ => panic!("expected ReceiveAnfrage"),
    };

    nb.process.execute(cmd).await.expect("execute");

    let state = nb.state().await;
    assert!(
        matches!(state, GeraeteubernahmeState::Abgelehnt { .. }),
        "validation failure must reject; got: {state:?}",
    );
}

#[test]
fn wim_geraeteubernahme_deadline_label_is_canonical() {
    assert_eq!(
        GERAETEUBERNAHME_ORDRSP_DEADLINE_LABEL, "wim-geraeteubernahme-ordrsp-deadline",
        "deadline label must match the canonical form expected by deadline_dispatch.rs",
    );
}
