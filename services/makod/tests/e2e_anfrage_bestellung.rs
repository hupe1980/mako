//! End-to-end test: LFN → NB GPKE Anfrage Daten der individuellen Bestellung
//! (UTILMD PID 55555, GPKE Teil 4).
//!
//! The Lieferant (LFN) queries the Netzbetreiber (NB) for data associated
//! with a specific Vorgang (individual order).  The NB must respond within
//! **24 wall-clock hours** (BNetzA BK6-22-024 §5).
//!
//! # Protocol trace
//!
//! ```text
//!   LFN ERP (wire fixture)                  NB ERP (MockNb)
//!   ──────────────────────────────────────────────────────────
//!                    ──── UTILMD 55555 ───►
//!                                           receive_anfrage(wire)
//!                                             → adapter: ReceiveAnfrage
//!                                             → state: ValidationPassed
//!                                           dispatch_response(data_provided)
//!                                             → DispatchResponse
//!                                             → state: ResponseDispatched
//!   ──────────────────────────────────────────────────────────
//! ```
//!
//! # Regulatory context
//!
//! - **PID 55555**: Anfrage Daten der individuellen Bestellung (LFN → NB, GPKE Teil 4)
//! - **BK6-24-174** — GPKE Teil 4 (eff. 2025-06-06)
//! - **APERAK Frist**: 24 wall-clock hours (BK6-22-024 §5)
//! - STS DE 9015 `E07` = Anfrage für aktiven/bestätigten Vorgang
//! - STS DE 9015 `E08` = Anfrage für noch nicht bestätigten Vorgang
//!
//! AHB validation is bypassed — the minimal fixture does not satisfy all
//! UTILMD Strom S2.1 profile rules; AHB conformance is tested separately.

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
    AnfrageBestellungCommand, AnfrageBestellungState, GpkeAnfrageBestellungWorkflow,
    anfrage_bestellung::ANFRAGE_WINDOW_LABEL,
};
use makod::adapters::gpke_anfrage_bestellung_registry;

// ── Constants ─────────────────────────────────────────────────────────────────

const LFN_ID: &str = "4012345000023"; // Lieferant (sender of UTILMD 55555)
const NB_ID: &str = "9900357000004"; // Netzbetreiber (receiver / process owner)
const VORGANG_ID: &str = "DE0000000000000000000000000012345"; // Vorgangsnummer (IDE+Z19)
const FV: &str = "FV2025-10-01";

// ── UTILMD 55555 wire fixture ─────────────────────────────────────────────────
//
// Minimal EDIFACT UTILMD — Anfrage Daten der individuellen Bestellung (PID 55555).
// Direction: LFN (sender NAD+MS) → NB (receiver NAD+MR).
//
// BGM qualifier E03 (Änderungsmeldung) + PID 55555 in element 2 per BDEW GPKE AHB.
// STS+E07 = Anfrage für aktiven/bestätigten Vorgang.
// IDE+Z19 carries the Vorgangsnummer.
// RFF+Z13 provides the correlating reference (UNH 0062 of the original UTILMD).
const UTILMD_55555_BYTES: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+ANFRAGE-2025-001'\
UNH+MSG-ANF-001+UTILMD:D:11A:UN:S2.1'\
BGM+E03+00055555'\
DTM+137:20250115:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+DE0000000000000000000000000012345::'\
STS+E07'\
RFF+Z13:ANFRAGE-REF-001'\
UNT+9+MSG-ANF-001'\
UNZ+1+ANFRAGE-2025-001'";

// ── Mock NB ERP backend ───────────────────────────────────────────────────────

/// Simulates the **NB ERP** receiving and processing a GPKE Anfrage 55555.
struct MockNb {
    process: Process<GpkeAnfrageBestellungWorkflow, InMemoryEventStore>,
    platform: Platform,
    fv: FormatVersion,
}

