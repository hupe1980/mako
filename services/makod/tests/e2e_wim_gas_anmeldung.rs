//! End-to-end test: MSBN ↔ NB WiM Gas Anmeldung MSB Gas (PID 44039).
//!
//! A mock NB (Netzbetreiber) receives a UTILMD G 44039 from the MSBN
//! (neuer Messstellenbetreiber Gas) and dispatches a positive or negative
//! APERAK response within **10 Werktage** (BK7-24-01-009).
//!
//! # Protocol trace
//!
//! ```text
//!   MSBN ERP (wire fixture)                     NB ERP (MockNb)
//!   ──────────────────────────────────────────────────────────────
//!                        ──── UTILMD G 44039 ─►
//!                                               receive_utilmd(wire)
//!                                                 → adapter: ReceiveUtilmd
//!                                                 → vacuous-validation guard
//!                                                 → state: Initiated (Rejected if AHB fails)
//!                                               dispatch_aperak(positive=true)
//!                                                 → DispatchAperak
//!                                                 → state: AperakSent
//!                                               complete()
//!                                                 → Complete
//!                                                 → state: Active
//!   ──────────────────────────────────────────────────────────────
//! ```
//!
//! # APERAK outbox
//!
//! `DispatchAperak` enqueues one `"Aperak"` [`OutboxMessage`] addressed to the
//! MSBN. The payload carries the PID, MaLo, `positive` flag, and
//! `orig_message_ref`.
//!
//! # Regulatory context
//!
//! - **PID 44039**: Anmeldung MSB Gas (MSBN → NB, WiM Gas AWH V2.0)
//! - **APERAK Frist**: **10 Werktage** (BNetzA BK7-24-01-009)
//! - **Saturday counts as a Werktag**; Sunday and federal public holidays do not.
//! - The NB state machine:
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
use mako_wim_gas::{WimGasAnmeldungCommand, WimGasAnmeldungState, WimGasAnmeldungWorkflow};
use makod::adapters::wim_gas_anmeldung_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSBN_ID: &str = "4012345000023"; // Neuer Messstellenbetreiber Gas (sender)
const NB_ID: &str = "9900357000004"; // Netzbetreiber (receiver)
const MALO_GAS_ID: &str = "52695662085"; // Marktlokations-ID (Gas)
const FV: &str = "FV2025-10-01";

// ── UTILMD G 44039 wire fixture ────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD G1.1 Anmeldung MSB Gas (PID 44039).
// Direction: MSBN (sender NAD+MS) → NB (receiver NAD+MR).
//
// NOTE: WiM Gas PIDs (44039–44053) are not yet in the `fv*_gas` AHB profile
// set. Until `cargo xtask import-xml-ahb` imports them, `msg.validate()` returns
// a vacuous pass. The adapter applies the `pid_has_ahb_rules()` guard and sets
// `validation_passed = false`, so this test bypasses validation explicitly
// in `receive_utilmd()`.
const UTILMD_44039_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+WG-2025-001'\
UNH+MSG-001+UTILMD:D:11A:UN:G1.1'\
BGM+E01:::+00044039::+9'\
DTM+137:20250115:102'\
RFF+Z13:WG-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+52695662085::'\
UNT+8+MSG-001'\
UNZ+1+WG-2025-001'";

