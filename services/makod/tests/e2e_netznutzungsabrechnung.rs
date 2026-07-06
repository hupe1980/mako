//! End-to-end test: GPKE Netznutzungsabrechnung / Mehr-Mindermengen —
//! INVOIC-based billing (PIDs 31001, 31002, 31005–31009).
//!
//! Models the NB (Netzbetreiber) side of the GPKE billing workflow.  The NB
//! processes an outbound INVOIC addressed to the supplier (LF) and tracks
//! whether the billing was settled, disputed, or timed out.
//!
//! **Note**: PID 31004 ("Stornorechnung") belongs to WiM Gas per
//! `docs/pid-reference.md`. It is NOT a GPKE PID.
//! **Note**: PID 31009 ("MSB-Rechnung", GPKE Teil 3) was moved here from
//! `mako-wim::rechnung` per `docs/pid-reference.md`.
//!
//! # Regulatory basis
//!
//! - **BK6-22-024 (LFW24)** — GPKE APERAK Frist: **24 wall-clock hours**
//! - **INVOIC AHB 1.0 / MIG 2.8e** — German energy-market invoice format
//! - **PIDs 31001/31002** — Abschlagsrechnung / NN-Rechnung (Netznutzungsabrechnung)
//! - **PIDs 31005–31008** — Mehr-/Mindermengen billing variants
//! - **PID 31009** — MSB-Rechnung (GPKE Teil 3, NB/MSB settlement)
//! - **PID 31004** — Stornorechnung (WiM Gas — NOT GPKE)
//!
//! # Lifecycle trace (settle — happy path)
//!
//! ```text
//!   NB (this workflow)                LF ERP
//!   ─────────────────────────────────────────────────────────────────
//!   receive_invoic(31001, valid)
//!     state: ValidationPassed
//!   settle_invoice()                 ──────────→ (CONTRL / ACK dispatched)
//!     state: Settled
//!   ─────────────────────────────────────────────────────────────────
//! ```
//!
//! # Lifecycle trace (dispute path)
//!
//! ```text
//!   NB (this workflow)                LF ERP
//!   ─────────────────────────────────────────────────────────────────
//!   receive_invoic(31002, valid)
//!     state: ValidationPassed
//!   dispute_invoice("reason")        ──────────→ (APERAK disputed dispatched)
//!     state: Disputed
//!   ─────────────────────────────────────────────────────────────────
//! ```

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{
    ABRECHNUNG_WINDOW_LABEL, AbrechnungCommand, AbrechnungState, GpkeAbrechnungWorkflow,
    INVOIC_PIDS,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const NB_ID: &str = "9900357000004"; // Netzbetreiber GLN (sender of INVOIC)
const LF_ID: &str = "4012345000023"; // Lieferant GLN (recipient)
const FV: &str = "FV2025-10-01";

// ── Mock NB backend ───────────────────────────────────────────────────────────

/// Simulates the **NB's** billing workflow tracking an outbound INVOIC.
///
/// The NB dispatches an INVOIC to the LF and waits for the LF's
/// acknowledgement (CONTRL). This workflow records the outcome — settled
/// (positive CONTRL), disputed (negative CONTRL / APERAK), or timed out.
struct MockNb {
    process: Process<GpkeAbrechnungWorkflow, InMemoryEventStore>,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("gpke-abrechnung", FV),
            ),
        }
    }

    /// Record an inbound INVOIC (or outbound billing event) on the NB side.
    ///
    /// `validation_passed = true` → `ValidationPassed`
    /// `validation_passed = false` → `Rejected`
    async fn receive_invoic(&self, pid: u32, invoice_ref: &str, validation_passed: bool) {
        let errors = if validation_passed {
            vec![]
        } else {
            vec!["profile rule Z01 violated: missing LIN segment".to_owned()]
        };
        self.process
            .execute(AbrechnungCommand::ReceiveInvoic {
                pid: Pruefidentifikator::new(pid).unwrap(),
                sender: MarktpartnerCode::new(NB_ID),
                recipient: MarktpartnerCode::new(LF_ID),
                invoice_ref: MessageRef::new(invoice_ref),
                document_date: "2025-01-15".to_owned(),
                validation_passed,
                validation_errors: errors,
                rechnung: None,
            })
            .await
            .expect("ReceiveInvoic");
    }

    /// NB settles the invoice (positive CONTRL received from LF).
    async fn settle_invoice(&self) {
        self.process
            .execute(AbrechnungCommand::SettleInvoice)
            .await
            .expect("SettleInvoice");
    }

    /// NB disputes the invoice (negative CONTRL / APERAK from LF).
    async fn dispute_invoice(&self, reason: &str) {
        self.process
            .execute(AbrechnungCommand::DisputeInvoice {
                reason: reason.to_owned(),
            })
            .await
            .expect("DisputeInvoice");
    }

    async fn state(&self) -> AbrechnungState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// GPKE Abrechnung — happy path: Abschlagsrechnung (PID 31001) received and
