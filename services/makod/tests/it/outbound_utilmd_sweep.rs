//! Every outbound UTILMD answer listed here renders to a message its own
//! Prüfschablone accepts — the check the pre-send gate
//! (`transport::as4_sender`) runs before any delivery.
//!
//! An answer goes out in the release of the process it answers, which keeps
//! the format version it started under until it completes (AGENTS.md
//! *Format-version coexistence*). So each case is swept once per process
//! start in [`STARTS`] — one before the 01.10.2026 cutover, one after — and
//! validated against that process's own release, not the one in force today.
//!
//! Each case drives its workflow as production does — the inbound Anfrage, then
//! the answer command — and hands the outbox entry the answer writes to the
//! renderer, the parser and the AHB validation. A PID enters [`SWEEP`] once it
//! passes, so a renderer or workflow change that drops a Muss place fails here
//! rather than dead-lettering the message in production.
//!
//! The Bestätigungen EoG 44014 and 55014 carry the Marktlokation's record
//! from marktd; their cases run the combined deployments of
//! `e2e_gnb_lf_combined_gas` and `e2e_nb_lf_combined_strom`, whose loopbacks
//! gate every UTILMD.

use mako_engine::outbox::PendingOutbox;
use mako_engine::types::{MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator};
use mako_engine::version::FormatVersion;
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

/// The days the swept processes start on: under fv20251001 and under
/// fv20261001, the two UTILMD Strom releases in force at once.
const STARTS: [time::Date; 2] = [
    time::macros::date!(2026 - 09 - 15),
    time::macros::date!(2026 - 10 - 15),
];

/// The format version a process started on `day` runs under — the newest in
/// force that day, which is what makod stamps on its `WorkflowId`.
fn started_on(day: time::Date) -> FormatVersion {
    edi_energy::registry::ReleaseRegistry::global()
        .format_versions_in_force_on(day)
        .into_iter()
        .filter_map(|s| FormatVersion::parse(&s).ok())
        .max()
        .expect("a format version in force")
}

#[test]
fn every_swept_answer_passes_its_profile() {
    let mut failures = Vec::new();
    for start in STARTS {
        let fv = started_on(start);
        for (expected, build) in SWEEP {
            let pending = build();
            assert_eq!(pending.payload["pid"], *expected, "{:#}", pending.payload);
            let (_, found) = findings(&pending, &fv);
            if !found.is_empty() {
                failures.push(format!("{expected} under {fv}:\n  {}", found.join("\n  ")));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "answers short of their profile:\n{}",
        failures.join("\n")
    );
}

/// A 55052 answering a process started on 15.09.2026 goes out in that
/// process's release — the UTILMD Strom release in force on its format
/// version's Anwendungszeitpunkt, fv20251001 — and passes that profile; it is
/// not rendered in the fv20261001 release in force on the day it is sent.
#[test]
fn an_answer_is_validated_against_the_release_of_its_process() {
    let fv = started_on(time::macros::date!(2026 - 09 - 15));
    let (release, found) = findings(&bestaetigung_ende_msb(), &fv);
    let fv20251001 = edi_energy::registry::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .find(|p| {
            p.message_type() == edi_energy::MessageType::Utilmd
                && p.release().track() == edi_energy::ReleaseTrack::Strom
                && p.valid_from() == Some(time::macros::date!(2025 - 10 - 01))
        })
        .expect("the fv20251001 UTILMD Strom profile")
        .release()
        .clone();
    assert_eq!(release, fv20251001, "{fv}");
    assert!(found.is_empty(), "55052 under {fv}: {found:#?}");
}

/// The Gas Bestätigung EoG, rendered from marktd's record and the LF's own
/// facts, passes its profile.
#[tokio::test]
async fn the_gas_bestaetigung_eog_passes_its_profile() {
    let om = crate::e2e_gnb_lf_combined_gas::rendered_44014().await;
    let pending = PendingOutbox::new(&*om.message_type, &*om.recipient, om.payload.clone());
    assert_eq!(pending.payload["pid"], 44014);
    let (_, found) = findings(&pending, &started_on(mako_fristen::heute()));
    assert!(found.is_empty(), "44014: {found:#?}");
}

/// The Strom Bestätigung EoG, rendered from marktd's record and the
/// Versorgungsart the E/G states, passes its profile.
#[tokio::test]
async fn the_strom_bestaetigung_eog_passes_its_profile() {
    let om = crate::e2e_nb_lf_combined_strom::rendered_55014().await;
    let pending = PendingOutbox::new(&*om.message_type, &*om.recipient, om.payload.clone());
    assert_eq!(pending.payload["pid"], 55014);
    let (_, found) = findings(&pending, &started_on(mako_fristen::heute()));
    assert!(found.is_empty(), "55014: {found:#?}");
    let wire = String::from_utf8(
        makod::edifact_renderer::render_to_wire_bytes(&om, &registry())
            .expect("renders")
            .bytes,
    )
    .expect("UNOC text");
    for place in [
        "STS+7++ZT6+ZW4'",
        "RFF+TN:",
        "SEQ+Z79",
        "SEQ+Z01'CCI+++Z18'CCI+Z36++ZC9'",
        "SEQ+Z75'CCI+Z61++ZF9'CAV+ZU6'",
        "NAD+Z09",
        "NAD+Z04",
    ] {
        assert!(wire.contains(place), "{place} in {wire}");
    }
    assert!(
        !wire.contains("LOC+"),
        "the 55014 names no Lokation: {wire}"
    );
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

/// The release `pending` renders in when its process runs under `fv`, and
/// what that release's profile says about it: empty when it passes.
fn findings(pending: &PendingOutbox, fv: &FormatVersion) -> (edi_energy::Release, Vec<String>) {
    use edi_energy::EdiEnergyMessage as _;
    let mut om = crate::support::outbox_message(
        &pending.message_type,
        &pending.recipient,
        pending.payload.clone(),
    );
    om.format_version = Some(fv.clone());
    let wire = match makod::edifact_renderer::render_to_wire_bytes(&om, &registry()) {
        Ok(w) => w,
        Err(e) => {
            return (
                edi_energy::Release::new("-"),
                vec![format!("does not render: {e:?}")],
            );
        }
    };
    let msg = edi_energy::parse(&wire.bytes).expect("a rendered UTILMD parses");
    let release = msg.detect_release().expect("a release").clone();
    let mut found: Vec<String> = msg
        .validate_on_date(fv.date().expect("an FV<YYYY>-<MM>-<DD>"))
        .expect("the AHB validation runs")
        .errors()
        .iter()
        .map(|e| e.message.to_string())
        .collect();
    if !found.is_empty() {
        found.push(String::from_utf8_lossy(&wire.bytes).replace('\'', "'\n    "));
    }
    (release, found)
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
