//! The INVOIC settle/dispute state machine, tested once.
//!
//! What each family chooses — its PIDs, its two role capabilities, its deadline
//! label — is tested in that family's own crate. What the process *does* is
//! tested here, once, for all four.
//!
//! ```text
//! ── Recipient (payer) ─────────────────────────────────────────────────────
//! New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed ──SettleInvoice──► Settled
//!                                        ╰─[invalid]──► Rejected        ╰─DisputeInvoice──► Disputed
//!
//! ── Issuer ────────────────────────────────────────────────────────────────
//! New ──SendInvoic──► InvoicSent ──ReceiveRemadv 33001──► PaymentConfirmed
//!                                ╰─ReceiveRemadv 33002/3/4──► PaymentDisputed
//!
//! Any non-terminal state ──TimeoutExpired──► Rejected
//! ```

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_invoic::{InvoicCommand, InvoicFamily, InvoicState, InvoicWorkflow};

// ── Test families ─────────────────────────────────────────────────────────────

/// A family that plays both roles and exchanges COMDIS — the GPKE/WiM shape.
struct BothRoles;

impl InvoicFamily for BothRoles {
    const WORKFLOW_NAME: &'static str = "test-both-roles";
    const DEADLINE_LABEL: &'static str = "test-settlement-deadline";
    const INVOIC_PIDS: &'static [u32] = &[31002, 31009];
    const SENDS_INVOIC: bool = true;
    const ANSWERS_COMDIS: bool = true;
}

/// A family that only ever receives invoices — the GaBi Gas shape.
struct PayerOnly;

impl InvoicFamily for PayerOnly {
    const WORKFLOW_NAME: &'static str = "test-payer-only";
    const DEADLINE_LABEL: &'static str = "test-payer-deadline";
    const INVOIC_PIDS: &'static [u32] = &[31010];
    const SENDS_INVOIC: bool = false;
    const ANSWERS_COMDIS: bool = true;
}

/// A family with no COMDIS leg — the GeLi Gas shape.
struct NoComdis;

impl InvoicFamily for NoComdis {
    const WORKFLOW_NAME: &'static str = "test-no-comdis";
    const DEADLINE_LABEL: &'static str = "test-no-comdis-deadline";
    const INVOIC_PIDS: &'static [u32] = &[31011];
    const SENDS_INVOIC: bool = true;
    const ANSWERS_COMDIS: bool = false;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn process<F: InvoicFamily>() -> Process<InvoicWorkflow<F>, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new(F::WORKFLOW_NAME, "FV2025-10-01"),
    )
}

fn issuer() -> MarktpartnerCode {
    MarktpartnerCode::new("9900357000004")
}

fn payer() -> MarktpartnerCode {
    MarktpartnerCode::new("4012345000023")
}

fn pid(p: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(p).unwrap()
}

fn receive(p: u32, validation_passed: bool) -> InvoicCommand {
    InvoicCommand::ReceiveInvoic {
        pid: pid(p),
        sender: issuer(),
        recipient: payer(),
        invoice_ref: MessageRef::new("INV-2026-001"),
        document_date: "20260601".to_owned(),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["INVOIC AHB segment MOA+77 missing mandatory net amount".to_owned()]
        },
        rechnung: None,
    }
}

fn send(p: u32) -> InvoicCommand {
    InvoicCommand::SendInvoic {
        pid: pid(p),
        sender: issuer(),
        recipient: payer(),
        document_date: "20260601".to_owned(),
        invoice_ref: MessageRef::new("INV-2026-002"),
    }
}

fn remadv(p: u32) -> InvoicCommand {
    InvoicCommand::ReceiveRemadv {
        pid: pid(p),
        remadv_ref: MessageRef::new("REM-2026-001"),
        sender: payer(),
    }
}

fn timeout(label: &str) -> InvoicCommand {
    InvoicCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: label.into(),
    }
}

// ── The recipient's leg ───────────────────────────────────────────────────────

#[tokio::test]
async fn a_valid_invoice_opens_the_settlement_window() {
    let p = process::<BothRoles>();
    p.execute(receive(31002, true)).await.unwrap();
    let state = p.state().await.unwrap();
    assert!(
        matches!(state, InvoicState::ValidationPassed(_)),
        "{}",
        state.label()
    );
}

#[tokio::test]
async fn a_failing_validation_rejects_the_process() {
    let p = process::<BothRoles>();
    p.execute(receive(31002, false)).await.unwrap();
    let state = p.state().await.unwrap();
    match state {
        InvoicState::Rejected { reason } => assert!(reason.contains("MOA+77"), "{reason}"),
        other => panic!("expected Rejected, got {}", other.label()),
    }
}

/// A validated invoice tells `invoicd` it is ready for plausibility checking.
/// Without it, nothing downstream learns the invoice exists.
#[tokio::test]
async fn a_validated_invoice_announces_itself() {
    let p = process::<BothRoles>();
    let (_, outbox) = p.execute_and_collect(receive(31002, true)).await.unwrap();
    let initiated = outbox
        .iter()
        .find(|o| &*o.message_type == "ProcessInitiated")
        .expect("a validated invoice emits ProcessInitiated");
    assert_eq!(
        initiated.payload["workflow"], "test-both-roles",
        "the payload names the family so a consumer can tell them apart"
    );
    assert_eq!(initiated.payload["pid"], 31002);
}

