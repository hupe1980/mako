//! Every LF answer must produce an outbound UTILMD carrying its Antwortcode.
//!
//! An `AntwortGesendet` event without an outbox entry records the process as
//! answered while the counterparty watches its Frist expire; an answer without
//! `SG4 STS+E01` is not one the AHB accepts. These tests pin both.

use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
use mako_engine::workflow::Workflow;

use mako_gpke::{
    AnkuendigungZuordnungLfCommand, AnkuendigungZuordnungLfState, BeendigungZuordnungCommand,
    BeendigungZuordnungState, GpkeAnkuendigungZuordnungLfWorkflow, GpkeBeendigungZuordnungWorkflow,
    GpkeLfAbmeldungWorkflow, LfAbmeldungCommand, LfAbmeldungState, LfAntwort,
};

const NB: &str = "9900357000004";
const LF: &str = "4012345000023";
const MALO: &str = "51238696012";

fn pid(code: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(code).expect("valid PID")
}

/// Drive a workflow to `ValidationPassed`, then answer, and return the outbox.
macro_rules! answer_and_collect {
    ($wf:ty, $state:ty, $receive:expr, $answer:expr) => {{
        let out = <$wf>::handle(&<$state>::default(), $receive).expect("Ankündigung accepted");
        let state = out.events.iter().fold(<$state>::default(), <$wf>::apply);
        <$wf>::handle(&state, $answer).expect("answer accepted")
    }};
}

/// **55007 → 55009.** An Ablehnung must be transmitted, with its `E_0609` code.
#[test]
fn the_lf_abmeldung_ablehnung_is_dispatched_with_its_antwortcode() {
    let out = answer_and_collect!(
        GpkeLfAbmeldungWorkflow,
        LfAbmeldungState,
        LfAbmeldungCommand::ReceiveAnkuendigung {
            pid: pid(55_007),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("ABMELD-001"),
            vorgang: mako_gpke::LfVorgangsdaten::default(),
            validation_passed: true,
            validation_errors: vec![],
        },
        LfAbmeldungCommand::SendAntwort {
            antwort: LfAntwort::ablehnung("A03", "E_0609"),
        }
    );

    let utilmd = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "UTILMD")
        .expect("the answer must produce an outbound UTILMD, not just an event");

    assert_eq!(utilmd.payload["pid"], 55_009, "A03 is an Ablehnungscode");
    assert_eq!(utilmd.payload["antwort_code"], "A03");
    assert_eq!(utilmd.payload["antwort_ebd"], "E_0609");
    assert_eq!(utilmd.payload["malo"], MALO);
    // The answer goes back to the NB that asked.
    assert_eq!(utilmd.payload["sender"], LF);
    assert_eq!(utilmd.payload["receiver"], NB);
}

/// **55007 → 55008.** The Zustimmungscode selects the Bestätigungs-PID.
#[test]
fn the_lf_abmeldung_zustimmung_rides_the_bestaetigungs_pid() {
    let out = answer_and_collect!(
        GpkeLfAbmeldungWorkflow,
        LfAbmeldungState,
        LfAbmeldungCommand::ReceiveAnkuendigung {
            pid: pid(55_007),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("ABMELD-002"),
            vorgang: mako_gpke::LfVorgangsdaten::default(),
            validation_passed: true,
            validation_errors: vec![],
        },
        LfAbmeldungCommand::SendAntwort {
            antwort: LfAntwort::zustimmung("A10", "E_0609"),
        }
    );

    let utilmd = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "UTILMD")
        .expect("outbound UTILMD");
    assert_eq!(utilmd.payload["pid"], 55_008);
    assert_eq!(utilmd.payload["antwort_code"], "A10");
}

