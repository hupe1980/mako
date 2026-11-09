//! The outbound legs that only a command can send.
//!
//! Each of these workflows routes its inbound side through the ingest
//! dispatcher and owes an outbound message no counterparty message triggers:
//! the NB's answer to a Stornierungsanfrage, the gMSB's Vorabinformation and
//! the wMSB's answer to it, the requester's REQOTE and the MSB's QUOTES. A
//! command that is not registered leaves the obligation unmeetable however
//! complete the workflow is, so each test posts the command and reads the
//! outbox for the message it must leave there.

use std::sync::Arc;

use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;

use super::*;
use crate::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP_ID: &str = "9900357000004";
const PARTNER_MP_ID: &str = "9905550000005";
const MELO_ID: &str = "DE0001000001234567890000000000001";
const VORGANG_ID: &str = "STORNO-VG-1";
/// DVGW-Codenummern for the Gas Stornierung: a Gas UTILMD admits NAD DE 3055
/// `9` or `332` only (UTILMD AHB Gas 1.2 Kap. 5.2), never the BDEW `293`.
const GAS_OWN_MP_ID: &str = "9807317000007";
const GAS_PARTNER_MP_ID: &str = "9870012345678";

async fn state_for(roles: &[&str]) -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    CommandsApiState {
        tenant_id: TenantId::from_party_id(OWN_MP_ID),
        sender_party_id: OWN_MP_ID.to_owned(),
        configured_marktrollen: roles.iter().map(|r| (*r).to_owned()).collect(),
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(CedarAuthorizer::unauthenticated().expect("infallible")),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
        marktd_client: None,
    }
}

fn ingest(state: &CommandsApiState) -> EdifactIngestDispatcher {
    EdifactIngestDispatcher::new(
        Arc::clone(&state.store),
        state.snapshot_store.clone(),
        100,
        state.tenant_id,
    )
}

/// The outbox entries of `message_type`, as `(recipient, payload)`.
async fn outbox(state: &CommandsApiState, message_type: &str) -> Vec<(String, serde_json::Value)> {
    state
        .store
        .pending_now(1_000)
        .await
        .expect("read outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == message_type)
        .map(|m| (m.recipient.to_string(), m.payload))
        .collect()
}

async fn seed<W>(state: &CommandsApiState, name: &'static str, command: W::Command, key: &str)
where
    W: mako_engine::workflow::Workflow,
    Arc<mako_engine::store_slatedb::SlateDbStore>: mako_engine::event_store::AtomicAppend,
{
    spawn_process::<W>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(name, latest_format_version()),
        command,
        &[],
        &[key],
    )
    .await
    .expect("seed the inbound leg");
}

fn roles(r: &[&str]) -> Vec<String> {
    r.iter().map(|s| (*s).to_owned()).collect()
}

// ── Stornierung (55022 → 55023/55024, 44022 → 44023/44024) ───────────────────

async fn seed_gpke_storno(state: &CommandsApiState) {
    seed::<mako_gpke::GpkeStornierungWorkflow>(
        state,
        mako_gpke::STORNIERUNG_GPKE_WORKFLOW_NAME,
        mako_gpke::GpkeStornierungCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::const_new(55022),
            sender: MarktpartnerCode::new(PARTNER_MP_ID),
            receiver: MarktpartnerCode::new(OWN_MP_ID),
            vorgang_id: MaLo::new(VORGANG_ID),
            document_code: "E01".to_owned(),
            document_date: "20261104".to_owned(),
            message_ref: MessageRef::new("STORNO-1"),
            validation_passed: true,
            validation_errors: vec![],
        },
        VORGANG_ID,
    )
    .await;
}

async fn seed_geli_storno(state: &CommandsApiState) {
    seed::<mako_geli_gas::GeliGasStornierungWorkflow>(
        state,
        mako_geli_gas::STORNIERUNG_WORKFLOW_NAME,
        mako_geli_gas::GeliGasStornierungCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::const_new(44022),
            sender: MarktpartnerCode::new(GAS_PARTNER_MP_ID),
            receiver: MarktpartnerCode::new(GAS_OWN_MP_ID),
            vorgang_id: MaLo::new(VORGANG_ID),
            document_code: "E01".to_owned(),
            document_date: "20261104".to_owned(),
            message_ref: MessageRef::new("STORNO-1"),
            validation_passed: true,
            validation_errors: vec![],
        },
        VORGANG_ID,
    )
    .await;
}

