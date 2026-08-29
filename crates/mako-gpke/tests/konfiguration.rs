//! Tests for `GpkeKonfigurationWorkflow` (ORDERS 17134/17135 — GPKE Teil 4).
//!
//! Covers:
//! - `NbSendsBeauftragung` → `BeauftragungGesendet` event + ORDERS outbox entry
//! - `ReceiveOrdrsp { accepted: true }` → `BestaetigungErhalten` event
//! - `ReceiveOrdrsp { accepted: false }` → `AblehungErhalten` event
//! - Wrong PID guard and wrong-state guards
//! - `TimeoutExpired` in terminal state → no-op
//! - `SendAntwort { accepted:true, pid:55001, obligations }` → MSCONS 13015 outbox
//! - `SendAntwort { accepted:true, pid:55001, msb obligations }` → +ORDERS 17134 outbox
//! - `SendAntwort { accepted:true, pid:55004 }` → no cross-domain outbox (Abmeldung 55004, not Anmeldung 55001)
//!
//! obligations are now computed by `post_acceptance::lieferbeginn_obligations`
//! and passed to `SendAntwort`; the workflow itself carries no cross-domain PID knowledge.

use std::sync::Arc;

use mako_engine::{
    builder::EngineBuilder,
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::Workflow,
};
use mako_gpke::{
    GpkeKonfigurationWorkflow, KonfigurationCommand, KonfigurationEvent, KonfigurationState,
};

fn spawn_process()
-> mako_engine::process::Process<GpkeKonfigurationWorkflow, Arc<InMemoryEventStore>> {
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .build();
    let tenant_id = TenantId::new();
    let workflow_id = WorkflowId::new("gpke-konfiguration", "FV2025-10-01");
    ctx.spawn::<GpkeKonfigurationWorkflow>(tenant_id, workflow_id)
}

fn beauftragung_cmd() -> KonfigurationCommand {
    KonfigurationCommand::NbSendsBeauftragung {
        orders_pid: Pruefidentifikator::new(17134).unwrap(),
        msb_mp_id: MarktpartnerCode::new("9904357000003"),
        malo: MaLo::new("51238696781"),
        new_supplier: MarktpartnerCode::new("4012345000023"),
        message_ref: MessageRef::new("ORD-2025-001"),
    }
}

// ── NbSendsBeauftragung emits BeauftragungGesendet + ORDERS outbox ────

/// `NbSendsBeauftragung` emits one `BeauftragungGesendet` event and produces
/// one outbox entry of type `"ORDERS"` addressed to the MSB.
///
/// Verified by calling `Workflow::handle` directly (pure function) to avoid
/// depending on `AtomicAppend` (which requires SlateDB).
#[test]
fn nb_sends_beauftragung_emits_event_and_outbox() {
    let state = KonfigurationState::New;
    let output = GpkeKonfigurationWorkflow::handle(&state, beauftragung_cmd())
        .expect("NbSendsBeauftragung in New state must succeed");

    assert_eq!(output.events.len(), 1, "exactly one event");
    assert!(
        matches!(
            &output.events[0],
            KonfigurationEvent::BeauftragungGesendet { .. }
        ),
        "event must be BeauftragungGesendet"
    );
    assert_eq!(output.outbox.len(), 1, "exactly one ORDERS outbox entry");
    assert_eq!(output.outbox[0].message_type.as_ref(), "ORDERS");
    assert_eq!(output.outbox[0].recipient.as_ref(), "9904357000003");

    let payload = &output.outbox[0].payload;
    assert_eq!(payload["pid"].as_u64().unwrap(), 17134);
    assert_eq!(payload["malo"].as_str().unwrap(), "51238696781");
}