impl MockNb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(NB_ID),
                WorkflowId::new("gpke-anfrage-bestellung", FV),
            ),
            platform: Platform::with_all_profiles(),
            fv: FormatVersion::new(FV),
        }
    }

    /// ERP notification: receive UTILMD 55555 wire bytes, adapt, and execute.
    ///
    /// AHB validation is forced to `true` — the minimal fixture does not
    /// satisfy all UTILMD Strom S2.1 profile rules; AHB conformance is tested
    /// separately in `edi-energy` tests.
    ///
    /// Asserts that the adapter correctly extracts: PID 55555, sender GLN (LFN),
    /// receiver GLN (NB), Vorgangsnummer, Bearbeitungsstatus, and message_ref.
    async fn receive_anfrage(&self, wire: &[u8]) {
        let raw = self
            .platform
            .parse(wire)
            .expect("NB: parse LFN UTILMD 55555 wire");

        let unh_ref = raw.message_ref().to_owned();
        assert!(
            !unh_ref.is_empty(),
            "UNH message_ref must be non-empty; got: {unh_ref:?}",
        );

        let cmd = gpke_anfrage_bestellung_registry()
            .dispatch(&raw as &dyn Any, &self.fv)
            .expect("NB: adapt UTILMD 55555 to AnfrageBestellungCommand");

        let cmd = match cmd {
            AnfrageBestellungCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                vorgang_id,
                bearbeitungsstatus,
                document_date,
                message_ref,
                ..
            } => {
                assert_eq!(pid.as_u32(), 55555, "adapter must extract PID 55555");
                assert_eq!(
                    sender.as_str(),
                    LFN_ID,
                    "adapter must extract sender GLN (LFN) from NAD+MS"
                );
                assert_eq!(
                    receiver.as_str(),
                    NB_ID,
                    "adapter must extract receiver GLN (NB) from NAD+MR"
                );
                assert_eq!(
                    vorgang_id.as_str(),
                    VORGANG_ID,
                    "adapter must extract Vorgangsnummer from IDE+Z19"
                );
                assert_eq!(
                    bearbeitungsstatus, "E07",
                    "adapter must extract STS E07 from STS segment"
                );
                assert_eq!(
                    message_ref.as_str(),
                    unh_ref.as_str(),
                    "adapter must preserve UNH message_ref"
                );
                assert!(!document_date.is_empty(), "document_date must be non-empty");
                AnfrageBestellungCommand::ReceiveAnfrage {
                    pid,
                    sender,
                    receiver,
                    vorgang_id,
                    bearbeitungsstatus,
                    document_date,
                    message_ref,
                    validation_passed: true, // bypass AHB profile check
                    validation_errors: vec![],
                }
            }
            _ => panic!("expected AnfrageBestellungCommand::ReceiveAnfrage"),
        };

        self.process
            .execute(cmd)
            .await
            .expect("NB: execute ReceiveAnfrage");
    }

    /// ERP action: NB dispatches a response — provides data or rejects.
    async fn dispatch_response(&self, data_provided: bool, reason: Option<&str>) {
        self.process
            .execute(AnfrageBestellungCommand::DispatchResponse {
                data_provided,
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("NB: execute DispatchResponse");
    }

    async fn state(&self) -> AnfrageBestellungState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Anfrage Bestellung — data provided path (UTILMD PID 55555 → ResponseDispatched).
///
/// LFN sends UTILMD 55555 querying Vorgang data; NB receives the Anfrage,
/// validates, and provides the requested data.
///
/// Per BNetzA BK6-22-024 §5 the NB must respond within 24 wall-clock hours.
#[tokio::test]
async fn e2e_anfrage_bestellung_data_provided() {
    let nb = MockNb::new();

    // ── NB ERP: receive LFN UTILMD 55555 ─────────────────────────────────────
    nb.receive_anfrage(UTILMD_55555_BYTES).await;
    assert!(
        matches!(
            nb.state().await,
            AnfrageBestellungState::ValidationPassed(_)
        ),
        "NB must be ValidationPassed after ReceiveAnfrage"
    );

    // ── NB ERP: provide the requested data ───────────────────────────────────
    nb.dispatch_response(true, None).await;

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, AnfrageBestellungState::ResponseDispatched(_)),
        "NB must be ResponseDispatched after successful DispatchResponse; got: {final_state:?}"
    );
    if let AnfrageBestellungState::ResponseDispatched(data) = final_state {
        assert_eq!(data.pruefidentifikator.as_u32(), 55555);
        assert_eq!(data.sender.as_str(), LFN_ID);
        assert_eq!(data.receiver.as_str(), NB_ID);
        assert_eq!(data.vorgang_id.as_str(), VORGANG_ID);
        assert_eq!(data.bearbeitungsstatus, "E07");
    }
}

