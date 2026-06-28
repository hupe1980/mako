//! End-to-end test: NB → LF GPKE NB-initiated Lieferende (PID 55007).
//!
//! The Netzbetreiber (NB) initiates a supply-end announcement; the Lieferant
//! (LF) receives the UTILMD 55007 Ankündigung, validates it, and responds
//! with acceptance (55008 Bestätigung) or rejection (55009 Ablehnung).
//!
//! # Protocol trace (accept path)
//!
//! ```text
//!   NB ERP (wire fixture)                      LF ERP (MockLf)
//!   ──────────────────────────────────────────────────────────────
//!                        ──── UTILMD 55007 ────►
//!                                               receive_ankuendigung(wire)
//!                                                 → adapter: ReceiveAnkuendigung
//!                                                 → state: ValidationPassed
//!                                               send_antwort(accepted=true)
//!                                                 → state: AntwortGesendet
//!                                               beenden_bestaetigen()
//!                                                 → state: Beendet
//!   ──────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 55007**: NB-initiated Lieferende Ankündigung (NB → LF, Strom)
//! - **PID 55008**: LF Bestätigung (LF → NB, outbound)
//! - **PID 55009**: LF Ablehnung (LF → NB, outbound)
//! - **Deadline**: 24 wall-clock hours for LF response (BK6-22-024 §4)
//! - **Regulatory basis**: UTILMD AHB Strom 2.1, GPKE domain
//!
//! AHB validation is bypassed for inbound `ReceiveAnkuendigung` because the
//! hand-crafted fixture does not satisfy all S2.1 profile rules.
//! AHB conformance is tested separately.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::MessageRef,
    version::{FormatVersion, WorkflowId},
};
use mako_gpke::{GpkeLfAbmeldungWorkflow, LfAbmeldungCommand, LfAbmeldungState};
use makod::adapters::gpke_lf_abmeldung_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const NB_ID: &str = "9900357000004"; // Netzbetreiber (Lieferende initiator)
const LF_ID: &str = "4012345000023"; // Lieferant (receiving party)
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── Wire fixture ──────────────────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD S2.1 PID 55007 — NB announces supply end to LF.
// The NB sends `NAD+MS` with its own GLN and `NAD+MR` with the LF's GLN.

const UTILMD_55007_BYTES: &[u8] = b"\
UNB+UNOC:3+9900357000004:14+4012345000023:14+250115:0800+LFSEG-2025-001'\
UNH+MSG-LFSEG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055007::+9'\
DTM+137:20250115:102'\
RFF+Z13:LFSEG-REF-001'\
NAD+MS+9900357000004::293'\
NAD+MR+4012345000023::293'\
IDE+Z19+51238696781::'\
DTM+92:20250401:102'\
UNT+9+MSG-LFSEG-001'\
UNZ+1+LFSEG-2025-001'";

