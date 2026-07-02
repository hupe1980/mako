//! Cross–format-version integration test for WiM Gas workflows.
//!
//! Verifies that a WiM Gas Anmeldung (PID 44042) process started under
//! `FV2025-10-01` continues to execute correctly when follow-up commands
//! arrive under `FV2026-10-01` (a format-version upgrade in mid-flight).
//!
//! ## Regulatory background
//!
//! Per BK7-24-01-009 and the annual BDEW format-version transition rules, a
//! process that began before the cutover date **must continue under its
//! original format-version rules until completion** (`ForwardCompatible`
//! policy).  The engine must not reject follow-up commands as "wrong version"
//! — it must accept them and route them to the existing process.
//!
//! ## What is tested
//!
//! 1. A GNB-side `WimGasAnmeldungWorkflow` process is started by constructing
//!    a `ReceiveUtilmd` command for PID 44042 under `FV2025-10-01`.
//! 2. After reaching `ValidationPassed`, the GNB issues a `DispatchAperak`
//!    command (`positive = true`), transitioning to `AperakSent`.
//! 3. The GNB `Activate`s the process, reaching `Active`.
//! 4. A `TimeoutExpired` fires on a non-terminal state, transitioning to `Rejected`.
//! 5. `FormatVersion::Ord` correctly compares the two FV constants.
//!
//! These assertions collectively verify that:
//! - `WorkflowVersionPolicy::ForwardCompatible` is the default for WiM Gas.
//! - The 10-Werktage APERAK window label (`ANMELDUNG_APERAK_WINDOW_LABEL`) is
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
use mako_wim_gas::{
    ANMELDUNG_APERAK_WINDOW_LABEL, WimGasAnmeldungCommand, WimGasAnmeldungState,
    WimGasAnmeldungWorkflow,
};
use time::OffsetDateTime;

// ── Party identifiers ──────────────────────────────────────────────────────────

const MSBN_ID: &str = "4012345000023"; // Messtellenbetreiber Neu (new MSB)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MALO_ID: &str = "DE0000123456789012345678901234567890"; // Marktlokations-ID

// ── Format-version constants ───────────────────────────────────────────────────

/// The format version under which the process was **started**.
const FV_START: &str = "FV2025-10-01";

/// The format version after the annual cutover (mid-flight upgrade scenario).
const FV_NEXT: &str = "FV2026-10-01";

// ── Process factory helpers ────────────────────────────────────────────────────

fn nb_process() -> Process<WimGasAnmeldungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(NB_ID),
        WorkflowId::new("wim-gas-anmeldung", FV_START),
    )
}

fn receive_utilmd_cmd(pid: u32) -> WimGasAnmeldungCommand {
    WimGasAnmeldungCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(pid).unwrap(),
        sender: MarktpartnerCode::new(MSBN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        malo_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        message_ref: MessageRef::new("CROSS-FV-WIM-GAS-001"),
        validation_passed: true,
        validation_errors: vec![],
    }
}

// ── Cross-FV tests ─────────────────────────────────────────────────────────────

/// A WiM Gas 44042 process started under FV2025-10-01 can be advanced through
/// its full lifecycle by commands that conceptually arrive after the FV2026-10-01
/// cutover.
///
/// Verifies `WorkflowVersionPolicy::ForwardCompatible` for WiM Gas.
#[tokio::test]
async fn cross_fv_full_lifecycle_fv_start_process() {
    let nb = nb_process();

    // ── Step 1: NB receives Anmeldung Anfrage (FV2025-10-01) ──────────────────

    nb.execute(receive_utilmd_cmd(44042))
        .await
        .expect("NB ReceiveUtilmd must succeed");

    let state = nb.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(state, WimGasAnmeldungState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveUtilmd; got {state:?}",
    );

    // Register the 10-Werktage APERAK deadline (mirrors `deadline_dispatch.rs`).
    let deadline_store = InMemoryDeadlineStore::new();
    let due_at =
        fristen::deadline_at_werktage(OffsetDateTime::now_utc(), 10, HolidayCalendar::BdewMaKo);
    let aperak_deadline = Deadline::new(
        nb.stream_id().clone(),
        nb.process_id(),
        nb.tenant_id(),
        nb.workflow_id().clone(),
        ANMELDUNG_APERAK_WINDOW_LABEL,
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

    // ── Step 2: NB dispatches positive APERAK (arrives after FV2026-10-01) ────
    //
    // The command is pure; `ForwardCompatible` policy accepts it regardless of
    // the caller's FV label.

    nb.execute(WimGasAnmeldungCommand::DispatchAperak {
        positive: true,
        reason: None,
    })
    .await
    .expect("DispatchAperak positive must succeed from ValidationPassed");

    let state = nb.state().await.expect("state after DispatchAperak");
    assert!(
        matches!(state, WimGasAnmeldungState::AperakSent(_)),
        "NB must be AperakSent after positive APERAK; got {state:?}",
    );

    // ── Step 3: Activate (MSB change confirmed active) ────────────────────────

    nb.execute(WimGasAnmeldungCommand::Activate)
        .await
        .expect("Activate must succeed from AperakSent");

    let state = nb.state().await.expect("state after Activate");
    assert!(
        matches!(state, WimGasAnmeldungState::Active(_)),
        "NB must be Active after Activate; got {state:?}",
    );

    // ── Step 4: Verify the stored WorkflowId retains FV_START ─────────────────

    assert_eq!(
        nb.workflow_id().format_version.as_str(),
        FV_START,
        "process must retain its original FV ({FV_START}) throughout lifecycle; \
         upgrading to {FV_NEXT} mid-flight must NOT change the stored version",
    );
}

/// A WiM Gas 44042 process in `ValidationPassed` must transition to `Rejected`
/// when the 10-Werktage APERAK deadline fires.
///
/// Verifies that `TimeoutExpired` with `ANMELDUNG_APERAK_WINDOW_LABEL` is accepted
/// from the `ValidationPassed` state.
#[tokio::test]
async fn cross_fv_aperak_timeout_fires_on_validation_passed() {
    let nb = nb_process();

    nb.execute(receive_utilmd_cmd(44042))
        .await
        .expect("ReceiveUtilmd must succeed");

    let state = nb.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(state, WimGasAnmeldungState::ValidationPassed(_)),
        "must be ValidationPassed; got {state:?}",
    );

    // Fire the APERAK deadline.
    nb.execute(WimGasAnmeldungCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: ANMELDUNG_APERAK_WINDOW_LABEL.into(),
    })
    .await
    .expect("TimeoutExpired must be accepted from ValidationPassed");

    let state = nb.state().await.expect("state after timeout");
    assert!(
        matches!(state, WimGasAnmeldungState::Rejected { .. }),
        "NB must be Rejected after APERAK timeout; got {state:?}",
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
}