#[tokio::test]
async fn the_nb_confirms_a_strom_stornierung_with_utilmd_55023() {
    let state = state_for(&["NB"]).await;
    seed_gpke_storno(&state).await;
    let body = serde_json::json!({ "vorgang_id": VORGANG_ID });
    dispatch_command(&state, "gpke.stornierung.bestaetigen", &body)
        .await
        .expect("the NB answers the 55022");
    let utilmd = outbox(&state, "UTILMD").await;
    assert_eq!(utilmd.len(), 1, "one answer: {utilmd:?}");
    let (recipient, payload) = &utilmd[0];
    assert_eq!(
        recipient, PARTNER_MP_ID,
        "the answer goes back to the sender"
    );
    assert_eq!(payload["pid"], 55023);
    assert_eq!(payload["antwort_code"], "E15");

    // Answered once; a second answer finds no ValidationPassed state.
    assert!(
        dispatch_command(&state, "gpke.stornierung.bestaetigen", &body)
            .await
            .is_err(),
        "an answered Stornierung is not answered again"
    );
}

#[tokio::test]
async fn the_nb_refuses_a_strom_stornierung_with_utilmd_55024() {
    let state = state_for(&["NB"]).await;
    seed_gpke_storno(&state).await;

    // E14 without its Erläuterung, a bemerkung on Z14, a code outside S_0087
    // and an unknown field are refused before anything is sent.
    for bad in [
        serde_json::json!({ "vorgang_id": VORGANG_ID, "antwortcode": "E14" }),
        serde_json::json!({ "vorgang_id": VORGANG_ID, "antwortcode": "Z14", "bemerkung": "x" }),
        serde_json::json!({ "vorgang_id": VORGANG_ID, "antwortcode": "A99" }),
        serde_json::json!({ "vorgang_id": VORGANG_ID, "antwortcode": "Z14", "grund": "x" }),
    ] {
        let out = dispatch_command(&state, "gpke.stornierung.ablehnen", &bad).await;
        assert!(
            matches!(out, Err(DispatchError::InvalidPayload(_))),
            "{bad} must be refused, got {out:?}"
        );
    }
    assert!(outbox(&state, "UTILMD").await.is_empty());

    dispatch_command(
        &state,
        "gpke.stornierung.ablehnen",
        &serde_json::json!({ "vorgang_id": VORGANG_ID, "antwortcode": "Z14" }),
    )
    .await
    .expect("the NB refuses the 55022");
    let utilmd = outbox(&state, "UTILMD").await;
    assert_eq!(utilmd.len(), 1);
    assert_eq!(utilmd[0].1["pid"], 55024);
    assert_eq!(utilmd[0].1["antwort_code"], "Z14");
    assert_eq!(utilmd[0].1["antwort_codeliste"], "S_0087");
}

#[tokio::test]
async fn the_gnb_answers_a_gas_stornierung_with_utilmd_44023_or_44024() {
    let state = state_for(&["GNB"]).await;
    seed_geli_storno(&state).await;
    dispatch_command(
        &state,
        "geli.stornierung.ablehnen",
        &serde_json::json!({
            "vorgang_id": VORGANG_ID,
            "antwortcode": "E14",
            "bemerkung": "Vorgang bereits in Bearbeitung",
        }),
    )
    .await
    .expect("the GNB refuses the 44022");
    let utilmd = outbox(&state, "UTILMD").await;
    assert_eq!(utilmd.len(), 1);
    assert_eq!(utilmd[0].0, GAS_PARTNER_MP_ID);
    assert_eq!(utilmd[0].1["pid"], 44024);
    assert_eq!(utilmd[0].1["antwort_code"], "E14");
    assert_eq!(utilmd[0].1["antwort_codeliste"], "G_0003");
    assert_renders_to_its_profile(&state, &utilmd[0]);

    let other = state_for(&["GNB"]).await;
    seed_geli_storno(&other).await;
    dispatch_command(
        &other,
        "geli.stornierung.bestaetigen",
        &serde_json::json!({ "vorgang_id": VORGANG_ID }),
    )
    .await
    .expect("the GNB confirms the 44022");
    let utilmd = outbox(&other, "UTILMD").await;
    assert_eq!(utilmd.len(), 1);
    assert_eq!(utilmd[0].1["pid"], 44023);
    assert_renders_to_its_profile(&other, &utilmd[0]);
}

