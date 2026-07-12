//! Bilateral E2E integration test: LFN ↔ NB Lieferbeginn Strom (PID 55001).
//!
//! Two in-memory engine instances — one acting as the Lieferant (LFN), one as
//! the Netzbetreiber (NB) — exchange messages in the correct GPKE sequence and
//! both reach their terminal states.
//!
//! ```text
//!  LFN Engine                            NB Engine
//!  ──────────────────────────────────    ──────────────────────────────────
//!  1. InitiateAnmeldung (55001)
//!     → Pending state
//!     → WorkflowOutput::outbox: UTILMD 55001 payload
//!        │
//!        │  render outbox payload → EDIFACT wire bytes  (UtilmdBuilder)
//!        ▼
//!  2.                                    parse & validate UTILMD 55001 bytes
//!                                        ReceiveUtilmd command
//!                                        → ValidationPassed state
//!                                        register 24h APERAK deadline
//!        │
//!        │  SendAntwort(accepted=true)
//!        │  → AntwortGesendet state
//!        │  → WorkflowOutput::outbox: MSCONS 13015 + ORDERS 17134
//!        │
//!        │  render UTILMD 55003 → EDIFACT wire bytes  (UtilmdBuilder)
//!        │  cancel APERAK deadline
//!        ▼
//!  3. parse UTILMD 55003 bytes
//!     HandleAntwort(accepted=true)
//!     → Active state
//! ```
//!
//! This test covers fristen wiring (24h APERAK deadline).
//!
//! # Design note
//!
//! [`Process::execute`] returns `Vec<EventEnvelope>` (persists events only).
//! Outbox entries live in [`WorkflowOutput::outbox`] and are inspected by
//! calling [`Workflow::handle`] directly — it is a pure function that needs no
//! store, making outbox assertions zero-infrastructure.

use edi_energy::builders::UtilmdBuilder;
use edi_energy::{AnyMessage, EdiEnergyMessage, ObjectType, Platform, Pruefidentifikator, Release};
use mako_engine::{
    deadline::{Deadline, DeadlineStore, InMemoryDeadlineStore},
    event_store::InMemoryEventStore,
    fristen,
    ids::TenantId,
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef},
    version::WorkflowId,
    workflow::Workflow,
};
use mako_gpke::{
    GPKE_PROCESS_RESPONSE_LABEL, GpkeLfAnmeldungWorkflow, GpkeSupplierChangeWorkflow,
    LfAnmeldungCommand, LfAnmeldungState, NB_RESPONSE_WINDOW_LABEL, SupplierChangeCommand,
    SupplierChangeState, post_acceptance,
};
use time::OffsetDateTime;

// ── Party identifiers (BDEW codes, DE 3055 agency "293") ─────────────────────

const LFN_ID: &str = "4012345000023"; // Lieferant (new supplier)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MSB_ID: &str = "9900111222333"; // Messstellenbetreiber
const MALO_ID: &str = "51238696781"; // MaLo identifier

// Format version used throughout this test.
const FV: &str = "FV2025-10-01";
// EDI@Energy release code matching the FV above.
const RELEASE: &str = "S2.1";

// ── Process factory helpers ───────────────────────────────────────────────────

fn lfn_process() -> Process<GpkeLfAnmeldungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(LFN_ID),
        WorkflowId::new("gpke-lf-anmeldung", FV),
    )
}

fn nb_process() -> Process<GpkeSupplierChangeWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::from_party_id(NB_ID),
        WorkflowId::new("gpke-supplier-change", FV),
    )
}

// ── EDIFACT rendering ─────────────────────────────────────────────────────────
//
// Replicates the minimal logic of `services/makod/src/edifact_renderer.rs`
// inline so the test is independent of the `makod` binary crate.

