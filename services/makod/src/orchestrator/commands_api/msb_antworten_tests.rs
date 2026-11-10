//! The MSB's answers to the ORDERS it receives, posted as commands.
//!
//! Each test seeds the received ORDERS as the ingest arm records it, posts the
//! answer command and reads the outbox: the door renders the answer and holds
//! it to its AHB profile before it writes, so an answer that reaches the outbox
//! is one the pre-send gate passes.

use std::sync::Arc;

use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;

use super::*;

const MSB_MP_ID: &str = "9900555000005";
const NB_MP_ID: &str = "9900357000004";
const MALO_ID: &str = "51238696012";

async fn state_for(own: &str, roles: &[&str]) -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    CommandsApiState {
        tenant_id: TenantId::from_party_id(own),
        sender_party_id: own.to_owned(),
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

// ── Anfrage Sperrung Strom (17116 → 19118 / 19119) ────────────────────────────

async fn seed_anfrage_sperrung_strom(state: &CommandsApiState) {
    use mako_gpke::sperrung_msb::{
        AnfrageSperrung, GpkeSperrungMsbWorkflow, SperrungMsbCommand, WORKFLOW_NAME,
    };
    seed::<GpkeSperrungMsbWorkflow>(
        state,
        WORKFLOW_NAME,
        SperrungMsbCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::const_new(17116),
            anfrage: AnfrageSperrung {
                netzbetreiber: MarktpartnerCode::new(NB_MP_ID),
                msb: MarktpartnerCode::new(MSB_MP_ID),
                malo: MaLo::new(MALO_ID),
                dokumentennummer: "SPERR-4711".to_owned(),
                message_ref: MessageRef::new("UNH-17116"),
            },
            validation_passed: true,
            validation_errors: vec![],
        },
        MALO_ID,
    )
    .await;
}

/// A Zustimmung of `E_0488` leaves an ORDRSP 19118 that passes its profile and
/// ends the MSB's process before its 3-WT window.
#[tokio::test]
async fn the_msb_answers_an_anfrage_sperrung_with_19118() {
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    seed_anfrage_sperrung_strom(&state).await;
    dispatch_command(
        &state,
        mako_markt::commands::GPKE_ANFRAGE_SPERRUNG_BEANTWORTEN,
        &serde_json::json!({ "malo_id": MALO_ID, "antwortcode": "A05" }),
    )
    .await
    .expect("the MSB answers the 17116");
    let ordrsp = outbox(&state, "ORDRSP").await;
    assert_eq!(ordrsp.len(), 1, "{ordrsp:?}");
    let (recipient, p) = &ordrsp[0];
    assert_eq!(recipient, NB_MP_ID);
    assert_eq!(p["pid"], 19_118);
    assert_eq!(p["order_reference"], "SPERR-4711");
    assert_eq!(p["antwort_codeliste"], "E_0488");
    let current = current_state::<mako_gpke::GpkeSperrungMsbWorkflow>(
        &state,
        MALO_ID,
        mako_gpke::sperrung_msb::WORKFLOW_NAME,
    )
    .await
    .expect("the process");
    assert!(current.is_terminal(), "{current:?}");
}

/// An Ablehnung rides 19119 with the problem named; without it the door
/// refuses and writes nothing.
#[tokio::test]
async fn an_ablehnung_names_the_problem_or_is_refused() {
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    seed_anfrage_sperrung_strom(&state).await;
    let ohne = dispatch_command(
        &state,
        mako_markt::commands::GPKE_ANFRAGE_SPERRUNG_BEANTWORTEN,
        &serde_json::json!({ "malo_id": MALO_ID, "antwortcode": "A03" }),
    )
    .await;
    assert!(ohne.is_err(), "{ohne:?}");
    assert!(outbox(&state, "ORDRSP").await.is_empty());
    dispatch_command(
        &state,
        mako_markt::commands::GPKE_ANFRAGE_SPERRUNG_BEANTWORTEN,
        &serde_json::json!({
            "malo_id": MALO_ID,
            "antwortcode": "A03",
            "freitext": "Zähler nicht zugänglich",
        }),
    )
    .await
    .expect("the MSB refuses the 17116");
    let ordrsp = outbox(&state, "ORDRSP").await;
    assert_eq!(ordrsp[0].1["pid"], 19_119);
}