/// The answer renders to a UTILMD its AHB column accepts — the check the
/// pre-send gate runs — so a 44023/44024 is never an APERAK in disguise nor a
/// message the gate dead-letters (UTILMD AHB Gas 1.2 Kap. 5.2).
fn assert_renders_to_its_profile(
    state: &CommandsApiState,
    (recipient, payload): &(String, serde_json::Value),
) {
    let pending =
        mako_engine::outbox::PendingOutbox::new("UTILMD", recipient.as_str(), payload.clone());
    let findings =
        crate::orchestrator::edifact_renderer::utilmd_ahb_findings(&pending, state.tenant_id)
            .expect("the answer renders");
    assert!(findings.is_empty(), "{}: {findings:#?}", payload["pid"]);
}

/// The answer is the Netzbetreiber's: the LF that sent the 55022 cannot post
/// it, and a Strom NB cannot answer as the GNB.
#[test]
fn only_the_netzbetreiber_answers_a_stornierung() {
    for (command, role) in [
        ("gpke.stornierung.bestaetigen", "LF"),
        ("gpke.stornierung.ablehnen", "LF"),
        ("geli.stornierung.bestaetigen", "LFG"),
        ("geli.stornierung.ablehnen", "NB"),
    ] {
        assert!(
            validate_command(command, None, &roles(&[role])).is_err(),
            "{command} must be refused for {role}"
        );
    }
    assert_eq!(
        validate_command("gpke.stornierung.ablehnen", None, &roles(&["NB"])).unwrap(),
        "NB"
    );
    assert_eq!(
        validate_command("geli.stornierung.bestaetigen", None, &roles(&["GNB"])).unwrap(),
        "GNB"
    );
}

// ── Ersteinbau eines iMS (21029 → 21030/21031) ───────────────────────────────

