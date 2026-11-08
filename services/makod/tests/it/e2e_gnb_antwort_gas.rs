//! The LF's answer to an Anfrage the GNB sent resumes the GNB's own process.
//!
//! The GNB starts three GeLi Gas processes itself — the Abmeldung NN 44007, the
//! Abmeldungsanfrage 44010 and the Ersatz-/Grundversorgung 44013 — and the LF
//! answers each with a Bestätigung or an Ablehnung: 44008/44009, 44011/44012,
//! 44014/44015 (UTILMD AHB Gas, LF → NB). Each answer must move the GNB's
//! process through `ReceiveGnbAntwort`, which records the `SG4 STS+E01` code and
//! the Entscheidungsbaum that defines it. Built as an Anfrage instead, the
//! answer refuses inside the workflow and the GNB's Verlauf never shows what the
//! LF said — while its Frist runs to a lapse that, for 44013, assigns the
//! Marktlokation to the Grundversorger the LF may already have declined
//! (GeLi Gas 3.0 Kap. 3.3).
//!
//! The process is spawned through the production door, `geli.eog.anmelden`,
//! and the answer arrives through the ingest dispatcher, so the test holds the
//! routing, the adapter and the correlation together.

use std::sync::Arc;

use mako_engine::event_store::EventStore as _;
use mako_engine::{
    ids::{ProcessId, StreamId, TenantId},
    store_slatedb::SlateDbStore,
};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

// A DVGW-Codenummer: NAD DE 3055 `332`, the code a Gas MIG admits — the
// 44013 is checked against its profile before the spawn.
const GNB_MP: &str = "9807317000007";
const LF_MP: &str = "4012345000023";
const MALO: &str = "51238696012";

async fn gnb() -> (
    makod::commands_api::CommandsApiState,
    EdifactIngestDispatcher,
) {
    use makod::{
        cedar_authz::{CedarAuthorizer, DefaultPolicy},
        commands_api::CommandsApiState,
        malo_cache::{MaloIdentResultCache, SlateDbMaloCache},
    };
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant_id = TenantId::from_party_id(GNB_MP);
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant_id,
    );
    let cedar = Arc::new(
        CedarAuthorizer::new(vec![], None, None, None, DefaultPolicy::PermitAll)
            .expect("cedar build"),
    );
    let state = CommandsApiState {
        tenant_id,
        sender_party_id: GNB_MP.to_owned(),
        configured_marktrollen: vec!["GNB".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar,
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
        marktd_client: Some(Arc::new(
            crate::support::stub_marktd(Some(crate::support::gas_stammdaten(MALO, GNB_MP))).await,
        )),
    };
    (state, dispatcher)
}

/// The profile's own skeleton of the answer, from the LF to this GNB, with the
/// `SG4 STS+E01` code set to `code`.
fn answer(pid: u32, code: &str) -> edi_energy::AnyMessage {
    use edi_energy::profile::SkeletonParties;
    let wanted = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(edi_energy::MessageType::Utilmd)
        .filter(|p| p.has_anwendungsfall(wanted))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no UTILMD profile carries {pid}"));
    let af = profile.anwendungsfall(pid).expect("carried");
    let wire = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: LF_MP.to_owned(),
                receiver: GNB_MP.to_owned(),
            },
        )
        .unwrap_or_else(|e| panic!("skeleton for {pid}: {e}"));
    let text = String::from_utf8(wire).expect("skeleton is UNOC text");
    let text = set_antwortcode(&text, code);
    let text = set_element(&text, "LOC+172+", MALO);
    edi_energy::parse(text.as_bytes()).unwrap_or_else(|e| panic!("answer {pid} parses: {e}"))
}

/// Replace DE 9013 of the `STS+E01` segment with `code`.
fn set_antwortcode(wire: &str, code: &str) -> String {
    set_element(wire, "STS+E01++", code)
}

/// Replace the first data element after `prefix` with `value`.
fn set_element(wire: &str, prefix: &str, value: &str) -> String {
    let start = wire
        .find(prefix)
        .unwrap_or_else(|| panic!("the answer carries {prefix}"));
    let at = start + prefix.len();
    let end = at
        + wire[at..]
            .find(['\'', ':', '+'])
            .unwrap_or_else(|| panic!("{prefix} is terminated"));
    format!("{}{value}{}", &wire[..at], &wire[end..])
}

fn malo_of(msg: &edi_energy::AnyMessage) -> String {
    let edi_energy::AnyMessage::Utilmd(u) = msg else {
        panic!("a UTILMD");
    };
    u.transactions()
        .first()
        .and_then(|t| t.lokation())
        .expect("the answer names its Marktlokation")
        .to_owned()
}