/// Wrong ORDERS PID (not 17134 or 17135) must be rejected.
#[test]
fn wrong_orders_pid_returns_error() {
    let state = KonfigurationState::New;
    let result = GpkeKonfigurationWorkflow::handle(
        &state,
        KonfigurationCommand::NbSendsBeauftragung {
            orders_pid: Pruefidentifikator::new(17001).unwrap(), // GeLi Gas PID — wrong
            msb_mp_id: MarktpartnerCode::new("9904357000003"),
            malo: MaLo::new("51238696781"),
            new_supplier: MarktpartnerCode::new("4012345000023"),
            message_ref: MessageRef::new("ORD-WRONG"),
        },
    );
    assert!(
        result.is_err(),
        "wrong PID must produce WorkflowError::Rejected"
    );
}

// ── ORDRSP 19001 acceptance → Bestaetigt ─────────────────────────────────────

/// After `BeauftragungGesendet`, receiving ORDRSP 19001 transitions to `Bestaetigt`.
#[tokio::test]
async fn receive_ordrsp_acceptance_transitions_to_bestaetigt() {
    let process = spawn_process();

    process
        .execute(beauftragung_cmd())
        .await
        .expect("setup must succeed");

    let envelopes = process
        .execute(KonfigurationCommand::ReceiveOrdrsp {
            pid: Pruefidentifikator::new(19001).unwrap(),
            accepted: true,
            reason: None,
            message_ref: MessageRef::new("ORDRSP-2025-001"),
        })
        .await
        .expect("ORDRSP acceptance must succeed");

    assert_eq!(envelopes.len(), 1);
    assert_eq!(
        envelopes[0].event_type.as_ref(),
        "KonfigurationBestaetigungErhalten",
    );

    let state = process.state().await.expect("state must rebuild");
    assert!(
        matches!(state, KonfigurationState::Bestaetigt { .. }),
        "state must be Bestaetigt after 19001; got {:?}",
        state.label()
    );
}

// ── ORDRSP 19002 rejection → Abgelehnt ───────────────────────────────────────

/// After `BeauftragungGesendet`, receiving ORDRSP 19002 transitions to `Abgelehnt`.
#[tokio::test]
async fn receive_ordrsp_rejection_transitions_to_abgelehnt() {
    let process = spawn_process();

    process
        .execute(beauftragung_cmd())
        .await
        .expect("setup must succeed");

    let envelopes = process
        .execute(KonfigurationCommand::ReceiveOrdrsp {
            pid: Pruefidentifikator::new(19002).unwrap(),
            accepted: false,
            reason: Some("Messpunkt nicht bekannt".to_owned()),
            message_ref: MessageRef::new("ORDRSP-2025-002"),
        })
        .await
        .expect("ORDRSP rejection must succeed");

    assert_eq!(envelopes.len(), 1);
    assert_eq!(
        envelopes[0].event_type.as_ref(),
        "KonfigurationAblehungErhalten",
    );

    let state = process.state().await.expect("state must rebuild");
    if let KonfigurationState::Abgelehnt { reason, .. } = state {
        assert_eq!(reason, "Messpunkt nicht bekannt");
    } else {
        panic!(
            "state must be Abgelehnt after 19002; got {:?}",
            state.label()
        );
    }
}

// ── ReceiveOrdrsp from New state → InvalidState ───────────────────────────────

#[test]
fn receive_ordrsp_from_new_state_returns_error() {
    let state = KonfigurationState::New;
    let result = GpkeKonfigurationWorkflow::handle(
        &state,
        KonfigurationCommand::ReceiveOrdrsp {
            pid: Pruefidentifikator::new(19001).unwrap(),
            accepted: true,
            reason: None,
            message_ref: MessageRef::new("ORDRSP-EARLY"),
        },
    );
    assert!(result.is_err(), "ReceiveOrdrsp in New state must fail");
}

// ── TimeoutExpired in terminal state is a no-op ───────────────────────────────

#[tokio::test]
async fn timeout_in_terminal_state_is_noop() {
    let process = spawn_process();

    // Advance to Bestaetigt terminal state.
    process.execute(beauftragung_cmd()).await.expect("setup");
    process
        .execute(KonfigurationCommand::ReceiveOrdrsp {
            pid: Pruefidentifikator::new(19001).unwrap(),
            accepted: true,
            reason: None,
            message_ref: MessageRef::new("ORDRSP-NOOP"),
        })
        .await
        .expect("accept");

    // Timeout in a terminal state must produce zero events.
    let envelopes = process
        .execute(KonfigurationCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: "konfiguration-deadline".into(),
        })
        .await
        .expect("timeout in terminal state must succeed");

    assert!(
        envelopes.is_empty(),
        "no events must be emitted for a timeout in a terminal state"
    );
}

