//! Full end-to-end test: trilateral Lieferantenwechsel Strom (PIDs 55001, 55016).
//!
//! Three mock ERP backends — [`MockLfn`] (new supplier), [`MockNb`]
//! (Netzbetreiber), and [`MockLfa`] (old supplier) — exchange EDIFACT over the
//! **production** render → wire bytes → parse → adapt pipeline.
//!
//! # Business context
//!
//! A Lieferantenwechsel Strom comprises two parallel GPKE legs both initiated
//! by the new supplier (LFN):
//!
//! 1. **LFN → NB**: Anfrage Lieferbeginn (PID 55001) — the NB must approve
//!    or reject the supply start within **24 wall-clock hours** (BK6-22-024).
//! 2. **LFN → LFA**: Kündigung Lieferbeginn (PID 55016) — the old supplier
//!    **must always accept** the cancellation per LFW24; there is no rejection
//!    path for PID 55016 (LFA responds with 55017 Bestätigung).
//!
//! Each leg is an independent [`GpkeLfAnmeldungWorkflow`] process on the LFN side
//! and an independent [`GpkeSupplierChangeWorkflow`] process on the receiving
//! party side.
//!
//! # Protocol trace
//!
//! ```text
//!   LFN ERP (MockLfn)                  NB ERP (MockNb)         LFA ERP (MockLfa)
//!   ──────────────────────────────────────────────────────────────────────────────
//!   [nb_leg]  submit_lieferbeginn(55001)
//!     → asserts outbox payload invariants
//!     → renders UTILMD 55001 wire bytes
//!                        ─── UTILMD 55001 ──►
//!                                            receive_utilmd(wire)
//!                                              → asserts UNH ref ≠ "1"
//!                                            send_antwort(accepted=true)
//!                                              → asserts UTILMD 55003 + MSCONS 13015
//!                        ◄── UTILMD 55003 ───
//!
//!   [lfa_leg] submit_kuendigung(55016)
//!     → asserts outbox payload invariants
//!     → renders UTILMD 55016 wire bytes
//!                                                        ─── UTILMD 55016 ──►
//!                                                        receive_kuendigung(wire)
//!                                                          → asserts UNH ref ≠ "1"
//!                                                          → asserts PID 55016
//!                                                        send_bestaetigung()
//!                                                          → asserts UTILMD 55017
//!                                                          → asserts no MSCONS
//!                                                        ◄── UTILMD 55017 ───
//!
//!   [nb_leg]  receive_antwort_von_nb(wire_55003)
//!   [lfa_leg] receive_antwort_von_lfa(wire_55017)
//!   ──────────────────────────────────────────────────────────────────────────────
//!   final:  nb_leg  = Active           AntwortGesendet         AntwortGesendet
//!           lfa_leg = Active
//! ```
//!
//! AHB validation is bypassed for NB/LFA `ReceiveUtilmd` because
//! `render_to_wire_bytes` generates minimal UTILMD messages that do not
//! satisfy all S2.1 profile rules.  AHB conformance is tested separately in
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

const LFN_ID: &str = "4012345000023"; // Lieferant Neu  (new supplier)
const LFA_ID: &str = "4012345000047"; // Lieferant Alt  (old supplier)
const NB_ID: &str = "9900357000004"; // Netzbetreiber
const MALO_ID: &str = "51238696781"; // Marktlokations-ID
const FV: &str = "FV2025-10-01";

// ── Mock LFN ERP backend ──────────────────────────────────────────────────────

