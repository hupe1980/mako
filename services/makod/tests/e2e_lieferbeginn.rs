//! Full end-to-end test: LFN ↔ NB Lieferbeginn Strom (PID 55001).
//!
//! Two mock ERP backends — [`MockLfn`] (Lieferant) and [`MockNb`]
//! (Netzbetreiber) — exchange EDIFACT over the **production**
//! render → wire bytes → parse → adapt pipeline.
//!
//! # Protocol trace
//!
//! ```text
//!   LFN ERP (MockLfn)                          NB ERP (MockNb)
//!   ──────────────────────────────────────────────────────────
//!   submit_anmeldung(55001)
//!     → asserts outbox payload invariants
//!     → renders UTILMD 55001 wire bytes
//!                        ──── UTILMD 55001 ────►
//!                                               receive_utilmd(wire)
//!                                                 → asserts UNH ref ≠ "1"
//!                                                 → asserts adapter preserves ref
//!                                               send_antwort(accepted=true)
//!                                                 → asserts MSCONS 13015 present
//!                                                 → renders UTILMD 55003 wire
//!                        ◄─── UTILMD 55003 ────
//!   receive_antwort(wire)
//!   ──────────────────────────────────────────────────────────
//!   final: Active                               AntwortGesendet
//! ```
//!
//! AHB validation is bypassed for the NB's inbound `ReceiveUtilmd` because
//! `render_to_wire_bytes` generates a minimal UTILMD that does not satisfy
//! all S2.1 profile rules.  AHB conformance is tested separately in
//! `crates/edi-energy/tests/`.

use std::any::Any;

use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::{MaLo, MarktpartnerCode, Pruefidentifikator},
    version::{FormatVersion, WorkflowId},
};
use mako_gpke::{
    GpkeLfAnmeldungWorkflow, GpkeSupplierChangeWorkflow, LfAnmeldungCommand, LfAnmeldungState,
    SupplierChangeCommand, SupplierChangeState, post_acceptance,
};
use makod::{
    adapters::{gpke_lf_anmeldung_registry, gpke_registry},
    config::PartyConfig,
    edifact_renderer::render_to_wire_bytes,
    party_registry::MpIdRegistry,
};

fn make_registry(mp_id: &str, role: &str) -> MpIdRegistry {
    MpIdRegistry::from_config(&[PartyConfig {
        mp_id: mp_id.to_owned(),
        roles: vec![role.to_owned()],
        primary: true,
        agency: None,
    }])
    .expect("test registry")
}

// ── Constants ─────────────────────────────────────────────────────────────────

const LFN_ID: &str = "4012345000023"; // Lieferant (new supplier)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── Mock LFN ERP backend ──────────────────────────────────────────────────────