/// An invoice that fails validation announces nothing — there is no invoice to
/// check.
#[tokio::test]
async fn a_rejected_invoice_announces_nothing() {
    let p = process::<BothRoles>();
    let (_, outbox) = p.execute_and_collect(receive(31002, false)).await.unwrap();
    assert!(
        !outbox
            .iter()
            .any(|o| &*o.message_type == "ProcessInitiated"),
        "{outbox:?}"
    );
}

#[tokio::test]
async fn settling_and_disputing_both_complete_the_process() {
    for (settle, expected) in [(true, "Settled"), (false, "Disputed")] {
        let p = process::<BothRoles>();
        p.execute(receive(31002, true)).await.unwrap();
        let cmd = if settle {
            InvoicCommand::SettleInvoice
        } else {
            InvoicCommand::DisputeInvoice {
                reason: "Netzentgelt weicht vom Preisblatt ab".to_owned(),
            }
        };
        let (_, outbox) = p.execute_and_collect(cmd).await.unwrap();
        assert_eq!(p.state().await.unwrap().label(), expected);
        let completed = outbox
            .iter()
            .find(|o| &*o.message_type == "ProcessCompleted")
            .expect("an answered invoice emits ProcessCompleted");
        assert_eq!(
            completed.payload["outcome"],
            if settle { "settled" } else { "disputed" }
        );
        assert_eq!(completed.payload["invoice_ref"], "INV-2026-001");
    }
}

#[tokio::test]
async fn an_invoice_cannot_be_settled_before_it_validates() {
    let p = process::<BothRoles>();
    assert!(
        p.execute(InvoicCommand::SettleInvoice).await.is_err(),
        "settling a process with no invoice in it must fail"
    );
}

#[tokio::test]
async fn a_foreign_pid_is_refused() {
    let p = process::<BothRoles>();
    let err = p
        .execute(receive(31011, true))
        .await
        .expect_err("31011 does not belong to this family");
    assert!(format!("{err}").contains("31011"), "{err}");
}

// ── The issuer's leg ──────────────────────────────────────────────────────────

#[tokio::test]
async fn an_outbound_invoice_awaits_its_remadv() {
    let p = process::<BothRoles>();
    p.execute(send(31009)).await.unwrap();
    assert_eq!(p.state().await.unwrap().label(), "InvoicSent");
}

/// **REMADV AHB 1.0a § 3 — settlement is „ganz oder gar nicht".** Only 33001
/// confirms; 33002, 33003 and 33004 are all Abweisungen.
///
/// Losing the distinction books a refused invoice as paid, so the command
/// carries the PID and the state has a `PaymentDisputed` to land in.
#[tokio::test]
async fn only_33001_confirms_payment() {
    for (remadv_pid, expected) in [
        (33001, "PaymentConfirmed"),
        (33002, "PaymentDisputed"),
        (33003, "PaymentDisputed"),
        (33004, "PaymentDisputed"),
    ] {
        let p = process::<BothRoles>();
        p.execute(send(31009)).await.unwrap();
        p.execute(remadv(remadv_pid)).await.unwrap();
        assert_eq!(
            p.state().await.unwrap().label(),
            expected,
            "REMADV {remadv_pid}"
        );
    }
}

#[tokio::test]
async fn a_remadv_without_an_outbound_invoice_is_refused() {
    let p = process::<BothRoles>();
    assert!(p.execute(remadv(33001)).await.is_err());
}

#[tokio::test]
async fn a_non_remadv_pid_cannot_answer_an_invoice() {
    let p = process::<BothRoles>();
    p.execute(send(31009)).await.unwrap();
    assert!(p.execute(remadv(29001)).await.is_err());
}

/// The first answer stands: a second REMADV does not overwrite it.
#[tokio::test]
async fn a_late_remadv_does_not_overwrite_the_first_answer() {
    let p = process::<BothRoles>();
    p.execute(send(31009)).await.unwrap();
    p.execute(remadv(33002)).await.unwrap();
    // The state is no longer InvoicSent, so the second is refused outright.
    assert!(p.execute(remadv(33001)).await.is_err());
    assert_eq!(p.state().await.unwrap().label(), "PaymentDisputed");
}

// ── Deadlines ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_unanswered_invoice_is_rejected_when_the_window_closes() {
    let p = process::<BothRoles>();
    p.execute(receive(31002, true)).await.unwrap();
    p.execute(timeout(BothRoles::DEADLINE_LABEL)).await.unwrap();
    assert_eq!(p.state().await.unwrap().label(), "Rejected");
}

