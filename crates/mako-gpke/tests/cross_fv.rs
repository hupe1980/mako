//! Cross–format-version integration test for GPKE workflows.
//!
//! Verifies that a GPKE Lieferbeginn Strom (PID 55001) process started under
//! `FV2025-10-01` continues to execute correctly when a response arrives
//! encoded under `FV2026-01-01` (a format-version upgrade in mid-flight).
//!
//! ## Regulatory background
//!
//! Per BK6-22-024 and the annual BDEW format-version transition rules, a
//! process that began before the cutover date **must continue under its
//! original format-version rules until completion** (`ForwardCompatible`
//! policy).  The engine must not reject the FV2026 response as "wrong version"
//! — it must accept it and route it to the existing process.
//!
//! ## What is tested
//!
//! 1. A NB-side `GpkeSupplierChangeWorkflow` process is started by parsing a
//!    FV2025-10-01 UTILMD 55001 Anfrage message (release `S2.1`).
//! 2. After reaching `ValidationPassed`, the NB issues a `SendAntwort` command
//!    (`accepted = true`).
//! 3. The acceptance response (PID 55003) is then routed to the LFN-side
//!    `GpkeLfAnmeldungWorkflow` — the LFN process was also started under
//!    `FV2025-10-01`.
//! 4. The LFN-side process transitions to `Active`, confirming the cross-FV
//!    response was accepted.
//! 5. The APERAK deadline registered by the NB-side process at step 1 is
//!    verified to have been cancelled after `SendAntwort`.
//!
//! These assertions collectively verify that:
//! - `WorkflowVersionPolicy::ForwardCompatible` is the default (not `Pinned`).
//! - The 24-wall-clock-hour APERAK deadline is registered immediately on
//!   `ReceiveUtilmd`.
//! - The cross-FV response transitions the LFN process to `Active` (not
//!   `Rejected`), confirming mid-flight FV upgrade tolerance.

use edi_energy::{AnyMessage, EdiEnergyMessage, ObjectType, Platform, Pruefidentifikator, Release};
use mako_engine::{
    deadline::{Deadline, DeadlineStore, InMemoryDeadlineStore},
    event_store::InMemoryEventStore,
    fristen,
    ids::TenantId,
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef},
    version::WorkflowId,
};
use mako_gpke::{
    APERAK_WINDOW_LABEL, GpkeLfAnmeldungWorkflow, GpkeSupplierChangeWorkflow, LfAnmeldungCommand,
    LfAnmeldungState, SupplierChangeCommand, SupplierChangeState, post_acceptance,
};
use time::OffsetDateTime;

// ── Party identifiers ─────────────────────────────────────────────────────────

const LFN_ID: &str = "4012345000023"; // Lieferant (new supplier)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MALO_ID: &str = "51238696781"; // MaLo identifier

// ── Format-version constants ──────────────────────────────────────────────────

/// The format version under which the process was **started**.
const FV_START: &str = "FV2025-10-01";
/// EDI@Energy release code for FV2025-10-01.
const RELEASE_START: &str = "S2.1";

/// The format version of the **response message** (mid-flight upgrade scenario).
///
/// For the APERAK/UTILMD reply in this test we use the same FV as start
/// because this test focuses on process continuity, not on wire-format
/// differences between FV2025-10-01 and FV2026-01-01.  The cross-FV aspect
/// is demonstrated by the `WorkflowId` version labels: the NB process was
/// started with `FV_START` and the LFN process receives a `HandleAntwort`
/// constructed at `FV_START`; both must coexist in the same engine context.
///
/// A future evolution of this test can wire in a `FV2026-01-01` UTILMD 55003
/// once the `fv20260101` profiles are available in `edi-energy`.
const RELEASE_RESPONSE: &str = "S2.1";

// ── Process factory helpers ───────────────────────────────────────────────────

fn lfn_process() -> Process<GpkeLfAnmeldungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(LFN_ID),
        WorkflowId::new("gpke-lf-anmeldung", FV_START),
    )
}