/// Simulates the **Lieferant's ERP** triggering and receiving MaKo commands.
///
/// Owns a single `GpkeLfAnmeldungWorkflow` process backed by an in-memory
/// store.  Each method corresponds to one ERP action or inbound notification.
struct MockLfn {
    process: Process<GpkeLfAnmeldungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockLfn {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LFN_ID),
                WorkflowId::new("gpke-lf-anmeldung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP action: submit a Lieferbeginn (or Lieferende / Kündigung) Anmeldung.
    ///
    /// Asserts that the resulting outbox payload contains **only** ERP-owned
    /// fields — `message_ref` and `document_date` must be absent because the
    /// EDIFACT renderer derives them at dispatch time from `causation_event_id`
    /// and the current clock respectively.
    ///
    /// Returns the rendered EDIFACT wire bytes for transport to the NB.
    async fn submit_anmeldung(&self, pid: u32, malo_id: &str, process_date: &str) -> Vec<u8> {
        let (_, outbox) = self
            .process
            .execute_and_collect(LfAnmeldungCommand::InitiateAnmeldung {
                pid: Pruefidentifikator::new(pid).unwrap(),
                sender: MarktpartnerCode::new(LFN_ID),
                receiver: MarktpartnerCode::new(NB_ID),
                location_id: MaLo::new(malo_id),
                process_date: process_date.to_owned(),
            })
            .await
            .expect("LFN: execute InitiateAnmeldung");

        assert_eq!(
            outbox.len(),
            1,
            "LFN must enqueue exactly one outbound message"
        );
        let msg = &outbox[0];
        assert_eq!(msg.message_type.as_ref(), "UTILMD");
        assert_eq!(msg.payload["pid"].as_u64().unwrap(), pid as u64);
        assert_eq!(msg.payload["malo"].as_str().unwrap(), malo_id);
        assert_eq!(msg.payload["sender"].as_str().unwrap(), LFN_ID);
        assert_eq!(msg.payload["receiver"].as_str().unwrap(), NB_ID);
        assert_eq!(
            msg.payload["process_date"].as_str().unwrap(),
            process_date,
            "outbox payload must carry the ERP-supplied process_date"
        );
        assert_eq!(
            msg.recipient.as_ref(),
            NB_ID,
            "UTILMD must be addressed to the NB"
        );
        assert!(
            msg.payload.get("message_ref").is_none(),
            "message_ref must not appear in the outbox payload — \
             the renderer derives it from causation_event_id"
        );
        assert!(
            msg.payload.get("document_date").is_none(),
            "document_date must not appear in the outbox payload — \
             the renderer sets it to today at dispatch time"
        );

        render_to_wire_bytes(msg, &make_registry(LFN_ID, "LF"))
            .expect("LFN: render_to_wire_bytes")
            .bytes
    }

    /// ERP notification: receive NB's UTILMD response wire bytes and process them.
    async fn receive_antwort(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LFN: parse NB UTILMD wire");
        let cmd = gpke_lf_anmeldung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LFN: adapt NB response to LfAnmeldungCommand");
        self.process
            .execute(cmd)
            .await
            .expect("LFN: execute HandleAntwort");
    }

    async fn state(&self) -> LfAnmeldungState {
        self.process.state().await.unwrap()
    }
}

// ── Mock NB ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Netzbetreiber's ERP** receiving and responding to MaKo
/// supplier-change requests.
///
/// Owns a single `GpkeSupplierChangeWorkflow` process backed by an in-memory
/// store.
struct MockNb {
    process: Process<GpkeSupplierChangeWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("gpke-supplier-change", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive LFN's UTILMD wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true` — the minimal rendered UTILMD does
    /// not satisfy all S2.1 profile rules; AHB conformance is tested separately.
    ///
    /// Asserts:
    /// - The UNH message reference is non-trivial (derived from the LFN's
    ///   `causation_event_id`, not the legacy fallback `"1"`).
    /// - The adapter preserved that reference in `ReceiveUtilmd.message_ref` so
    ///   the NB can echo it in any subsequent APERAK `orig_message_ref`.
    async fn receive_utilmd(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse LFN UTILMD wire");

        // Assert the renderer derived a real UNH ref from causation_event_id.
        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty() && unh_ref != "1",
            "UNH message_ref must be derived from causation_event_id; got: {unh_ref:?}",
        );

        let cmd = gpke_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt LFN UTILMD to SupplierChangeCommand");

        let cmd = match cmd {
            SupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                ..
            } => {
                // The adapter must pass the UNH ref through so the NB can echo
                // it in APERAK orig_message_ref.
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref from parsed UTILMD",
                );
                SupplierChangeCommand::ReceiveUtilmd {
                    pid,
                    sender,
                    receiver,
                    location_id,
                    document_date,
                    process_date,
                    message_ref,
                    received_at: time::OffsetDateTime::now_utc(),
                    bilanzierungsgebiet: None,
                    bilanzierungsmethode: None,
                    fallgruppe: None,
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected SupplierChangeCommand::ReceiveUtilmd"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveUtilmd");
    }

    /// ERP action: send Bestätigung (`accepted = true`) or Ablehnung
    /// (`accepted = false`) for the pending supplier-change request.
    ///
    /// Asserts outbox content:
    /// - `accepted = true`  → UTILMD 55003 + MSCONS 13015 (Bewegungsdaten).
    /// - `accepted = false` → UTILMD 55004 only (no MSCONS).
    ///
    /// Returns the rendered UTILMD wire bytes for transport back to the LFN.
    async fn send_antwort(&self, accepted: bool, reason: Option<&str>) -> Vec<u8> {
        let malo = mako_engine::types::MaLo::from(MALO_ID);
        let new_supplier = mako_engine::types::MarktpartnerCode::from(LFN_ID);
        let obligations = if accepted {
            post_acceptance::lieferbeginn_obligations(55001, &malo, &new_supplier, None)
        } else {
            vec![]
        };
        let (_, outbox) = self
            .process
            .execute_and_collect(SupplierChangeCommand::SendAntwort {
                accepted,
                reason: reason.map(str::to_owned),
                obligations,
            })
            .await
            .expect("NB: execute SendAntwort");

        let expected_pid: u64 = if accepted { 55003 } else { 55004 };
        let utilmd = outbox
            .iter()
            .find(|e| e.message_type.as_ref() == "UTILMD")
            .expect("NB outbox must have a UTILMD Antwort");
        assert_eq!(
            utilmd.payload["pid"].as_u64().unwrap(),
            expected_pid,
            "NB outbox UTILMD must be PID {expected_pid}"
        );
        assert_eq!(
            utilmd.recipient.as_ref(),
            LFN_ID,
            "NB Antwort must be addressed to the LFN"
        );

        if accepted {
            // Bestätigung also enqueues MSCONS 13015 (Bewegungsdaten-Anforderung).
            let mscons = outbox
                .iter()
                .find(|e| e.message_type.as_ref() == "MSCONS")
                .expect("accepted SendAntwort must include MSCONS 13015");
            assert_eq!(
                mscons.payload["pid"].as_u64().unwrap(),
                13015,
                "MSCONS must be PID 13015 (Bewegungsdaten)"
            );
            assert_eq!(
                mscons.payload["malo"].as_str().unwrap(),
                MALO_ID,
                "MSCONS must reference the same MaLo"
            );
            assert_eq!(
                mscons.recipient.as_ref(),
                LFN_ID,
                "MSCONS 13015 must be addressed to the LFN"
            );
        } else {
            assert_eq!(
                outbox.len(),
                1,
                "rejection must enqueue only UTILMD {expected_pid} (no MSCONS)"
            );
        }

        render_to_wire_bytes(utilmd, &make_registry(NB_ID, "NB"))
            .expect("NB: render_to_wire_bytes")
            .bytes
    }

