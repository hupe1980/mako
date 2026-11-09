//! Every outbound UTILMD answer listed here renders to a message its own
//! Prüfschablone accepts — the check the pre-send gate
//! (`transport::as4_sender`) runs before any delivery.
//!
//! Each case drives its workflow as production does — the inbound Anfrage, then
//! the answer command — and hands the outbox entry the answer writes to the
//! renderer, the parser and the AHB validation. A PID enters [`SWEEP`] once it
//! passes, so a renderer or workflow change that drops a Muss place fails here
//! rather than dead-lettering the message in production.
//!
//! The Gas Bestätigung EoG 44014 carries the Marktlokation's record from
//! marktd; its case runs the combined GNB+LF deployment of
//! `e2e_gnb_lf_combined_gas`, whose loopback gates every UTILMD.

use mako_engine::outbox::PendingOutbox;
use mako_engine::types::{MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator};
use mako_engine::workflow::Workflow;
use mako_gpke::{
    AnkuendigungZuordnungLfCommand, GpkeAnkuendigungZuordnungLfWorkflow, GpkeNeuanlageWorkflow,
    LfAntwort, LfVorgangsdaten, NeuanlageCommand,
};

const NB: &str = "9900357000004";
const LF: &str = "9900000000001";
const MALO: &str = "51238696012";
const MELO: &str = "DE0000000000000000000000000000042";

/// `11XBK-STD------Y`, the Strom Bilanzkreis EIC the profile skeleton samples.
const BILANZKREIS: &str = "11XBK-STD------Y";

/// Builds the outbox entry one answer writes.
type Antwort = fn() -> PendingOutbox;

/// The answers that pass their profile, by Prüfidentifikator.
const SWEEP: &[(u32, Antwort)] = &[
    (55052, bestaetigung_ende_msb),
    (55604, || neuanlage(55600, ablehnung("E_0608"))),
    (55605, || neuanlage(55601, ablehnung("E_0608"))),
    (55608, || {
        zuordnung_lf(LfAntwort::zustimmung("A01", "E_0603").with_bilanzkreis(BILANZKREIS))
    }),
    (55609, || zuordnung_lf(ablehnung("E_0603"))),
];

#[test]
fn every_swept_answer_passes_its_profile() {
    let mut failures = Vec::new();
    for (expected, build) in SWEEP {
        let pending = build();
        assert_eq!(pending.payload["pid"], *expected, "{:#}", pending.payload);
        let found = findings(&pending);
        if !found.is_empty() {
            failures.push(format!("{expected}:\n  {}", found.join("\n  ")));
        }
    }
    assert!(
        failures.is_empty(),
        "answers short of their profile:\n{}",
        failures.join("\n")
    );
}

/// The Gas Bestätigung EoG, rendered from marktd's record and the LF's own
/// facts, passes its profile.
#[tokio::test]
async fn the_gas_bestaetigung_eog_passes_its_profile() {
    let om = crate::e2e_gnb_lf_combined_gas::rendered_44014().await;
    let pending = PendingOutbox::new(&*om.message_type, &*om.recipient, om.payload.clone());
    assert_eq!(pending.payload["pid"], 44014);
    let found = findings(&pending);
    assert!(found.is_empty(), "44014: {found:#?}");
}

fn registry() -> makod::party_registry::MpIdRegistry {
    use makod::config::PartyConfig;
    makod::party_registry::MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: NB.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        },
        PartyConfig {
            mp_id: LF.to_owned(),
            roles: vec!["LF".to_owned(), "MSB".to_owned()],
            primary: false,
            agency: None,
            as4: None,
        },
    ])
    .expect("registry")
}

/// What the pre-send gate says about `pending`: empty when it passes.
fn findings(pending: &PendingOutbox) -> Vec<String> {
    use edi_energy::EdiEnergyMessage as _;
    let om = crate::support::outbox_message(
        &pending.message_type,
        &pending.recipient,
        pending.payload.clone(),
    );
    let wire = match makod::edifact_renderer::render_to_wire_bytes(&om, &registry()) {
        Ok(w) => w,
        Err(e) => return vec![format!("does not render: {e:?}")],
    };
    let msg = edi_energy::parse(&wire.bytes).expect("a rendered UTILMD parses");
    let mut found: Vec<String> = msg
        .validate_on_date(mako_fristen::heute())
        .expect("the AHB validation runs")
        .errors()
        .iter()
        .map(|e| e.message.to_string())
        .collect();
    if !found.is_empty() {
        found.push(String::from_utf8_lossy(&wire.bytes).replace('\'', "'\n    "));
    }
    found
}

fn fold<W: Workflow>(state: W::State, events: &[W::Event]) -> W::State {
    events.iter().fold(state, |s, e| W::apply(s, e))
}