/// settled.
///
/// Lifecycle: New → ValidationPassed → Settled.
#[tokio::test]
async fn e2e_gpke_abrechnung_31001_settle() {
    let nb = MockNb::new();

    nb.receive_invoic(31001, "INVOIC-AB-2025-001", true).await;

    let state = nb.state().await;
    match &state {
        AbrechnungState::ValidationPassed(d) => {
            assert_eq!(d.pruefidentifikator.as_u32(), 31001);
            assert_eq!(d.sender.as_str(), NB_ID);
            assert_eq!(d.recipient.as_str(), LF_ID);
            assert_eq!(d.invoice_ref.as_str(), "INVOIC-AB-2025-001");
            assert_eq!(d.document_date, "2025-01-15");
        }
        _ => panic!("expected ValidationPassed; got: {state:?}"),
    }

    nb.settle_invoice().await;

    let state = nb.state().await;
    match state {
        AbrechnungState::Settled(d) => {
            assert_eq!(d.pruefidentifikator.as_u32(), 31001);
            assert_eq!(d.sender.as_str(), NB_ID);
            assert_eq!(d.recipient.as_str(), LF_ID);
        }
        _ => panic!("expected Settled; got: {state:?}"),
    }
}

/// GPKE Abrechnung — happy path: NN-Rechnung (PID 31002) received and settled.
///
/// PID 31002 is the annual Netznutzungsabrechnung (network-use invoice).
#[tokio::test]
async fn e2e_gpke_abrechnung_31002_settle() {
    let nb = MockNb::new();

    nb.receive_invoic(31002, "INVOIC-NN-2025-001", true).await;

    let state = nb.state().await;
    assert!(
        matches!(state, AbrechnungState::ValidationPassed(ref d) if d.pruefidentifikator.as_u32() == 31002),
        "expected ValidationPassed(31002); got: {state:?}"
    );

    nb.settle_invoice().await;

    assert!(
        matches!(nb.state().await, AbrechnungState::Settled(_)),
        "must be Settled"
    );
}

/// GPKE Abrechnung — happy path: MMM-Rechnung (PID 31005).
///
/// PID 31005 covers Mehr-/Mindermengensaldo billing.  The state machine is
/// identical to 31001/31002 — only the PID embedded in the process differs.
#[tokio::test]
async fn e2e_gpke_abrechnung_31005_mmm_settle() {
    let nb = MockNb::new();

    nb.receive_invoic(31005, "INVOIC-MMM-2025-001", true).await;

    let state = nb.state().await;
    assert!(
        matches!(state, AbrechnungState::ValidationPassed(ref d) if d.pruefidentifikator.as_u32() == 31005),
        "expected ValidationPassed(31005); got: {state:?}"
    );

    nb.settle_invoice().await;

    assert!(matches!(nb.state().await, AbrechnungState::Settled(_)));
}

