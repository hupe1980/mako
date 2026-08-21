//! Every **NB** answer must produce an outbound UTILMD carrying its Antwortcode.
//!
//! The AHB marks `SG4 STS+E01` Muss on every Antwortnachricht, and the renderer
//! only emits the segment when `antwort_code` is in the outbox payload. An
//! answer dispatched as a bare `accepted: bool` therefore rendered a well-formed
//! UTILMD that stated no Grund at all — valid EDIFACT the counterparty could not
//! act on. These tests pin the code onto the wire for the NB's own answers, the
//! way `lf_antwort_reaches_the_wire` does for the supplier's.

use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
use mako_engine::workflow::Workflow;

use mako_gpke::{
    GpkeSupplierChangeWorkflow, LfAntwort, SupplierChangeCommand, SupplierChangeState,
};

const NB: &str = "9900357000004";
const LF: &str = "4012345000023";
const MALO: &str = "51238696012";

fn pid(code: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(code).expect("valid PID")
}

/// Receive an Anmeldung/Abmeldung, then answer it, and return the outbox.
fn answer(
    anfrage_pid: u32,
    antwort: LfAntwort,
) -> mako_engine::workflow::WorkflowOutput<mako_gpke::SupplierChangeEvent> {
    let receive = SupplierChangeCommand::ReceiveUtilmd {
        pid: pid(anfrage_pid),
        sender: MarktpartnerCode::new(LF),
        receiver: MarktpartnerCode::new(NB),
        location_id: MaLo::new(MALO),
        document_date: "20260820".to_owned(),
        process_date: "20260901".to_owned(),
        bilanzierungsgebiet: None,
        bilanzierungsmethode: None,
        fallgruppe: None,
        transaktionsgrund: Some("E03".to_owned()),
        transaktionsgrund_ergaenzung: Some("ZW4".to_owned()),
        veraeusserungsform: None,
        message_ref: MessageRef::new("ANM-001"),
        received_at: time::OffsetDateTime::now_utc(),
        validation_passed: true,
        validation_errors: vec![],
    };
    let out = GpkeSupplierChangeWorkflow::handle(&SupplierChangeState::default(), receive)
        .expect("Anfrage accepted");
    let state = out.events.iter().fold(
        SupplierChangeState::default(),
        GpkeSupplierChangeWorkflow::apply,
    );
    GpkeSupplierChangeWorkflow::handle(
        &state,
        SupplierChangeCommand::SendAntwort {
            antwort,
            obligations: vec![],
        },
    )
    .expect("answer accepted")
}

fn utilmd(
    out: &mako_engine::workflow::WorkflowOutput<mako_gpke::SupplierChangeEvent>,
) -> &serde_json::Value {
    &out.outbox
        .iter()
        .find(|o| &*o.message_type == "UTILMD")
        .expect("the answer must produce an outbound UTILMD, not just an event")
        .payload
}

/// **55001 → 55002.** A Bestätigung is not the absence of a code: it states
/// `A51`, which `E_0623` publishes as the Zustimmung of a Lieferbeginn.
#[test]
fn the_anmeldung_bestaetigung_carries_a51_from_e0623() {
    let out = answer(55_001, LfAntwort::zustimmung("A51", "E_0623"));
    let p = utilmd(&out);
    assert_eq!(p["pid"], 55_002);
    assert_eq!(
        p["antwort_code"], "A51",
        "SG4 STS+E01 is Muss on every Antwortnachricht"
    );
    assert_eq!(p["antwort_ebd"], "E_0623");
    assert_eq!(p["sender"], NB, "the NB answers the LF that asked");
    assert_eq!(p["receiver"], LF);
}

/// **55001 → 55003.** The Ablehnung carries the `E_0622` code that produced it,
/// and the Bemerkung the operator can read.
#[test]
fn the_anmeldung_ablehnung_carries_its_e0622_code_and_bemerkung() {
    let mut antwort = LfAntwort::ablehnung("A07", "E_0622");
    antwort.bemerkung = Some("Vorlauffrist nicht eingehalten".to_owned());
    let out = answer(55_001, antwort);
    let p = utilmd(&out);
    assert_eq!(p["pid"], 55_003);
    assert_eq!(p["antwort_code"], "A07");
    assert_eq!(p["antwort_ebd"], "E_0622");
    assert_eq!(p["bemerkung"], "Vorlauffrist nicht eingehalten");
}

/// **55004 → 55005.** The Abmeldung answers out of `E_0607`, whose Zustimmung
/// is `A11` — a different tree and a different code from the Anmeldung's.
#[test]
fn the_abmeldung_bestaetigung_carries_a11_from_e0607() {
    let out = answer(55_004, LfAntwort::zustimmung("A11", "E_0607"));
    let p = utilmd(&out);
    assert_eq!(p["pid"], 55_005);
    assert_eq!(p["antwort_code"], "A11");
    assert_eq!(p["antwort_ebd"], "E_0607");
}

/// **55077 → 55078.** The erzeugende Marktlokation answers out of the same
/// `E_0623` but with `A58`, and the PID follows the inbound Anwendungsfall.
#[test]
fn the_erzeugende_anmeldung_bestaetigung_carries_a58() {
    let out = answer(55_077, LfAntwort::zustimmung("A58", "E_0623"));
    let p = utilmd(&out);
    assert_eq!(p["pid"], 55_078);
    assert_eq!(p["antwort_code"], "A58");
}

/// Every code these tests put on the wire is one its named tree actually
/// publishes — the same check `makod` runs before dispatching.
#[test]
fn every_code_used_here_is_published_by_its_tree() {
    for (ebd, code) in [
        ("E_0623", "A51"),
        ("E_0623", "A58"),
        ("E_0622", "A07"),
        ("E_0607", "A11"),
    ] {
        let entry = mako_pruefung::codes::lookup(ebd, code)
            .unwrap_or_else(|| panic!("{code} must be published by {ebd}"));
        assert_eq!(entry.code, code);
    }
    // …and the Strom codes are not Gas codes.
    assert!(mako_pruefung::codes::lookup("E_3005", "A07").is_none());
    assert!(mako_pruefung::codes::lookup("E_3019", "A11").is_none());
}
