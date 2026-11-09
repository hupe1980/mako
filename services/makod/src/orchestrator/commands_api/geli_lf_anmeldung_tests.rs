//! The GeLi Gas Anmeldung NN 44001 goes out as a message its AHB column
//! accepts (UTILMD AHB Gas 1.2 Kap. 5.6, S. 43–66).
//!
//! The column makes the Lieferant's contract facts Muss — Identifikationslogik,
//! Abrechnungsintervall, Gruppenzuordnung, Netznutzung, Gemeinderabatt, the
//! Kunde, his Korrespondenzanschrift and the Ablesekarte. A 44001 without them
//! is a message the pre-send gate dead-letters, so `geli.lieferbeginn.anmelden`
//! takes every one of them and refuses the command that lacks one.

use std::sync::Arc;

use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::workflow::Workflow as _;

use super::*;

const LF_MP_ID: &str = "9805550000003";
const GNB_MP_ID: &str = "9800357000005";
const MALO_ID: &str = "51238696012";

async fn lfg_state() -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let state = CommandsApiState {
        tenant_id: TenantId::from_party_id(LF_MP_ID),
        sender_party_id: LF_MP_ID.to_owned(),
        configured_marktrollen: vec!["LFG".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(CedarAuthorizer::unauthenticated().expect("infallible")),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
        marktd_client: None,
    };
    let record: energy_api::models::electricity::MaloIdentResultPositive =
        serde_json::from_value(serde_json::json!({
            "dataMarketLocation": {
                "maloId": MALO_ID,
                "energyDirection": "consumption",
                "measurementTechnologyClassification": "conventionalMeasuringSystem",
                "optionalChangeForecastBasis": "notPossible",
                "dataMarketLocationProperties": [],
                "dataMarketLocationNetworkOperators": [
                    { "marketPartnerId": GNB_MP_ID, "executionTimeFrom": "2000-01-01T00:00:00Z" }
                ],
                "dataMarketLocationTransmissionSystemOperators": []
            }
        }))
        .expect("MaLo fixture matches the energy-api schema");
    state
        .malo_cache
        .upsert(&state.tenant_id.to_string(), &record)
        .await
        .expect("seed MaLo cache");
    state
}

/// A 44001 body as a Lieferant's full contract states it.
fn full_body() -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO_ID,
        "process_date": "20261201",
        "transaktionsgrund": "E03",
        "bilanzkreis": "THE0BFH000000001",
        "abrechnungsintervall_monate": 12,
        "haushaltskunde": true,
        "netznutzungsvertrag": "LIEFERANT_NB",
        "netznutzung_zahler": "LIEFERANT",
        "gemeinderabatt_prozent": "0",
        "kunde": {
            "vorname": "Erika",
            "nachname": "Mustermann",
            "anschrift": {
                "strasse": "Hauptstraße", "hausnummer": "1",
                "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
            }
        },
        "ablesekarte": {
            "vorname": "Erika",
            "nachname": "Mustermann",
            "anschrift": {
                "strasse": "Hauptstraße", "hausnummer": "1",
                "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
            }
        }
    })
}

/// The UTILMD outbox entries, as the outbox worker reads them.
async fn utilmd(state: &CommandsApiState) -> Vec<mako_engine::outbox::OutboxMessage> {
    state
        .store
        .pending_now(1_000)
        .await
        .expect("read outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == "UTILMD")
        .collect()
}

/// Render an outbox entry as the LFG sends it.
fn wire(msg: &mako_engine::outbox::OutboxMessage) -> String {
    let registry = crate::core::party_registry::MpIdRegistry::from_config(&[
        crate::core::config::PartyConfig {
            mp_id: LF_MP_ID.to_owned(),
            roles: vec!["LFG".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        },
    ])
    .expect("registry");
    let rendered = crate::orchestrator::edifact_renderer::render_to_wire_bytes(msg, &registry)
        .unwrap_or_else(|e| panic!("the 44001 renders: {e}"));
    // UNOC is ISO 8859-1: one byte, one character.
    rendered.bytes.iter().map(|&b| char::from(b)).collect()
}

/// What the gate says about the 44001 the workflow writes for `stammdaten`.
fn gate_findings(stammdaten: mako_engine::payload::Stammdaten) -> Vec<String> {
    let cmd = GeliGasLfAnmeldungCommand::InitiateAnmeldung {
        pid: Pruefidentifikator::new(44001).expect("PID"),
        sender: MarktpartnerCode::new(LF_MP_ID),
        receiver: MarktpartnerCode::new(GNB_MP_ID),
        malo_id: MaLo::new(MALO_ID),
        process_date: "20261201".to_owned(),
        transaktionsgrund: Some("E03".to_owned()),
        bilanzkreis: Some("THE0BFH000000001".to_owned()),
        stammdaten,
        received_at: time::OffsetDateTime::now_utc(),
    };
    let out = GeliGasLfAnmeldungWorkflow::handle(&mako_geli_gas::GeliGasLfAnmeldungState::New, cmd)
        .expect("the workflow writes the 44001");
    let pending = out
        .outbox
        .iter()
        .find(|p| &*p.message_type == "UTILMD")
        .expect("a UTILMD");
    crate::orchestrator::edifact_renderer::utilmd_ahb_findings(
        pending,
        TenantId::from_party_id(LF_MP_ID),
    )
    .expect("the 44001 renders")
}

/// The 44001 without the contract facts — Bilanzkreis, Transaktionsgrund and
/// Marktlokation alone — is refused by the pre-send gate, naming the Muss
/// segments it lacks. (The `IMD` is the renderer's; without it the full
/// contract below fails its column too.)
#[test]
fn a_44001_without_the_contract_facts_fails_the_pre_send_gate() {
    let findings = gate_findings(mako_engine::payload::Stammdaten::default());
    assert!(!findings.is_empty());
    let all = findings.join("\n");
    for place in [
        "Termine der Marktlokation",
        "Gruppenzuordnung",
        "Netznutzung",
        "Gemeinderabatt",
        "Kunde des Lieferanten",
        "Korrespondenzanschrift",
        "Ablesekarte",
    ] {
        assert!(all.contains(place), "{place} not named: {all}");
    }
}

/// The 44001 a full contract states passes the gate and its column, and
/// carries every Muss segment of Kap. 5.6 where the MIG puts it.
#[tokio::test]
async fn a_44001_from_a_full_contract_passes_its_profile() {
    let state = lfg_state().await;
    dispatch_command(&state, "geli.lieferbeginn.anmelden", &full_body())
        .await
        .expect("a full 44001 is accepted");
    let sent = utilmd(&state).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].recipient.as_ref(), GNB_MP_ID);

    let edi = wire(&sent[0]);
    let parsed = edi_energy::parse(&edi.chars().map(|c| c as u8).collect::<Vec<u8>>())
        .expect("the 44001 parses back");
    let report = edi_energy::EdiEnergyMessage::validate_on_date(&parsed, mako_fristen::heute())
        .expect("validates");
    assert!(report.is_valid(), "{:#?}\n{edi}", report.errors());

    for segment in [
        "IMD++Z36+Z12'",
        "STS+7++E03'",
        "LOC+172+51238696012'",
        "RFF+Z18'DTM+Z20:12:802'",
        "SEQ+Z01'CCI+Z19++THE0BFH000000001'CCI+++Z15'CCI+++Z88'CAV+Z74:::Z09'CAV+Z73:::Z11'",
        "SEQ+Z12'QTY+Z16:0:P1'",
        "NAD+Z09+++Erika Mustermann:::::Z01'",
        "NAD+Z04+++Erika Mustermann:::::Z01+Hauptstraße 1+Köln++50667+DE'",
        "NAD+Z05+++Erika Mustermann:::::Z01+Hauptstraße 1+Köln++50667+DE'",
        "NAD+MR+9800357000005::332'",
    ] {
        assert!(edi.contains(segment), "{segment} missing:\n{edi}");
    }
    assert!(!edi.contains("zaehlpunkt"), "{edi}");
}