/// A body field the door does not declare is refused.
#[tokio::test]
async fn an_unknown_field_is_refused() {
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    seed_anfrage_sperrung_strom(&state).await;
    let err = dispatch_command(
        &state,
        mako_markt::commands::GPKE_ANFRAGE_SPERRUNG_BEANTWORTEN,
        &serde_json::json!({ "malo_id": MALO_ID, "antwortcode": "A05", "pid": 19118 }),
    )
    .await;
    assert!(err.is_err());
}

// ── Anfrage Sperrung Gas (17116 → 19118 / 19119, `E_1001`) ────────────────────

const GMSB_MP_ID: &str = "9807317000007";
const GNB_MP_ID: &str = "9870012345678";

/// The gMSB's answer names `E_1001` and passes the Gas column of ORDRSP AHB
/// 1.1b Kap. 4.3.1 (`[493]`).
#[tokio::test]
async fn the_gmsb_answers_an_anfrage_sperrung_out_of_e_1001() {
    use mako_geli_gas::sperrung_msb::{
        AnfrageSperrungGas, GeliGasSperrungMsbWorkflow, SperrungMsbGasCommand, WORKFLOW_NAME,
    };
    let state = state_for(GMSB_MP_ID, &["MSB"]).await;
    seed::<GeliGasSperrungMsbWorkflow>(
        &state,
        WORKFLOW_NAME,
        SperrungMsbGasCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::const_new(17116),
            anfrage: AnfrageSperrungGas {
                netzbetreiber: MarktpartnerCode::new(GNB_MP_ID),
                msb: MarktpartnerCode::new(GMSB_MP_ID),
                malo: MaLo::new(MALO_ID),
                dokumentennummer: "SPERR-GAS-1".to_owned(),
                message_ref: MessageRef::new("UNH-17116"),
            },
            validation_passed: true,
            validation_errors: vec![],
        },
        MALO_ID,
    )
    .await;
    dispatch_command(
        &state,
        mako_markt::commands::GELI_ANFRAGE_SPERRUNG_BEANTWORTEN,
        &serde_json::json!({
            "malo_id": MALO_ID,
            "antwortcode": "A02",
            "freitext": "Verhinderungsgrund an der Marktlokation angezeigt",
        }),
    )
    .await
    .expect("the gMSB refuses the 17116");
    let ordrsp = outbox(&state, "ORDRSP").await;
    assert_eq!(ordrsp.len(), 1, "{ordrsp:?}");
    assert_eq!(ordrsp[0].0, GNB_MP_ID);
    assert_eq!(ordrsp[0].1["pid"], 19_119);
    assert_eq!(ordrsp[0].1["antwort_codeliste"], "E_1001");
}

// ── Änderung der Technik (17011 → 19005 / 19006) ─────────────────────────────

const MELO_ID: &str = "DE00056266802AO6G56M11SN51G21M24S";

async fn seed_technik_auftrag(state: &CommandsApiState) {
    use mako_wim::technik_aenderung::{
        AuftragKopf, BGM_DIREKTE_BEAUFTRAGUNG, TechnikAenderungCommand, WORKFLOW_NAME,
        WimTechnikAenderungWorkflow, auftrag_key,
    };
    seed::<WimTechnikAenderungWorkflow>(
        state,
        WORKFLOW_NAME,
        TechnikAenderungCommand::ReceiveAuftrag {
            orders_pid: Pruefidentifikator::const_new(17011),
            sender: MarktpartnerCode::new(NB_MP_ID),
            location_id: Some(MELO_ID.to_owned()),
            message_ref: MessageRef::new("UNH-17011"),
            kopf: AuftragKopf {
                empfaenger: MSB_MP_ID.to_owned(),
                dokumentennummer: "TECH-4711".to_owned(),
                beauftragung: BGM_DIREKTE_BEAUFTRAGUNG.to_owned(),
                ausfuehrungsdatum: Some("202612312300".to_owned()),
            },
            validation_passed: true,
            validation_errors: vec![],
        },
        &auftrag_key(NB_MP_ID, "TECH-4711"),
    )
    .await;
}