/// **55010 → 55012.** `A35` „Es besteht eine Vertragsbindung" is an Ablehnung,
/// and the PID follows from the code rather than from a separate flag.
#[test]
fn the_beendigung_zuordnung_answer_is_dispatched() {
    let out = answer_and_collect!(
        GpkeBeendigungZuordnungWorkflow,
        BeendigungZuordnungState,
        BeendigungZuordnungCommand::ReceiveAnfrage {
            pid: pid(55_010),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("BEEND-001"),
            vorgang: mako_gpke::LfVorgangsdaten::default(),
            validation_passed: true,
            validation_errors: vec![],
        },
        BeendigungZuordnungCommand::SendAntwort {
            antwort: LfAntwort::ablehnung("A35", "E_0624"),
        }
    );

    let utilmd = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "UTILMD")
        .expect("the LFA answer must be transmitted");
    assert_eq!(utilmd.payload["pid"], 55_012);
    assert_eq!(utilmd.payload["antwort_code"], "A35");
    assert_eq!(utilmd.payload["antwort_ebd"], "E_0624");
}

/// **55010 → 55011.** `A34` states the LFA's *own* Lieferendedatum, which must
/// replace the requested one on the wire.
#[test]
fn a_stated_lieferendedatum_replaces_the_requested_one() {
    let out = answer_and_collect!(
        GpkeBeendigungZuordnungWorkflow,
        BeendigungZuordnungState,
        BeendigungZuordnungCommand::ReceiveAnfrage {
            pid: pid(55_010),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("BEEND-002"),
            vorgang: mako_gpke::LfVorgangsdaten::default(),
            validation_passed: true,
            validation_errors: vec![],
        },
        BeendigungZuordnungCommand::SendAntwort {
            antwort: LfAntwort::zustimmung("A34", "E_0624").with_termin("20260831"),
        }
    );

    let utilmd = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "UTILMD")
        .expect("outbound UTILMD");
    assert_eq!(utilmd.payload["pid"], 55_011);
    assert_eq!(
        utilmd.payload["process_date"], "20260831",
        "A34 carries the LFA's own Lieferendedatum"
    );
}

/// **55607 → 55609.** The LFN's answer to the Ankündigung Zuordnung LF is
/// transmitted too, with its Erläuterung in `FTX+ACB` — which `A99` requires.
#[test]
fn the_zuordnung_lf_answer_is_dispatched_with_its_bemerkung() {
    let out = answer_and_collect!(
        GpkeAnkuendigungZuordnungLfWorkflow,
        AnkuendigungZuordnungLfState,
        AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
            pid: pid(55_607),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("ZUORD-001"),
            vorgang: mako_gpke::LfVorgangsdaten::default(),
            validation_passed: true,
            validation_errors: vec![],
        },
        AnkuendigungZuordnungLfCommand::SendAntwort {
            antwort: LfAntwort::ablehnung("A99", "E_0603")
                .with_bemerkung("Marktlokation unbekannt"),
        }
    );

    let utilmd = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "UTILMD")
        .expect("outbound UTILMD");
    assert_eq!(utilmd.payload["pid"], 55_609);
    assert_eq!(utilmd.payload["bemerkung"], "Marktlokation unbekannt");
    assert_eq!(
        utilmd.payload["antwort_ebd"], "E_0603",
        "55607–55609 is governed by E_0603…E_0606, one per Anwendungsfall"
    );
}

/// **55016 → 55018.** The Kündigung answer is transmitted with its `E_0614` code.
///
/// It runs on its own `gpke-kuendigung` workflow: `gpke-supplier-change` is
/// keyed by Marktlokation and hosts the NB's Anmeldung, so a shared workflow
/// would let an LFA answer resume the grid operator's Vorgang.
#[test]
fn the_kuendigung_answer_is_dispatched() {
    use mako_gpke::{GpkeKuendigungWorkflow, KuendigungCommand, KuendigungState};

    let out = answer_and_collect!(
        GpkeKuendigungWorkflow,
        KuendigungState,
        KuendigungCommand::ReceiveKuendigung {
            pid: pid(55_016),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("KUEND-001"),
            vorgang: mako_gpke::LfVorgangsdaten::default(),
            validation_passed: true,
            validation_errors: vec![],
        },
        KuendigungCommand::SendAntwort {
            antwort: LfAntwort::ablehnung("A06", "E_0614"),
        }
    );

    let utilmd = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "UTILMD")
        .expect("the LFA answer must be transmitted");
    assert_eq!(utilmd.payload["pid"], 55_018, "A06 is an Ablehnungscode");
    assert_eq!(utilmd.payload["antwort_code"], "A06");
    assert_eq!(utilmd.payload["antwort_ebd"], "E_0614");
}