/// Start the GNB's 44013 through the ERP door and return its process.
async fn eog_anmelden(state: &makod::commands_api::CommandsApiState, malo: &str) -> ProcessId {
    let outcome = makod::commands_api::dispatch_command(
        state,
        "geli.eog.anmelden",
        &crate::support::geli_eog_anmeldung(malo, LF_MP),
    )
    .await
    .expect("geli.eog.anmelden spawns");
    match outcome {
        makod::commands_api::DispatchOutcome::Spawned { process_id } => process_id,
        other => panic!("geli.eog.anmelden must spawn, got {other:?}"),
    }
}

/// The events of `process`, with the answer the inbound message recorded.
async fn verlauf(
    state: &makod::commands_api::CommandsApiState,
    process: &ProcessId,
) -> Vec<mako_engine::envelope::EventEnvelope> {
    state
        .store
        .load(&StreamId::for_process(state.tenant_id, process))
        .await
        .expect("stream loads")
}

async fn answered(
    pid: u32,
    code: &str,
) -> (
    Vec<mako_engine::envelope::EventEnvelope>,
    mako_geli_gas::GasSupplierChangeEvent,
) {
    let (state, dispatcher) = gnb().await;
    let msg = answer(pid, code);
    let malo = malo_of(&msg);
    let process = eog_anmelden(&state, &malo).await;

    let outcome = dispatcher
        .dispatch(
            &msg,
            mako_geli_gas::WORKFLOW_NAME,
            pid,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap_or_else(|e| panic!("{pid} must reach the GNB's process: {e}"));
    match outcome {
        IngestOutcome::Dispatched { process_id, .. } => assert_eq!(
            process_id, process,
            "{pid} resumes the 44013 process the GNB started"
        ),
        other => panic!("{pid} is an answer and must resume, got {other:?}"),
    }

    let events = verlauf(&state, &process).await;
    let last = events.last().expect("the answer is recorded");
    let event: mako_geli_gas::GasSupplierChangeEvent =
        serde_json::from_value(last.payload.clone()).expect("a GeLi Gas event");
    (events, event)
}

/// **An inbound 44014 resumes the GNB's EoG process** and its Verlauf shows the
/// Bestätigung's code worded by `E_3008`, the tree the 44013 is answered from.
#[tokio::test]
async fn an_inbound_44014_resumes_the_gnb_eog_process_with_its_code() {
    let (events, event) = answered(44014, "E15").await;
    let mako_geli_gas::GasSupplierChangeEvent::GnbAntwortErhalten {
        response_pid,
        accepted,
        ..
    } = event
    else {
        panic!("44014 records GnbAntwortErhalten, got {event:?}");
    };
    assert_eq!(response_pid.as_u32(), 44014);
    assert!(accepted, "44014 is the Bestätigung");

    let last = events.last().expect("recorded");
    let verlauf = last.verlauf.as_ref().expect("an inbound market message");
    assert_eq!(verlauf.pid, Some(44014));
    assert!(matches!(
        verlauf.richtung,
        mako_engine::envelope::Richtung::Eingang
    ));
    let antwort = last.antwort.as_ref().expect("the answer code is kept");
    assert_eq!(antwort.code, "E15");
    assert_eq!(antwort.codeliste.as_deref(), Some("E_3008"));
    let worded = mako_pruefung::codes::lies("E_3008", &antwort.code)
        .expect("E_3008 publishes the code the LF answered with");
    assert!(
        !worded.bedeutung.is_empty(),
        "the Verlauf reads the BDEW wording"
    );
}

/// **A 44015 is recorded as not accepted**, with its Ablehnungscode.
#[tokio::test]
async fn an_inbound_44015_is_recorded_as_a_refusal() {
    let (events, event) = answered(44015, "E17").await;
    let mako_geli_gas::GasSupplierChangeEvent::GnbAntwortErhalten {
        response_pid,
        accepted,
        ..
    } = event
    else {
        panic!("44015 records GnbAntwortErhalten, got {event:?}");
    };
    assert_eq!(response_pid.as_u32(), 44015);
    assert!(!accepted, "44015 is the Ablehnung");
    let antwort = events
        .last()
        .and_then(|e| e.antwort.as_ref())
        .expect("the Ablehnungscode is kept");
    assert_eq!(antwort.code, "E17");
    assert_eq!(antwort.codeliste.as_deref(), Some("E_3008"));
}

/// **An answer with no GNB process to resume opens none.**
#[tokio::test]
async fn an_answer_without_its_anfrage_does_not_spawn() {
    for pid in [44008, 44009, 44011, 44012, 44014, 44015] {
        let (_, dispatcher) = gnb().await;
        let msg = answer(pid, "E15");
        let outcome = dispatcher
            .dispatch(
                &msg,
                mako_geli_gas::WORKFLOW_NAME,
                pid,
                time::OffsetDateTime::now_utc(),
            )
            .await;
        assert!(
            !matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
            "{pid} is an answer and must not open a process, got {outcome:?}"
        );
    }
}