// ── Bewegungsdaten outbox from GpkeSupplierChangeWorkflow ─────────────

/// `SendAntwort { accepted:true, anfrage_pid:55001 }` with obligations built by
/// `post_acceptance::lieferbeginn_obligations` must emit exactly one `"MSCONS"`
/// outbox entry for MSCONS 13015 (Bewegungsdaten im Kalenderjahr vor
/// Lieferbeginn Strom, BDEW GPKE Teil 3, BK6-22-024).
///
/// The workflow itself carries no cross-domain PID knowledge.
#[test]
fn send_antwort_lieferbeginn_accepted_emits_mscons_13015_outbox() {
    use mako_gpke::{
        GpkeSupplierChangeWorkflow, SupplierChangeCommand, SupplierChangeState, post_acceptance,
        wechselprozesse::InitiatedData,
    };

    let malo = MaLo::new("51238696781");
    let new_supplier = MarktpartnerCode::new("4012345000023");
    let data = InitiatedData {
        transaktionsgrund: None,
        location_id: malo.clone(),
        new_supplier: new_supplier.clone(),
        grid_operator: MarktpartnerCode::new("9900357000004"),
        document_date: "20250115".to_owned(),
        process_date: "20251001".to_owned(),
        pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
    };
    let state = SupplierChangeState::ValidationPassed(data);
    let obligations = post_acceptance::lieferbeginn_obligations(55001, &malo, &new_supplier, None);

    let output = GpkeSupplierChangeWorkflow::handle(
        &state,
        SupplierChangeCommand::SendAntwort {
            antwort: nb_antwort(true, None),
            obligations,
            lfa_lieferende: None,
        },
    )
    .expect("SendAntwort must succeed in ValidationPassed");

    // One AntwortGesendet event + UTILMD 55003 + MSCONS 13015.
    assert_eq!(output.events.len(), 1, "exactly one AntwortGesendet event");
    assert_eq!(
        output.outbox.len(),
        2,
        "UTILMD 55003 + MSCONS 13015 outbox entries"
    );

    let entry = output
        .outbox
        .iter()
        .find(|e| e.message_type.as_ref() == "MSCONS")
        .expect("MSCONS must be present");
    assert_eq!(entry.message_type.as_ref(), "MSCONS");
    assert_eq!(entry.recipient.as_ref(), "4012345000023"); // new LFN
    assert_eq!(entry.payload["pid"].as_u64().unwrap(), 13015);
    assert_eq!(
        entry.payload["type"].as_str().unwrap(),
        "MovementDataRequired"
    );
    assert_eq!(entry.payload["malo"].as_str().unwrap(), "51238696781");
}

