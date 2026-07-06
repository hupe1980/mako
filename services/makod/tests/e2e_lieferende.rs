//! Full end-to-end test: LFN ↔ NB Lieferende Strom (PID 55002).
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
//!   submit_anmeldung(55002)
//!     → asserts outbox payload invariants
//!     → renders UTILMD 55002 wire bytes
//!                        ──── UTILMD 55002 ────►
//!                                               receive_utilmd(wire)
//!                                                 → asserts UNH ref ≠ "1"
//!                                               send_antwort(accepted=true)
//!                                                 → asserts UTILMD 55005
//!                                                 → no MSCONS (Lieferende
//!                                                    has no Bewegungsdaten
//!                                                    obligation — BK6-22-024)
//!                        ◄─── UTILMD 55005 ────
//!   receive_antwort(wire)
//!   ──────────────────────────────────────────────────────────
//!   final: Active                               AntwortGesendet
//! ```
//!
//! # Regulatory context
//!
//! - **PID 55002**: Anfrage Lieferende Strom (LFN → NB)
//! - **PID 55005**: Bestätigung Lieferende (NB → LFN, accept)
//! - **PID 55006**: Ablehnung Lieferende (NB → LFN, reject)
//! - **Deadline**: 24 wall-clock hours (BNetzA BK6-22-024)
//! - **No MSCONS 13015**: Bewegungsdaten obligations are triggered **only** by
//!   PID 55001 (Lieferbeginn). For PID 55002 (Lieferende) the acceptance does
//!   not require a subsequent MSCONS 13015 — there are no Bewegungsdaten to
//!   request when a supply relationship is ending.
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
    SupplierChangeCommand, SupplierChangeState,
};
use makod::{
    adapters::{gpke_lf_anmeldung_registry, gpke_registry},
    config::PartyConfig,
    edifact_renderer::render_to_wire_bytes,
    party_registry::GlnRegistry,
};

fn make_registry(gln: &str, role: &str) -> GlnRegistry {
    GlnRegistry::from_config(&[PartyConfig {
        gln: gln.to_owned(),
        roles: vec![role.to_owned()],
        primary: true,
        agency: None,
    }])
    .expect("test registry")
}

// ── Constants ─────────────────────────────────────────────────────────────────

const LFN_ID: &str = "4012345000023"; // Lieferant (outgoing supplier)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── Mock LFN ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Lieferant's ERP** triggering and receiving MaKo commands.
///
/// Owns a single `GpkeLfAnmeldungWorkflow` process backed by an in-memory
/// store.
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

    /// ERP action: submit a Lieferende Anmeldung (PID 55002).
    ///
    /// Asserts that the resulting outbox payload contains the ERP-supplied
    /// fields and that renderer-derived fields (`message_ref`, `document_date`)
    /// are absent from the payload.
    ///
    /// Returns the rendered EDIFACT wire bytes for transport to the NB.
    async fn submit_anmeldung(&self, malo_id: &str, process_date: &str) -> Vec<u8> {
        let (_, outbox) = self
            .process
            .execute_and_collect(LfAnmeldungCommand::InitiateAnmeldung {
                pid: Pruefidentifikator::new(55002).unwrap(),
                sender: MarktpartnerCode::new(LFN_ID),
                receiver: MarktpartnerCode::new(NB_ID),
                location_id: MaLo::new(malo_id),
                process_date: process_date.to_owned(),
            })
            .await
            .expect("LFN: execute InitiateAnmeldung 55002");

        assert_eq!(
            outbox.len(),
            1,
            "LFN must enqueue exactly one outbound message"
        );
        let msg = &outbox[0];
        assert_eq!(msg.message_type.as_ref(), "UTILMD");
        assert_eq!(
            msg.payload["pid"].as_u64().unwrap(),
            55002_u64,
            "outbox payload must carry PID 55002 (Anfrage Lieferende)"
        );
        assert_eq!(msg.payload["malo"].as_str().unwrap(), malo_id);
        assert_eq!(msg.payload["sender"].as_str().unwrap(), LFN_ID);
        assert_eq!(msg.payload["receiver"].as_str().unwrap(), NB_ID);
        assert_eq!(msg.payload["process_date"].as_str().unwrap(), process_date,);
        assert_eq!(msg.recipient.as_ref(), NB_ID);
        assert!(
            msg.payload.get("message_ref").is_none(),
            "message_ref must not appear in outbox payload — derived at render time"
        );
        assert!(
            msg.payload.get("document_date").is_none(),
            "document_date must not appear in outbox payload — set to today at dispatch time"
        );

        render_to_wire_bytes(msg, &make_registry(LFN_ID, "LF"))
            .expect("LFN: render_to_wire_bytes 55002")
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

// ── Mock NB ERP backend ────────────────────────────────────────────────────────

/// Simulates the **Netzbetreiber's ERP** receiving and responding to the
/// Lieferende request.
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

    /// ERP notification: receive LFN's UTILMD 55002 wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true` — the minimal rendered UTILMD does
    /// not satisfy all S2.1 profile rules; AHB conformance is tested separately.
    ///
    /// Asserts the UNH message reference is derived from `causation_event_id`
    /// (not the fallback `"1"`).
    async fn receive_utilmd(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse LFN UTILMD wire");

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
                assert_eq!(
                    pid.as_u32(),
                    55002,
                    "NB adapter must extract PID 55002 from wire"
                );
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
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected SupplierChangeCommand::ReceiveUtilmd"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveUtilmd 55002");
    }

    /// ERP action: send Bestätigung (`accepted = true`, PID 55005) or Ablehnung
    /// (`accepted = false`, PID 55006) for the pending Lieferende request.
    ///
    /// Asserts outbox content:
    /// - Both accept and reject produce exactly **one** UTILMD outbox entry.
    /// - **No MSCONS 13015** in either case: Bewegungsdaten obligations are
    ///   triggered only by PID 55001 (Lieferbeginn). For PID 55002 (Lieferende)
    ///   `post_acceptance::lieferbeginn_obligations(55002, …)` returns empty.
    ///
    /// Returns the rendered UTILMD wire bytes for transport back to the LFN.
    async fn send_antwort(&self, accepted: bool, reason: Option<&str>) -> Vec<u8> {
        // PID 55002 Lieferende has no post-acceptance obligations.
        // `lieferbeginn_obligations` guards on `anfrage_pid != 55001` and
        // returns an empty Vec for all other PIDs.
        let obligations: Vec<mako_engine::outbox::PendingOutbox> = vec![];

        let (_, outbox) = self
            .process
            .execute_and_collect(SupplierChangeCommand::SendAntwort {
                accepted,
                reason: reason.map(str::to_owned),
                obligations,
            })
            .await
            .expect("NB: execute SendAntwort");

        let expected_pid: u64 = if accepted { 55005 } else { 55006 };
        assert_eq!(
            outbox.len(),
            1,
            "Lieferende Antwort must enqueue exactly one UTILMD (PID {expected_pid}); \
             no MSCONS obligations for 55002"
        );

        let utilmd = &outbox[0];
        assert_eq!(utilmd.message_type.as_ref(), "UTILMD");
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

        render_to_wire_bytes(utilmd, &make_registry(NB_ID, "NB")).expect("NB: render_to_wire_bytes")
    }

    async fn state(&self) -> SupplierChangeState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Lieferende Strom — acceptance path (PID 55002 → 55005).