/// GPKE Abrechnung — all active INVOIC PIDs are accepted by the workflow.
///
/// Validates that `INVOIC_PIDS` = {31001, 31002, 31005, 31006, 31007,
/// 31008, 31009} all produce `ValidationPassed` (no PID-guard rejections).
/// PID 31004 was removed — it belongs to WiM Gas per `docs/pid-reference.md`.
#[tokio::test]
async fn e2e_gpke_abrechnung_all_pids_accepted() {
    for &pid in INVOIC_PIDS {
        let nb = MockNb::new();
        nb.receive_invoic(pid, &format!("INVOIC-{pid}-2025"), true)
            .await;

        let state = nb.state().await;
        assert!(
            matches!(state, AbrechnungState::ValidationPassed(ref d) if d.pruefidentifikator.as_u32() == pid),
            "PID {pid}: expected ValidationPassed; got: {state:?}"
        );
    }
}

/// GPKE Abrechnung — dispute path: NN-Rechnung (PID 31002) received and
/// disputed.
///
/// The LF raises a billing dispute (APERAK / negative CONTRL).  State
/// transitions: New → ValidationPassed → Disputed.
#[tokio::test]
async fn e2e_gpke_abrechnung_dispute() {
    let nb = MockNb::new();

    nb.receive_invoic(31002, "INVOIC-NN-DISPUTE-001", true)
        .await;

    nb.dispute_invoice("Messstellenkosten für DE0001000001234567890 weichen um 18 % ab")
        .await;

    let state = nb.state().await;
    match &state {
        AbrechnungState::Disputed { data, reason } => {
            assert_eq!(data.pruefidentifikator.as_u32(), 31002);
            assert!(
                reason.contains("DE0001000001234567890"),
                "dispute reason must be preserved verbatim; got: {reason:?}"
            );
        }
        _ => panic!("expected Disputed; got: {state:?}"),
    }
}

/// GPKE Abrechnung — validation failure: INVOIC with profile errors is rejected
/// immediately without requiring an ERP action.
///
/// `validation_passed = false` → `Rejected` (no `SettleInvoice` needed).
/// This models the case where the incoming INVOIC fails AHB profile validation.
#[tokio::test]
async fn e2e_gpke_abrechnung_validation_failure_rejected() {
    let nb = MockNb::new();

    nb.receive_invoic(31001, "INVOIC-INVALID-001", false).await;

    let state = nb.state().await;
    match &state {
        AbrechnungState::Rejected { reason } => {
            assert!(
                reason.contains("Z01"),
                "rejection reason must carry validation error details; got: {reason:?}"
            );
        }
        _ => panic!("expected Rejected after validation failure; got: {state:?}"),
    }
}

/// GPKE Abrechnung — guard: non-GPKE PID rejected before any event.
///
/// PID 31003 (WiM-Rechnung) does not belong to this workflow and must return
/// a `WorkflowError` without emitting events.  State must remain `New`.
#[tokio::test]
async fn e2e_gpke_abrechnung_invalid_pid_rejected() {
    let nb = MockNb::new();

    let result = nb
        .process
        .execute(AbrechnungCommand::ReceiveInvoic {
            pid: Pruefidentifikator::new(31003).unwrap(),
            sender: MarktpartnerCode::new(NB_ID),
            recipient: MarktpartnerCode::new(LF_ID),
            invoice_ref: MessageRef::new("INVOIC-WRONG-PID"),
            document_date: "2025-01-15".to_owned(),
            validation_passed: true,
            validation_errors: vec![],
            rechnung: None,
        })
        .await;

    assert!(
        result.is_err(),
        "PID 31003 (WiM-Rechnung) must be rejected by GpkeAbrechnungWorkflow"
    );
    let state = nb.state().await;
    assert!(
        matches!(state, AbrechnungState::New),
        "state must remain New after rejected PID; got: {state:?}"
    );
}

