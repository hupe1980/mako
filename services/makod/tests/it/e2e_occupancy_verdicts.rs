//! On the ingest path a settled process does not block its business key, and a
//! live one does.
//!
//! The correlation index is append-only, so an entry proves a process once
//! existed and never that one still runs. Resuming on the entry alone feeds a
//! second initiating message into a finished process, whose initiating command
//! refuses anything outside its initial state: the message is dead-lettered and
//! no process can ever spawn for that key again. Each arm therefore asks the
//! verdict its workflow's crate publishes (`OccupiesBusinessKey`).

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN: &str = "9900001000001";

/// A Marktlokation for the families whose skeleton leaves `LOC+172` empty.
const MALO: &str = "51238696781";

/// The skeleton of `pid` with its `LOC+172` naming [`MALO`] — the business key
/// an MSCONS is correlated by.
fn located(mt: edi_energy::MessageType, pid: u32) -> edi_energy::AnyMessage {
    let wire = String::from_utf8(skeleton(mt, pid)).expect("UTF-8");
    let at = wire.find("LOC+172").expect("the skeleton has a LOC+172");
    let end = at + wire[at..].find('\'').expect("segment terminator");
    let wire = format!("{}LOC+172+{MALO}{}", &wire[..at], &wire[end..]);
    edi_energy::parse(wire.as_bytes()).expect("the located skeleton parses")
}

fn fixture(mt: edi_energy::MessageType, pid: u32) -> edi_energy::AnyMessage {
    edi_energy::parse(&skeleton(mt, pid)).expect("the skeleton parses")
}

fn skeleton(mt: edi_energy::MessageType, pid: u32) -> Vec<u8> {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(mt)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a profile carries the PID");
    let af = profile.anwendungsfall(pid).expect("carried");
    profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: "4012345000023".to_owned(),
                receiver: OWN.to_owned(),
            },
        )
        .expect("skeleton")
}

async fn dispatcher() -> EdifactIngestDispatcher {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    )
}

/// Two messages of `pid` for one business key: the first settles its process
/// on receipt, so the second must open a new one.
async fn second_spawns(mt: edi_energy::MessageType, pid: u32, workflow: &str) {
    second_spawns_of(fixture(mt, pid), pid, workflow).await;
}

async fn second_spawns_of(msg: edi_energy::AnyMessage, pid: u32, workflow: &str) {
    let d = dispatcher().await;
    let first = d
        .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
        .await
        .expect("first");
    let IngestOutcome::Spawned { process_id: a, .. } = first else {
        panic!("the first {pid} spawns: {first:?}");
    };
    let second = d
        .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
        .await;
    match second {
        Ok(IngestOutcome::Spawned { process_id: b, .. }) => assert_ne!(a, b),
        other => panic!(
            "a second {pid} for a settled {workflow} process must spawn a new one, got {other:?}"
        ),
    }
}

/// MaBiS Clearingliste: a received list is settled once validated.
#[tokio::test]
async fn a_settled_clearingliste_does_not_block_the_next_one() {
    second_spawns(
        edi_energy::MessageType::Utilmd,
        55067,
        "mabis-clearingliste",
    )
    .await;
}

/// WiM Rechnungsabwicklung: a recorded Bestellung (ORDERS 17005) is terminal.
#[tokio::test]
async fn a_recorded_rechnungsabwicklung_does_not_block_the_next_one() {
    second_spawns(
        edi_energy::MessageType::Orders,
        17005,
        "wim-rechnungsabwicklung",
    )
    .await;
}

/// GPKE EoG: a Zuordnung being answered still holds the MaLo, so a second one
/// is not a new process.
#[tokio::test]
async fn a_live_eog_zuordnung_still_blocks_the_malo() {
    let d = dispatcher().await;
    let msg = fixture(edi_energy::MessageType::Utilmd, 55013);
    let first = d
        .dispatch(&msg, "gpke-eog", 55013, time::OffsetDateTime::now_utc())
        .await
        .expect("first");
    assert!(matches!(first, IngestOutcome::Spawned { .. }), "{first:?}");
    let second = d
        .dispatch(&msg, "gpke-eog", 55013, time::OffsetDateTime::now_utc())
        .await;
    assert!(
        !matches!(second, Ok(IngestOutcome::Spawned { .. })),
        "a live EoG Zuordnung must not be duplicated: {second:?}"
    );
}

/// GPKE Messwerte: an MSCONS is settled once validated, so the next delivery
/// for the same Marktlokation is its own process.
#[tokio::test]
async fn a_settled_messwerte_delivery_does_not_block_the_next_one() {
    second_spawns_of(
        located(edi_energy::MessageType::Mscons, 13018),
        13018,
        "gpke-messwerte",
    )
    .await;
}

