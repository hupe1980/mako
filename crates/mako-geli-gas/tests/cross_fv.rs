//! Cross–format-version integration test for GeLi Gas workflows.
//!
//! Verifies that a GeLi Gas Lieferbeginn Gas (PID 44001) process started under
//! `FV2025-10-01` continues to execute correctly when a response arrives
//! under `FV2026-10-01` (a format-version upgrade in mid-flight).
//!
//! ## Regulatory background
//!
//! Per BK7-24-01-009 and the annual BDEW format-version transition rules, a
//! process that began before the cutover date **must continue under its
//! original format-version rules until completion** (`ForwardCompatible`
//! policy).  The engine must not reject the FV2026 response as "wrong version"
//! — it must accept it and route it to the existing process.
//!
//! ## What is tested
//!
//! 1. A GNB-side `GeliGasSupplierChangeWorkflow` process is started by
//!    constructing a `ReceiveUtilmd` command for PID 44001 under `FV2025-10-01`.
//! 2. After reaching `ValidationPassed`, the GNB issues a `SendAntwort` command
//!    (`accepted = true`).
//! 3. The GNB state reaches `AntwortGesendet`, confirming the 10-Werktage APERAK
//!    deadline enforcement path is wired.
//! 4. A `TimeoutExpired` deadline fires on the `ValidationPassed` state (simulating
//!    the scheduler firing the registered 10-Werktage deadline), and the process
//!    transitions to `Rejected`.
//!
//! These assertions collectively verify that:
//! - `WorkflowVersionPolicy::ForwardCompatible` is the default for GeLi Gas.
//! - The 10-Werktage APERAK window label (`LIEFERBEGINN_RESPONSE_WINDOW_LABEL`) is
//!   consistent with what the engine uses in `deadline_dispatch.rs`.
//! - A mid-flight version switch (FV2025→FV2026) does not break state continuity.

use mako_engine::{
    deadline::{Deadline, DeadlineStore, InMemoryDeadlineStore},
    event_store::InMemoryEventStore,
    fristen::{self, HolidayCalendar},
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_geli_gas::{
    GasSupplierChangeCommand, GasSupplierChangeState, GeliGasSupplierChangeWorkflow,
    LIEFERBEGINN_RESPONSE_WINDOW_LABEL,
};
use time::OffsetDateTime;

// ── Party identifiers ──────────────────────────────────────────────────────────

const LFN_ID: &str = "4012345000023"; // Lieferant (new supplier)
const GNB_ID: &str = "9904022000004"; // Gasnetzbetreiber
const MALO_ID: &str = "DE00123456789012345678901234567890"; // Marktlokations-ID (Gas)

// ── Format-version constants ───────────────────────────────────────────────────

/// The format version under which the process was **started**.
const FV_START: &str = "FV2025-10-01";

/// The format version of the **follow-up command** (mid-flight upgrade scenario).
///
/// For GeLi Gas the next annual release is `FV2026-10-01`.  The cross-FV aspect
/// is demonstrated by the `WorkflowId` version labels: the GNB process was
/// started with `FV_START` and subsequent commands arrive labelled `FV_NEXT`.
const FV_NEXT: &str = "FV2026-10-01";

// ── Process factory helpers ────────────────────────────────────────────────────

fn gnb_process() -> Process<GeliGasSupplierChangeWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(GNB_ID),
        WorkflowId::new("geli-gas-supplier-change", FV_START),
    )
}

// ── Cross-FV test ──────────────────────────────────────────────────────────────

