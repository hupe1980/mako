//! Every APERAK mako sends must pass the APERAK AHB.
//!
//! The APERAK is the one message every inbound EDIFACT stream produces — 28
//! workflow sites enqueue one — and it is the one message no workflow test
//! reads back off the wire. That combination hid three defects at once:
//!
//! - `AperakBuilder` defaulted `BGM` DE 1001 to `"1000"`, the generic
//!   UN/EDIFACT code. The EDI@Energy MIG admits `312` and `313` and nothing
//!   else.
//! - The Anerkennungsmeldung was enqueued under PID **29001**, whose
//!   Prüfschablone admits only `BGM+313` and requires an `SG4`
//!   Fehlerbeschreibung. Its Anwendungsfall is **29002**.
//! - No site carried `orig_message_ref`, so `SG2` — Muss in *both*
//!   Anwendungsfälle — was never emitted.
//!
//! Each of those renders, parses and looks plausible; only the AHB check says
//! otherwise. So this suite renders what the workflows enqueue and runs mako's
//! own validator over the result.

use edi_energy::EdiEnergyMessage as _;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::{OutboxMessage, PendingOutbox};
use makod::config::PartyConfig;
use makod::edifact_renderer::{RenderError, render_to_wire_bytes};
use makod::party_registry::MpIdRegistry;

// Strom MP-IDs: `99…` resolves to the BDEW Codevergabe (`NAD` DE 3055 = 293).
// A `98…` DVGW MP-ID is deliberately not used here — APERAK AHB 1.0 admits
// `332` for the Absender on the Fehlermeldung only, and Gas answers a clean
// message with silence rather than an Anerkennungsmeldung anyway.
const NB: &str = "9900123456789";
const LF: &str = "9900987654321";
const ORIG_REF: &str = "MSG-0001";
const MALO: &str = "51238696012";

fn registry() -> MpIdRegistry {
    MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: NB.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
        },
        PartyConfig {
            mp_id: LF.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: false,
            agency: None,
        },
    ])
    .expect("valid registry")
}

/// Materialise a `PendingOutbox` the way the engine does, so the test drives
/// the exact payload a workflow produces rather than a hand-written copy.
fn materialise(pending: PendingOutbox) -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("process/aperak-test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        pending.message_type,
        pending.recipient,
        pending.payload,
    )
}