/// GeLi Gas MSCONS: the same for a Gas delivery.
#[tokio::test]
async fn a_settled_gas_mscons_delivery_does_not_block_the_next_one() {
    second_spawns_of(
        located(edi_energy::MessageType::Mscons, 13008),
        13008,
        "geli-gas-mscons",
    )
    .await;
}

/// Dispatch `msg`, settle the process it spawned with the `settle` commands,
/// and dispatch it again: a settled process must not hold the key.
async fn settled_then_second_spawns<W>(
    msg: edi_energy::AnyMessage,
    pid: u32,
    workflow: &str,
    settle: Vec<W::Command>,
) where
    W: mako_engine::workflow::Workflow + 'static,
    W::Command: mako_engine::workflow::CommandPayload + Clone,
    W::State: serde::Serialize,
{
    use mako_engine::registry::ProcessRegistry as _;
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let d = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    );
    let first = d
        .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
        .await
        .expect("first");
    let IngestOutcome::Spawned { process_id: a, .. } = first else {
        panic!("the first {pid} spawns: {first:?}");
    };
    // A located message is keyed on its location, a PRICAT on its sender.
    let key = match makod::ingest_dispatcher::extract_malo_from_msg(&msg).as_str() {
        "" => makod::ingest_dispatcher::extract_sender_key(&msg),
        located => located.to_owned(),
    };
    let identity = store
        .as_process_registry()
        .lookup_correlated(TenantId::from_party_id(OWN), &key)
        .await
        .expect("lookup")
        .into_iter()
        .find(|i| i.process_id == a)
        .expect("the spawned process is indexed");
    let process = mako_engine::process::Process::<W, Arc<SlateDbStore>>::from_identity(
        Arc::new(store.clone()),
        identity,
    );
    for cmd in settle {
        process
            .run(cmd, mako_engine::process::RunOptions::new())
            .await
            .expect("the process settles");
    }
    let second = d
        .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
        .await;
    match second {
        Ok(IngestOutcome::Spawned { process_id: b, .. }) => assert_ne!(a, b),
        other => panic!(
            "a second {pid} for a settled {workflow} process must spawn a new one, got {other:?}"
        ),
    }
}

/// GPKE Stammdatenänderung: an Änderung answered with its Rückmeldung is
/// settled (`StammdatenState::is_terminal`).
#[tokio::test]
async fn an_answered_stammdatenaenderung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeStammdatenaenderungWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55616),
        55616,
        "gpke-stammdatenaenderung",
        vec![mako_gpke::StammdatenCommand::SendRueckmeldung {
            qualitaet: mako_gpke::Qualitaet::Uebernommen,
        }],
    )
    .await;
}

/// GeLi Gas Stammdatenänderung: an Änderung answered with its Zustimmung is
/// settled (`GasStammdatenState::is_terminal`).
#[tokio::test]
async fn an_answered_gas_stammdatenaenderung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_geli_gas::GeliGasStammdatenaenderungWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 44112),
        44112,
        "geli-gas-stammdatenaenderung",
        vec![mako_geli_gas::GasStammdatenCommand::SendAntwort {
            antwort_code: "E15".to_owned(),
        }],
    )
    .await;
}

/// GPKE Abrechnungsdaten: a Rückmeldung answered with its Bearbeitungsstand is
/// settled (`AbrechnungsdatenState::is_terminal`).
#[tokio::test]
async fn an_answered_abrechnungsdaten_rueckmeldung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeAbrechnungsdatenWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55156),
        55156,
        "gpke-abrechnungsdaten",
        vec![
            mako_gpke::abrechnungsdaten::AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code: "A05".to_owned(),
                sendet_stammdatenaenderung: true,
                bemerkung: None,
            },
        ],
    )
    .await;
}

/// GPKE Kündigung (LFA side): a Kündigung answered with an Ablehnung is
/// settled, so the next one for the Marktlokation is a new Vorgang.
#[tokio::test]
async fn a_refused_kuendigung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeKuendigungWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55016),
        55016,
        "gpke-kuendigung",
        vec![mako_gpke::KuendigungCommand::SendAntwort {
            antwort: mako_gpke::LfAntwort::ablehnung("A35", "E_0624"),
        }],
    )
    .await;
}

/// GPKE Beendigung der Zuordnung (LFA side): the same for a 55010.
#[tokio::test]
async fn a_refused_beendigung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeBeendigungZuordnungWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55010),
        55010,
        "gpke-beendigung-zuordnung",
        vec![mako_gpke::BeendigungZuordnungCommand::SendAntwort {
            antwort: mako_gpke::LfAntwort::ablehnung("A35", "E_0624"),
        }],
    )
    .await;
}