/// Render a UTILMD to EDIFACT wire bytes from domain fields.
fn render_utilmd(pid_u32: u32, sender: &str, receiver: &str, malo: &str, msg_ref: &str) -> Vec<u8> {
    UtilmdBuilder::new(Release::new(RELEASE))
        .pruefidentifikator(Pruefidentifikator::new(pid_u32).unwrap())
        .sender(sender)
        .receiver(receiver)
        .message_ref(msg_ref)
        .document_date("20250115")
        .rff("Z13", msg_ref)
        .transaction(ObjectType::Marktlokation, malo)
        .done()
        .serialize()
        .unwrap_or_else(|e| panic!("UTILMD {pid_u32} serialization failed: {e}"))
}

// ── Field extraction helper ───────────────────────────────────────────────────

/// Parse EDIFACT bytes on the receiving side and return the domain fields
/// needed to build a `ReceiveUtilmd` command.
fn extract_utilmd_fields(
    platform: &Platform,
    bytes: &[u8],
) -> (
    mako_engine::types::Pruefidentifikator,
    MarktpartnerCode,
    MarktpartnerCode,
    MaLo,
    MessageRef,
    bool,
    Vec<String>,
) {
    let msg = platform.parse(bytes).expect("UTILMD must parse");
    let report = msg.validate().expect("validate() must not error");
    let pid_raw = msg
        .detect_pruefidentifikator()
        .expect("PID detection must succeed");
    let pid = mako_engine::types::Pruefidentifikator::new(pid_raw.as_u32()).unwrap();

    let AnyMessage::Utilmd(utilmd) = &msg else {
        panic!("expected AnyMessage::Utilmd");
    };

    let sender = MarktpartnerCode::new(
        utilmd
            .sender()
            .and_then(|n| n.party_id.as_deref())
            .unwrap_or("UNKNOWN"),
    );
    let receiver = MarktpartnerCode::new(
        utilmd
            .receiver()
            .and_then(|n| n.party_id.as_deref())
            .unwrap_or("UNKNOWN"),
    );
    let malo = MaLo::new(
        utilmd
            .transactions()
            .first()
            .and_then(|tx| tx.ide.object_id.as_deref())
            .unwrap_or("MALO-UNKNOWN"),
    );
    let msg_ref = MessageRef::new(
        utilmd
            .references()
            .iter()
            .next()
            .and_then(|r| r.rff.reference.as_deref())
            .unwrap_or("REF-UNKNOWN"),
    );
    let errors: Vec<String> = report.errors().iter().map(|e| e.message.clone()).collect();

    (
        pid,
        sender,
        receiver,
        malo,
        msg_ref,
        report.is_valid(),
        errors,
    )
}

// ── Happy-path bilateral test ─────────────────────────────────────────────────