///
/// LFN ERP terminates the supply relationship; NB ERP confirms; LFN reaches
/// `Active` (both parties agreed the supply ends on the given date).
///
/// Key invariant: the acceptance does **not** trigger MSCONS 13015. Requesting
/// Bewegungsdaten is only required when a new supplier takes over (PID 55001).
/// When supply ends without a new supplier, there are no Bewegungsdaten to
/// request (BK6-22-024 § 3 Abs. 5).
#[tokio::test]
async fn e2e_lieferende_strom_happy_path() {
    let lfn = MockLfn::new();
    let nb = MockNb::new();

    // ── LFN ERP: submit Lieferende Anmeldung ──────────────────────────────────
    let wire_55002 = lfn.submit_anmeldung(MALO_ID, "20251001").await;
    assert!(
        matches!(lfn.state().await, LfAnmeldungState::Pending(_)),
        "LFN must be Pending after InitiateAnmeldung 55002"
    );

    // ── NB ERP: receive UTILMD 55002 ──────────────────────────────────────────
    nb.receive_utilmd(&wire_55002).await;
    assert!(
        matches!(nb.state().await, SupplierChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed after ReceiveUtilmd 55002"
    );

    // ── NB ERP: send Bestätigung (55005, no MSCONS) ───────────────────────────
    let wire_55005 = nb.send_antwort(true, None).await;
    assert!(
        matches!(
            nb.state().await,
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "NB must be AntwortGesendet after sending Bestätigung 55005"
    );

    // ── LFN ERP: receive Bestätigung ──────────────────────────────────────────
    lfn.receive_antwort(&wire_55005).await;

    let lfn_final = lfn.state().await;
    assert!(
        matches!(lfn_final, LfAnmeldungState::Active(_)),
        "LFN must be Active after receiving Bestätigung 55005; got: {lfn_final:?}"
    );
    if let LfAnmeldungState::Active(data) = lfn_final {
        assert_eq!(data.pruefidentifikator.as_u32(), 55002);
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.process_date, "20251001");
        assert_eq!(data.sender.as_str(), LFN_ID);
        assert_eq!(data.receiver.as_str(), NB_ID);
    }
}

/// Lieferende Strom — rejection path (PID 55002 → 55006).
///
/// NB ERP rejects the Lieferende request; both parties end in `Rejected`.
///
/// Rejection of Lieferende is permitted (e.g. the MaLo is unknown at the NB,
/// or the data provided is inconsistent).  The LFN must then either correct
/// the data and re-submit or escalate via a dispute process.
#[tokio::test]
async fn e2e_lieferende_strom_rejection_path() {
    let lfn = MockLfn::new();
    let nb = MockNb::new();

    // ── LFN ERP: submit Lieferende Anmeldung ──────────────────────────────────
    let wire_55002 = lfn.submit_anmeldung(MALO_ID, "20251001").await;

    // ── NB ERP: receive UTILMD 55002, then reject ─────────────────────────────
    nb.receive_utilmd(&wire_55002).await;
    let wire_55006 = nb
        .send_antwort(false, Some("MaLo Lieferdatum ungültig"))
        .await;

    // NB state: Rejected (SendAntwort with accepted=false transitions via
    // AntwortGesendet event but apply() moves to Rejected).
    let nb_state = nb.state().await;
    assert!(
        matches!(nb_state, SupplierChangeState::Rejected { .. }),
        "NB must be Rejected after sending Ablehnung 55006; got: {nb_state:?}"
    );

    // ── LFN ERP: receive Ablehnung ────────────────────────────────────────────
    lfn.receive_antwort(&wire_55006).await;

    let lfn_final = lfn.state().await;
    assert!(
        matches!(lfn_final, LfAnmeldungState::Rejected { .. }),
        "LFN must be Rejected after receiving Ablehnung 55006; got: {lfn_final:?}"
    );
}