    async fn state(&self) -> SupplierChangeState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Lieferbeginn Strom — acceptance path (PID 55001 → 55003).
///
/// LFN ERP initiates a supplier change; NB ERP confirms; LFN reaches `Active`.
#[tokio::test]
async fn e2e_lieferbeginn_strom_happy_path() {
    let lfn = MockLfn::new();
    let nb = MockNb::new();

    // ── LFN ERP: submit Lieferbeginn Anmeldung ────────────────────────────────
    let wire_55001 = lfn.submit_anmeldung(55001, MALO_ID, "20251001").await;
    assert!(
        matches!(lfn.state().await, LfAnmeldungState::Pending(_)),
        "LFN must be Pending after InitiateAnmeldung"
    );

    // ── NB ERP: receive UTILMD 55001 ─────────────────────────────────────────
    nb.receive_utilmd(&wire_55001).await;
    assert!(
        matches!(nb.state().await, SupplierChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveUtilmd"
    );

    // ── NB ERP: send Bestätigung ──────────────────────────────────────────────
    let wire_55003 = nb.send_antwort(true, None).await;
    assert!(
        matches!(
            nb.state().await,
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "NB must be AntwortGesendet after sending Bestätigung"
    );

    // ── LFN ERP: receive Bestätigung ─────────────────────────────────────────
    lfn.receive_antwort(&wire_55003).await;

    // ── Final state: LFN Active — assert all business data fields ────────────
    let lfn_final = lfn.state().await;
    assert!(
        matches!(lfn_final, LfAnmeldungState::Active(_)),
        "LFN must be Active after receiving Bestätigung; got: {lfn_final:?}"
    );
    if let LfAnmeldungState::Active(data) = lfn_final {
        assert_eq!(data.pruefidentifikator.as_u32(), 55001);
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.process_date, "20251001");
        assert_eq!(data.sender.as_str(), LFN_ID);
        assert_eq!(data.receiver.as_str(), NB_ID);
    }
}

/// Lieferbeginn Strom — rejection path (PID 55001 → 55004).
///
/// NB ERP rejects the Anmeldung; both parties end in `Rejected`.
#[tokio::test]
async fn e2e_lieferbeginn_strom_rejection_path() {
    let lfn = MockLfn::new();
    let nb = MockNb::new();

    // ── LFN ERP: submit Lieferbeginn Anmeldung ────────────────────────────────
    let wire_55001 = lfn.submit_anmeldung(55001, MALO_ID, "20251001").await;

    // ── NB ERP: receive UTILMD 55001, then reject ─────────────────────────────
    nb.receive_utilmd(&wire_55001).await;
    let wire_55004 = nb.send_antwort(false, Some("Stammdaten unbekannt")).await;
    // When accepted=false, apply() transitions NB to Rejected (not AntwortGesendet) —
    // the AntwortGesendet *event* is emitted but the state machine moves to Rejected.
    assert!(
        matches!(nb.state().await, SupplierChangeState::Rejected { .. }),
        "NB must be Rejected after sending negative Antwort"
    );

    // ── LFN ERP: receive Ablehnung ────────────────────────────────────────────
    lfn.receive_antwort(&wire_55004).await;
    assert!(
        matches!(lfn.state().await, LfAnmeldungState::Rejected { .. }),
        "LFN must be Rejected after receiving Ablehnung"
    );
}