// ── Mock NB ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Netzbetreiber's ERP** receiving and processing a WiM Gas
/// Anmeldung MSB Gas request.
struct MockNb {
    process: Process<WimGasAnmeldungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("wim-gas-anmeldung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive MSBN's UTILMD G 44039 wire bytes, adapt,
    /// and execute.
    ///
    /// The vacuous-validation guard in the adapter sets `validation_passed = false`
    /// because WiM Gas PIDs have no AHB profile yet. This test bypasses it with
    /// explicit `validation_passed: true` so the state machine can proceed — this
    /// mirrors how a real NB ERP would behave once profiles are imported.
    ///
    /// Asserts that the adapter correctly extracts PID, sender, receiver,
    /// MaLo, and message reference from the wire bytes.
    async fn receive_utilmd(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse MSBN UTILMD G wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = wim_gas_anmeldung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt UTILMD G 44039 to WimGasAnmeldungCommand");

        let cmd = match cmd {
            WimGasAnmeldungCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(pid.as_u32(), 44039, "adapter must extract PID 44039");
                assert_eq!(
                    sender.as_str(),
                    MSBN_ID,
                    "adapter must extract sender GLN (MSBN) from NAD+MS"
                );
                assert_eq!(
                    receiver.as_str(),
                    NB_ID,
                    "adapter must extract receiver GLN (NB) from NAD+MR"
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
                // Bypass AHB validation — WiM Gas PIDs have no profile yet;
                // validation_passed = true to allow state machine to proceed.
                WimGasAnmeldungCommand::ReceiveUtilmd {
                    pid,
                    sender,
                    receiver,
                    malo_id,
                    document_date,
                    message_ref,
                    validation_passed: true, // bypass: no AHB profile for WiM Gas PIDs yet
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected WimGasAnmeldungCommand::ReceiveUtilmd"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveUtilmd 44039");
    }

    /// ERP action: dispatch positive or negative APERAK for the pending Anmeldung.
    ///
    /// Returns outbox entries for assertion.
    async fn dispatch_aperak(&self, positive: bool, reason: Option<&str>) -> Vec<OutboxMessage> {
        let (_, outbox) = self
            .process
            .execute_and_collect(WimGasAnmeldungCommand::DispatchAperak {
                positive,
                reason: reason.map(String::from),
            })
            .await
            .expect("NB: execute DispatchAperak");

        outbox
    }

    /// ERP action: mark the MSB change as activated (process complete).
    async fn activate(&self) {
        self.process
            .execute(WimGasAnmeldungCommand::Activate)
            .await
            .expect("NB: execute Activate");
    }

    /// Query the current process state.
    async fn state(&self) -> WimGasAnmeldungState {
        self.process.state().await.expect("NB: load process state")
    }
}

// ── Test: positive APERAK (happy path) ────────────────────────────────────────

#[tokio::test]
async fn wim_gas_anmeldung_positive_aperak() {
    let nb = MockNb::new();

    // Step 1: MSBN sends UTILMD G 44039 → NB adapts and executes.
    nb.receive_utilmd(UTILMD_44039_BYTES).await;

    let state = nb.state().await;
    assert!(
        matches!(state, WimGasAnmeldungState::ValidationPassed(_)),
        "after ReceiveUtilmd with valid fixture, state must be ValidationPassed; got: {state:?}",
    );

    // Step 2: NB dispatches positive APERAK within 10 Werktage.
    let outbox = nb.dispatch_aperak(true, None).await;

    assert_eq!(
        outbox.len(),
        1,
        "positive APERAK must produce exactly one outbox entry"
    );
    let aperak = &outbox[0];
    assert_eq!(
        aperak.message_type.as_ref(),
        "Aperak",
        "outbox entry must have type 'Aperak'"
    );
    assert_eq!(
        aperak.recipient.as_ref(),
        MSBN_ID,
        "APERAK must be addressed to the MSBN (original sender)"
    );
    let payload = &aperak.payload;
    assert_eq!(
        payload["pid"].as_u64().unwrap(),
        44039,
        "outbox payload must contain the original PID"
    );
    assert_eq!(
        payload["malo"].as_str().unwrap(),
        MALO_GAS_ID,
        "outbox payload must contain the MaLo"
    );
    assert!(
        payload["positive"].as_bool().unwrap(),
        "outbox payload must record positive=true"
    );
    assert_eq!(
        payload["orig_message_ref"].as_str().unwrap(),
        "MSG-001",
        "outbox payload must carry orig_message_ref for APERAK construction"
    );

    let state = nb.state().await;
    assert!(
        matches!(state, WimGasAnmeldungState::AperakSent(_)),
        "after positive DispatchAperak, state must be AperakSent; got: {state:?}",
    );

    // Step 3: NB marks MSB change as active.
    nb.activate().await;

    let state = nb.state().await;
    assert!(
        matches!(state, WimGasAnmeldungState::Active(_)),
        "after Activate, state must be Active; got: {state:?}",
    );
}

// ── Test: negative APERAK (rejection) ─────────────────────────────────────────

#[tokio::test]
async fn wim_gas_anmeldung_negative_aperak() {
    let nb = MockNb::new();

    nb.receive_utilmd(UTILMD_44039_BYTES).await;

    let outbox = nb
        .dispatch_aperak(false, Some("Messstelle nicht wechselbereit"))
        .await;

    assert_eq!(
        outbox.len(),
        1,
        "negative APERAK must produce exactly one outbox entry"
    );
    let aperak = &outbox[0];
    assert!(
        !aperak.payload["positive"].as_bool().unwrap(),
        "negative APERAK must have positive=false"
    );
    assert!(
        aperak.payload.get("reason").is_some(),
        "negative APERAK outbox must carry rejection reason"
    );

    let state = nb.state().await;
    assert!(
        matches!(state, WimGasAnmeldungState::Rejected { .. }),
        "after negative DispatchAperak, state must be Rejected; got: {state:?}",
    );
}

// ── Test: duplicate ReceiveUtilmd rejected ────────────────────────────────────

#[tokio::test]
async fn wim_gas_anmeldung_duplicate_receive_rejected() {
    let nb = MockNb::new();

    // First receive succeeds.
    nb.receive_utilmd(UTILMD_44039_BYTES).await;

    // Second receive on non-New state must be an engine error (invalid state).
    let raw = nb
        .platform
        .parse(UTILMD_44039_BYTES)
        .expect("parse UTILMD G");

    let cmd = wim_gas_anmeldung_registry()
        .dispatch(&raw as &dyn Any, &nb.fv)
        .expect("adapt UTILMD G");

    let cmd = match cmd {
        WimGasAnmeldungCommand::ReceiveUtilmd {
            pid,
            sender,
            receiver,
            malo_id,
            document_date,
            message_ref,
            ..
        } => WimGasAnmeldungCommand::ReceiveUtilmd {
            pid,
            sender,
            receiver,
            malo_id,
            document_date,
            message_ref,
            validation_passed: true,
            validation_errors: vec![],
        },
        _ => panic!("expected ReceiveUtilmd"),
    };

    let result = nb.process.execute(cmd).await;
    assert!(
        result.is_err(),
        "second ReceiveUtilmd must fail (process no longer in New state)"
    );
    if let Err(mako_engine::error::EngineError::Workflow(wf_err)) = result {
        let msg = wf_err.to_string();
        assert!(
            msg.contains("New") || msg.contains("state"),
            "error must mention expected state 'New'; got: {msg}",
        );
    }
}

// ── Test: APERAK_WINDOW_LABEL is correct ─────────────────────────────────────

#[test]
fn wim_gas_aperak_window_label_is_canonical() {
    assert_eq!(
        mako_wim_gas::ANMELDUNG_APERAK_WINDOW_LABEL,
        "wim-gas-aperak-10-werktage",
        "APERAK_WINDOW_LABEL must match the string used in the deadline scheduler",
    );
}