fn technik_antwort(besteller: &str, code: &str) -> serde_json::Value {
    serde_json::json!({
        "absender_mp_id": NB_MP_ID,
        "dokumentennummer": "TECH-4711",
        "besteller": besteller,
        "antwortcode": code,
        "ansprechpartner": {
            "name": "Technik-Hotline",
            "kommunikation": [{ "art": "telefon", "adresse": "+4922112345" }],
        },
    })
}

/// An answered `AuftragEingegangen` reaches its end state: the MSB's 19005
/// out of `E_0249` passes its profile and closes the process.
#[tokio::test]
async fn an_answered_technik_auftrag_reaches_its_end_state() {
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    seed_technik_auftrag(&state).await;
    dispatch_command(
        &state,
        mako_markt::commands::WIM_TECHNIK_AENDERUNG_BEANTWORTEN,
        &technik_antwort("NB", "A02"),
    )
    .await
    .expect("the MSB confirms the 17011");
    let ordrsp = outbox(&state, "ORDRSP").await;
    assert_eq!(ordrsp.len(), 1, "{ordrsp:?}");
    assert_eq!(ordrsp[0].0, NB_MP_ID);
    assert_eq!(ordrsp[0].1["pid"], 19_005);
    assert_eq!(ordrsp[0].1["antwort_codeliste"], "E_0249");
    let current = current_state::<mako_wim::technik_aenderung::WimTechnikAenderungWorkflow>(
        &state,
        &mako_wim::technik_aenderung::auftrag_key(NB_MP_ID, "TECH-4711"),
        mako_wim::technik_aenderung::WORKFLOW_NAME,
    )
    .await
    .expect("the process");
    assert!(current.is_terminal(), "{current:?}");
}

/// The Frist Ablehnung `A01` of `E_0249` rides 19006; a code of the LF's tree
/// on an NB's Auftrag is refused and writes nothing.
#[tokio::test]
async fn a_technik_ablehnung_comes_from_the_besteller_s_tree() {
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    seed_technik_auftrag(&state).await;
    let fremd = dispatch_command(
        &state,
        mako_markt::commands::WIM_TECHNIK_AENDERUNG_BEANTWORTEN,
        &technik_antwort("NB", "A03"),
    )
    .await;
    assert!(fremd.is_err(), "{fremd:?}");
    assert!(outbox(&state, "ORDRSP").await.is_empty());
    dispatch_command(
        &state,
        mako_markt::commands::WIM_TECHNIK_AENDERUNG_BEANTWORTEN,
        &technik_antwort("NB", "A01"),
    )
    .await
    .expect("the MSB refuses the 17011");
    assert_eq!(outbox(&state, "ORDRSP").await[0].1["pid"], 19_006);
}

// ── The one pre-send gate at the door ────────────────────────────────────────

