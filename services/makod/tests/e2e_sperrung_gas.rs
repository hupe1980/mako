//! End-to-end test: GNB → LFN Anweisung Sperrung Gas (PID 44555).
//!
//! A mock LFN Gas (Gaslieferant) receives a UTILMD G 44555 Anweisung Sperrung
//! from the GNB (Gasnetzbetreiber) and confirms or denies execution.
//!
//! # Protocol trace
//!
//! ```text
//!   GNB ERP (wire fixture)                      LFN Gas ERP (MockLfn)
//!   ──────────────────────────────────────────────────────────────────
//!                        ──── UTILMD G 44555 ─►
//!                                               receive_sperrung(wire)
//!                                                 → adapter: ReceiveSperrung
//!                                                 → state: ValidationPassed
//!                                               confirm_execution(true)
//!                                                 → BestaetigueSperrung
//!                                                 → state: Ausgefuehrt
//!   ──────────────────────────────────────────────────────────────────
//! ```
//!
//! The GNB side does **not** have a `mako-engine` workflow for sending the
//! Sperrung order — the GNB's internal system generates the EDIFACT UTILMD G
//! 44555 directly. This test exercises only the receiving (LFN) side of the
//! protocol.
//!
//! # APERAK Frist
//!
//! The regulatory **confirmation deadline is 10 Werktage** from receipt
//! (BNetzA BK7 GeLi Gas 3.0, BK7-24-01-009). Saturday counts as a Werktag;
//! Sunday and federal public holidays do not. This is distinct from:
//! - GPKE (24 wall-clock hours, BK6-22-024)
//! - WiM (5 Werktage)
//!
//! # Regulatory context
//!
//! - **PID 44555**: Anweisung Sperrung/Entsperrung Gas (GNB → LFN)
//! - **BNetzA BK7**: GeLi Gas 3.0, Beschluss 12.09.2025 (BK7-24-01-009)
//! - The LFN state machine:
//!   `New → AnweisungErhalten → ValidationPassed → Ausgefuehrt` (success)
//!   `New → AnweisungErhalten → ValidationPassed → Rejected` (cannot execute)

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    version::{FormatVersion, WorkflowId},
};
use mako_geli_gas::{GasSperrungCommand, GasSperrungState, GeliGasSperrungWorkflow};
use makod::adapters::geli_gas_sperrung_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const GNB_ID: &str = "9900357000004"; // GNB: issuer of the Sperrung order
const LFN_GAS_ID: &str = "4012345000023"; // LFN Gas: recipient of the order
const MALO_GAS_ID: &str = "52695662085"; // Marktlokations-ID (Gas)
const FV: &str = "FV2025-10-01";

// ── UTILMD G 44555 wire fixture ───────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD G1.1 Anweisung Sperrung Gas (PID 44555).
// Direction: GNB (sender NAD+MS) → LFN Gas (receiver NAD+MR).
//
// Source: GeLi Gas AHB G1.1 (BNetzA BK7), FV2025-10-01.
const UTILMD_44555_BYTES: &[u8] = b"\
UNB+UNOC:3+9900357000004:14+4012345000023:14+250115:1000+GAS-2025-555'\
UNH+MSG-555+UTILMD:D:11A:UN:G1.1'\
BGM+E03:::+00044555::+9'\
DTM+137:20250115:102'\
RFF+Z13:GAS-REF-555'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::293'\
IDE+Z19+52695662085::'\
UNT+8+MSG-555'\
UNZ+1+GAS-2025-555'";

// ── Mock LFN Gas ERP backend ──────────────────────────────────────────────────