/// GPKE Abrechnung — guard: `ReceiveInvoic` on a non-New process is rejected.
///
/// Ensures idempotency is enforced: a second INVOIC for the same process must
/// return an error and leave state unchanged.
#[tokio::test]
async fn e2e_gpke_abrechnung_duplicate_receive_rejected() {
    let nb = MockNb::new();

    nb.receive_invoic(31001, "INVOIC-DUP-001", true).await;
    assert!(matches!(
        nb.state().await,
        AbrechnungState::ValidationPassed(_)
    ));

    let result = nb
        .process
        .execute(AbrechnungCommand::ReceiveInvoic {
            pid: Pruefidentifikator::new(31001).unwrap(),
            sender: MarktpartnerCode::new(NB_ID),
            recipient: MarktpartnerCode::new(LF_ID),
            invoice_ref: MessageRef::new("INVOIC-DUP-002"),
            document_date: "2025-01-15".to_owned(),
            validation_passed: true,
            validation_errors: vec![],
            rechnung: None,
        })
        .await;

    assert!(
        result.is_err(),
        "ReceiveInvoic on non-New state must return error"
    );
    // State must remain ValidationPassed — the second receive must be idempotent
    assert!(
        matches!(nb.state().await, AbrechnungState::ValidationPassed(_)),
        "state must remain ValidationPassed after duplicate receive"
    );
}

/// GPKE Abrechnung — deadline: 24h settlement window expires.
///
/// If neither `SettleInvoice` nor `DisputeInvoice` is dispatched within 24
/// wall-clock hours (BK6-22-024), the deadline fires and the process
/// transitions to `Rejected`.
#[tokio::test]
async fn e2e_gpke_abrechnung_timeout_fires_rejected() {
    let nb = MockNb::new();

    nb.receive_invoic(31001, "INVOIC-TIMEOUT-001", true).await;

    let deadline_id = DeadlineId::new();
    nb.process
        .execute(AbrechnungCommand::TimeoutExpired {
            deadline_id,
            label: ABRECHNUNG_WINDOW_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired must be accepted from ValidationPassed");

    let state = nb.state().await;
    match &state {
        AbrechnungState::Rejected { reason } => {
            assert!(
                reason.contains(ABRECHNUNG_WINDOW_LABEL),
                "rejection reason must name the deadline label; got: {reason:?}"
            );
        }
        _ => panic!("expected Rejected after timeout; got: {state:?}"),
    }
}

/// GPKE Abrechnung — late deadline absorbed on Settled process.
///
/// A late-firing deadline on an already-Settled process must be a no-op:
/// the event is absorbed and the state remains `Settled`.
#[tokio::test]
async fn e2e_gpke_abrechnung_late_timeout_absorbed_on_settled() {
    let nb = MockNb::new();

    nb.receive_invoic(31001, "INVOIC-LATE-001", true).await;
    nb.settle_invoice().await;
    assert!(matches!(nb.state().await, AbrechnungState::Settled(_)));

    let deadline_id = DeadlineId::new();
    nb.process
        .execute(AbrechnungCommand::TimeoutExpired {
            deadline_id,
            label: ABRECHNUNG_WINDOW_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired on Settled must be absorbed without error");

    assert!(
        matches!(nb.state().await, AbrechnungState::Settled(_)),
        "state must remain Settled after late timeout"
    );
}

/// GPKE Abrechnung — late deadline absorbed on Disputed process.
///
/// Same absorption guarantee as the Settled case — terminals are immune.
#[tokio::test]
async fn e2e_gpke_abrechnung_late_timeout_absorbed_on_disputed() {
    let nb = MockNb::new();

    nb.receive_invoic(31002, "INVOIC-DISP-LATE-001", true).await;
    nb.dispute_invoice("Rechnungsbetrag nicht nachvollziehbar")
        .await;
    assert!(matches!(nb.state().await, AbrechnungState::Disputed { .. }));

    let deadline_id = DeadlineId::new();
    nb.process
        .execute(AbrechnungCommand::TimeoutExpired {
            deadline_id,
            label: ABRECHNUNG_WINDOW_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired on Disputed must be absorbed without error");

    assert!(
        matches!(nb.state().await, AbrechnungState::Disputed { .. }),
        "state must remain Disputed after late timeout"
    );
}