/// The Kündigung no longer shares the supplier change's PID table, so an
/// Anmeldung can never be answered as one.
#[test]
fn the_kuendigung_left_the_supplier_change_pid_table() {
    assert!(
        !mako_gpke::UTILMD_ANFRAGE_PIDS.contains(&55_016),
        "55016 has its own workflow and must not spawn gpke-supplier-change"
    );
    assert_eq!(mako_gpke::kuendigung::KUENDIGUNG_PIDS, &[55_016]);
}

// ── The other half: the request has to reach a decider ────────────────────────

/// An LF-answered Vorgang must emit `de.mako.process.initiated`.
///
/// `makod` delivers a CloudEvent only for an outbox entry, and the APERAK these
/// workflows emit is a technical acknowledgement — `processd`'s LF module
/// subscribes to `de.mako.process.initiated` and to nothing else. Without this
/// entry the answer automation is unreachable and every Frist expires
/// unanswered, which is exactly what happened before these four workflows
/// emitted one.
#[test]
fn an_inbound_vorgang_notifies_the_decider() {
    let vorgang = mako_gpke::LfVorgangsdaten {
        transaktionsgrund: Some("Z33".to_owned()),
        transaktionsgrund_ergaenzung: Some("ZW4".to_owned()),
        vorgangsnummer: Some("VORGANG-0001".to_owned()),
        ..Default::default()
    };
    let out = GpkeLfAbmeldungWorkflow::handle(
        &LfAbmeldungState::default(),
        LfAbmeldungCommand::ReceiveAnkuendigung {
            pid: pid(55_007),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("ABMELD-002"),
            vorgang,
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .expect("Ankündigung accepted");

    let notice = out
        .outbox
        .iter()
        .find(|o| &*o.message_type == "ProcessInitiated")
        .expect("an inbound Vorgang must reach the ERP fan-out");

    assert_eq!(notice.payload["pid"], 55_007);
    assert_eq!(notice.payload["malo_id"], MALO);
    assert_eq!(notice.payload["termin"], "20260901");
    // The three facts `E_0609` branches on. Dropping any of them turns every
    // decision into an operator escalation.
    assert_eq!(notice.payload["transaktionsgrund"], "Z33");
    assert_eq!(notice.payload["transaktionsgrund_ergaenzung"], "ZW4");
    assert_eq!(notice.payload["vorgangsnummer"], "VORGANG-0001");
}

/// A message that failed AHB validation is refused, not decided: the APERAK
/// 313 goes out and no decider is asked to answer a message the syntax layer
/// already rejected.
#[test]
fn a_rejected_vorgang_notifies_nobody() {
    let out = GpkeLfAbmeldungWorkflow::handle(
        &LfAbmeldungState::default(),
        LfAbmeldungCommand::ReceiveAnkuendigung {
            pid: pid(55_007),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20260820".to_owned(),
            process_date: "20260901".to_owned(),
            message_ref: MessageRef::new("ABMELD-003"),
            vorgang: mako_gpke::LfVorgangsdaten::default(),
            validation_passed: false,
            validation_errors: vec!["SG4 STS+7: missing".to_owned()],
        },
    )
    .expect("handled");

    assert!(
        !out.outbox
            .iter()
            .any(|o| &*o.message_type == "ProcessInitiated")
    );
    assert!(out.outbox.iter().any(|o| &*o.message_type == "APERAK"));
}