/// Simulates the **LFN Gas ERP** receiving and processing a GeLi Gas
/// Anweisung Sperrung order from the GNB.
struct MockLfn {
    process: Process<GeliGasSperrungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockLfn {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LFN_GAS_ID),
                WorkflowId::new("geli-gas-sperrung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive GNB UTILMD G 44555 wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true` — the hand-crafted fixture does not
    /// satisfy all G1.1 profile rules; AHB conformance is tested separately in
    /// `crates/edi-energy/tests/`.
    async fn receive_sperrung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LFN: parse GNB UTILMD G wire");

        let unh_ref = raw.message_ref().to_owned();

        let cmd = geli_gas_sperrung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LFN: adapt UTILMD G 44555 to GasSperrungCommand");

        let cmd = match cmd {
            GasSperrungCommand::ReceiveSperrung {
                pid,
                gnb,
                lieferant,
                malo_id,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(pid.as_u32(), 44555, "adapter must extract PID 44555");
                assert_eq!(gnb.as_str(), GNB_ID, "gnb GLN must match NAD+MS");
                assert_eq!(
                    lieferant.as_str(),
                    LFN_GAS_ID,
                    "lieferant GLN must match NAD+MR"
                );
                assert_eq!(malo_id.as_str(), MALO_GAS_ID, "MaLo must match IDE+Z19");
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "message_ref must preserve UNH ref"
                );
                GasSperrungCommand::ReceiveSperrung {
                    pid,
                    gnb,
                    lieferant,
                    malo_id,
                    document_date,
                    message_ref,
                    validation_passed: true,
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected GasSperrungCommand::ReceiveSperrung"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("LFN: execute ReceiveSperrung 44555");
    }

    /// ERP action: confirm (or deny) execution of the Sperrung.
    ///
    /// - `durchgefuehrt = true`  → Ausgefuehrt
    /// - `durchgefuehrt = false` → Rejected
    async fn confirm_execution(&self, durchgefuehrt: bool, reason: Option<&str>) {
        self.process
            .execute(GasSperrungCommand::BestaetigueSperrung {
                durchgefuehrt,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("LFN: execute BestaetigueSperrung");
    }

    async fn state(&self) -> GasSperrungState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// GeLi Gas Sperrung — execution success path (PID 44555 → Ausgefuehrt).
///
/// The LFN Gas receives the Sperrung order, validates it, and confirms that
/// the disconnection was carried out successfully.
///
/// BNetzA BK7 GeLi Gas 3.0: confirmation must be sent within **10 Werktage**.
/// Saturday counts as a Werktag; Sunday and federal holidays do not.
#[tokio::test]
async fn e2e_sperrung_gas_execution_success() {
    let lfn = MockLfn::new();

    lfn.receive_sperrung(UTILMD_44555_BYTES).await;
    assert!(
        matches!(lfn.state().await, GasSperrungState::ValidationPassed { .. }),
        "LFN must be ValidationPassed after ReceiveSperrung 44555"
    );

    lfn.confirm_execution(true, None).await;
    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, GasSperrungState::Ausgefuehrt(_)),
        "LFN must be Ausgefuehrt after BestaetigueSperrung(true); got: {final_state:?}"
    );

    if let GasSperrungState::Ausgefuehrt(data) = final_state {
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            44555,
            "persisted data must carry PID 44555"
        );
        assert_eq!(data.gnb.as_str(), GNB_ID, "gnb must be the issuing GNB");
        assert_eq!(
            data.lieferant.as_str(),
            LFN_GAS_ID,
            "lieferant must be the receiving LFN"
        );
        assert_eq!(data.malo_id.as_str(), MALO_GAS_ID);
    }
}

/// GeLi Gas Sperrung — execution failure path (PID 44555 → Rejected).
///
/// The LFN Gas cannot execute the Sperrung (e.g. meter inaccessible) and
/// reports the failure via `BestaetigueSperrung(durchgefuehrt=false)`.
#[tokio::test]
async fn e2e_sperrung_gas_execution_failure() {
    let lfn = MockLfn::new();

    lfn.receive_sperrung(UTILMD_44555_BYTES).await;
    assert!(
        matches!(lfn.state().await, GasSperrungState::ValidationPassed { .. }),
        "LFN must be ValidationPassed after ReceiveSperrung"
    );

    lfn.confirm_execution(
        false,
        Some("Zähler nicht zugänglich — Gaszähler im versperrten Keller"),
    )
    .await;

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, GasSperrungState::Rejected { .. }),
        "LFN must be Rejected after BestaetigueSperrung(false); got: {final_state:?}"
    );
}

/// GeLi Gas Sperrung — AHB validation failure (PID 44555 → immediate Rejected).
///
/// If the received UTILMD G 44555 fails AHB validation, the workflow transitions
/// to `Rejected` immediately after `ReceiveSperrung`, without requiring a
/// `BestaetigueSperrung` step.
#[tokio::test]
async fn e2e_sperrung_gas_ahb_validation_failure() {
    let lfn = MockLfn::new();

    lfn.process
        .execute(GasSperrungCommand::ReceiveSperrung {
            pid: mako_engine::types::Pruefidentifikator::new(44555).unwrap(),
            gnb: mako_engine::types::MarktpartnerCode::new(GNB_ID),
            lieferant: mako_engine::types::MarktpartnerCode::new(LFN_GAS_ID),
            malo_id: mako_engine::types::MaLo::new(MALO_GAS_ID),
            document_date: "2025-01-15".to_owned(),
            message_ref: mako_engine::types::MessageRef::new("MSG-GAS-555"),
            validation_passed: false,
            validation_errors: vec![
                "UTILMD G segment BGM missing mandatory Pruefidentifikator qualifier".to_owned(),
            ],
        })
        .await
        .expect("ReceiveSperrung with invalid message must not panic");

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, GasSperrungState::Rejected { .. }),
        "invalid UTILMD G 44555 must reach Rejected without BestaetigueSperrung; got: {final_state:?}"
    );
}