/// A deadline that fires after the answer was given changes nothing. Deadlines
/// are never cancelled, so they fire on the healthy path too.
#[tokio::test]
async fn a_deadline_firing_after_the_answer_is_absorbed() {
    for terminal in ["Settled", "PaymentDisputed"] {
        let p = process::<BothRoles>();
        if terminal == "Settled" {
            p.execute(receive(31002, true)).await.unwrap();
            p.execute(InvoicCommand::SettleInvoice).await.unwrap();
        } else {
            p.execute(send(31009)).await.unwrap();
            p.execute(remadv(33002)).await.unwrap();
        }
        p.execute(timeout(BothRoles::DEADLINE_LABEL)).await.unwrap();
        assert_eq!(p.state().await.unwrap().label(), terminal);
    }
}

// ── Role capabilities ─────────────────────────────────────────────────────────

/// A family that never issues an invoice must not be able to reach the issuer
/// states. Accepting an inbound REMADV there inverts the direction of the
/// conversation: after *receiving* an invoice, this platform sends the REMADV.
#[tokio::test]
async fn a_payer_only_family_refuses_the_issuer_leg() {
    let p = process::<PayerOnly>();
    let err = p.execute(send(31010)).await.expect_err("issuer leg");
    assert!(format!("{err}").contains("issuer role"), "{err}");

    let p = process::<PayerOnly>();
    let err = p.execute(remadv(33001)).await.expect_err("inbound REMADV");
    assert!(format!("{err}").contains("never issues"), "{err}");
}

/// A family with no COMDIS leg refuses one rather than recording a message its
/// process does not publish.
#[tokio::test]
async fn a_family_without_comdis_refuses_one() {
    let p = process::<NoComdis>();
    p.execute(receive(31011, true)).await.unwrap();
    let err = p
        .execute(InvoicCommand::ReceiveComdis {
            comdis_ref: MessageRef::new("COM-1"),
        })
        .await
        .expect_err("this family publishes no COMDIS leg");
    assert!(format!("{err}").contains("COMDIS"), "{err}");
}

#[tokio::test]
async fn a_comdis_refuses_our_payment_advice() {
    let p = process::<BothRoles>();
    p.execute(receive(31002, true)).await.unwrap();
    p.execute(InvoicCommand::ReceiveComdis {
        comdis_ref: MessageRef::new("COM-1"),
    })
    .await
    .unwrap();
    assert_eq!(p.state().await.unwrap().label(), "ComdisRejected");
}

/// A COMDIS refuses a REMADV **we** sent, and we send one as the *payer*. The
/// issuer's states are therefore not places one can arrive: there we would be
/// the sender, so an inbound one is a routing error. `handle` and `apply` admit
/// exactly the same states: an event `apply` would ignore is refused outright.
#[tokio::test]
async fn a_comdis_is_refused_in_the_issuers_states() {
    let p = process::<BothRoles>();
    p.execute(send(31009)).await.unwrap();
    let err = p
        .execute(InvoicCommand::ReceiveComdis {
            comdis_ref: MessageRef::new("COM-1"),
        })
        .await
        .expect_err("in InvoicSent we are the issuer — we send COMDIS, not receive it");
    assert!(format!("{err}").contains("InvoicSent"), "{err}");

    let p = process::<BothRoles>();
    p.execute(send(31009)).await.unwrap();
    p.execute(remadv(33001)).await.unwrap();
    assert!(
        p.execute(InvoicCommand::ReceiveComdis {
            comdis_ref: MessageRef::new("COM-1"),
        })
        .await
        .is_err()
    );
}

/// …and accepted in every payer state that has produced, or is about to
/// produce, a REMADV.
#[tokio::test]
async fn a_comdis_is_accepted_in_every_payer_state() {
    for answer in [None, Some(true), Some(false)] {
        let p = process::<BothRoles>();
        p.execute(receive(31002, true)).await.unwrap();
        match answer {
            Some(true) => p.execute(InvoicCommand::SettleInvoice).await.unwrap(),
            Some(false) => p
                .execute(InvoicCommand::DisputeInvoice {
                    reason: "Preisblatt".to_owned(),
                })
                .await
                .unwrap(),
            None => vec![],
        };
        p.execute(InvoicCommand::ReceiveComdis {
            comdis_ref: MessageRef::new("COM-1"),
        })
        .await
        .expect("a payer can always have its REMADV refused");
        assert_eq!(p.state().await.unwrap().label(), "ComdisRejected");
    }
}

/// A COMDIS can only refuse a REMADV that could have been sent.
#[tokio::test]
async fn a_comdis_before_any_invoice_is_refused() {
    let p = process::<BothRoles>();
    assert!(
        p.execute(InvoicCommand::ReceiveComdis {
            comdis_ref: MessageRef::new("COM-1"),
        },)
            .await
            .is_err()
    );
}

// ── The facts survive ─────────────────────────────────────────────────────────

#[tokio::test]
async fn the_invoice_facts_survive_replay() {
    let p = process::<BothRoles>();
    p.execute(receive(31002, true)).await.unwrap();
    let state = p.state().await.unwrap();
    let data = state.data().expect("an invoice was received");
    assert_eq!(data.pruefidentifikator.as_u32(), 31002);
    assert_eq!(data.sender.as_str(), issuer().as_str());
    assert_eq!(data.recipient.as_str(), payer().as_str());
    assert_eq!(data.invoice_ref.as_str(), "INV-2026-001");
    assert_eq!(data.document_date, "20260601");
}