/// Simulates the **new supplier's (LFN) ERP** initiating and tracking both
/// GPKE legs of a Lieferantenwechsel:
///
/// - `nb_leg`:  `GpkeLfAnmeldungWorkflow` for the NB-directed Anfrage (PID 55001).
/// - `lfa_leg`: `GpkeLfAnmeldungWorkflow` for the LFA-directed Kündigung (PID 55016).
///
/// Each leg is an entirely independent event-sourced process backed by its own
/// `InMemoryEventStore`.  They share no state and no event log.
struct MockLfn {
    /// LFN → NB leg: Anfrage Lieferbeginn Strom (55001 → 55003/55004).
    nb_leg: Process<GpkeLfAnmeldungWorkflow, InMemoryEventStore>,
    /// LFN → LFA leg: Kündigung Lieferbeginn (55016 → 55017 accepted).
    lfa_leg: Process<GpkeLfAnmeldungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockLfn {
    fn new() -> Self {
        Self {
            nb_leg: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LFN_ID),
                WorkflowId::new("gpke-lf-anmeldung", FV),
            ),
            lfa_leg: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LFN_ID),
                WorkflowId::new("gpke-lf-anmeldung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP action: submit Anfrage Lieferbeginn (PID 55001) to the NB.
    ///
    /// Asserts that the resulting outbox payload contains only ERP-owned fields
    /// (`message_ref` and `document_date` must be absent — derived by the
    /// renderer from `causation_event_id` and the current clock).
    ///
    /// Returns the rendered UTILMD 55001 wire bytes for transport to the NB.
    async fn submit_lieferbeginn(&self, malo_id: &str, process_date: &str) -> Vec<u8> {
        let (_, outbox) = self
            .nb_leg
            .execute_and_collect(LfAnmeldungCommand::InitiateAnmeldung {
                pid: Pruefidentifikator::new(55001).unwrap(),
                sender: MarktpartnerCode::new(LFN_ID),
                receiver: MarktpartnerCode::new(NB_ID),
                location_id: MaLo::new(malo_id),
                process_date: process_date.to_owned(),
            })
            .await
            .expect("LFN: execute InitiateAnmeldung (55001)");

        assert_eq!(
            outbox.len(),
            1,
            "nb_leg must enqueue exactly one outbound message"
        );
        let msg = &outbox[0];
        assert_eq!(msg.message_type.as_ref(), "UTILMD");
        assert_eq!(msg.payload["pid"].as_u64().unwrap(), 55001_u64);
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
            "UTILMD 55001 must be addressed to the NB"
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
            .expect("LFN: render_to_wire_bytes (55001)")
    }

    /// ERP action: submit Kündigung Lieferbeginn (PID 55016) to the old supplier.
    ///
    /// Per LFW24 the old supplier must always accept — this leg is never
    /// rejected (LFA responds with 55017 Bestätigung).  Asserts outbox payload
    /// invariants and returns the rendered UTILMD 55016 wire bytes for transport
    /// to the LFA.
    async fn submit_kuendigung(&self, malo_id: &str, process_date: &str) -> Vec<u8> {
        let (_, outbox) = self
            .lfa_leg
            .execute_and_collect(LfAnmeldungCommand::InitiateAnmeldung {
                pid: Pruefidentifikator::new(55016).unwrap(),
                sender: MarktpartnerCode::new(LFN_ID),
                receiver: MarktpartnerCode::new(LFA_ID),
                location_id: MaLo::new(malo_id),
                process_date: process_date.to_owned(),
            })
            .await
            .expect("LFN: execute InitiateAnmeldung (55016)");

        assert_eq!(
            outbox.len(),
            1,
            "lfa_leg must enqueue exactly one outbound message"
        );
        let msg = &outbox[0];
        assert_eq!(msg.message_type.as_ref(), "UTILMD");
        assert_eq!(msg.payload["pid"].as_u64().unwrap(), 55016_u64);
        assert_eq!(
            msg.payload["process_date"].as_str().unwrap(),
            process_date,
            "outbox payload must carry the ERP-supplied process_date"
        );
        assert_eq!(
            msg.recipient.as_ref(),
            LFA_ID,
            "UTILMD 55016 must be addressed to the LFA"
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
            .expect("LFN: render_to_wire_bytes (55016)")
    }

    /// ERP notification: receive NB's UTILMD response (55003/55004) and execute on `nb_leg`.
    async fn receive_antwort_von_nb(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LFN: parse NB UTILMD wire");
        let cmd = gpke_lf_anmeldung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LFN: adapt NB response to LfAnmeldungCommand");
        self.nb_leg
            .execute(cmd)
            .await
            .expect("LFN: execute HandleAntwort (nb_leg)");
    }

    /// ERP notification: receive LFA's UTILMD 55017 and execute on `lfa_leg`.
    async fn receive_antwort_von_lfa(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LFN: parse LFA UTILMD wire");
        let cmd = gpke_lf_anmeldung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LFN: adapt LFA response to LfAnmeldungCommand");
        self.lfa_leg
            .execute(cmd)
            .await
            .expect("LFN: execute HandleAntwort (lfa_leg)");
    }

    async fn nb_leg_state(&self) -> LfAnmeldungState {
        self.nb_leg.state().await.unwrap()
    }

    async fn lfa_leg_state(&self) -> LfAnmeldungState {
        self.lfa_leg.state().await.unwrap()
    }
}

// ── Mock NB ERP backend ───────────────────────────────────────────────────────

/// Simulates the **Netzbetreiber's ERP** receiving and responding to the new
/// supplier's Anfrage Lieferbeginn (PID 55001).
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

    /// ERP notification: receive LFN's UTILMD 55001 wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true`.  Asserts that the UNH message
    /// reference is non-trivial (derived from causation_event_id, not `"1"`) and
    /// that the adapter preserved it in `ReceiveUtilmd.message_ref`.
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
                    validation_passed: true,
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
    /// (`accepted = false`).
    ///
    /// - `accepted = true`  → asserts UTILMD 55003 + MSCONS 13015 in outbox.
    /// - `accepted = false` → asserts UTILMD 55004 only (no MSCONS).
    ///
    /// Returns the rendered UTILMD wire bytes for transport to the LFN.
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
            let mscons = outbox
                .iter()
                .find(|e| e.message_type.as_ref() == "MSCONS")
                .expect("accepted SendAntwort (55001) must include MSCONS 13015");
            assert_eq!(
                mscons.payload["pid"].as_u64().unwrap(),
                13015_u64,
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

        render_to_wire_bytes(utilmd, &make_registry(NB_ID, "NB")).expect("NB: render_to_wire_bytes")
    }

    async fn state(&self) -> SupplierChangeState {
        self.process.state().await.unwrap()
    }
}

// ── Mock LFA ERP backend ──────────────────────────────────────────────────────

/// Simulates the **old supplier's (LFA) ERP** receiving LFN's Kündigung
/// Lieferbeginn (PID 55016) and issuing the mandatory Bestätigung Kündigung
/// (PID 55017).
///
/// Per LFW24 (BK6-22-024): a Kündigung Lieferbeginn **must always be accepted**
/// by LFA — there is no rejection path for PID 55016.
struct MockLfa {
    process: Process<GpkeSupplierChangeWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockLfa {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(LFA_ID),
                WorkflowId::new("gpke-supplier-change", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive LFN's UTILMD 55016 wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true`.  Asserts:
    /// - UNH message_ref is non-trivial (derived from LFN's `causation_event_id`).
    /// - Adapter preserved the ref in `ReceiveUtilmd.message_ref`.
    /// - Inbound PID is 55016 (Kündigung Lieferbeginn), not an Anfrage PID.
    async fn receive_kuendigung(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("LFA: parse LFN UTILMD wire");
        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty() && unh_ref != "1",
            "UNH message_ref must be derived from causation_event_id; got: {unh_ref:?}",
        );

        let cmd = gpke_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("LFA: adapt LFN UTILMD to SupplierChangeCommand");
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
                    55016,
                    "LFA must receive PID 55016 (Kündigung Lieferbeginn), got {pid}"
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
                    received_at: time::OffsetDateTime::now_utc(),
                    validation_passed: true,
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected SupplierChangeCommand::ReceiveUtilmd"),
        };
        self.process
            .execute(cmd)
            .await
            .expect("LFA: execute ReceiveUtilmd (55016)");
    }