/// GPKE Ankündigung Zuordnung (LF side): the same for a 55607.
#[tokio::test]
async fn a_refused_ankuendigung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeAnkuendigungZuordnungLfWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55607),
        55607,
        "gpke-ankuendigung-zuordnung-lf",
        vec![mako_gpke::AnkuendigungZuordnungLfCommand::SendAntwort {
            antwort: mako_gpke::LfAntwort {
                bemerkung: Some("Testfall".to_owned()),
                ..mako_gpke::LfAntwort::ablehnung("A99", "E_0603")
            },
        }],
    )
    .await;
}

/// GPKE Neuanlage (NB side): the same for a 55600.
#[tokio::test]
async fn a_refused_neuanlage_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeNeuanlageWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55600),
        55600,
        "gpke-neuanlage",
        vec![mako_gpke::NeuanlageCommand::SendAntwort {
            antwort: mako_gpke::LfAntwort::ablehnung("A03", "E_0608"),
            marktlokation: None,
            tranche: None,
        }],
    )
    .await;
}

/// GPKE Stornierung: an Anfrage Stornierung answered with its Ablehnung is
/// settled, so the next one for the Vorgang is its own process.
#[tokio::test]
async fn a_refused_stornierung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeStornierungWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55022),
        55022,
        "gpke-stornierung",
        vec![mako_gpke::GpkeStornierungCommand::SendAntwort {
            ablehnung: Some(mako_gpke::StornoAblehnung::SchonBeantwortet),
        }],
    )
    .await;
}

/// WiM Preisanfrage: a concluded Anfrage is settled
/// (`PreisanfrageState: OccupiesBusinessKey`), so the next REQOTE for the
/// location is its own process.
#[tokio::test]
async fn a_concluded_preisanfrage_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_wim::WimPreisanfrageWorkflow>(
        fixture(edi_energy::MessageType::Reqote, 35001),
        35001,
        "wim-preisanfrage",
        vec![
            mako_wim::PreisanfrageCommand::SendAngebot {
                message_ref: mako_engine::types::MessageRef::new("ANGEBOT-1"),
                location: MALO.to_owned(),
            },
            mako_wim::PreisanfrageCommand::Abschliessen,
        ],
    )
    .await;
}

/// GeLi Gas Stornierung: an Anfrage Stornierung answered with its Ablehnung
/// is settled, so the next one is its own process.
#[tokio::test]
async fn a_refused_gas_stornierung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_geli_gas::GeliGasStornierungWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 44022),
        44022,
        "geli-gas-stornierung",
        vec![mako_geli_gas::GeliGasStornierungCommand::SendAntwort {
            ablehnung: Some(mako_geli_gas::StornoAblehnung::SchonBeantwortet),
        }],
    )
    .await;
}

/// GPKE PARTIN: Kommunikationsdaten are keyed on the Marktpartner that sends
/// them, and a validated message is settled.
#[tokio::test]
async fn a_settled_partin_does_not_block_the_next_one() {
    second_spawns(edi_energy::MessageType::Partin, 37000, "gpke-partin").await;
}

/// GeLi Gas PARTIN: the same.
#[tokio::test]
async fn a_settled_gas_partin_does_not_block_the_next_one() {
    second_spawns(edi_energy::MessageType::Partin, 37008, "geli-gas-partin").await;
}

/// GPKE UTILTS: keyed on its `LOC+172`, settled once validated.
#[tokio::test]
async fn a_settled_utilts_does_not_block_the_next_one() {
    second_spawns(edi_energy::MessageType::Utilts, 25001, "gpke-utilts").await;
}

/// WiM Preisliste: keyed on the MSB that publishes it, settled once recorded.
#[tokio::test]
async fn a_recorded_preisliste_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_wim::WimPreislisteWorkflow>(
        fixture(edi_energy::MessageType::Pricat, 27002),
        27002,
        "wim-preisliste",
        vec![mako_wim::PreislisteCommand::Aufzeichnen],
    )
    .await;
}

/// GPKE Anfrage Daten der individuellen Bestellung (55555): answered is
/// settled.
#[tokio::test]
async fn an_answered_anfrage_bestellung_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_gpke::GpkeAnfrageBestellungWorkflow>(
        fixture(edi_energy::MessageType::Utilmd, 55555),
        55555,
        "gpke-anfrage-bestellung",
        vec![mako_gpke::AnfrageBestellungCommand::DispatchResponse {
            data_provided: false,
            reason: Some("keine Bestellung".to_owned()),
        }],
    )
    .await;
}

/// GeLi Gas Datenabruf (GNB side): a delivered Datenabruf is settled.
#[tokio::test]
async fn a_delivered_datenabruf_does_not_block_the_next_one() {
    settled_then_second_spawns::<mako_geli_gas::GeliGasDatenabrufWorkflow>(
        fixture(edi_energy::MessageType::Orders, 17103),
        17103,
        mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
        vec![mako_geli_gas::GeliGasDatenabrufCommand::NotifyDatenGeliefert],
    )
    .await;
}