fn berlin_date_in(days: i64) -> String {
    let d = mako_fristen::heute() + time::Duration::days(days);
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// An IFTSTA 21030/21031 from the wMSB, `LOC+172` the Messlokation.
fn ersteinbau_antwort(pid: u32, code: &str) -> String {
    let status = if pid == 21031 { "Z31" } else { "Z30" };
    format!(
        "UNB+UNOC:3+{PARTNER_MP_ID}:500+{OWN_MP_ID}:500+230101:0000+1'\
UNH+1+IFTSTA:D:18A:UN:2.1'\
BGM+Z03+000{pid}'\
DTM+137:202301010000?+00:303'\
NAD+MS+{PARTNER_MP_ID}::293'\
NAD+MR+{OWN_MP_ID}::293'\
LOC+172+{MELO_ID}'\
STS+Z19+{status}+{code}:E_0233'\
UNT+8+1'\
UNZ+1+1'"
    )
}

/// The gMSB's Vorabinformation command spawns the Vorgang keyed on the
/// Messlokation, queues the IFTSTA 21029 to the wMSB, and the wMSB's inbound
/// 21030 resumes that very process.
#[tokio::test]
async fn the_gmsb_vorabinformation_opens_the_vorgang_the_inbound_21030_resumes() {
    let state = state_for(&["MSB"]).await;
    let body = serde_json::json!({
        "melo_id": MELO_ID,
        "wmsb_mp_id": PARTNER_MP_ID,
        "umstellungszeitpunkt": berlin_date_in(200),
    });
    let DispatchOutcome::Spawned { process_id } =
        dispatch_command(&state, "wim.ersteinbau.vorabinformation-senden", &body)
            .await
            .expect("the gMSB sends its Vorabinformation")
    else {
        panic!("the Vorabinformation opens a process");
    };
    let iftsta = outbox(&state, "IFTSTA").await;
    assert_eq!(iftsta.len(), 1);
    assert_eq!(iftsta[0].0, PARTNER_MP_ID);
    assert_eq!(iftsta[0].1["pid"], 21029);
    assert_eq!(iftsta[0].1["melo"], MELO_ID);

    // A second Vorabinformation on the same Messlokation is refused while the
    // first is unanswered.
    assert!(matches!(
        dispatch_command(&state, "wim.ersteinbau.vorabinformation-senden", &body).await,
        Err(DispatchError::DuplicateProcess { .. })
    ));

    // The gMSB does not answer its own Vorabinformation.
    assert!(
        dispatch_command(
            &state,
            "wim.ersteinbau.beantworten",
            &serde_json::json!({ "melo_id": MELO_ID, "antwortcode": "A03" }),
        )
        .await
        .is_err(),
        "the E_0233 answer is the wMSB's"
    );

    let edi = ersteinbau_antwort(21030, "A03");
    let msg = edi_energy::parse(edi.as_bytes()).expect("parse IFTSTA 21030");
    let outcome = ingest(&state)
        .dispatch(
            &msg,
            mako_wim::ersteinbau::WORKFLOW_NAME,
            21030,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch 21030");
    match outcome {
        IngestOutcome::Dispatched {
            process_id: resumed,
            ..
        } => assert_eq!(resumed, process_id, "the 21030 resumes the gMSB's Vorgang"),
        other => panic!("the 21030 must resume the Vorgang, got {other:?}"),
    }
}

/// A Vorabinformation inside the 3 Monate und 3 WT before the
/// Umstellungszeitpunkt is refused, and nothing is sent.
#[tokio::test]
async fn a_late_vorabinformation_is_refused() {
    let state = state_for(&["MSB"]).await;
    let out = dispatch_command(
        &state,
        "wim.ersteinbau.vorabinformation-senden",
        &serde_json::json!({
            "melo_id": MELO_ID,
            "wmsb_mp_id": PARTNER_MP_ID,
            "umstellungszeitpunkt": berlin_date_in(30),
        }),
    )
    .await;
    assert!(
        matches!(out, Err(DispatchError::InvalidPayload(ref m)) if m.contains("late")),
        "{out:?}"
    );
    assert!(outbox(&state, "IFTSTA").await.is_empty());
}

async fn seed_wmsb_vorgang(state: &CommandsApiState) {
    seed::<mako_wim::ersteinbau::WimErsteinbauWorkflow>(
        state,
        mako_wim::ersteinbau::WORKFLOW_NAME,
        mako_wim::ersteinbau::ErsteinbauCommand::ReceiveVorabinformation {
            pid: Pruefidentifikator::const_new(21029),
            gmsb: MarktpartnerCode::new(PARTNER_MP_ID),
            wmsb: MarktpartnerCode::new(OWN_MP_ID),
            melo_id: MeLo::new(MELO_ID),
            umstellungszeitpunkt: "20270601".to_owned(),
            message_ref: MessageRef::new("VORAB-1"),
            validation_passed: true,
            validation_errors: vec![],
        },
        MELO_ID,
    )
    .await;
}

/// The wMSB's `E_0233` answer: `A03` rides 21030, `A04` rides 21031 — the
/// code's cluster chooses, and the answer goes to the gMSB.
#[tokio::test]
async fn the_wmsb_answers_the_vorabinformation_with_21030_or_21031() {
    for (code, pid) in [("A03", 21030), ("A04", 21031)] {
        let state = state_for(&["MSB"]).await;
        seed_wmsb_vorgang(&state).await;
        let body = serde_json::json!({ "melo_id": MELO_ID, "antwortcode": code });
        dispatch_command(&state, "wim.ersteinbau.beantworten", &body)
            .await
            .unwrap_or_else(|e| panic!("the wMSB answers {code}: {e:?}"));
        let iftsta = outbox(&state, "IFTSTA").await;
        assert_eq!(iftsta.len(), 1);
        assert_eq!(iftsta[0].0, PARTNER_MP_ID, "the answer goes to the gMSB");
        assert_eq!(iftsta[0].1["pid"], pid);
        assert_eq!(iftsta[0].1["antwort_code"], code);
        // Answered; the Vorgang owes nothing further.
        assert!(
            dispatch_command(&state, "wim.ersteinbau.beantworten", &body)
                .await
                .is_err()
        );
    }
}

#[test]
fn only_an_msb_runs_the_ersteinbau() {
    for command in [
        "wim.ersteinbau.vorabinformation-senden",
        "wim.ersteinbau.beantworten",
    ] {
        for role in ["NB", "LF"] {
            assert!(
                validate_command(command, None, &roles(&[role])).is_err(),
                "{command} must be refused for {role}"
            );
        }
        assert_eq!(
            validate_command(command, None, &roles(&["MSB"])).unwrap(),
            "MSB"
        );
    }
}

// ── Preisanfrage (REQOTE 3500n → QUOTES 1500n) ───────────────────────────────

/// A QUOTES Angebot from the MSB, `LOC+172` the location the REQOTE named.
fn quotes(pid: u32) -> String {
    format!(
        "UNB+UNOC:3+{PARTNER_MP_ID}:500+{OWN_MP_ID}:500+230101:0000+1'\
UNH+1+QUOTES:D:10A:UN:1.3c'\
BGM+310+000{pid}'\
DTM+137:202301010000?+00:303'\
RFF+Z13:{pid}'\
NAD+MS+{PARTNER_MP_ID}::293'\
NAD+MR+{OWN_MP_ID}::293'\
LOC+172+{MELO_ID}'\
UNT+8+1'\
UNZ+1+1'"
    )
}

/// The requester's command queues the REQOTE and opens the process the
/// MSB's QUOTES 15001 resumes by the location.
#[tokio::test]
async fn the_requester_sends_a_reqote_the_answering_quotes_resumes() {
    let state = state_for(&["NMSB"]).await;
    let body = serde_json::json!({
        "pid": 35001,
        "empfaenger_mp_id": PARTNER_MP_ID,
        "melo_id": MELO_ID,
    });
    let DispatchOutcome::Spawned { process_id } =
        dispatch_command(&state, "wim.preisanfrage.anfragen", &body)
            .await
            .expect("the MSBN sends its REQOTE")
    else {
        panic!("the REQOTE opens a process");
    };
    let reqote = outbox(&state, "REQOTE").await;
    assert_eq!(reqote.len(), 1);
    assert_eq!(reqote[0].0, PARTNER_MP_ID);
    assert_eq!(reqote[0].1["pid"], 35001);
    assert_eq!(reqote[0].1["location"], MELO_ID);

    assert!(matches!(
        dispatch_command(&state, "wim.preisanfrage.anfragen", &body).await,
        Err(DispatchError::DuplicateProcess { .. })
    ));

    let msg = edi_energy::parse(quotes(15001).as_bytes()).expect("parse QUOTES 15001");
    let outcome = ingest(&state)
        .dispatch(
            &msg,
            mako_wim::PREISANFRAGE_WORKFLOW_NAME,
            15001,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch QUOTES 15001");
    match outcome {
        IngestOutcome::Dispatched {
            process_id: resumed,
            ..
        } => assert_eq!(resumed, process_id),
        other => panic!("the QUOTES must resume the Preisanfrage, got {other:?}"),
    }
}

/// 35003 is the ESA Werteanfrage, not a Preisanfrage; a body without a
/// location, or naming two, is refused.
#[tokio::test]
async fn a_reqote_outside_the_preisanfrage_or_without_one_location_is_refused() {
    let state = state_for(&["NMSB"]).await;
    for bad in [
        serde_json::json!({ "pid": 35003, "empfaenger_mp_id": PARTNER_MP_ID, "melo_id": MELO_ID }),
        serde_json::json!({ "pid": 35001, "empfaenger_mp_id": PARTNER_MP_ID }),
        serde_json::json!({
            "pid": 35002, "empfaenger_mp_id": PARTNER_MP_ID,
            "melo_id": MELO_ID, "malo_id": "51238696012",
        }),
    ] {
        let out = dispatch_command(&state, "wim.preisanfrage.anfragen", &bad).await;
        assert!(
            matches!(out, Err(DispatchError::InvalidPayload(_))),
            "{bad} must be refused, got {out:?}"
        );
    }
    assert!(outbox(&state, "REQOTE").await.is_empty());
}

/// `angebot-senden` leaves a QUOTES in the outbox — under the PID that
/// answers the stored REQOTE PID, to the REQOTE's sender, echoing the location.
#[tokio::test]
async fn angebot_senden_leaves_a_quotes_in_the_outbox() {
    let state = state_for(&["MSB"]).await;
    seed::<WimPreisanfrageWorkflow>(
        &state,
        mako_wim::PREISANFRAGE_WORKFLOW_NAME,
        PreisanfrageCommand::ReceiveReqote {
            pid: Pruefidentifikator::const_new(35001),
            sender: MarktpartnerCode::new(PARTNER_MP_ID),
            receiver: MarktpartnerCode::new(OWN_MP_ID),
            message_ref: MessageRef::new("REQOTE-IN-1"),
            validation_passed: true,
            validation_errors: vec![],
        },
        MELO_ID,
    )
    .await;
    dispatch_command(
        &state,
        "wim.preisanfrage.angebot-senden",
        &serde_json::json!({ "melo_id": MELO_ID }),
    )
    .await
    .expect("the MSB answers the REQOTE");
    let quotes = outbox(&state, "QUOTES").await;
    assert_eq!(quotes.len(), 1, "the Angebot is a QUOTES on the wire");
    assert_eq!(quotes[0].0, PARTNER_MP_ID);
    assert_eq!(quotes[0].1["pid"], 15001);
    assert_eq!(quotes[0].1["sender"], OWN_MP_ID);
    assert_eq!(quotes[0].1["location"], MELO_ID);
}

#[test]
fn the_reqote_is_the_requesters_to_send() {
    for role in ["NMSB", "MSB", "LF", "NB"] {
        assert_eq!(
            validate_command("wim.preisanfrage.anfragen", Some(role), &roles(&[role])).unwrap(),
            role
        );
    }
    assert!(validate_command("wim.preisanfrage.anfragen", Some("BKV"), &roles(&["BKV"])).is_err());
}

// ── Geschäftsdatenanfrage (17132 → 55035/55095/55060 · 19101) ────────────────

/// The pending outbox entries of `message_type`, whole.
async fn outbox_messages(
    state: &CommandsApiState,
    message_type: &str,
) -> Vec<mako_engine::outbox::OutboxMessage> {
    state
        .store
        .pending_now(1_000)
        .await
        .expect("read outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == message_type)
        .collect()
}

/// Render an outbox entry as `own` would send it and require the result to
/// pass its AHB column.
fn render_ahb_valid(msg: &mako_engine::outbox::OutboxMessage, own: &str, role: &str) -> String {
    let registry = crate::core::party_registry::MpIdRegistry::from_config(&[
        crate::core::config::PartyConfig {
            mp_id: own.to_owned(),
            roles: vec![role.to_owned()],
            primary: true,
            agency: None,
            as4: None,
        },
    ])
    .expect("registry");
    let rendered = crate::orchestrator::edifact_renderer::render_to_wire_bytes(msg, &registry)
        .unwrap_or_else(|e| panic!("{} renders: {e}", msg.message_type));
    let wire = String::from_utf8_lossy(&rendered.bytes).into_owned();
    edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
        .expect("validate")
        .into_error_result()
        .unwrap_or_else(|e| panic!("must be AHB-conformant: {e:#?}\n{wire}"));
    wire
}

/// The profile's own skeleton of an Anwendungsfall, from the NB to the partner.
fn gda_skeleton(pid: u32) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("a Prüfidentifikator");
    let profile = edi_energy::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no profile carries PID {pid}"));
    let af = profile.anwendungsfall(pid).expect("the column");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: OWN_MP_ID.to_owned(),
                receiver: PARTNER_MP_ID.to_owned(),
            },
        )
        .expect("skeleton");
    String::from_utf8_lossy(&bytes).into_owned()
}

const GDA_NUMMER: &str = "GDA-4711";

/// The NB's side: an inbound 17132 from the partner, open and owed an answer.
async fn seed_gda_eingang(state: &CommandsApiState) {
    use mako_gpke::geschaeftsdatenanfrage::{GdaAnfrage, GdaCommand, netzbetreiber_key};
    seed::<mako_gpke::GpkeGeschaeftsdatenanfrageWorkflow>(
        state,
        mako_gpke::GDA_WORKFLOW_NAME,
        GdaCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::const_new(17132),
            anfrage: GdaAnfrage {
                anfragender: MarktpartnerCode::new(PARTNER_MP_ID),
                netzbetreiber: MarktpartnerCode::new(OWN_MP_ID),
                lokation_id: "51238696012".to_owned(),
                anfrage_nummer: MessageRef::new(GDA_NUMMER),
            },
            message_ref: MessageRef::new("UNH17132"),
            validation_passed: true,
            validation_errors: vec![],
        },
        &netzbetreiber_key(PARTNER_MP_ID, GDA_NUMMER),
    )
    .await;
}