/// Full bilateral Lieferbeginn Strom (PID 55001) — acceptance path.
///
/// Exercises,
///.
#[tokio::test]
async fn bilateral_lieferbeginn_strom_happy_path() {
    let platform = Platform::with_all_profiles();
    let deadline_store = InMemoryDeadlineStore::new();
    let anfrage_ref = "MSG-LFN-2025-001";

    // ── 1. LFN initiates Lieferbeginn ─────────────────────────────────────────

    let lfn = lfn_process();
    let initiate_cmd = LfAnmeldungCommand::InitiateAnmeldung {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        process_date: "20250201".to_owned(),
    };

    // Inspect outbox via the pure Workflow::handle — no store required.
    let lfn_output = GpkeLfAnmeldungWorkflow::handle(&LfAnmeldungState::New, initiate_cmd.clone())
        .expect("InitiateAnmeldung must succeed");

    assert_eq!(
        lfn_output.outbox.len(),
        1,
        "one UTILMD outbox entry expected"
    );
    let pending = &lfn_output.outbox[0];
    assert_eq!(pending.message_type.as_ref(), "UTILMD");
    assert_eq!(pending.recipient.as_ref(), NB_ID);
    assert_eq!(pending.payload["pid"].as_u64().unwrap(), 55001);
    assert_eq!(pending.payload["sender"].as_str().unwrap(), LFN_ID);
    assert_eq!(pending.payload["receiver"].as_str().unwrap(), NB_ID);
    assert_eq!(pending.payload["malo"].as_str().unwrap(), MALO_ID);

    // Also persist via Process::execute so state advances.
    lfn.execute(initiate_cmd)
        .await
        .expect("LFN execute Initiate");

    assert!(
        matches!(lfn.state().await.unwrap(), LfAnmeldungState::Pending(_)),
        "LFN must be Pending after InitiateAnmeldung",
    );

    // ── 2. AS4 sender renders the outbox payload → EDIFACT wire bytes ─────────
    //
    // In production `edifact_renderer::render_to_wire_bytes` does this from
    // the OutboxMessage.payload JSON.  We render inline from the PendingOutbox.

    let utilmd_55001_bytes = render_utilmd(
        pending.payload["pid"].as_u64().unwrap() as u32,
        pending.payload["sender"].as_str().unwrap(),
        pending.payload["receiver"].as_str().unwrap(),
        pending.payload["malo"].as_str().unwrap(),
        anfrage_ref,
    );

    // Quick sanity: rendered bytes must parse back to PID 55001.
    assert_eq!(
        platform
            .parse(&utilmd_55001_bytes)
            .unwrap()
            .detect_pruefidentifikator()
            .unwrap()
            .as_u32(),
        55001,
    );

    // ── 3. NB receives the UTILMD 55001 ──────────────────────────────────────

    let nb = nb_process();
    let (
        nb_pid,
        nb_sender,
        nb_receiver,
        nb_malo,
        nb_msg_ref,
        _validation_passed,
        _validation_errors,
    ) = extract_utilmd_fields(&platform, &utilmd_55001_bytes);

    nb.execute(SupplierChangeCommand::ReceiveUtilmd {
        pid: nb_pid,
        sender: nb_sender,
        receiver: nb_receiver,
        location_id: nb_malo,
        document_date: "20250115".to_owned(),
        process_date: "20250301".to_owned(),
        message_ref: nb_msg_ref,
        // Force validation_passed=true: this test exercises the bilateral state-machine
        // flow (routing, commands, outbox), not AHB profile conformance.  Profile
        // validation is exercised separately in crates/edi-energy/tests/conformance.rs.
        received_at: time::OffsetDateTime::now_utc(),
        validation_passed: true,
        validation_errors: vec![],
        bilanzierungsgebiet: None,
        bilanzierungsmethode: None,
        fallgruppe: None,
    })
    .await
    .expect("NB ReceiveUtilmd must succeed");

    let nb_state: SupplierChangeState = nb.state().await.unwrap();
    assert!(
        matches!(nb_state, SupplierChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed; got: {:?}",
        nb_state.label()
    );

    // ── 4. NB registers 24h APERAK deadline (BK6-22-024) ─────────────────────

    let aperak_due = fristen::add_hours(OffsetDateTime::now_utc(), 24);
    let aperak_dl = Deadline::new(
        nb.stream_id().clone(),
        nb.process_id(),
        nb.tenant_id(),
        nb.workflow_id().clone(),
        GPKE_PROCESS_RESPONSE_LABEL,
        aperak_due,
    );
    let aperak_dl_id = aperak_dl.deadline_id();
    deadline_store
        .register(&aperak_dl)
        .await
        .expect("deadline register");

    // Freshly registered deadline must not fire yet.
    assert!(
        deadline_store
            .due_now(10)
            .await
            .unwrap()
            .deadlines
            .is_empty(),
        "24h deadline must not be immediately due",
    );

    // ── 5. NB accepts — inspect outbox, then persist ──────────────────────────

    let antwort_ref = "MSG-NB-2025-001";
    let msb = MarktpartnerCode::new(MSB_ID);
    let malo_for_obligations = MaLo::new(MALO_ID);
    let new_supplier_for_obligations = MarktpartnerCode::new(LFN_ID);
    let send_antwort = SupplierChangeCommand::SendAntwort {
        accepted: true,
        reason: None,
        obligations: post_acceptance::lieferbeginn_obligations(
            55001,
            &malo_for_obligations,
            &new_supplier_for_obligations,
            Some(&msb),
        ),
    };

    // Pure handle call to verify outbox without a store.
    let nb_output = GpkeSupplierChangeWorkflow::handle(&nb_state, send_antwort.clone())
        .expect("NB SendAntwort must succeed");

    assert_eq!(
        nb_output.outbox.len(),
        3,
        "accepted 55001 must produce UTILMD 55003 + MSCONS + ORDERS outbox entries"
    );

    let utilmd_55003_ob = nb_output
        .outbox
        .iter()
        .find(|e| e.message_type.as_ref() == "UTILMD")
        .expect("UTILMD 55003 response must be present");
    assert_eq!(utilmd_55003_ob.payload["pid"].as_u64().unwrap(), 55003);
    assert_eq!(utilmd_55003_ob.recipient.as_ref(), LFN_ID);

    let mscons = nb_output
        .outbox
        .iter()
        .find(|e| e.message_type.as_ref() == "MSCONS")
        .expect("MSCONS 13015 must be present");
    assert_eq!(mscons.payload["pid"].as_u64().unwrap(), 13015);
    assert_eq!(mscons.payload["malo"].as_str().unwrap(), MALO_ID);

    let orders = nb_output
        .outbox
        .iter()
        .find(|e| e.message_type.as_ref() == "ORDERS")
        .expect("ORDERS 17134 must be present");
    assert_eq!(orders.payload["pid"].as_u64().unwrap(), 17134);
    assert_eq!(orders.recipient.as_ref(), MSB_ID);

    // Persist the state change.
    nb.execute(send_antwort)
        .await
        .expect("NB execute SendAntwort");

    assert!(
        matches!(
            nb.state().await.unwrap(),
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "NB must be AntwortGesendet after accepting",
    );

    // ── 6. NB renders UTILMD 55003 (Bestätigung) for the LFN ─────────────────

    let utilmd_55003_bytes = render_utilmd(55003, NB_ID, LFN_ID, MALO_ID, antwort_ref);

    // Verify round-trip: PID 55003, sender NB, receiver LFN.
    let msg_55003 = platform
        .parse(&utilmd_55003_bytes)
        .expect("55003 must parse");
    assert_eq!(
        msg_55003.detect_pruefidentifikator().unwrap().as_u32(),
        55003
    );
    if let AnyMessage::Utilmd(utilmd) = &msg_55003 {
        assert_eq!(
            utilmd.sender().and_then(|n| n.party_id.as_deref()),
            Some(NB_ID)
        );
        assert_eq!(
            utilmd.receiver().and_then(|n| n.party_id.as_deref()),
            Some(LFN_ID)
        );
    }

    // ── 7. NB cancels the APERAK deadline (response was sent in time) ─────────

    deadline_store
        .cancel(aperak_dl_id)
        .await
        .expect("cancel deadline");
    assert!(
        deadline_store
            .due_now(10)
            .await
            .unwrap()
            .deadlines
            .is_empty(),
        "cancelled deadline must not appear in due_now",
    );

    // ── 8. LFN receives the UTILMD 55003 and handles the acceptance ───────────

    let pid_55003 = mako_engine::types::Pruefidentifikator::new(
        msg_55003.detect_pruefidentifikator().unwrap().as_u32(),
    )
    .unwrap();
    // PID 55003 = Bestätigung Lieferbeginn — always accepted in GPKE.
    let accepted = pid_55003.as_u32() == 55003;

    lfn.execute(LfAnmeldungCommand::HandleAntwort {
        response_pid: pid_55003,
        accepted,
        reason: None,
        response_ref: MessageRef::new(antwort_ref),
    })
    .await
    .expect("LFN HandleAntwort must succeed");

    // ── 9. Final state assertions ─────────────────────────────────────────────

    let final_lfn: LfAnmeldungState = lfn.state().await.unwrap();
    assert!(
        matches!(final_lfn, LfAnmeldungState::Active(_)),
        "LFN must be Active after 55003 acceptance; got: {final_lfn:?}",
    );
    if let LfAnmeldungState::Active(data) = &final_lfn {
        assert_eq!(data.location_id, MaLo::new(MALO_ID));
        assert_eq!(data.pruefidentifikator.as_u32(), 55001);
        assert_eq!(data.sender, MarktpartnerCode::new(LFN_ID));
        assert_eq!(data.receiver, MarktpartnerCode::new(NB_ID));
    }

    // NB is AntwortGesendet — Activate is dispatched by a separate ERP event.
    assert!(
        matches!(
            nb.state().await.unwrap(),
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "NB must be AntwortGesendet; Activate not yet dispatched",
    );
}

// ── Rejection path ────────────────────────────────────────────────────────────

/// NB rejects the 55001 Anfrage (Ablehnung, PID 55004).
/// Both sides must reach `Rejected` state and no MSCONS/ORDERS are enqueued.
#[tokio::test]
async fn bilateral_lieferbeginn_rejection_path() {
    let platform = Platform::with_all_profiles();
    let anfrage_ref = "MSG-LFN-REJ-001";

    let lfn = lfn_process();
    lfn.execute(LfAnmeldungCommand::InitiateAnmeldung {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        process_date: "20250301".to_owned(),
    })
    .await
    .expect("LFN Initiate");

    let nb = nb_process();
    let bytes = render_utilmd(55001, LFN_ID, NB_ID, MALO_ID, anfrage_ref);
    let (pid, sender, receiver, malo, msg_ref, _, _) = extract_utilmd_fields(&platform, &bytes);

    nb.execute(SupplierChangeCommand::ReceiveUtilmd {
        pid,
        sender,
        receiver,
        location_id: malo,
        document_date: "20250115".to_owned(),
        process_date: "20250301".to_owned(),
        message_ref: msg_ref,
        // Bypass AHB validation: bilateral test checks state-machine flow only.
        received_at: time::OffsetDateTime::now_utc(),
        validation_passed: true,
        validation_errors: vec![],
        bilanzierungsgebiet: None,
        bilanzierungsmethode: None,
        fallgruppe: None,
    })
    .await
    .expect("NB ReceiveUtilmd");

    let nb_state: SupplierChangeState = nb.state().await.unwrap();
    let reject_cmd = SupplierChangeCommand::SendAntwort {
        accepted: false,
        reason: Some("MaLo hat laufenden Vertrag".to_owned()),
        obligations: vec![],
    };

    // Rejection enqueues UTILMD 55004 (Ablehnung) but no MSCONS/ORDERS.
    let nb_out = GpkeSupplierChangeWorkflow::handle(&nb_state, reject_cmd.clone())
        .expect("NB SendAntwort(rejected) must succeed");
    assert_eq!(
        nb_out.outbox.len(),
        1,
        "rejected 55001 must produce only UTILMD 55004 (no MSCONS/ORDERS)",
    );
    assert_eq!(nb_out.outbox[0].message_type.as_ref(), "UTILMD");
    assert_eq!(nb_out.outbox[0].payload["pid"].as_u64().unwrap(), 55004);

    nb.execute(reject_cmd)
        .await
        .expect("NB execute SendAntwort(rejected)");

    // NB renders UTILMD 55004 (Ablehnung) for the LFN.
    let bytes_55004 = render_utilmd(55004, NB_ID, LFN_ID, MALO_ID, "MSG-NB-REJ-001");
    let msg_55004 = platform.parse(&bytes_55004).expect("55004 must parse");
    let pid_55004 = mako_engine::types::Pruefidentifikator::new(
        msg_55004.detect_pruefidentifikator().unwrap().as_u32(),
    )
    .unwrap();
    assert_eq!(pid_55004.as_u32(), 55004);

    lfn.execute(LfAnmeldungCommand::HandleAntwort {
        response_pid: pid_55004,
        accepted: false,
        reason: Some("MaLo hat laufenden Vertrag".to_owned()),
        response_ref: MessageRef::new("MSG-NB-REJ-001"),
    })
    .await
    .expect("LFN HandleAntwort(rejected)");

    assert!(matches!(
        lfn.state().await.unwrap(),
        LfAnmeldungState::Rejected { .. }
    ));
    assert!(matches!(
        nb.state().await.unwrap(),
        SupplierChangeState::Rejected { .. }
    ));
}

// ── Deadline expiry path ──────────────────────────────────────────────────────

/// The 24h APERAK deadline fires when the NB does not respond in time.
/// Both sides must reach `Rejected` state after the timeout is dispatched.
#[tokio::test]
async fn bilateral_24h_aperak_deadline_fires_on_timeout() {
    let platform = Platform::with_all_profiles();
    let anfrage_ref = "MSG-LFN-TO-001";

    let lfn = lfn_process();
    lfn.execute(LfAnmeldungCommand::InitiateAnmeldung {
        pid: mako_engine::types::Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new(LFN_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        process_date: "20250301".to_owned(),
    })
    .await
    .unwrap();

    let nb = nb_process();
    let bytes = render_utilmd(55001, LFN_ID, NB_ID, MALO_ID, anfrage_ref);
    let (pid, sender, receiver, malo, msg_ref, _, _) = extract_utilmd_fields(&platform, &bytes);

    nb.execute(SupplierChangeCommand::ReceiveUtilmd {
        pid,
        sender,
        receiver,
        location_id: malo,
        document_date: "20250115".to_owned(),
        process_date: "20250301".to_owned(),
        message_ref: msg_ref,
        // Bypass AHB validation: bilateral test checks state-machine flow only.
        received_at: time::OffsetDateTime::now_utc(),
        validation_passed: true,
        validation_errors: vec![],
        bilanzierungsgebiet: None,
        bilanzierungsmethode: None,
        fallgruppe: None,
    })
    .await
    .unwrap();

    // Register deadlines already in the past (simulates 24h elapsed on both sides).
    let deadline_store = InMemoryDeadlineStore::new();
    let already_past = OffsetDateTime::now_utc() - time::Duration::seconds(1);

    let nb_dl = Deadline::new(
        nb.stream_id().clone(),
        nb.process_id(),
        nb.tenant_id(),
        nb.workflow_id().clone(),
        GPKE_PROCESS_RESPONSE_LABEL,
        already_past,
    );
    let nb_dl_id = nb_dl.deadline_id();
    deadline_store.register(&nb_dl).await.unwrap();

    let lfn_dl = Deadline::new(
        lfn.stream_id().clone(),
        lfn.process_id(),
        lfn.tenant_id(),
        lfn.workflow_id().clone(),
        NB_RESPONSE_WINDOW_LABEL,
        already_past,
    );
    let lfn_dl_id = lfn_dl.deadline_id();
    deadline_store.register(&lfn_dl).await.unwrap();

    // Scheduler poll: both must be due.
    assert_eq!(
        deadline_store.due_now(10).await.unwrap().deadlines.len(),
        2,
        "both past deadlines must be due",
    );

    // Dispatch TimeoutExpired to both processes.
    nb.execute(SupplierChangeCommand::TimeoutExpired {
        deadline_id: nb_dl_id,
        label: GPKE_PROCESS_RESPONSE_LABEL.into(),
    })
    .await
    .unwrap();

    lfn.execute(LfAnmeldungCommand::TimeoutExpired {
        deadline_id: lfn_dl_id,
        label: NB_RESPONSE_WINDOW_LABEL.into(),
    })
    .await
    .unwrap();

    // Acknowledge both fired deadlines.
    deadline_store.cancel(nb_dl_id).await.unwrap();
    deadline_store.cancel(lfn_dl_id).await.unwrap();

    // Both processes must be Rejected.
    assert!(matches!(
        nb.state().await.unwrap(),
        SupplierChangeState::Rejected { .. }
    ));
    assert!(matches!(
        lfn.state().await.unwrap(),
        LfAnmeldungState::Rejected { .. }
    ));
    assert!(
        deadline_store
            .due_now(10)
            .await
            .unwrap()
            .deadlines
            .is_empty()
    );
}
