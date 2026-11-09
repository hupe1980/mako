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
    answer_with(anfrage_pid, antwort, None)
}

/// The same, with the Fall-b Zuordnungsende the Altlieferant answered with.
fn answer_with(
    anfrage_pid: u32,
    antwort: LfAntwort,
    lfa_lieferende: Option<&str>,
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
        tranchengroesse_prozent: None,
        vorgangsnummer: None,
        produktpaket_id: Some("1".to_owned()),
        kunde_name: None,
        kunde_namensformat: None,
        message_ref: MessageRef::new("ANM-001"),
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
            lfa_lieferende: lfa_lieferende.map(ToOwned::to_owned),
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
    assert_eq!(p["antwort_codeliste"], "E_0623");
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
    assert_eq!(p["antwort_codeliste"], "E_0622");
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
    assert_eq!(p["antwort_codeliste"], "E_0607");
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

// ── Fall b: the interval the Bestätigung leaves behind ────────────────────────

/// **Fall b is not a segment of the 55002.** `E_0624` `A34` lets the LFA
/// release the Marktlokation *before* the Zuordnungsbeginn the NB then
/// confirms; the Bestätigung still names the Zuordnungsbeginn the LFN asked
/// for, and the LFA's date has no place in the UTILMD — a key there is one no
/// renderer reads.
#[test]
fn a_bestaetigung_keeps_the_zuordnungsbeginn_beside_an_earlier_lieferende() {
    let out = answer_with(
        55001,
        LfAntwort::zustimmung("A51", "E_0623"),
        Some("20260815"),
    );
    let payload = utilmd(&out);
    assert_eq!(payload["pid"], 55002);
    assert_eq!(payload["process_date"], "20260901");
    assert!(payload.get("lfa_lieferende").is_none());

    // The date rides the completion, beside the Zuordnungsbeginn it precedes:
    // the two ends of the interval marktd announces as the § 38 gap.
    let completed = completed(&out).expect("the 55002 queues its completion");
    assert_eq!(completed["pid"], 55002);
    assert_eq!(completed["malo_id"], MALO);
    assert_eq!(completed["accepted"], true);
    assert_eq!(completed["process_date"], "20260901");
    assert_eq!(completed["lfa_lieferende"], "20260815");
}

/// The `ProcessCompleted` notification the answer queued, if any.
fn completed(
    out: &mako_engine::workflow::WorkflowOutput<mako_gpke::SupplierChangeEvent>,
) -> Option<&serde_json::Value> {
    out.outbox
        .iter()
        .find(|o| &*o.message_type == "ProcessCompleted")
        .map(|o| &o.payload)
}

/// The ordinary answer states none: the LFA releases exactly at the
/// Zuordnungsbeginn and there is no interval to report.
#[test]
fn an_ordinary_bestaetigung_states_no_lieferende() {
    let out = answer(55001, LfAntwort::zustimmung("A51", "E_0623"));
    assert!(utilmd(&out).get("lfa_lieferende").is_none());
    let completed = completed(&out).expect("the 55002 queues its completion");
    assert!(completed.get("lfa_lieferende").is_none());
}

/// An **Ablehnung** leaves the Altlieferant exactly where it was, so no interval
/// opens — carrying the date there would open a § 38 case for days that are
/// still supplied.
#[test]
fn an_ablehnung_never_reports_a_gap() {
    let out = answer_with(
        55001,
        LfAntwort::ablehnung("A07", "E_0622"),
        Some("20260815"),
    );
    let payload = utilmd(&out);
    assert_eq!(payload["pid"], 55003);
    assert!(payload.get("lfa_lieferende").is_none());
    let completed = completed(&out).expect("the 55003 queues its completion");
    assert_eq!(completed["accepted"], false);
    assert!(completed.get("lfa_lieferende").is_none());
}