/// The LF's Anfrage leaves an AHB-valid ORDERS 17132 carrying its
/// Anfrage-Nummer in `BGM` DE 1004, and the NB's UTILMD 55035 that echoes the
/// number in `SG6 RFF+AAV` resumes that very process.
#[tokio::test]
async fn the_anfrage_sends_17132_and_the_inbound_55035_resumes_it() {
    let state = state_for(&["LF"]).await;
    let body = serde_json::json!({
        "lokation_id": "51238696012",
        "nb_mp_id": PARTNER_MP_ID,
        "anfrage_nummer": GDA_NUMMER,
    });
    let DispatchOutcome::Spawned { process_id } =
        dispatch_command(&state, "gpke.geschaeftsdatenanfrage.anfragen", &body)
            .await
            .expect("the LF sends its Anfrage")
    else {
        panic!("the Anfrage opens a process");
    };
    let orders = outbox_messages(&state, "ORDERS").await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].recipient.as_ref(), PARTNER_MP_ID);
    let wire = render_ahb_valid(&orders[0], OWN_MP_ID, "LF");
    assert!(wire.contains(&format!("BGM+Z14+{GDA_NUMMER}'")), "{wire}");
    assert!(wire.contains("RFF+Z13:17132'"), "{wire}");
    assert!(wire.contains("LOC+172+51238696012'"), "{wire}");

    // A second Anfrage under the same number is refused while the first waits.
    assert!(matches!(
        dispatch_command(&state, "gpke.geschaeftsdatenanfrage.anfragen", &body).await,
        Err(DispatchError::DuplicateProcess { .. })
    ));

    let edi = gda_skeleton(55_035)
        .replace("RFF+AAV:REF55035", &format!("RFF+AAV:{GDA_NUMMER}"))
        // The NB answers: every party — UNB and NAD+MS/MR — the other way round.
        .replace(OWN_MP_ID, "\u{0}")
        .replace(PARTNER_MP_ID, OWN_MP_ID)
        .replace('\u{0}', PARTNER_MP_ID);
    assert!(edi.contains(&format!("RFF+AAV:{GDA_NUMMER}")), "{edi}");
    let msg = edi_energy::parse(edi.as_bytes()).expect("parse UTILMD 55035");
    let outcome = ingest(&state)
        .dispatch(
            &msg,
            mako_gpke::GDA_WORKFLOW_NAME,
            55_035,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch 55035");
    match outcome {
        IngestOutcome::Dispatched {
            process_id: resumed,
            ..
        } => assert_eq!(resumed, process_id, "the 55035 resumes the LF's Anfrage"),
        other => panic!("the 55035 must resume the Anfrage, got {other:?}"),
    }
}

