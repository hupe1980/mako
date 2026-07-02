//! End-to-end test: NB → LFN GPKE Ankündigung Zuordnung LF (PID 55607).
//!
//! After a Lieferantenwechsel the Netzbetreiber (NB) announces the completed
//! supplier assignment to the new Lieferant (LFN) by sending UTILMD 55607.
//! The LFN must respond within 24 wall-clock hours (BK6-22-024 §4) with
//! Bestätigung (55608) or Ablehnung (55609).
//!
//! # Protocol trace (accept path)
//!
//! ```text
//!   NB ERP (wire fixture)                      LFN ERP (MockLfn)
//!   ──────────────────────────────────────────────────────────────
//!                        ──── UTILMD 55607 ────►
//!                                               receive_ankuendigung(wire)
//!                                                 → adapter: ReceiveAnkuendigung
//!                                                 → state: ValidationPassed
//!                                               send_antwort(accepted=true)
//!                                                 → state: AntwortGesendet
//!                                               zuordnung_bestaetigen()
//!                                                 → state: Zugeordnet
//!   ──────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 55607**: NB announces supplier assignment (NB → LFN, Strom)
//! - **PID 55608**: LFN Bestätigung (LFN → NB, outbound)
//! - **PID 55609**: LFN Ablehnung (LFN → NB, outbound)
//! - **Deadline**: 24 wall-clock hours for LFN response (BK6-22-024 §4)
//! - **Regulatory basis**: UTILMD AHB Strom 2.1/2.2, BK6-24-174, GPKE domain
//!
//! AHB validation is bypassed for inbound `ReceiveAnkuendigung` because the
//! hand-crafted wire fixture does not satisfy all S2.1 profile rules.
//! AHB conformance is tested separately via `cargo xtask validate-pruefids`.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::MessageRef,
    version::{FormatVersion, WorkflowId},
};
use mako_gpke::{
    AnkuendigungZuordnungLfCommand, AnkuendigungZuordnungLfState,
    GpkeAnkuendigungZuordnungLfWorkflow,
};
use makod::adapters::gpke_ankuendigung_zuordnung_lf_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const NB_ID: &str = "9900357000004"; // Netzbetreiber (assignment initiator)
const LFN_ID: &str = "4012345000023"; // Neue Lieferant (receiving party)
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── Wire fixture ──────────────────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD S2.1 PID 55607 — NB announces supplier assignment to LFN.
// NAD+MS carries the NB's GLN; NAD+MR carries the LFN's GLN.

const UTILMD_55607_BYTES: &[u8] = b"\
UNB+UNOC:3+9900357000004:14+4012345000023:14+250115:0800+ZUORD-2025-001'\
UNH+MSG-ZUORD-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055607::+9'\
DTM+137:20250115:102'\
RFF+Z13:ZUORD-REF-001'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::293'\
IDE+Z19+51238696781::'\
DTM+92:20250401:102'\
UNT+9+MSG-ZUORD-001'\
UNZ+1+ZUORD-2025-001'";

// ── Mock LFN ERP backend ──────────────────────────────────────────────────────