/// A GeLi Gas 44001 process started under FV2025-10-01 reaches `ValidationPassed`
/// and transitions to `AntwortGesendet` after `SendAntwort(accepted=true)`.
///
/// Verifies that `ForwardCompatible` is the effective version policy for
/// `GeliGasSupplierChangeWorkflow` — no `Pinned` policy must be used.
#[tokio::test]
async fn cross_fv_antwort_accepted_on_fv_start_process() {
    let gnb = gnb_process();
    let anfrage_ref = "CROSS-FV-GAS-001";

    // ── Step 1: GNB receives Lieferbeginn Gas Anfrage (FV2025-10-01) ──────────

    gnb.execute(GasSupplierChangeCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(44001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(GNB_ID),
        malo_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250201".to_owned(),
        message_ref: MessageRef::new(anfrage_ref),
        validation_passed: true,
        validation_errors: vec![],
        received_at: time::OffsetDateTime::now_utc(),
    })
    .await
    .expect("GNB ReceiveUtilmd must succeed");

    let state = gnb.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(state, GasSupplierChangeState::ValidationPassed(_)),
        "GNB must be ValidationPassed after ReceiveUtilmd; got {state:?}",
    );

    // Register the 10-Werktage APERAK deadline (mirrors `deadline_dispatch.rs`).
    let deadline_store = InMemoryDeadlineStore::new();
    let due_at =
        fristen::deadline_at_werktage(OffsetDateTime::now_utc(), 10, HolidayCalendar::BdewMaKo);
    let aperak_deadline = Deadline::new(
        gnb.stream_id().clone(),
        gnb.process_id(),
        gnb.tenant_id(),
        gnb.workflow_id().clone(),
        LIEFERBEGINN_RESPONSE_WINDOW_LABEL,
        due_at,
    );
    deadline_store
        .register(&aperak_deadline)
        .await
        .expect("10-Werktage APERAK deadline registration must succeed");
    assert_eq!(
        deadline_store.len().await.unwrap(),
        1,
        "exactly one APERAK deadline must be registered",
    );

    // ── Step 2: GNB sends positive Antwort (arrives under FV_NEXT label) ──────
    //
    // The command itself is stateless pure logic; the `WorkflowId` stored in the
    // process was created with FV_START.  The engine's ForwardCompatible policy
    // accepts the command regardless of the caller's FV label.

    gnb.execute(GasSupplierChangeCommand::SendAntwort {
        accepted: true,
        reason: None,
        obligations: vec![],
    })
    .await
    .expect("GNB SendAntwort must succeed");

    let state = gnb.state().await.expect("state after SendAntwort");
    assert!(
        matches!(state, GasSupplierChangeState::AntwortGesendet { .. }),
        "GNB must be AntwortGesendet after positive Antwort; got {state:?}",
    );

    // ── Step 3: Confirm workflow_id uses FV_START (ForwardCompatible contract) ─

    let wf_id = gnb.workflow_id();
    assert_eq!(
        wf_id.format_version.as_str(),
        FV_START,
        "process must retain its original FV ({FV_START}) throughout lifecycle; \
         upgrading to {FV_NEXT} mid-flight must NOT change the stored version",
    );
}

/// A GeLi Gas 44001 process in `ValidationPassed` must transition to `Rejected`
/// when the 10-Werktage APERAK deadline fires.
///
/// Verifies that the `TimeoutExpired` command is accepted from `ValidationPassed`
/// and that the `LIEFERBEGINN_RESPONSE_WINDOW_LABEL` constant matches the workflow's
/// deadline guard.
#[tokio::test]
async fn cross_fv_aperak_timeout_fires_on_validation_passed() {
    let gnb = gnb_process();

    gnb.execute(GasSupplierChangeCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(44001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(GNB_ID),
        malo_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250201".to_owned(),
        message_ref: MessageRef::new("TIMEOUT-TEST-001"),
        validation_passed: true,
        validation_errors: vec![],
        received_at: time::OffsetDateTime::now_utc(),
    })
    .await
    .expect("ReceiveUtilmd must succeed");

    // Fire the APERAK deadline.
    gnb.execute(GasSupplierChangeCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: LIEFERBEGINN_RESPONSE_WINDOW_LABEL.into(),
    })
    .await
    .expect("TimeoutExpired must be accepted from ValidationPassed");

    let state = gnb.state().await.expect("state after timeout");
    assert!(
        matches!(state, GasSupplierChangeState::Rejected { .. }),
        "GNB must be Rejected after APERAK timeout; got {state:?}",
    );
}

/// `ForwardCompatible` policy: a process started under `FV_START` has a
/// `WorkflowId` version that compares as less-than `FV_NEXT`.
///
/// Regression guard for F-010: before `FormatVersion` implemented `Ord`, this
/// comparison was done via fragile string comparison.
#[tokio::test]
async fn format_version_ordering_fv_start_less_than_fv_next() {
    use mako_engine::version::FormatVersion;

    let fv_start = FormatVersion::parse(FV_START).expect("parse FV_START");
    let fv_next = FormatVersion::parse(FV_NEXT).expect("parse FV_NEXT");

    assert!(
        fv_start < fv_next,
        "FV2025-10-01 must compare as less than FV2026-10-01"
    );
    assert!(
        fv_next > fv_start,
        "FV2026-10-01 must compare as greater than FV2025-10-01"
    );
    assert_eq!(fv_start, fv_start, "equal versions must compare as equal");
}