/// Without a number the command issues one that fits `BGM` DE 1004.
#[tokio::test]
async fn the_anfrage_issues_its_own_number() {
    let state = state_for(&["MSB"]).await;
    let body = serde_json::json!({
        "lokation_id": MELO_ID,
        "nb_mp_id": PARTNER_MP_ID,
    });
    dispatch_command(&state, "gpke.geschaeftsdatenanfrage.anfragen", &body)
        .await
        .expect("the MSB sends its Anfrage");
    let orders = outbox(&state, "ORDERS").await;
    let nummer = orders[0].1["document_id"].as_str().expect("a number");
    assert!(nummer.starts_with("GDA") && nummer.len() == 35, "{nummer}");
    assert_eq!(orders[0].1["location"], MELO_ID);
}

/// The NB's answer is built from marktd's record, never from the caller: a
/// body carrying Stammdaten is refused as an unknown key, and without a
/// readable marktd the answer is refused `503`, nothing is sent and the
/// Anfrage stays open — its 1-WT window keeps running. The answer from a
/// marktd that holds the bündel is `tests/it/e2e_gpke_gda_marktd.rs`.
#[tokio::test]
async fn the_nb_answer_reads_marktd_and_refuses_without_it() {
    for pid in [55_035, 55_095, 55_060] {
        let state = state_for(&["NB"]).await;
        seed_gda_eingang(&state).await;
        let mut body = serde_json::json!({
            "anfragender_mp_id": PARTNER_MP_ID,
            "anfrage_nummer": GDA_NUMMER,
            "antwort_pid": pid,
        });
        assert!(
            matches!(
                dispatch_command(&state, "gpke.geschaeftsdatenanfrage.beantworten", &body).await,
                Err(DispatchError::MarktdUnavailable(_))
            ),
            "{pid}: without marktd there is no answer"
        );
        body["stammdaten"] = serde_json::json!({ "lokationen": [], "bloecke": [] });
        assert!(
            matches!(
                dispatch_command(&state, "gpke.geschaeftsdatenanfrage.beantworten", &body).await,
                Err(DispatchError::InvalidPayload(_))
            ),
            "{pid}: the caller's Stammdaten are not a field"
        );
        assert!(outbox(&state, "UTILMD").await.is_empty(), "{pid}");
        let current = super::current_state::<mako_gpke::GpkeGeschaeftsdatenanfrageWorkflow>(
            &state,
            &mako_gpke::geschaeftsdatenanfrage::netzbetreiber_key(PARTNER_MP_ID, GDA_NUMMER),
            mako_gpke::GDA_WORKFLOW_NAME,
        )
        .await
        .expect("state");
        assert!(
            matches!(current, mako_gpke::GdaState::Offen(_)),
            "{pid}: the Anfrage stays open, got {current:?}"
        );
    }
}

