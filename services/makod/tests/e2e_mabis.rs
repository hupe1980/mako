//! End-to-end test: MABIS Bilanzkreisabrechnung Strom (PID 13003).
//!
//! Models the BKV (Bilanzkreisverantwortlicher) side of the MaBiS billing
//! process (BNetzA BK6-24-174). The BIKO sends an Abrechnungssummenzeitreihe
//! to the BKV; the BKV must respond with a Prüfmitteilung within 1 Werktag.
//!
//! # Lifecycle trace
//!
//! ```text
//!   BIKO                                BKV (this workflow)
//!   ────────────────────────────────────────────────────────────────────────
//!   Abrechnungssummenzeitreihe ──────→  ReceiveSummenzeitreihe
//!                                           state: SummenzeitreiheReceived
//!                              ←──────── SendPruefmitteilungPositiv (≤ 1 WT)
//!                                           state: PruefmitteilungSent
//!   Datenstatus                ──────→  ReceiveDatastatus
//!                                           state: Settled
//!   ────────────────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 13003**: Bilanzkreisabrechnung Strom (MaBiS BK6-24-174)
//! - **BIKO**: Bilanzkoordinator — central actor; sends Summenzeitreihen
//! - **BKV**: Bilanzkreisverantwortlicher — must respond within 1 Werktag
//! - **Prüfmitteilung Frist**: 1 Werktag after receiving Summenzeitreihe (§13.8)

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{BikoId, BillingPeriod, BkvId, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    BillingCommand, BillingState, BillingVersion, DataStatus, MabisBillingWorkflow,
    PRUEFMITTEILUNG_DEADLINE_LABEL,
};

// ── Constants ──────────────────────────────────────────────────────────────────

const BKV_ID: &str = "4033872000022"; // BKV GLN
const BIKO_ID: &str = "10YDE-VE-TRANSMIX"; // BIKO EIC code
const BILLING_PERIOD: &str = "2025-09";
const FV: &str = "FV2025-10-01";

// ── Mock BKV backend ───────────────────────────────────────────────────────────

/// Simulates the **BKV's** process handler receiving MaBiS messages from BIKO.
struct MockBkv {
    process: Process<MabisBillingWorkflow, InMemoryEventStore>,
}

impl MockBkv {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(BKV_ID),
                WorkflowId::new("mabis-billing", FV),
            ),
        }
    }

    /// BIKO sent Abrechnungssummenzeitreihe — open billing period from BKV perspective.
    async fn receive_summenzeitreihe(&self, version: BillingVersion) {
        self.process
            .execute(BillingCommand::ReceiveSummenzeitreihe {
                pid: Pruefidentifikator::new(13003).unwrap(),
                billing_period: BillingPeriod::new(BILLING_PERIOD),
                bkv_id: BkvId::new(BKV_ID),
                biko_id: BikoId::new(BIKO_ID),
                version,
                message_ref: MessageRef::new("MSCONS-BKA-2025-09-001"),
            })
            .await
            .expect("BKV: ReceiveSummenzeitreihe");
    }

    /// BKV sends positive Prüfmitteilung to BIKO (accepts billing).
    async fn send_pruefmitteilung_positiv(&self) {
        self.process
            .execute(BillingCommand::SendPruefmitteilungPositiv {
                message_ref: MessageRef::new("PRUEF-POS-2025-09-001"),
            })
            .await
            .expect("BKV: SendPruefmitteilungPositiv");
    }

    /// BKV sends negative Prüfmitteilung to BIKO (disputes billing).
    async fn send_pruefmitteilung_negativ(&self, reason: &str) {
        self.process
            .execute(BillingCommand::SendPruefmitteilungNegativ {
                message_ref: MessageRef::new("PRUEF-NEG-2025-09-001"),
                reason: reason.to_owned(),
            })
            .await
            .expect("BKV: SendPruefmitteilungNegativ");
    }

    /// BIKO sent Datenstatus — settlement confirmed.
    async fn receive_datenstatus(&self, data_status: DataStatus) {
        self.process
            .execute(BillingCommand::ReceiveDatastatus { data_status })
            .await
            .expect("BKV: ReceiveDatastatus");
    }

    async fn state(&self) -> BillingState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// MABIS Bilanzkreisabrechnung — happy path (PID 13003 → Settled).
///
/// Full lifecycle:
/// New → SummenzeitreiheReceived → PruefmitteilungSent → Settled.
///
/// Asserts data invariants at each state transition.
#[tokio::test]
async fn e2e_mabis_billing_happy_path() {
    let bkv = MockBkv::new();

    // ── Step 1: Receive Abrechnungssummenzeitreihe ────────────────────────────
    bkv.receive_summenzeitreihe(BillingVersion::Vorlaeufig)
        .await;

    let state = bkv.state().await;
    match &state {
        BillingState::SummenzeitreiheReceived(d) => {
            assert_eq!(d.billing_period.as_str(), BILLING_PERIOD);
            assert_eq!(d.bkv_id.as_str(), BKV_ID);
            assert_eq!(d.biko_id.as_str(), BIKO_ID);
            assert_eq!(d.pruefidentifikator.as_u32(), 13003);
            assert_eq!(d.version, BillingVersion::Vorlaeufig);
        }
        _ => panic!("expected SummenzeitreiheReceived; got: {state:?}"),
    }

    // ── Step 2: BKV sends positive Prüfmitteilung within 1 Werktag ───────────
    bkv.send_pruefmitteilung_positiv().await;

    let state = bkv.state().await;
    assert!(
        matches!(state, BillingState::PruefmitteilungSent(_)),
        "must be PruefmitteilungSent; got: {state:?}"
    );

    // ── Step 3: BIKO sends Datenstatus → Settled ──────────────────────────────
    bkv.receive_datenstatus(DataStatus::AbgerechtneteDaten)
        .await;

    let state = bkv.state().await;
    match state {
        BillingState::Settled(d) => {
            assert_eq!(d.billing_period.as_str(), BILLING_PERIOD);
            assert_eq!(d.bkv_id.as_str(), BKV_ID);
            assert_eq!(d.biko_id.as_str(), BIKO_ID);
        }
        _ => panic!("expected Settled; got: {state:?}"),
    }
}

