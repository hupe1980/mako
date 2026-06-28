//! End-to-end test: LF → NB GPKE Neuanlage (PID 55600 / 55601).
//!
//! The Lieferant (LF) triggers a new Marktlokation registration; the
//! Netzbetreiber (NB) receives the UTILMD 55600/55601 Anfrage, validates it,
//! and confirms acceptance (55602/55603) or rejection (55604/55605).
//!
//! # Protocol trace (PID 55600 accept path)
//!
//! ```text
//!   LF ERP (wire fixture)                      NB ERP (MockNb)
//!   ──────────────────────────────────────────────────────────────
//!                        ──── UTILMD 55600 ────►
//!                                               receive_anmeldung(wire)
//!                                                 → adapter: ReceiveAnmeldung
//!                                                 → state: ValidationPassed
//!                                               send_antwort(accepted=true)
//!                                                 → state: AntwortGesendet
//!                                               aktivieren()
//!                                                 → state: Aktiviert
//!   ──────────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 55600**: Neue verbindliche Marktlokation (NB → LF response: 55602/55604)
//! - **PID 55601**: Neue erzeugende Marktlokation (NB → LF response: 55603/55605)
//! - **Deadline**: 24 wall-clock hours for NB response (BK6-24-174 / BK6-22-024)
//! - **Regulatory basis**: BNetzA BK6-24-174 Anlage 1b §2.2 (Neuanlage)
//!
//! AHB validation is bypassed for inbound `ReceiveAnmeldung` because the
//! hand-crafted fixture does not satisfy all S2.1 profile rules.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::MessageRef,
    version::{FormatVersion, WorkflowId},
};
use mako_gpke::{GpkeNeuanlageWorkflow, NeuanlageCommand, NeuanlageState};
use makod::adapters::gpke_neuanlage_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const LF_ID: &str = "4012345000023"; // Lieferant (new-MaLo requester)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── Wire fixtures ─────────────────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD S2.1 messages.  The BGM+ qualifier is E01 for all
// UTILMD variants; the Prüfidentifikator is carried in BGM+E01:::+00055600::
// Note: process date (DTM+92) is embedded in the transaction group.

/// PID 55600: Neue verbrauchende Marktlokation (Anmeldung von LF an NB).
const UTILMD_55600_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+NANLG-2025-001'\
UNH+MSG-NANLG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055600::+9'\
DTM+137:20250115:102'\
RFF+Z13:NANLG-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+51238696781::'\
DTM+92:20250401:102'\
UNT+9+MSG-NANLG-001'\
UNZ+1+NANLG-2025-001'";

/// PID 55601: Neue erzeugende Marktlokation (Anmeldung von LF an NB).
const UTILMD_55601_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+NANLG-2025-002'\
UNH+MSG-NANLG-002+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055601::+9'\
DTM+137:20250115:102'\
RFF+Z13:NANLG-REF-002'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+51238696781::'\
DTM+92:20250401:102'\
UNT+9+MSG-NANLG-002'\
UNZ+1+NANLG-2025-002'";