/// Simulates the **neue Lieferant ERP** receiving and responding to the NB's
/// Ankündigung Zuordnung LF (PID 55607).
///
/// Owns a single `GpkeAnkuendigungZuordnungLfWorkflow` process backed by an
/// in-memory event store.
struct MockLfn {
    process: Process<GpkeAnkuendigungZuordnungLfWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockLfn {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LFN_ID),
                WorkflowId::new(mako_gpke::ankuendigung_zuordnung_lf::WORKFLOW_NAME, FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive NB's UTILMD 55607 wire bytes, adapt, and execute.
    ///
    /// `validation_passed` is forced to `true` so the minimal wire fixture
    /// advances to `ValidationPassed` without requiring full S2.1 compliance.
    async fn receive_ankuendigung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LFN: parse NB UTILMD 55607 wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = gpke_ankuendigung_zuordnung_lf_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LFN: adapt UTILMD 55607 to AnkuendigungZuordnungLfCommand");

        // Override validation_passed to bypass AHB profile rules for this fixture.
        let cmd = match cmd {
            AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                ..
            } => {
                assert_eq!(
                    pid.as_u32(),
                    55607,
                    "adapter must extract PID 55607 from wire"
                );
                assert_eq!(
                    sender.as_str(),
                    NB_ID,
                    "adapter must extract sender GLN (NB) from NAD+MS"
                );
                assert_eq!(
                    receiver.as_str(),
                    LFN_ID,
                    "adapter must extract receiver GLN (LFN) from NAD+MR"
                );
                assert_eq!(
                    location_id.as_str(),
                    MALO_ID,
                    "adapter must extract MaLo from IDE+Z19"
                );
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref"
                );
                AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
                    pid,
                    sender,
                    receiver,
                    location_id,
                    document_date,
                    process_date,
                    message_ref,
                    validation_passed: true,
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("LFN: execute ReceiveAnkuendigung");
    }

    /// ERP action: respond to the NB's assignment announcement.
    ///
    /// `accepted = true`  → LFN issues 55608 Bestätigung.
    /// `accepted = false` → LFN issues 55609 Ablehnung.
    async fn send_antwort(&self, accepted: bool, reason: Option<&str>) {
        self.process
            .execute(AnkuendigungZuordnungLfCommand::SendAntwort {
                accepted,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("LFN: execute SendAntwort");
    }

    /// ERP action: confirm the assignment acknowledgement is complete.
    async fn zuordnung_bestaetigen(&self) {
        self.process
            .execute(AnkuendigungZuordnungLfCommand::ZuordnungBestaetigen)
            .await
            .expect("LFN: execute ZuordnungBestaetigen");
    }

    async fn state(&self) -> AnkuendigungZuordnungLfState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// GPKE Ankündigung Zuordnung LF PID 55607 — accept path → Zugeordnet.
///
/// NB sends UTILMD 55607; LFN receives the announcement, confirms acceptance
/// (→ 55608 Bestätigung), and acknowledges the assignment as complete.
///
/// Per BNetzA BK6-22-024 §4, the LFN must respond within 24 wall-clock hours.
#[tokio::test]
async fn e2e_ankuendigung_zuordnung_lf_accepted_and_zugeordnet() {
    let lfn = MockLfn::new();

    // ── LFN: receive 55607 Ankündigung ────────────────────────────────────────
    lfn.receive_ankuendigung(UTILMD_55607_BYTES).await;
    assert!(
        matches!(
            lfn.state().await,
            AnkuendigungZuordnungLfState::ValidationPassed(_)
        ),
        "LFN must be ValidationPassed after ReceiveAnkuendigung"
    );

    // ── LFN: accept the assignment (→ 55608 Bestätigung) ─────────────────────
    lfn.send_antwort(true, None).await;
    assert!(
        matches!(
            lfn.state().await,
            AnkuendigungZuordnungLfState::AntwortGesendet { .. }
        ),
        "LFN must be AntwortGesendet after SendAntwort(accepted)"
    );

    // ── LFN: confirm assignment complete ──────────────────────────────────────
    lfn.zuordnung_bestaetigen().await;

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, AnkuendigungZuordnungLfState::Zugeordnet(_)),
        "LFN must be Zugeordnet after ZuordnungBestaetigen; got: {final_state:?}"
    );
    if let AnkuendigungZuordnungLfState::Zugeordnet(data) = final_state {
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.sender.as_str(), NB_ID);
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55607,
            "persisted data must carry PID 55607"
        );
    }
}