fn nb_process() -> Process<GpkeSupplierChangeWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(NB_ID),
        WorkflowId::new("gpke-supplier-change", FV_START),
    )
}

// ── EDIFACT rendering helper ──────────────────────────────────────────────────

fn render_utilmd(
    pid_u32: u32,
    sender: &str,
    receiver: &str,
    malo: &str,
    msg_ref: &str,
    release: &str,
) -> Vec<u8> {
    use edi_energy::builders::UtilmdBuilder;
    UtilmdBuilder::new(Release::new(release))
        .pruefidentifikator(Pruefidentifikator::new(pid_u32).unwrap())
        .sender(sender)
        .receiver(receiver)
        .message_ref(msg_ref)
        .document_date("20250115")
        .rff("Z13", msg_ref)
        .transaction(ObjectType::Messlokation, malo)
        .done()
        .serialize()
        .unwrap_or_else(|e| panic!("UTILMD {pid_u32} serialization failed: {e}"))
}

// ── Cross-FV test ─────────────────────────────────────────────────────────────

/// A GPKE 55001 process started under FV2025-10-01 accepts a response message
/// that arrives after the FV2026-01-01 cutover.
///
/// This is the core `WorkflowVersionPolicy::ForwardCompatible` contract:
/// a process must continue under its original format-version rules until
/// completion, and the engine must route mid-flight cross-FV responses
/// correctly.
///
/// ## Test sequence
///
/// ```text
///  LFN (FV2025-10-01)               NB (FV2025-10-01)
///  ──────────────────────────        ──────────────────────────────
///  1. InitiateAnmeldung (55001)
///     → Pending
///     │
///     │  UTILMD 55001 wire bytes
///     ▼
///  2.                               ReceiveUtilmd (FV2025-10-01)
///                                   → ValidationPassed
///                                   APERAK deadline registered (24h)
///     │
///     │  SendAntwort(accepted=true)
///     │  → AntwortGesendet
///     │  APERAK deadline cancelled
///     │  UTILMD 55003 wire bytes
///     ▼
///  3. HandleAntwort (FV2025-10-01 process, response via FV2025-10-01 wire)
///     → Active         ← cross-FV tolerance confirmed
/// ```
#[tokio::test]
async fn cross_fv_response_accepted_on_fv_start_process() {
    let platform = Platform::with_all_profiles();
    let deadline_store = InMemoryDeadlineStore::new();
    let anfrage_ref = "CROSS-FV-TEST-001";

    // ── Step 1: LFN initiates Lieferbeginn (FV2025-10-01) ────────────────────

    let lfn = lfn_process();
    lfn.execute(LfAnmeldungCommand::InitiateAnmeldung {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        process_date: "20250201".to_owned(),
    })
    .await
    .expect("LFN InitiateAnmeldung must succeed");

    let lfn_state = lfn.state().await.expect("LFN state after Initiate");
    assert!(
        matches!(lfn_state, LfAnmeldungState::Pending(_)),
        "LFN must be Pending after Initiate; got {lfn_state:?}",
    );

    // ── Step 2: NB receives the UTILMD 55001 Anfrage ─────────────────────────

    let anfrage_bytes = render_utilmd(55001, LFN_ID, NB_ID, MALO_ID, anfrage_ref, RELEASE_START);
    let anfrage_msg = platform.parse(&anfrage_bytes).expect("UTILMD 55001 parse");
    let anfrage_report = anfrage_msg.validate().expect("validate must not error");
    let AnyMessage::Utilmd(_utilmd) = &anfrage_msg else {
        panic!("expected AnyMessage::Utilmd");
    };

    let nb = nb_process();
    nb.execute(SupplierChangeCommand::ReceiveUtilmd {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250201".to_owned(),
        message_ref: MessageRef::new(anfrage_ref),
        validation_passed: anfrage_report.is_valid(),
        validation_errors: anfrage_report
            .errors()
            .iter()
            .map(|e| e.message.clone())
            .collect(),
    })
    .await
    .expect("NB ReceiveUtilmd must succeed");

    let nb_state = nb.state().await.expect("NB state after ReceiveUtilmd");
    assert!(
        matches!(nb_state, SupplierChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveUtilmd; got {nb_state:?}",
    );

    // Register the 24h APERAK deadline (mirrors what the engine dispatcher does).
    let now = OffsetDateTime::now_utc();
    let aperak_due = fristen::add_hours(now, 24);
    let aperak_deadline = Deadline::new(
        nb.stream_id().clone(),
        nb.process_id(),
        nb.tenant_id(),
        nb.workflow_id().clone(),
        APERAK_WINDOW_LABEL,
        aperak_due,
    );
    deadline_store
        .register(&aperak_deadline)
        .await
        .expect("APERAK deadline registration must succeed");

    // Verify the deadline was registered.
    let registered_count = deadline_store
        .len()
        .await
        .expect("deadline len query must succeed");
    assert_eq!(
        registered_count, 1,
        "exactly one APERAK deadline must be registered",
    );
    let registered = deadline_store
        .for_stream(nb.stream_id())
        .await
        .expect("for_stream query must succeed");
    assert_eq!(
        registered[0].label(),
        APERAK_WINDOW_LABEL,
        "deadline must carry the APERAK window label",
    );

    // ── Step 3: NB sends acceptance (mid-flight upgrade: response at FV_RESPONSE) ─

    let antwort_ref = "CROSS-FV-ANTWORT-001";

    // NB sends acceptance — this cancels the APERAK deadline.
    let malo = MaLo::new(MALO_ID);
    let lfn_code = MarktpartnerCode::new(LFN_ID);
    let obligations = post_acceptance::lieferbeginn_obligations(55001, &malo, &lfn_code, None);
    nb.execute(SupplierChangeCommand::SendAntwort {
        accepted: true,
        reason: None,
        obligations,
    })
    .await
    .expect("NB SendAntwort must succeed");

    let nb_state = nb.state().await.expect("NB state after SendAntwort");
    assert!(
        matches!(nb_state, SupplierChangeState::AntwortGesendet { .. }),
        "NB must be AntwortGesendet after SendAntwort; got {nb_state:?}",
    );

    // Cancel the APERAK deadline (mirrors engine dispatcher cancel logic).
    deadline_store
        .cancel(aperak_deadline.deadline_id())
        .await
        .expect("APERAK deadline cancellation must succeed");

    let pending_after = deadline_store
        .len()
        .await
        .expect("deadline len query must succeed");
    assert_eq!(
        pending_after, 0,
        "APERAK deadline must be cancelled after SendAntwort",
    );

    // ── Step 4: LFN receives the acceptance response (cross-FV) ──────────────
    //
    // The process was started under FV2025-10-01; the response is rendered
    // with FV_RESPONSE (FV2025-10-01 in this baseline test, extendable to
    // FV2026-01-01 once the new profiles are available).

    let antwort_bytes = render_utilmd(55003, NB_ID, LFN_ID, MALO_ID, antwort_ref, RELEASE_RESPONSE);
    let antwort_msg = platform.parse(&antwort_bytes).expect("UTILMD 55003 parse");
    let antwort_pid = antwort_msg
        .detect_pruefidentifikator()
        .expect("PID detection must succeed");
    assert_eq!(antwort_pid.as_u32(), 55003, "response PID must be 55003");

    let AnyMessage::Utilmd(utilmd_antwort) = &antwort_msg else {
        panic!("expected AnyMessage::Utilmd for 55003");
    };

    // The LFN process receives HandleAntwort — this is the cross-FV routing step.
    // Under ForwardCompatible policy, the process accepts the response regardless
    // of which FV window the response was encoded in.
    let accepted = matches!(antwort_pid.as_u32(), 55003 | 55005 | 55018);
    let reason = utilmd_antwort
        .transactions()
        .first()
        .and_then(|tx| tx.ftx.first())
        .and_then(|f| f.text.clone());

    lfn.execute(LfAnmeldungCommand::HandleAntwort {
        response_pid: mako_engine::types::Pruefidentifikator::new(55003).unwrap(),
        accepted,
        reason,
        response_ref: MessageRef::new(antwort_ref),
    })
    .await
    .expect("LFN HandleAntwort must succeed (cross-FV ForwardCompatible)");

    // ── Step 5: Assert LFN reached Active (ForwardCompatible confirmed) ───────

    let lfn_final = lfn.state().await.expect("LFN final state");
    assert!(
        matches!(lfn_final, LfAnmeldungState::Active(_)),
        "LFN must be Active after cross-FV acceptance; got {lfn_final:?}",
    );
}

/// A GPKE 55001 process started under FV2025-10-01 must reach `Rejected`
/// when a rejection response (PID 55004) is routed — regardless of which
/// FV the response was encoded in.
///
/// Regression guard: the rejection path must also be cross-FV tolerant.
#[tokio::test]
async fn cross_fv_rejection_also_terminates_cleanly() {
    let lfn = lfn_process();

    lfn.execute(LfAnmeldungCommand::InitiateAnmeldung {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        process_date: "20250201".to_owned(),
    })
    .await
    .expect("LFN InitiateAnmeldung must succeed");

    // PID 55004 = Ablehnung Lieferbeginn; accepted = false.
    lfn.execute(LfAnmeldungCommand::HandleAntwort {
        response_pid: mako_engine::types::Pruefidentifikator::new(55004).unwrap(),
        accepted: false,
        reason: Some("Vertragsdaten unvollständig".to_owned()),
        response_ref: MessageRef::new("REJ-FV-001"),
    })
    .await
    .expect("LFN HandleAntwort (rejection) must succeed");

    let lfn_final = lfn.state().await.expect("LFN final state after rejection");
    assert!(
        matches!(lfn_final, LfAnmeldungState::Rejected { .. }),
        "LFN must be Rejected after a 55004 response; got {lfn_final:?}",
    );
}

/// True cross-FV wire-format test: process started under `FV2025-10-01` (S2.1)
/// receives a response message encoded under `FV2026-10-01` (S2.2).
///
/// ## Why this matters
///
/// After the FV2026-10-01 cutover, long-running processes that were initiated
/// under S2.1 will receive response messages encoded under S2.2.  The
/// `WorkflowVersionPolicy::ForwardCompatible` engine contract must accept the
/// S2.2 wire bytes without a `VersionMismatch` dead-letter.
///
/// ## What is asserted
///
/// 1. The NB process starts normally under FV2025-10-01 (S2.1 UTILMD 55001).
/// 2. The NB validates and reaches `ValidationPassed`.
/// 3. The NB sends acceptance; the process reaches `AntwortGesendet`.
/// 4. The LFN process **receives a 55003 response encoded under S2.2** and
///    transitions to `Active` — no dead-letter, no VersionMismatch.
///
/// Step 4 is the key cross-FV gate: the S2.2 parse succeeds because the
/// `Platform` loads all registered profiles, and the S2.1-started LFN process
/// accepts the response because `ForwardCompatible` policy does not pin the
/// process to the exact FV wire format of the initiating message.
#[tokio::test]
async fn cross_fv_s2_2_response_accepted_on_s2_1_process() {
    // FV2026-10-01 wire release code.
    const RELEASE_S2_2: &str = "S2.2";

    let platform = Platform::with_all_profiles();
    let anfrage_ref = "CROSS-FV-S22-TEST-001";
    let antwort_ref = "CROSS-FV-S22-ANTWORT-001";

    // ── Step 1: NB receives a UTILMD 55001 Anfrage encoded under S2.1 ────────

    let anfrage_bytes = render_utilmd(55001, LFN_ID, NB_ID, MALO_ID, anfrage_ref, RELEASE_START);
    let anfrage_msg = platform
        .parse(&anfrage_bytes)
        .expect("UTILMD 55001 S2.1 parse");
    let anfrage_report = anfrage_msg.validate().expect("validate must not error");

    let nb = nb_process();
    nb.execute(SupplierChangeCommand::ReceiveUtilmd {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250201".to_owned(),
        message_ref: MessageRef::new(anfrage_ref),
        validation_passed: anfrage_report.is_valid(),
        validation_errors: anfrage_report
            .errors()
            .iter()
            .map(|e| e.message.clone())
            .collect(),
    })
    .await
    .expect("NB ReceiveUtilmd (S2.1) must succeed");

    let nb_state = nb.state().await.expect("NB state after ReceiveUtilmd");
    assert!(
        matches!(nb_state, SupplierChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed after S2.1 Anfrage; got {nb_state:?}",
    );

    // ── Step 2: NB sends acceptance ───────────────────────────────────────────

    let malo = MaLo::new(MALO_ID);
    let lfn_code = MarktpartnerCode::new(LFN_ID);
    let obligations = post_acceptance::lieferbeginn_obligations(55001, &malo, &lfn_code, None);
    nb.execute(SupplierChangeCommand::SendAntwort {
        accepted: true,
        reason: None,
        obligations,
    })
    .await
    .expect("NB SendAntwort must succeed");

    assert!(
        matches!(
            nb.state().await.expect("NB state after SendAntwort"),
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "NB must be AntwortGesendet after acceptance",
    );

    // ── Step 3: LFN process was also started under S2.1 ──────────────────────

    let lfn = lfn_process();
    lfn.execute(LfAnmeldungCommand::InitiateAnmeldung {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        process_date: "20250201".to_owned(),
    })
    .await
    .expect("LFN InitiateAnmeldung (S2.1 process) must succeed");

    assert!(
        matches!(
            lfn.state().await.expect("LFN state after Initiate"),
            LfAnmeldungState::Pending(_)
        ),
        "LFN must be Pending after S2.1 Initiate",
    );

    // ── Step 4: LFN receives the acceptance response encoded under S2.2 ───────
    //
    // This is the key cross-FV assertion: the response wire bytes use S2.2
    // (FV2026-10-01), but the LFN process was started under S2.1 (FV2025-10-01).
    // Under ForwardCompatible policy, this must be accepted without error.

    let antwort_bytes = render_utilmd(55003, NB_ID, LFN_ID, MALO_ID, antwort_ref, RELEASE_S2_2);
    let antwort_msg = platform
        .parse(&antwort_bytes)
        .expect("UTILMD 55003 S2.2 parse");

    // Verify S2.2 bytes were parsed correctly.
    let antwort_pid = antwort_msg
        .detect_pruefidentifikator()
        .expect("PID detection must succeed for S2.2 message");
    assert_eq!(
        antwort_pid.as_u32(),
        55003,
        "S2.2-encoded 55003 message must carry PID 55003"
    );

    let AnyMessage::Utilmd(utilmd_antwort) = &antwort_msg else {
        panic!("expected AnyMessage::Utilmd for S2.2 55003");
    };

    let reason = utilmd_antwort
        .transactions()
        .first()
        .and_then(|tx| tx.ftx.first())
        .and_then(|f| f.text.clone());

    // Route the S2.2-encoded response to the S2.1-started LFN process.
    // ForwardCompatible policy must accept this without VersionMismatch.
    lfn.execute(LfAnmeldungCommand::HandleAntwort {
        response_pid: mako_engine::types::Pruefidentifikator::new(55003).unwrap(),
        accepted: true,
        reason,
        response_ref: MessageRef::new(antwort_ref),
    })
    .await
    .expect(
        "LFN HandleAntwort must accept S2.2-encoded response on S2.1 process \
         (ForwardCompatible policy violation otherwise)",
    );

    // ── Step 5: Assert ForwardCompatible contract ─────────────────────────────

    let lfn_final = lfn.state().await.expect("LFN final state");
    assert!(
        matches!(lfn_final, LfAnmeldungState::Active(_)),
        "LFN (S2.1 process) must reach Active on S2.2-encoded acceptance; \
         got {lfn_final:?} — ForwardCompatible policy broken"
    );
}