/// An answer that is no UTILMD answer to a 17132 is refused and nothing is
/// sent.
#[tokio::test]
async fn an_answer_outside_the_column_is_refused() {
    let state = state_for(&["NB"]).await;
    seed_gda_eingang(&state).await;
    let body = serde_json::json!({
        "anfragender_mp_id": PARTNER_MP_ID,
        "anfrage_nummer": GDA_NUMMER,
        "antwort_pid": 55_036,
    });
    assert!(matches!(
        dispatch_command(&state, "gpke.geschaeftsdatenanfrage.beantworten", &body).await,
        Err(DispatchError::InvalidPayload(_))
    ));
    assert!(outbox(&state, "UTILMD").await.is_empty());
}

/// The NB's Ablehnung leaves an AHB-valid ORDRSP 19101 naming the ORDERS by
/// `SG1 RFF+ON` and the code with its EBD in `SG2 AJT`.
#[tokio::test]
async fn the_nb_refuses_with_an_ahb_valid_ordrsp_19101() {
    for (anfragender, code, ebd) in [("LF", "A01", "E_0441"), ("MSB", "A02", "E_0443")] {
        let state = state_for(&["NB"]).await;
        seed_gda_eingang(&state).await;
        let body = serde_json::json!({
            "anfragender_mp_id": PARTNER_MP_ID,
            "anfrage_nummer": GDA_NUMMER,
            "anfragender": anfragender,
            "antwortcode": code,
        });
        dispatch_command(&state, "gpke.geschaeftsdatenanfrage.ablehnen", &body)
            .await
            .unwrap_or_else(|e| panic!("the NB refuses ({anfragender}): {e:?}"));
        let ordrsp = outbox_messages(&state, "ORDRSP").await;
        assert_eq!(ordrsp.len(), 1);
        let wire = render_ahb_valid(&ordrsp[0], OWN_MP_ID, "NB");
        assert!(wire.contains("BGM+Z14+"), "{wire}");
        assert!(wire.contains(&format!("RFF+ON:{GDA_NUMMER}'")), "{wire}");
        assert!(wire.contains("RFF+Z13:19101'"), "{wire}");
        assert!(wire.contains(&format!("AJT+{code}+{ebd}'")), "{wire}");
    }
    // `A02` „Kein iMS verbaut" is not a ground of `E_0441`.
    let state = state_for(&["NB"]).await;
    seed_gda_eingang(&state).await;
    let body = serde_json::json!({
        "anfragender_mp_id": PARTNER_MP_ID,
        "anfrage_nummer": GDA_NUMMER,
        "anfragender": "LF",
        "antwortcode": "A02",
    });
    assert!(
        dispatch_command(&state, "gpke.geschaeftsdatenanfrage.ablehnen", &body)
            .await
            .is_err()
    );
}