/// The door's gate renders every EDIFACT type, not only UTILMD: an ORDRSP and
/// an ORDERS whose rendering violates its profile are refused before anything
/// is written, while a conformant ORDRSP passes.
#[tokio::test]
async fn an_ordrsp_and_an_orders_with_a_profile_defect_are_refused_at_the_door() {
    use mako_engine::outbox::PendingOutbox;
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    let fv = latest_format_version();
    let ordrsp = |document_code: &str| {
        PendingOutbox::new(
            "ORDRSP",
            NB_MP_ID,
            serde_json::json!({
                "pid": 19_118,
                "sender": MSB_MP_ID,
                "receiver": NB_MP_ID,
                "message_ref": "ORDRSP1",
                "document_code": document_code,
                "document_id": "ORDRSP1",
                "order_reference": "SPERR-4711",
                "antwort_code": "A05",
                "antwort_codeliste": "E_0488",
            }),
        )
    };
    super::eog_stammdaten::gate_verdict(&state, &fv, &[ordrsp("Z51")])
        .expect("a conformant 19118 passes");
    // `BGM` DE 1001 `Z52` is not admitted on 19118 (ORDRSP AHB 1.1b Kap. 4.3.1).
    let refused = super::eog_stammdaten::gate_verdict(&state, &fv, &[ordrsp("Z52")]);
    assert!(
        matches!(&refused, Err(DispatchError::InvalidPayload(m)) if m.contains("ORDRSP")),
        "{refused:?}"
    );

    // An Anfrage Sperrung 17116 without its Marktlokation (`SG2 LOC+172` Muss).
    let orders = PendingOutbox::new(
        "ORDERS",
        MSB_MP_ID,
        serde_json::json!({ "pid": 17_116, "sender": NB_MP_ID }),
    );
    let refused = super::eog_stammdaten::gate_verdict(&state, &fv, &[orders]);
    assert!(
        matches!(&refused, Err(DispatchError::InvalidPayload(m)) if m.contains("ORDERS")),
        "{refused:?}"
    );
}

// ── Datenabruf Ablehnungen (19102 / 19007 / 19114) ───────────────────────────

async fn seed_werte_anfrage(state: &CommandsApiState, pid: u32, bgm: &str, leistung: &str) {
    use mako_gpke::datenabruf_msb::{
        DatenabrufMsbCommand, GpkeDatenabrufMsbWorkflow, WORKFLOW_NAME, WerteAnfrage, anfrage_key,
    };
    seed::<GpkeDatenabrufMsbWorkflow>(
        state,
        WORKFLOW_NAME,
        DatenabrufMsbCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::new(pid).expect("pid"),
            anfrage: WerteAnfrage {
                anfragender: MarktpartnerCode::new(NB_MP_ID),
                empfaenger: MarktpartnerCode::new(MSB_MP_ID),
                bgm: bgm.to_owned(),
                leistung: Some(leistung.to_owned()),
                dokumentennummer: format!("DOK{pid}"),
                lokation_id: MELO_ID.to_owned(),
                message_ref: MessageRef::new("UNH-W"),
            },
            validation_passed: true,
            validation_errors: vec![],
        },
        &anfrage_key(NB_MP_ID, &format!("DOK{pid}")),
    )
    .await;
}

/// Each Ablehnung leaves its ORDRSP — 19007 under `S_0073` to a NB, 19114 with
/// its Ansprechpartner, 19102 out of `E_0442` — passing its profile, and the
/// MSB's process ends.
#[tokio::test]
async fn an_answered_werte_anfrage_reaches_its_end_state() {
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    seed_werte_anfrage(&state, 17_004, "7", "Z13").await;
    seed_werte_anfrage(&state, 17_113, "Z34", "Z11").await;
    seed_werte_anfrage(&state, 17_102, "7", "Z12").await;
    let ablehnen = |pid: u32, tree: &str, code: &str| {
        let mut body = serde_json::json!({
            "anfragender_mp_id": NB_MP_ID,
            "dokumentennummer": format!("DOK{pid}"),
            "codeliste": tree,
            "antwortcode": code,
        });
        if pid == 17_113 {
            body["ansprechpartner"] = serde_json::json!({
                "name": "Messwesen",
                "kommunikation": [{ "art": "email", "adresse": "messwesen@example.de" }],
            });
        }
        body
    };
    for (pid, tree, code) in [
        (17_004, "E_0219", "Z19"),
        (17_113, "E_0226", "Z54"),
        (17_102, "E_0442", "A04"),
    ] {
        dispatch_command(
            &state,
            mako_markt::commands::GPKE_DATENABRUF_ABLEHNEN,
            &ablehnen(pid, tree, code),
        )
        .await
        .unwrap_or_else(|e| panic!("{pid}: {e:?}"));
        let current = current_state::<mako_gpke::GpkeDatenabrufMsbWorkflow>(
            &state,
            &mako_gpke::datenabruf_msb::anfrage_key(NB_MP_ID, &format!("DOK{pid}")),
            mako_gpke::datenabruf_msb::WORKFLOW_NAME,
        )
        .await
        .expect("the process");
        assert!(current.is_terminal(), "{pid}: {current:?}");
    }
    let mut pids: Vec<u64> = outbox(&state, "ORDRSP")
        .await
        .iter()
        .filter_map(|(_, p)| p["pid"].as_u64())
        .collect();
    pids.sort_unstable();
    assert_eq!(pids, vec![19_007, 19_102, 19_114]);
}