fn pid(code: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(code).expect("a valid PID")
}

/// The one UTILMD an answer writes.
fn the_utilmd(outbox: Vec<PendingOutbox>) -> PendingOutbox {
    let mut utilmd: Vec<_> = outbox
        .into_iter()
        .filter(|p| &*p.message_type == "UTILMD")
        .collect();
    assert_eq!(utilmd.len(), 1, "one UTILMD per answer");
    utilmd.remove(0)
}

/// `A99` „Sonstiges" out of `ebd`, with the Bemerkung it makes Muss.
fn ablehnung(ebd: &str) -> LfAntwort {
    LfAntwort::ablehnung("A99", ebd).with_bemerkung("Sonstiges")
}

/// The NB's answer to an Anmeldung neue Marktlokation 55600 / 55601.
fn neuanlage(anfrage: u32, antwort: LfAntwort) -> PendingOutbox {
    let received = GpkeNeuanlageWorkflow::handle(
        &Default::default(),
        NeuanlageCommand::ReceiveAnmeldung {
            pid: pid(anfrage),
            sender: MarktpartnerCode::new(LF),
            receiver: MarktpartnerCode::new(NB),
            location_id: MaLo::new(MALO),
            document_date: "20261001".into(),
            process_date: "20261201".into(),
            message_ref: MessageRef::new("NANLG1"),
            validation_passed: true,
            validation_errors: vec![],
            vorgang: Box::new(LfVorgangsdaten {
                transaktionsgrund: Some("E02".into()),
                vorgangsnummer: Some("NANLGVG1".into()),
                ..Default::default()
            }),
        },
    )
    .expect("the Anmeldung is received");
    let state = fold::<GpkeNeuanlageWorkflow>(Default::default(), &received.events);
    let answered = GpkeNeuanlageWorkflow::handle(&state, NeuanlageCommand::SendAntwort { antwort })
        .expect("the answer is accepted");
    the_utilmd(answered.outbox)
}

/// The LFN's answer to an Ankündigung Zuordnung LF 55607.
fn zuordnung_lf(antwort: LfAntwort) -> PendingOutbox {
    let received = GpkeAnkuendigungZuordnungLfWorkflow::handle(
        &Default::default(),
        AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
            pid: pid(55607),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            location_id: MaLo::new(MALO),
            document_date: "20261001".into(),
            process_date: "20261201".into(),
            message_ref: MessageRef::new("ZUORD1"),
            vorgang: Box::new(LfVorgangsdaten {
                transaktionsgrund: Some("E03".into()),
                transaktionsgrund_ergaenzung: Some("ZW8".into()),
                vorgangsnummer: Some("ZUORDVG1".into()),
                ..Default::default()
            }),
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .expect("the Ankündigung is received");
    let state = fold::<GpkeAnkuendigungZuordnungLfWorkflow>(Default::default(), &received.events);
    let answered = GpkeAnkuendigungZuordnungLfWorkflow::handle(
        &state,
        AnkuendigungZuordnungLfCommand::SendAntwort { antwort },
    )
    .expect("the answer is accepted");
    the_utilmd(answered.outbox)
}

/// The NB's Bestätigung of an Abmeldung Messstellenbetrieb 55051.
fn bestaetigung_ende_msb() -> PendingOutbox {
    use mako_wim::{DeviceChangeCommand, WimDeviceChangeWorkflow};
    let received = WimDeviceChangeWorkflow::handle(
        &Default::default(),
        DeviceChangeCommand::ReceiveUtilmd {
            pid: pid(55051),
            sender: MarktpartnerCode::new(LF),
            receiver: MarktpartnerCode::new(NB),
            melo_id: MeLo::new(MELO),
            device_id: mako_engine::types::DeviceId::new("ZAEHLER1"),
            document_date: "20261001".into(),
            message_ref: MessageRef::new("WIM1"),
            vorgangsnummer: Some("WIMVG1".into()),
            process_date: Some("20261201".into()),
            transaktionsgrund: Some("E03".into()),
            validation_passed: true,
            validation_errors: vec![],
            received_at: time::OffsetDateTime::now_utc(),
        },
    )
    .expect("the Abmeldung is received");
    let state = fold::<WimDeviceChangeWorkflow>(Default::default(), &received.events);
    let answered = WimDeviceChangeWorkflow::handle(
        &state,
        DeviceChangeCommand::DispatchAntwort {
            bestaetigt: true,
            antwort_code: "E15".into(),
            bemerkung: None,
            abweichender_termin: None,
        },
    )
    .expect("the Bestätigung is accepted");
    the_utilmd(answered.outbox)
}