/// GPKE Ankündigung Zuordnung LF PID 55607 — rejection path → Rejected.
///
/// LFN cannot accept the assignment (e.g. incorrect MaLo, contractual issue)
/// and responds with 55609 Ablehnung.
#[tokio::test]
async fn e2e_ankuendigung_zuordnung_lf_rejected() {
    let lfn = MockLfn::new();

    lfn.receive_ankuendigung(UTILMD_55607_BYTES).await;
    assert!(
        matches!(
            lfn.state().await,
            AnkuendigungZuordnungLfState::ValidationPassed(_)
        ),
        "LFN must be ValidationPassed before rejection"
    );

    // LFN rejects: unknown MaLo or contractual dispute.
    lfn.send_antwort(false, Some("Unbekannte Marktlokation"))
        .await;

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, AnkuendigungZuordnungLfState::Rejected { .. }),
        "LFN must be Rejected after SendAntwort(rejected); got: {final_state:?}"
    );
}

/// GPKE Ankündigung Zuordnung LF — AHB validation failure → Rejected immediately.
///
/// If the EDIFACT validator rejects the incoming 55607 (e.g. missing mandatory
/// segment), the workflow must reject without waiting for SendAntwort.
#[tokio::test]
async fn e2e_ankuendigung_zuordnung_lf_validation_failure_rejects() {
    use mako_engine::types::{MaLo, MarktpartnerCode, Pruefidentifikator};

    let lfn = MockLfn::new();

    let cmd = AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
        pid: Pruefidentifikator::new(55607).unwrap(),
        sender: MarktpartnerCode::new(NB_ID),
        receiver: MarktpartnerCode::new(LFN_ID),
        location_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250401".to_owned(),
        message_ref: MessageRef::new("MSG-BAD-55607"),
        validation_passed: false,
        validation_errors: vec!["NAD+MR missing party qualifier".to_owned()],
    };

    lfn.process
        .execute(cmd)
        .await
        .expect("execute validation failure");

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, AnkuendigungZuordnungLfState::Rejected { .. }),
        "LFN must be Rejected after validation failure; got: {final_state:?}"
    );
}

/// GPKE Ankündigung Zuordnung LF — timeout expiry → Rejected.
///
/// If the LFN does not respond within 24h, the deadline fires and the
/// process closes as Rejected.
#[tokio::test]
async fn e2e_ankuendigung_zuordnung_lf_timeout_closes_process() {
    use mako_engine::ids::DeadlineId;
    use mako_gpke::ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL;

    let lfn = MockLfn::new();

    lfn.receive_ankuendigung(UTILMD_55607_BYTES).await;
    assert!(
        matches!(
            lfn.state().await,
            AnkuendigungZuordnungLfState::ValidationPassed(_)
        ),
        "must be ValidationPassed before timeout"
    );

    // Simulate deadline firing before LFN responds.
    lfn.process
        .execute(AnkuendigungZuordnungLfCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL.into(),
        })
        .await
        .expect("execute TimeoutExpired");

    let final_state = lfn.state().await;
    assert!(
        matches!(final_state, AnkuendigungZuordnungLfState::Rejected { .. }),
        "LFN must be Rejected after timeout; got: {final_state:?}"
    );
}

/// GPKE Ankündigung Zuordnung LF — adapter correctly maps all wire fields.
///
/// Verifies that the UTILMD 55607 fixture maps to the expected command fields:
/// PID, sender GLN, receiver GLN, MaLo, and message_ref.
#[tokio::test]
async fn e2e_ankuendigung_zuordnung_lf_adapter_field_mapping() {
    let platform = Platform::with_all_profiles();
    let fv = FormatVersion::new(FV);

    let raw = platform
        .parse(UTILMD_55607_BYTES)
        .expect("parse UTILMD 55607");

    let cmd = gpke_ankuendigung_zuordnung_lf_registry()
        .dispatch(&raw as &dyn Any, &fv)
        .expect("dispatch UTILMD 55607");

    match cmd {
        AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
            pid,
            sender,
            receiver,
            location_id,
            message_ref,
            ..
        } => {
            assert_eq!(pid.as_u32(), 55607);
            assert_eq!(sender.as_str(), NB_ID);
            assert_eq!(receiver.as_str(), LFN_ID);
            assert_eq!(location_id.as_str(), MALO_ID);
            assert_eq!(message_ref.as_str(), "MSG-ZUORD-001");
        }
        _ => panic!("expected ReceiveAnkuendigung command"),
    }
}