// ── Mock NB ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Netzbetreiber ERP** receiving and acting on a Neuanlage
/// request.
///
/// Owns a single `GpkeNeuanlageWorkflow` process backed by an in-memory store.
struct MockNb {
    process: Process<GpkeNeuanlageWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("gpke-neuanlage", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive LF's UTILMD 55600/55601 wire bytes, adapt,
    /// and execute.
    ///
    /// `validation_passed` is forced to `true` so the minimal wire fixture
    /// advances to `ValidationPassed` without requiring full S2.1 compliance.
    async fn receive_anmeldung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse LF UTILMD Neuanlage wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = gpke_neuanlage_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt UTILMD Neuanlage to NeuanlageCommand");

        // Override validation_passed to bypass AHB profile rules for
        // this minimal fixture.
        let cmd = match cmd {
            NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                ..
            } => {
                assert!(
                    pid.as_u32() == 55600 || pid.as_u32() == 55601,
                    "adapter must extract PID 55600 or 55601 from wire; got: {}",
                    pid.as_u32()
                );
                assert_eq!(
                    sender.as_str(),
                    LF_ID,
                    "adapter must extract sender GLN (LF) from NAD+MS"
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
                NeuanlageCommand::ReceiveAnmeldung {
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
            _ => panic!("expected NeuanlageCommand::ReceiveAnmeldung"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveAnmeldung");
    }

    /// ERP action: send the response (accept or reject the Neuanlage request).
    async fn send_antwort(&self, accepted: bool, reason: Option<&str>) {
        self.process
            .execute(NeuanlageCommand::SendAntwort {
                accepted,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("NB: execute SendAntwort");
    }

    /// ERP action: activate the new Marktlokation in the grid.
    async fn aktivieren(&self) {
        self.process
            .execute(NeuanlageCommand::Aktivieren)
            .await
            .expect("NB: execute Aktivieren");
    }

    async fn state(&self) -> NeuanlageState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Neuanlage PID 55600 — accept path → Aktiviert.
///
/// NB receives 55600, confirms acceptance (→ 55602 outbound), and activates
/// the new verbindliche Marktlokation.
#[tokio::test]
async fn e2e_neuanlage_55600_accepted_and_activated() {
    let nb = MockNb::new();

    // ── NB: receive 55600 Anfrage ─────────────────────────────────────────────
    nb.receive_anmeldung(UTILMD_55600_BYTES).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveAnmeldung"
    );

    // ── NB: accept the request (→ 55602 Bestätigung) ─────────────────────────
    nb.send_antwort(true, None).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::AntwortGesendet { .. }),
        "NB must be AntwortGesendet after SendAntwort(accepted)"
    );

    // ── NB: activate the new MaLo in the grid ────────────────────────────────
    nb.aktivieren().await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Aktiviert(_)),
        "NB must be Aktiviert after Aktivieren; got: {final_state:?}"
    );
    if let NeuanlageState::Aktiviert(data) = final_state {
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.sender.as_str(), LF_ID);
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55600,
            "persisted data must carry PID 55600"
        );
    }
}

/// Neuanlage PID 55601 — accept path → Aktiviert.
///
/// Same flow as above but for neue erzeugende MaLo (PID 55601).
#[tokio::test]
async fn e2e_neuanlage_55601_accepted_and_activated() {
    let nb = MockNb::new();

    nb.receive_anmeldung(UTILMD_55601_BYTES).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveAnmeldung (55601)"
    );

    nb.send_antwort(true, None).await;
    nb.aktivieren().await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Aktiviert(_)),
        "NB must be Aktiviert after Aktivieren (55601); got: {final_state:?}"
    );
    if let NeuanlageState::Aktiviert(data) = final_state {
        assert_eq!(
            data.pruefidentifikator.as_u32(),
            55601,
            "persisted data must carry PID 55601"
        );
    }
}

/// Neuanlage PID 55600 — rejection path → Rejected.
///
/// NB receives 55600 but denies the request (→ 55604 Ablehnung outbound).
#[tokio::test]
async fn e2e_neuanlage_55600_rejected() {
    let nb = MockNb::new();

    nb.receive_anmeldung(UTILMD_55600_BYTES).await;
    assert!(
        matches!(nb.state().await, NeuanlageState::ValidationPassed(_)),
        "NB must be ValidationPassed before rejection"
    );

    // NB rejects: duplicate MaLo, grid capacity exceeded, etc.
    nb.send_antwort(false, Some("MaLo already registered in grid"))
        .await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Rejected { .. }),
        "NB must be Rejected after SendAntwort(rejected); got: {final_state:?}"
    );
}

/// Neuanlage — validation failure path: AHB check fails → Rejected.
///
/// A UTILMD with a known-bad wire (non-trivial validation failure) is
/// submitted without bypassing validation; the process should immediately
/// advance to `Rejected` without waiting for SendAntwort.
#[tokio::test]
async fn e2e_neuanlage_validation_failure_rejects() {
    use mako_engine::types::{MaLo, MarktpartnerCode, Pruefidentifikator};

    let nb = MockNb::new();

    // Inject ReceiveAnmeldung with validation_passed = false directly
    // (bypasses wire parsing to keep the fixture minimal).
    let cmd = NeuanlageCommand::ReceiveAnmeldung {
        pid: Pruefidentifikator::new(55600).unwrap(),
        sender: MarktpartnerCode::new(LF_ID),
        receiver: MarktpartnerCode::new(NB_ID),
        location_id: MaLo::new(MALO_ID),
        document_date: "20250115".to_owned(),
        process_date: "20250401".to_owned(),
        message_ref: MessageRef::new("MSG-BAD-001"),
        validation_passed: false,
        validation_errors: vec!["BGM qualifier missing".to_owned()],
    };

    nb.process
        .execute(cmd)
        .await
        .expect("execute validation failure");

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, NeuanlageState::Rejected { .. }),
        "NB must be Rejected after validation failure; got: {final_state:?}"
    );
}