/// The LF or MSB asks and only the NB answers; the Anfrage, open to both,
/// states its `marktrolle`.
#[test]
fn the_geschaeftsdatenanfrage_runs_on_its_roles() {
    for role in ["NB", "ESA"] {
        assert!(
            validate_command(
                "gpke.geschaeftsdatenanfrage.anfragen",
                None,
                &roles(&[role])
            )
            .is_err(),
            "the Anfrage must be refused for {role}"
        );
    }
    // Open to two roles, so the Anfrage always states which one asks.
    for role in ["LF", "MSB"] {
        assert_eq!(
            validate_command(
                "gpke.geschaeftsdatenanfrage.anfragen",
                Some(role),
                &roles(&[role])
            )
            .unwrap(),
            role
        );
        assert!(
            validate_command(
                "gpke.geschaeftsdatenanfrage.anfragen",
                None,
                &roles(&[role])
            )
            .is_err(),
            "{role} must state the marktrolle"
        );
    }
    for command in [
        "gpke.geschaeftsdatenanfrage.beantworten",
        "gpke.geschaeftsdatenanfrage.ablehnen",
    ] {
        for role in ["LF", "MSB"] {
            assert!(
                validate_command(command, None, &roles(&[role])).is_err(),
                "{command} must be refused for {role}"
            );
        }
        assert_eq!(
            validate_command(command, None, &roles(&["NB"])).unwrap(),
            "NB"
        );
    }
}