/// Render, parse, and run the MIG + AHB check. Returns the wire text.
fn render_and_validate(pending: PendingOutbox) -> String {
    let msg = materialise(pending);
    let rendered = render_to_wire_bytes(&msg, &registry()).expect("APERAK must render");
    let wire = String::from_utf8(rendered.bytes.clone()).expect("utf-8");

    let parsed = edi_energy::parse(&rendered.bytes).expect("rendered APERAK must parse");
    let report = parsed.validate().expect("validation must run");
    assert!(
        report.errors().is_empty(),
        "APERAK failed its own AHB:\n{wire}\n{}",
        report
            .errors()
            .iter()
            .map(|i| format!("  [{}] {}", i.rule_id().unwrap_or("-"), i.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    wire
}

/// PID 29002 `BGM+312`, `SG2` complete, no `SG4`.
#[test]
fn anerkennungsmeldung_passes_the_ahb() {
    let wire = render_and_validate(PendingOutbox::aperak_anerkennung(NB, LF, ORIG_REF));

    assert!(wire.contains("BGM+312+29002'"), "{wire}");
    // SG2 Nr 00004/00005 — which message, and when it was sent.
    assert!(wire.contains(&format!("RFF+ACE:{ORIG_REF}'")), "{wire}");
    assert!(wire.contains("DTM+171:"), "{wire}");
    // SG2 Nr 00006 — the Dokumentennummer of the referenced message. It sits
    // in SG2 here, not in the SG5 that only a Fehlerbeschreibung opens.
    assert!(wire.contains(&format!("RFF+AGO:{ORIG_REF}'")), "{wire}");
    // An Anerkennungsmeldung reports nothing wrong.
    assert!(!wire.contains("ERC+"), "{wire}");
    assert!(!wire.contains("RFF+ACW:"), "{wire}");
}

/// PID 29001 `BGM+313`, `SG4` Fehlerbeschreibung with its `SG5` references.
#[test]
fn fehlermeldung_passes_the_ahb() {
    let wire = render_and_validate(PendingOutbox::aperak_fehler(
        NB,
        LF,
        ORIG_REF,
        mako_engine::erc::codes::Z29,
        "SG8 Produktpaket fehlt",
    ));

    assert!(wire.contains("BGM+313+29001'"), "{wire}");
    assert!(wire.contains("RFF+ACE:"), "{wire}");
    assert!(wire.contains("ERC+Z29'"), "{wire}");
    assert!(wire.contains("FTX+ABO+++SG8 Produktpaket fehlt'"), "{wire}");
    // SG5 Nr 00017 — „Ortsangabe des AHB-Fehlers", Muss for Z29 (AHB
    // Bedingung [5]) and defaulted from the error text.
    assert!(wire.contains("FTX+Z02+++SG8 Produktpaket fehlt'"), "{wire}");
    // SG5 Nr 00014/00015 — inside the Fehlerbeschreibung this time.
    assert!(wire.contains(&format!("RFF+ACW:{ORIG_REF}'")), "{wire}");
    assert!(wire.contains(&format!("RFF+AGO:{ORIG_REF}'")), "{wire}");
}

/// `SG2` is Muss in both Anwendungsfälle and there is no second field the
/// reference could be recovered from, so a payload without it must not reach
/// the wire at all.
#[test]
fn an_aperak_without_the_acknowledged_reference_is_refused() {
    let msg = materialise(PendingOutbox::new(
        "APERAK",
        LF,
        serde_json::json!({ "sender": NB, "receiver": LF, "pid": 29002 }),
    ));

    match render_to_wire_bytes(&msg, &registry()) {
        Err(RenderError::MissingField { field, .. }) => {
            assert_eq!(&*field, "orig_message_ref");
        }
        Err(other) => panic!("expected MissingField, got {other}"),
        Ok(r) => panic!(
            "an APERAK with no SG2 reached the wire: {}",
            String::from_utf8_lossy(&r.bytes)
        ),
    }
}

/// The two Anwendungsfälle differ only in whether an error is reported, so the
/// constructors — not the caller — decide the PID and the `BGM` code. A
/// rejection stamped 29002, or an Anerkennung stamped 29001, is refused by the
/// AHB, and both were shipping before this test existed.
#[test]
fn the_constructors_pair_the_pid_with_the_bgm_code() {
    let ack = PendingOutbox::aperak_anerkennung(NB, LF, ORIG_REF);
    assert_eq!(
        ack.payload["pid"],
        mako_engine::outbox::APERAK_PID_ANERKENNUNG
    );
    assert!(ack.payload.get("error_code").is_none());

    let nak = PendingOutbox::aperak_fehler(NB, LF, ORIG_REF, "Z29", "why");
    assert_eq!(nak.payload["pid"], mako_engine::outbox::APERAK_PID_FEHLER);
    assert_eq!(nak.payload["error_code"], "Z29");

    // Neither states `document_code`: it follows from the Anwendungsfall, and
    // two independent spellings of one fact is how they drifted apart.
    assert!(ack.payload.get("document_code").is_none());
    assert!(nak.payload.get("document_code").is_none());
}

/// The APERAK a real workflow enqueues renders and passes the AHB.
///
/// The tests above drive the constructors directly, which proves the payload
/// shape. This drives `GpkeSupplierChangeWorkflow` — the path an inbound 55001
/// actually takes — and renders what it puts in the outbox, so the *seam*
/// between the workflow's payload keys and the renderer's contract is covered
/// too. That seam is where `orig_message_ref` was missing: every workflow
/// enqueued an APERAK, the renderer required a field none of them sent, and the
/// outbox worker retried it forever without a line in the log.
#[test]
fn the_supplier_change_workflow_enqueues_a_renderable_aperak() {
    use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
    use mako_engine::workflow::Workflow as _;

    let out = mako_gpke::wechselprozesse::GpkeSupplierChangeWorkflow::handle(
        &Default::default(),
        mako_gpke::wechselprozesse::SupplierChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(55001).expect("valid PID"),
            sender: MarktpartnerCode::new(LF),
            receiver: MarktpartnerCode::new(NB),
            location_id: MaLo::new(MALO),
            document_date: "20260701".to_owned(),
            process_date: "20261001".to_owned(),
            bilanzierungsgebiet: None,
            bilanzierungsmethode: None,
            fallgruppe: None,
            transaktionsgrund: Some("E01".to_owned()),
            transaktionsgrund_ergaenzung: Some("ZW4".to_owned()),
            veraeusserungsform: None,
            tranchengroesse_prozent: None,
            vorgangsnummer: Some("VORGANG0001".to_owned()),
            produktpaket_id: Some("1".to_owned()),
            kunde_name: Some("Mustermann".to_owned()),
            kunde_namensformat: Some("Z01".to_owned()),
            message_ref: MessageRef::new("MSG-001"),
            received_at: time::OffsetDateTime::now_utc(),
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .expect("a clean 55001 is accepted");

    let aperak = out
        .outbox
        .iter()
        .find(|p| p.message_type.as_ref() == "APERAK")
        .expect("a clean Strom UTILMD is always acknowledged (APERAK AHB 1.0 §2.4)");

    // It answers the message it received, and it is the Anerkennungsmeldung.
    assert_eq!(aperak.payload["orig_message_ref"], "MSG-001");
    assert_eq!(aperak.payload["pid"], 29002);

    let msg = materialise(aperak.clone());
    let rendered = render_to_wire_bytes(&msg, &registry())
        .expect("the APERAK the workflow enqueued must render");
    let parsed = edi_energy::parse(&rendered.bytes).expect("parses");
    let report = parsed.validate().expect("validation runs");
    assert!(
        report.errors().is_empty(),
        "the workflow's own APERAK fails the AHB:\n{}\n{}",
        String::from_utf8_lossy(&rendered.bytes).replace('\'', "'\n"),
        report
            .errors()
            .iter()
            .map(|i| format!("  [{}] {}", i.rule_id.as_deref().unwrap_or("-"), i.message))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