// ── GPKE Teil 3 Konfiguration — the recipient's answer ───────────────────────

async fn seed_konfiguration_auftrag(state: &CommandsApiState, pid: u32, bgm: &str) {
    use mako_gpke::konfiguration_aenderung::{EingangAuftrag, auftrag_key};
    use mako_gpke::{
        GpkeKonfigurationAenderungWorkflow, KONFIGURATION_AENDERUNG_WORKFLOW_NAME,
        KonfigurationAenderungCommand,
    };
    seed::<GpkeKonfigurationAenderungWorkflow>(
        state,
        KONFIGURATION_AENDERUNG_WORKFLOW_NAME,
        KonfigurationAenderungCommand::ReceiveAuftrag {
            pid: Pruefidentifikator::new(pid).expect("pid"),
            auftrag: EingangAuftrag {
                absender: MarktpartnerCode::new(NB_MP_ID),
                empfaenger: MarktpartnerCode::new(MSB_MP_ID),
                lokation: Some(MELO_ID.to_owned()),
                dokumentennummer: format!("KONF{pid}"),
                bgm: bgm.to_owned(),
                message_ref: MessageRef::new("UNH-K"),
            },
            validation_passed: true,
            validation_errors: vec![],
        },
        &auftrag_key(NB_MP_ID, &format!("KONF{pid}")),
    )
    .await;
}