/// MABIS Bilanzkreisabrechnung — happy path with endgültig (final) billing.
///
/// The Endgueltig version follows the same lifecycle, dispatched by BIKO at
/// the 42nd Werktag after the billing month.
#[tokio::test]
async fn e2e_mabis_billing_endgueltig_happy_path() {
    let bkv = MockBkv::new();

    bkv.receive_summenzeitreihe(BillingVersion::Endgueltig)
        .await;

    let state = bkv.state().await;
    if let BillingState::SummenzeitreiheReceived(d) = &state {
        assert_eq!(d.version, BillingVersion::Endgueltig);
    } else {
        panic!("expected SummenzeitreiheReceived; got: {state:?}");
    }

    bkv.send_pruefmitteilung_positiv().await;
    bkv.receive_datenstatus(DataStatus::AbgerechtneteDatenKbka)
        .await;

    let state = bkv.state().await;
    assert!(
        matches!(state, BillingState::Settled(_)),
        "must be Settled; got: {state:?}"
    );
}

/// MABIS Bilanzkreisabrechnung — dispute via negative Prüfmitteilung.
///
/// If the BKV finds discrepancies in the Abrechnungssummenzeitreihe, they
/// send a negative Prüfmitteilung, which transitions the process to Disputed.
#[tokio::test]
async fn e2e_mabis_billing_negative_pruefmitteilung_to_disputed() {
    let bkv = MockBkv::new();

    bkv.receive_summenzeitreihe(BillingVersion::Vorlaeufig)
        .await;

    bkv.send_pruefmitteilung_negativ(
        "Zählpunkt DE00123456789012345678901234567890 fehlt in der Summenzeitreihe",
    )
    .await;

    let state = bkv.state().await;
    match &state {
        BillingState::Disputed { reason, .. } => {
            assert!(
                reason.contains("DE00123"),
                "dispute reason must be preserved; got: {reason:?}"
            );
        }
        _ => panic!("expected Disputed; got: {state:?}"),
    }
}

/// MABIS Bilanzkreisabrechnung — guard: PID other than 13003 is rejected.
///
/// Only PID 13003 is implemented in the MABIS crate. Any other PID must
/// return a `WorkflowError` and leave the state as `New`.
#[tokio::test]
async fn e2e_mabis_billing_invalid_pid_rejected() {
    let bkv = MockBkv::new();

    let result = bkv
        .process
        .execute(BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(13002).unwrap(),
            billing_period: BillingPeriod::new(BILLING_PERIOD),
            bkv_id: BkvId::new(BKV_ID),
            biko_id: BikoId::new(BIKO_ID),
            version: BillingVersion::Vorlaeufig,
            message_ref: MessageRef::new("REF-001"),
        })
        .await;

    assert!(
        result.is_err(),
        "ReceiveSummenzeitreihe with PID 13002 must return error"
    );
    let state = bkv.state().await;
    assert!(
        matches!(state, BillingState::New),
        "state must remain New after rejected command; got: {state:?}"
    );
}

/// MABIS Bilanzkreisabrechnung — guard: Prüfmitteilung deadline fires (1 Werktag).
///
/// If the BKV does not respond within 1 Werktag, the engine fires
/// `PruefmitteilungDeadlineExpired` and the process transitions to
/// `DeadlineExpired`.
#[tokio::test]
async fn e2e_mabis_billing_pruefmitteilung_deadline_fires() {
    let bkv = MockBkv::new();

    bkv.receive_summenzeitreihe(BillingVersion::Vorlaeufig)
        .await;

    let deadline_id = DeadlineId::new();
    bkv.process
        .execute(BillingCommand::PruefmitteilungDeadlineExpired {
            deadline_id,
            label: PRUEFMITTEILUNG_DEADLINE_LABEL.into(),
        })
        .await
        .expect("deadline must be accepted from SummenzeitreiheReceived");

    let state = bkv.state().await;
    assert!(
        matches!(state, BillingState::DeadlineExpired(_)),
        "must be DeadlineExpired after deadline; got: {state:?}"
    );
}

/// MABIS Bilanzkreisabrechnung — deadline absorbed on Settled process.
///
/// A late-firing deadline on an already-Settled process must be absorbed
/// without changing state or returning an error.
#[tokio::test]
async fn e2e_mabis_billing_late_deadline_absorbed_on_settled() {
    let bkv = MockBkv::new();

    bkv.receive_summenzeitreihe(BillingVersion::Vorlaeufig)
        .await;
    bkv.send_pruefmitteilung_positiv().await;
    bkv.receive_datenstatus(DataStatus::AbgerechtneteDaten)
        .await;

    let deadline_id = DeadlineId::new();
    bkv.process
        .execute(BillingCommand::PruefmitteilungDeadlineExpired {
            deadline_id,
            label: PRUEFMITTEILUNG_DEADLINE_LABEL.into(),
        })
        .await
        .expect("late deadline on Settled must be absorbed");

    let state = bkv.state().await;
    assert!(
        matches!(state, BillingState::Settled(_)),
        "state must remain Settled; got: {state:?}"
    );
}