/// Anfrage Bestellung — rejection path (UTILMD PID 55555 → Rejected).
///
/// LFN sends UTILMD 55555; NB receives the Anfrage but cannot provide the
/// requested data (e.g. Vorgang not found or access denied).
#[tokio::test]
async fn e2e_anfrage_bestellung_rejected_by_nb() {
    let nb = MockNb::new();

    nb.receive_anfrage(UTILMD_55555_BYTES).await;
    assert!(
        matches!(
            nb.state().await,
            AnfrageBestellungState::ValidationPassed(_)
        ),
        "NB must be ValidationPassed after ReceiveAnfrage"
    );

    nb.dispatch_response(false, Some("Vorgang nicht gefunden"))
        .await;

    let final_state = nb.state().await;
    match &final_state {
        AnfrageBestellungState::Rejected { reason } => {
            assert!(
                reason.contains("Vorgang nicht gefunden"),
                "rejection reason must carry the NB message; got: {reason:?}"
            );
        }
        _ => panic!("NB must be Rejected after rejection DispatchResponse; got: {final_state:?}"),
    }
}

/// Anfrage Bestellung — validation failure path (UTILMD PID 55555, malformed).
///
/// If the received UTILMD 55555 fails AHB validation, the workflow must
/// immediately transition to `Rejected` without requiring a `DispatchResponse`.
#[tokio::test]
async fn e2e_anfrage_bestellung_validation_failure() {
    let nb = MockNb::new();

    nb.process
        .execute(AnfrageBestellungCommand::ReceiveAnfrage {
            pid: mako_engine::types::Pruefidentifikator::new(55555).unwrap(),
            sender: mako_engine::types::MarktpartnerCode::new(LFN_ID),
            receiver: mako_engine::types::MarktpartnerCode::new(NB_ID),
            vorgang_id: mako_engine::types::MaLo::new(VORGANG_ID),
            bearbeitungsstatus: "E07".to_owned(),
            document_date: "20250115".to_owned(),
            message_ref: MessageRef::new("MSG-ANF-002"),
            validation_passed: false,
            validation_errors: vec!["IDE segment missing mandatory Vorgangsnummer".to_owned()],
        })
        .await
        .expect("ReceiveAnfrage with invalid message must not panic");

    let final_state = nb.state().await;
    assert!(
        matches!(final_state, AnfrageBestellungState::Rejected { .. }),
        "invalid Anfrage must reach Rejected immediately; got: {final_state:?}"
    );
}

/// Anfrage Bestellung — deadline label is canonical.
///
/// Regression guard: changing `ANFRAGE_WINDOW_LABEL` would silently break
/// the deadline scheduler's timeout dispatch.  Assert the constant is stable.
#[test]
fn anfrage_bestellung_deadline_label_is_canonical() {
    assert_eq!(
        ANFRAGE_WINDOW_LABEL, "gpke-anfrage-bestellung-24h",
        "deadline label must match the scheduler's DISPATCH_TABLE entry"
    );
}