/// Each Muss fact the caller leaves out is refused by name, and nothing is
/// written.
#[tokio::test]
async fn each_missing_contract_fact_is_refused_by_name() {
    let state = lfg_state().await;
    for key in [
        "transaktionsgrund",
        "bilanzkreis",
        "abrechnungsintervall_monate",
        "haushaltskunde",
        "netznutzungsvertrag",
        "netznutzung_zahler",
        "gemeinderabatt_prozent",
        "kunde",
        "ablesekarte",
    ] {
        let mut body = full_body();
        body.as_object_mut().expect("object").remove(key);
        let refused = dispatch_command(&state, "geli.lieferbeginn.anmelden", &body).await;
        assert!(
            matches!(&refused, Err(DispatchError::InvalidPayload(m)) if m.contains(key)),
            "{key}: {refused:?}"
        );
    }
    assert!(utilmd(&state).await.is_empty());
}

/// A value the column cannot carry is refused naming its segment and
/// Bedingung; a key the body does not declare is refused too.
#[tokio::test]
async fn a_value_the_column_cannot_carry_is_refused() {
    let state = lfg_state().await;
    let cases: [(&str, serde_json::Value, &str); 6] = [
        (
            "abrechnungsintervall_monate",
            serde_json::json!(2),
            "DTM+Z20",
        ),
        ("netznutzung_zahler", serde_json::json!("KUNDE"), "[216]"),
        (
            "gemeinderabatt_prozent",
            serde_json::json!("10.5"),
            "QTY+Z16",
        ),
        (
            "gemeinderabatt_prozent",
            serde_json::json!("1.255"),
            "QTY+Z16",
        ),
        (
            "kunde",
            serde_json::json!({ "nachname": "Mustermann" }),
            "anschrift",
        ),
        ("zaehlpunkt", serde_json::json!("DE0001"), "zaehlpunkt"),
    ];
    for (key, value, named) in cases {
        let mut body = full_body();
        body[key] = value;
        let refused = dispatch_command(&state, "geli.lieferbeginn.anmelden", &body).await;
        assert!(
            matches!(&refused, Err(DispatchError::InvalidPayload(m)) if m.contains(named)),
            "{key}: {refused:?}"
        );
    }
    assert!(utilmd(&state).await.is_empty());

    // The Kunde pays the Netznutzung under his own Netznutzungsvertrag.
    let mut body = full_body();
    body["netznutzungsvertrag"] = "KUNDE_NB".into();
    body["netznutzung_zahler"] = "KUNDE".into();
    dispatch_command(&state, "geli.lieferbeginn.anmelden", &body)
        .await
        .expect("Z08 with Z10 is the pair [216] admits");
    assert!(wire(&utilmd(&state).await[0]).contains("CAV+Z74:::Z08'CAV+Z73:::Z10'"));
}

/// The Abmeldung names its Marktlokation alone — no Zählpunkt is asked for.
#[tokio::test]
async fn a_44004_needs_no_zaehlpunkt() {
    let state = lfg_state().await;
    dispatch_command(
        &state,
        "geli.lieferende.anmelden",
        &serde_json::json!({ "malo_id": MALO_ID, "process_date": "20261231" }),
    )
    .await
    .expect("the Abmeldung is accepted");
    let sent = utilmd(&state).await;
    assert_eq!(sent.len(), 1);
    assert!(sent[0].payload.get("zaehlpunkt").is_none());
}