/// When `msb_mp_id` is provided to `lieferbeginn_obligations`, a second outbox
/// entry (ORDERS 17134) must be emitted alongside MSCONS 13015.
///
/// The workflow routes all obligations from the command — no PID knowledge
/// inside the state machine.
#[test]
fn send_antwort_lieferbeginn_with_msb_emits_orders_17134_outbox() {
    use mako_gpke::{
        GpkeSupplierChangeWorkflow, SupplierChangeCommand, SupplierChangeState, post_acceptance,
        wechselprozesse::InitiatedData,
    };

    let malo = MaLo::new("51238696781");
    let new_supplier = MarktpartnerCode::new("4012345000023");
    let msb_mp_id = MarktpartnerCode::new("9904357000003");
    let data = InitiatedData {
        transaktionsgrund: None,
        location_id: malo.clone(),
        new_supplier: new_supplier.clone(),
        grid_operator: MarktpartnerCode::new("9900357000004"),
        document_date: "20250115".to_owned(),
        process_date: "20251001".to_owned(),
        pruefidentifikator: Pruefidentifikator::new(55001).unwrap(),
    };
    let state = SupplierChangeState::ValidationPassed(data);
    let obligations =
        post_acceptance::lieferbeginn_obligations(55001, &malo, &new_supplier, Some(&msb_mp_id));

    let output = GpkeSupplierChangeWorkflow::handle(
        &state,
        SupplierChangeCommand::SendAntwort {
            antwort: nb_antwort(true, None),
            obligations,
            lfa_lieferende: None,
        },
    )
    .expect("SendAntwort must succeed");

    // One event + three outbox entries (UTILMD 55003 + MSCONS 13015 + ORDERS 17134).
    assert_eq!(
        output.outbox.len(),
        3,
        "UTILMD 55003 + MSCONS 13015 + ORDERS 17134 must be emitted"
    );

    let mscons = output
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "MSCONS")
        .expect("MSCONS missing");
    let orders = output
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "ORDERS")
        .expect("ORDERS missing");

    assert_eq!(mscons.payload["pid"].as_u64().unwrap(), 13015);
    assert_eq!(orders.payload["pid"].as_u64().unwrap(), 17134);
    assert_eq!(orders.recipient.as_ref(), "9904357000003"); // MSB GLN
}

/// For PID 55004 (Abmeldung) accepted, `lieferbeginn_obligations` returns an
/// empty slice (MSCONS 13015 applies only to Lieferbeginn/PID 55001) and the
/// workflow emits only UTILMD 55005 — no MSCONS or ORDERS entries.
#[test]
fn send_antwort_abmeldung_accepted_no_cross_domain_outbox() {
    use mako_gpke::{
        GpkeSupplierChangeWorkflow, SupplierChangeCommand, SupplierChangeState, post_acceptance,
        wechselprozesse::InitiatedData,
    };

    let malo = MaLo::new("51238696781");
    let new_supplier = MarktpartnerCode::new("4012345000023");
    let data = InitiatedData {
        transaktionsgrund: None,
        location_id: malo.clone(),
        new_supplier: new_supplier.clone(),
        grid_operator: MarktpartnerCode::new("9900357000004"),
        document_date: "20250115".to_owned(),
        process_date: "20261001".to_owned(),
        pruefidentifikator: Pruefidentifikator::new(55004).unwrap(), // Abmeldung
    };
    let state = SupplierChangeState::ValidationPassed(data);
    // lieferbeginn_obligations returns empty vec for non-55001 PIDs.
    let obligations = post_acceptance::lieferbeginn_obligations(55004, &malo, &new_supplier, None);
    assert!(
        obligations.is_empty(),
        "non-55001 PID must produce no obligations"
    );

    let output = GpkeSupplierChangeWorkflow::handle(
        &state,
        SupplierChangeCommand::SendAntwort {
            antwort: nb_antwort(true, None),
            obligations,
            lfa_lieferende: None,
        },
    )
    .expect("SendAntwort must succeed for Abmeldung");

    assert_eq!(
        output.outbox.len(),
        1,
        "PID 55004 accepted must enqueue only UTILMD 55005 (no MSCONS/ORDERS)"
    );
    assert_eq!(output.outbox[0].message_type.as_ref(), "UTILMD");
    assert_eq!(output.outbox[0].payload["pid"].as_u64().unwrap(), 55005);
}

/// The NB's answer code for a Lieferbeginn — `A51` out of `E_0623`.
/// `SG4 STS+E01` is Muss on every Antwortnachricht.
fn nb_antwort(accepted: bool, reason: Option<&str>) -> mako_gpke::LfAntwort {
    let (code, ebd) = if accepted {
        ("A51", "E_0623")
    } else {
        ("A07", "E_0622")
    };
    mako_gpke::LfAntwort {
        antwort_code: code.to_owned(),
        ebd: Some(ebd.to_owned()),
        zustimmung: accepted,
        bemerkung: reason.map(ToOwned::to_owned),
        bilanzkreis: None,
        termin: None,
    }
}