    /// ERP action: send Bestätigung Kündigung (PID 55017).
    ///
    /// A Kündigung Lieferbeginn (55016) is **always accepted** per LFW24 —
    /// `accepted = true` is not optional here.
    ///
    /// Asserts:
    /// - Exactly one outbox entry (UTILMD 55017 only — no MSCONS for Kündigung,
    ///   only PID 55001 Lieferbeginn triggers GPKE Teil 3 Bewegungsdaten).
    /// - UTILMD PID is 55017 (Bestätigung Kündigung).
    /// - Recipient is the LFN.
    ///
    /// Returns the rendered UTILMD 55017 wire bytes for transport back to LFN.
    async fn send_bestaetigung(&self) -> Vec<u8> {
        let (_, outbox) = self
            .process
            .execute_and_collect(SupplierChangeCommand::SendAntwort {
                accepted: true, // mandatory: LFA cannot reject PID 55016 per LFW24
                reason: None,
                obligations: vec![],
            })
            .await
            .expect("LFA: execute SendAntwort (55016 → 55017)");

        assert_eq!(
            outbox.len(),
            1,
            "LFA outbox must have exactly one entry: UTILMD 55017 \
             (no MSCONS for Kündigung — GPKE Teil 3 Bewegungsdaten is for PID 55001 only)"
        );
        let msg = &outbox[0];
        assert_eq!(msg.message_type.as_ref(), "UTILMD");
        assert_eq!(
            msg.payload["pid"].as_u64().unwrap(),
            55017_u64,
            "LFA must respond with PID 55017 (Bestätigung Kündigung Lieferbeginn)"
        );
        assert_eq!(
            msg.recipient.as_ref(),
            LFN_ID,
            "UTILMD 55017 must be addressed to the LFN"
        );

        render_to_wire_bytes(msg, &make_registry(LFA_ID, "LF"))
            .expect("LFA: render_to_wire_bytes (55017)")
    }