/// A received 17130 is answered with a 19132 out of `E_0526` and a 17121 with
/// a 19120; each passes its profile and ends the recipient's process.
#[tokio::test]
async fn the_recipient_answers_a_konfiguration_orders() {
    let state = state_for(MSB_MP_ID, &["MSB"]).await;
    seed_konfiguration_auftrag(&state, 17_130, "Z73").await;
    seed_konfiguration_auftrag(&state, 17_121, "Z68").await;
    for (pid, code) in [(17_130, "A07"), (17_121, "A05")] {
        dispatch_command(
            &state,
            mako_markt::commands::GPKE_KONFIGURATION_BEANTWORTEN,
            &serde_json::json!({
                "absender_mp_id": NB_MP_ID,
                "dokumentennummer": format!("KONF{pid}"),
                "codeliste": "E_0526",
                "antwortcode": code,
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("{pid}: {e:?}"));
    }
    let mut pids: Vec<u64> = outbox(&state, "ORDRSP")
        .await
        .iter()
        .filter_map(|(_, p)| p["pid"].as_u64())
        .collect();
    pids.sort_unstable();
    assert_eq!(pids, vec![19_120, 19_132]);
}

// ── The ORDERS columns the requesting doors send ─────────────────────────────

/// 17001, 17009 and 17004 render their AHB column (ORDERS AHB 1.1b Kap. 4.9.2,
/// 4.10, 4.13) and pass the gate.
#[tokio::test]
async fn the_requesting_orders_render_their_columns() {
    use mako_engine::outbox::PendingOutbox;
    let state = state_for(NB_MP_ID, &["NB", "MSB"]).await;
    let fv = latest_format_version();
    let cases = [
        serde_json::json!({
            "pid": 17_001,
            "sender": NB_MP_ID,
            "document_id": "BEST-1",
            "angebotsnummer": "QUO-1",
            "ausfuehrungsdatum": "2026-12-01",
            "sparte": "strom",
            "ansprechpartner": {
                "name": "Technik",
                "kommunikation": [{ "art": "telefon", "adresse": "+4922112345" }],
            },
            "positionen": ["1", "2"],
        }),
        serde_json::json!({
            "pid": 17_009,
            "sender": NB_MP_ID,
            "document_id": "GWA-1",
            "location": MELO_ID,
            "ausfuehrungsdatum": "2026-12-01",
            "sparte": "strom",
            "umfang": "Z58",
            "geraete": [{ "geraeteart": "ZD4", "geraetenummern": ["1ESY1161234567"] }],
        }),
        serde_json::json!({
            "pid": 17_004,
            "sender": NB_MP_ID,
            "document_id": "DOK-1",
            "location": MELO_ID,
            "grund": "Z49",
            "wertzeitpunkt": "2026-11-01",
        }),
    ];
    for payload in cases {
        let pid = payload["pid"].clone();
        let pending = PendingOutbox::new("ORDERS", MSB_MP_ID, payload);
        super::eog_stammdaten::gate_verdict(&state, &fv, &[pending])
            .unwrap_or_else(|e| panic!("{pid}: {e:?}"));
    }
}

/// A command-sent 17004 leaves an ORDERS whose `BGM` DE 1004 is the
/// Dokumentennummer, and the MSB's 19007 echoing it in `SG1 RFF+ON` resumes
/// the process and ends it Abgelehnt.
#[tokio::test]
async fn a_command_sent_17004_is_resumed_by_its_19007() {
    use crate::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
    let state = state_for(NB_MP_ID, &["NB"]).await;
    dispatch_command(
        &state,
        mako_markt::commands::GPKE_DATENABRUF_ANFORDERN,
        &serde_json::json!({
            "msb_mp_id": MSB_MP_ID,
            "lokation_id": MELO_ID,
            "grund": "Z49",
            "wertzeitpunkt": "2026-11-01",
            "dokumentennummer": "WA-4711",
        }),
    )
    .await
    .expect("the Anforderung is sent");
    let orders = outbox(&state, "ORDERS").await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].1["document_id"], "WA-4711");

    let wire = {
        use edi_energy::profile::SkeletonParties;
        let code = edi_energy::Pruefidentifikator::new(19_007).expect("pid");
        let profile = edi_energy::ReleaseRegistry::global()
            .all_profiles()
            .iter()
            .filter(|p| p.has_anwendungsfall(code))
            .max_by_key(|p| p.valid_from())
            .expect("a 19007 profile");
        let af = profile.anwendungsfall(19_007).expect("column");
        let bytes = profile
            .skeleton_interchange(
                af,
                &SkeletonParties {
                    sender: MSB_MP_ID.to_owned(),
                    receiver: NB_MP_ID.to_owned(),
                },
            )
            .expect("skeleton");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let at = wire.find("RFF+ON:").expect("RFF+ON") + "RFF+ON:".len();
    let end = at + wire[at..].find(['\'', ':', '+']).expect("terminated");
    let wire = format!("{}WA-4711{}", &wire[..at], &wire[end..]);
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::clone(&state.store),
        state.snapshot_store.clone(),
        100,
        state.tenant_id,
    );
    let outcome = dispatcher
        .dispatch(
            &msg,
            mako_gpke::DATENABRUF_WORKFLOW_NAME,
            19_007,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "{outcome:?}"
    );
    let current = current_state::<mako_gpke::GpkeDatenabrufWorkflow>(
        &state,
        "WA-4711",
        mako_gpke::DATENABRUF_WORKFLOW_NAME,
    )
    .await
    .expect("the process");
    assert!(current.is_terminal(), "{current:?}");
}

/// The newest profile's skeleton of `pid` from `sender` to `receiver`, with
/// the first `RFF+ON` set to `order_ref`.
fn skeleton_answering(pid: u32, sender: &str, receiver: &str, order_ref: &str) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("pid");
    let profile = edi_energy::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a profile");
    let af = profile.anwendungsfall(pid).expect("column");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: sender.to_owned(),
                receiver: receiver.to_owned(),
            },
        )
        .expect("skeleton");
    let wire = String::from_utf8_lossy(&bytes).into_owned();
    let at = wire.find("RFF+ON:").expect("RFF+ON") + "RFF+ON:".len();
    let end = at + wire[at..].find(['\'', ':', '+']).expect("terminated");
    format!("{}{order_ref}{}", &wire[..at], &wire[end..])
}