// ── Mock LF ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Lieferant ERP** receiving and responding to an NB-initiated
/// supply-end announcement.
///
/// Owns a single `GpkeLfAbmeldungWorkflow` process backed by an in-memory store.
struct MockLf {
    process: Process<GpkeLfAbmeldungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockLf {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LF_ID),
                WorkflowId::new("gpke-lf-abmeldung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive NB's UTILMD 55007 wire bytes, adapt, and
    /// execute.
    ///
    /// `validation_passed` is forced to `true` so the minimal wire fixture
    /// advances to `ValidationPassed` without requiring full S2.1 compliance.
    async fn receive_ankuendigung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LF: parse NB UTILMD 55007 wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = gpke_lf_abmeldung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LF: adapt UTILMD 55007 to LfAbmeldungCommand");

        // Override validation_passed to bypass AHB profile rules for this
        // minimal fixture.
        let cmd = match cmd {
            LfAbmeldungCommand::ReceiveAnkuendigung {
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
                    55007,
                    "adapter must extract PID 55007 from wire"
                );
                assert_eq!(
                    sender.as_str(),
                    NB_ID,
                    "adapter must extract sender GLN (NB) from NAD+MS"
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
                LfAbmeldungCommand::ReceiveAnkuendigung {
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
            _ => panic!("expected LfAbmeldungCommand::ReceiveAnkuendigung"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("LF: execute ReceiveAnkuendigung");
    }

    /// ERP action: respond to the NB's supply-end announcement.
    ///
    /// `accepted = true`  → LF issues 55008 Bestätigung.
    /// `accepted = false` → LF issues 55009 Ablehnung.
    async fn send_antwort(&self, accepted: bool, reason: Option<&str>) {
        self.process
            .execute(LfAbmeldungCommand::SendAntwort {
                accepted,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("LF: execute SendAntwort");
    }

    /// ERP action: confirm that the supply relationship has ended.
    async fn beenden_bestaetigen(&self) {
        self.process
            .execute(LfAbmeldungCommand::BeendenBestaetigen)
            .await
            .expect("LF: execute BeendenBestaetigen");
    }

    async fn state(&self) -> LfAbmeldungState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// NB-initiated Lieferende PID 55007 — accept path → Beendet.
///
/// NB sends UTILMD 55007; LF receives the announcement, confirms acceptance
/// (→ 55008 Bestätigung), and marks the supply relationship as ended.
///
/// Per BNetzA BK6-22-024 §4, the LF must respond within 24 wall-clock hours.
#[tokio::test]
async fn e2e_lf_abmeldung_accepted_and_beendet() {
    let lf = MockLf::new();

    // ── LF: receive 55007 Ankündigung ─────────────────────────────────────────
    lf.receive_ankuendigung(UTILMD_55007_BYTES).await;
    assert!(
        matches!(lf.state().await, LfAbmeldungState::ValidationPassed(_)),
        "LF must be ValidationPassed after ReceiveAnkuendigung"
    );

    // ── LF: accept the announcement (→ 55008 Bestätigung) ────────────────────
    lf.send_antwort(true, None).await;
    assert!(
        matches!(lf.state().await, LfAbmeldungState::AntwortGesendet { .. }),
        "LF must be AntwortGesendet after SendAntwort(accepted)"
    );

    // ── LF: confirm supply end ────────────────────────────────────────────────
    lf.beenden_bestaetigen().await;

    let final_state = lf.state().await;
    assert!(
        matches!(final_state, LfAbmeldungState::Beendet(_)),
        "LF must be Beendet after BeendenBestaetigen; got: {final_state:?}"
    );
    if let LfAbmeldungState::Beendet(data) = final_state {
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.sender.as_str(), NB_ID);
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55007,
            "persisted data must carry PID 55007"
        );
    }
}

/// NB-initiated Lieferende PID 55007 — rejection path → Rejected.
///
/// NB sends UTILMD 55007; LF receives the announcement but cannot accept
/// the supply end (e.g. contractual dispute, incorrect MaLo).
/// LF responds with 55009 Ablehnung.
#[tokio::test]
async fn e2e_lf_abmeldung_rejected() {
    let lf = MockLf::new();

    lf.receive_ankuendigung(UTILMD_55007_BYTES).await;
    assert!(
        matches!(lf.state().await, LfAbmeldungState::ValidationPassed(_)),
        "LF must be ValidationPassed before rejection"
    );

    // LF rejects: contractual dispute or incorrect MaLo reference.
    lf.send_antwort(false, Some("Unbekannte Marktlokation"))
        .await;

    let final_state = lf.state().await;
    assert!(
        matches!(final_state, LfAbmeldungState::Rejected { .. }),
        "LF must be Rejected after SendAntwort(rejected); got: {final_state:?}"
    );
}

/// NB-initiated Lieferende — validation failure → Rejected immediately.
///
/// The AHB validator rejects the incoming message (e.g. missing mandatory
/// segment); the workflow must reject without waiting for SendAntwort.
#[tokio::test]
async fn e2e_lf_abmeldung_validation_failure_rejects() {
    use mako_engine::types::{MaLo, MarktpartnerCode, Pruefidentifikator};

    let lf = MockLf::new();

    let cmd = LfAbmeldungCommand::ReceiveAnkuendigung {
        pid: Pruefidentifikator::new(55007).unwrap(),
        sender: MarktpartnerCode::new(NB_ID),
        receiver: MarktpartnerCode::new(LF_ID),
        location_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250401".to_owned(),
        message_ref: MessageRef::new("MSG-BAD-55007"),
        validation_passed: false,
        validation_errors: vec!["NAD+MR missing party qualifier".to_owned()],
    };

    lf.process
        .execute(cmd)
        .await
        .expect("execute validation failure");

    let final_state = lf.state().await;
    assert!(
        matches!(final_state, LfAbmeldungState::Rejected { .. }),
        "LF must be Rejected after validation failure; got: {final_state:?}"
    );
}