    async fn state(&self) -> SupplierChangeState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Lieferantenwechsel Strom — happy path (55001→55003, 55016→55017).
///
/// Both GPKE legs complete successfully:
/// - NB accepts the Lieferbeginn Anfrage  → UTILMD 55003 + MSCONS 13015
/// - LFA accepts the Kündigung (mandatory) → UTILMD 55017
///
/// Final state:
/// - LFN `nb_leg`  → `Active`           (received 55003 Bestätigung from NB)
/// - LFN `lfa_leg` → `Active`           (received 55017 Bestätigung from LFA)
/// - NB             → `AntwortGesendet`
/// - LFA            → `AntwortGesendet`
#[tokio::test]
async fn e2e_lieferantenwechsel_strom_happy_path() {
    let lfn = MockLfn::new();
    let nb = MockNb::new();
    let lfa = MockLfa::new();

    // ── LFN: submit both legs (parallel in real systems; sequential here) ─────
    let wire_55001 = lfn.submit_lieferbeginn(MALO_ID, "20251001").await;
    let wire_55016 = lfn.submit_kuendigung(MALO_ID, "20251001").await;
    assert!(
        matches!(lfn.nb_leg_state().await, LfAnmeldungState::Pending(_)),
        "nb_leg must be Pending after InitiateAnmeldung (55001)"
    );
    assert!(
        matches!(lfn.lfa_leg_state().await, LfAnmeldungState::Pending(_)),
        "lfa_leg must be Pending after InitiateAnmeldung (55016)"
    );

    // ── NB: receive UTILMD 55001, validate, send 55003 + MSCONS 13015 ─────────
    nb.receive_utilmd(&wire_55001).await;
    assert!(
        matches!(nb.state().await, SupplierChangeState::ValidationPassed(_)),
        "NB must be ValidationPassed after receiving UTILMD 55001"
    );
    let wire_55003 = nb.send_antwort(true, None).await;
    assert!(
        matches!(
            nb.state().await,
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "NB must be AntwortGesendet after sending Bestätigung"
    );

    // ── LFA: receive UTILMD 55016, send 55017 ────────────────────────────────────
    lfa.receive_kuendigung(&wire_55016).await;
    assert!(
        matches!(lfa.state().await, SupplierChangeState::ValidationPassed(_)),
        "LFA must be ValidationPassed after receiving UTILMD 55016"
    );
    let wire_55017 = lfa.send_bestaetigung().await;
    assert!(
        matches!(
            lfa.state().await,
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "LFA must be AntwortGesendet after sending Bestätigung Kündigung"
    );

    // ── LFN: receive both responses ───────────────────────────────────────────
    lfn.receive_antwort_von_nb(&wire_55003).await;
    lfn.receive_antwort_von_lfa(&wire_55017).await;

    // ── Final state — assert all business data fields ─────────────────────────
    let nb_leg_final = lfn.nb_leg_state().await;
    assert!(
        matches!(nb_leg_final, LfAnmeldungState::Active(_)),
        "LFN nb_leg must be Active after receiving 55003 Bestätigung; got: {nb_leg_final:?}"
    );
    if let LfAnmeldungState::Active(data) = nb_leg_final {
        assert_eq!(data.pruefidentifikator.as_u32(), 55001);
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.process_date, "20251001");
        assert_eq!(data.sender.as_str(), LFN_ID);
        assert_eq!(data.receiver.as_str(), NB_ID);
    }

    let lfa_leg_final = lfn.lfa_leg_state().await;
    assert!(
        matches!(lfa_leg_final, LfAnmeldungState::Active(_)),
        "LFN lfa_leg must be Active after receiving 55017 Bestätigung; got: {lfa_leg_final:?}"
    );
    if let LfAnmeldungState::Active(data) = lfa_leg_final {
        assert_eq!(data.pruefidentifikator.as_u32(), 55016);
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.process_date, "20251001");
        assert_eq!(data.sender.as_str(), LFN_ID);
        assert_eq!(data.receiver.as_str(), LFA_ID);
    }
}

/// Lieferantenwechsel Strom — NB rejects the Anfrage (55001 → 55004).
///
/// The NB rejection closes the `nb_leg`, but the `lfa_leg` Kündigung runs
/// independently and completes successfully.  The two GPKE legs are
/// structurally independent event-sourced processes — one leg's terminal state
/// does not affect the other.
///
/// Final state:
/// - LFN `nb_leg`  → `Rejected`         (received 55004 Ablehnung from NB)
/// - LFN `lfa_leg` → `Active`           (received 55017 Bestätigung from LFA)
/// - NB             → `Rejected`
/// - LFA            → `AntwortGesendet`
#[tokio::test]
async fn e2e_lieferantenwechsel_strom_nb_rejects() {
    let lfn = MockLfn::new();
    let nb = MockNb::new();
    let lfa = MockLfa::new();

    // ── LFN: submit both legs ─────────────────────────────────────────────────
    let wire_55001 = lfn.submit_lieferbeginn(MALO_ID, "20251001").await;
    let wire_55016 = lfn.submit_kuendigung(MALO_ID, "20251001").await;

    // ── NB: reject the Anfrage Lieferbeginn ───────────────────────────────────
    nb.receive_utilmd(&wire_55001).await;
    let wire_55004 = nb
        .send_antwort(false, Some("Stammdaten nicht bekannt"))
        .await;
    // NB state: applying AntwortGesendet { accepted: false } transitions to Rejected.
    assert!(
        matches!(nb.state().await, SupplierChangeState::Rejected { .. }),
        "NB must be Rejected after sending Ablehnung 55004"
    );

    // ── LFA: accept Kündigung regardless (mandatory per LFW24) ────────────────
    lfa.receive_kuendigung(&wire_55016).await;
    let wire_55017 = lfa.send_bestaetigung().await;
    assert!(
        matches!(
            lfa.state().await,
            SupplierChangeState::AntwortGesendet { .. }
        ),
        "LFA must be AntwortGesendet — the Kündigung leg is independent of the NB leg"
    );

    // ── LFN: receive both responses ───────────────────────────────────────────
    lfn.receive_antwort_von_nb(&wire_55004).await;
    lfn.receive_antwort_von_lfa(&wire_55017).await;

    // ── Final state assertions ────────────────────────────────────────────────
    let nb_leg_final = lfn.nb_leg_state().await;
    assert!(
        matches!(nb_leg_final, LfAnmeldungState::Rejected { .. }),
        "LFN nb_leg must be Rejected after receiving 55004 Ablehnung; got: {nb_leg_final:?}"
    );

    let lfa_leg_final = lfn.lfa_leg_state().await;
    assert!(
        matches!(lfa_leg_final, LfAnmeldungState::Active(_)),
        "LFN lfa_leg must be Active — the Kündigung leg is independent of the NB decision; \
         got: {lfa_leg_final:?}"
    );
    if let LfAnmeldungState::Active(data) = lfa_leg_final {
        assert_eq!(data.pruefidentifikator.as_u32(), 55016);
        assert_eq!(data.location_id.as_str(), MALO_ID);
        assert_eq!(data.sender.as_str(), LFN_ID);
        assert_eq!(data.receiver.as_str(), LFA_ID);
    }
}