/// The MSBN's Gerätewechselabsicht leaves an ORDERS 17009 that passes its
/// profile, and the MSBA's 19016 echoing its `BGM` DE 1004 resumes the
/// process and ends it Abgelehnt.
#[tokio::test]
async fn the_msbn_sends_a_17009_and_a_19016_ends_it_abgelehnt() {
    use crate::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
    use mako_wim::geraeteubernahme_msbn::{
        MsbnState, WORKFLOW_NAME, WimGeraeteubernahmeMsbnWorkflow,
    };
    let state = state_for(NB_MP_ID, &["MSB"]).await;
    dispatch_command(
        &state,
        mako_markt::commands::WIM_GERAETEWECHSELABSICHT_ANZEIGEN,
        &serde_json::json!({
            "msba_mp_id": MSB_MP_ID,
            "melo_id": MELO_ID,
            "geraetewechseltermin": "2026-12-01",
            "umfang": "Z58",
            "geraete": [{ "geraeteart": "ZD4", "geraetenummern": ["1ESY1161234567"] }],
            "sparte": "strom",
            "dokumentennummer": "GWA-4711",
        }),
    )
    .await
    .expect("the 17009 is sent");
    let orders = outbox(&state, "ORDERS").await;
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].1["pid"], 17_009);

    let msg =
        edi_energy::parse(skeleton_answering(19_016, MSB_MP_ID, NB_MP_ID, "GWA-4711").as_bytes())
            .expect("parses");
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::clone(&state.store),
        state.snapshot_store.clone(),
        100,
        state.tenant_id,
    );
    let outcome = dispatcher
        .dispatch(&msg, WORKFLOW_NAME, 19_016, time::OffsetDateTime::now_utc())
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "{outcome:?}"
    );
    let current =
        current_state::<WimGeraeteubernahmeMsbnWorkflow>(&state, "GWA-4711", WORKFLOW_NAME)
            .await
            .expect("the process");
    assert!(
        matches!(current, MsbnState::Abgelehnt { .. }),
        "{current:?}"
    );
}

/// The MSBN's Bestellung leaves an ORDERS 17001 and a 19001 confirms it.
#[tokio::test]
async fn the_msbn_orders_with_17001_and_a_19001_confirms_it() {
    use crate::ingest_dispatcher::EdifactIngestDispatcher;
    use mako_wim::geraeteubernahme_msbn::{
        MsbnState, WORKFLOW_NAME, WimGeraeteubernahmeMsbnWorkflow,
    };
    let state = state_for(NB_MP_ID, &["MSB"]).await;
    dispatch_command(
        &state,
        mako_markt::commands::WIM_GERAETEUEBERNAHME_BESTELLEN,
        &serde_json::json!({
            "msba_mp_id": MSB_MP_ID,
            "angebotsnummer": "QUO-1",
            "ausfuehrungsdatum": "2026-12-01",
            "ansprechpartner": {
                "name": "Technik",
                "kommunikation": [{ "art": "telefon", "adresse": "+4922112345" }],
            },
            "positionen": ["1"],
            "sparte": "strom",
            "dokumentennummer": "GUE-4711",
        }),
    )
    .await
    .expect("the 17001 is sent");
    let msg =
        edi_energy::parse(skeleton_answering(19_001, MSB_MP_ID, NB_MP_ID, "GUE-4711").as_bytes())
            .expect("parses");
    EdifactIngestDispatcher::new(
        Arc::clone(&state.store),
        state.snapshot_store.clone(),
        100,
        state.tenant_id,
    )
    .dispatch(&msg, WORKFLOW_NAME, 19_001, time::OffsetDateTime::now_utc())
    .await
    .expect("dispatch");
    let current =
        current_state::<WimGeraeteubernahmeMsbnWorkflow>(&state, "GUE-4711", WORKFLOW_NAME)
            .await
            .expect("the process");
    assert!(
        matches!(current, MsbnState::Bestaetigt { .. }),
        "{current:?}"
    );
}
