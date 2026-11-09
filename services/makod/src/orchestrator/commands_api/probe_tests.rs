//! A rehearsal runs the door up to its pre-send check and writes nothing:
//! no event stream, no outbox entry, no idempotency
//! record — after a passing rehearsal and after a failing one alike. Its
//! verdict is the door's own: where the real command is refused, the
//! rehearsal reports that refusal word for word.

use std::sync::Arc;

use mako_engine::event_store::EventStore as _;
use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;

use super::super::*;
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

/// The 44004 Abmeldung a Lieferant sends.
fn abmeldung(grund: &str) -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO_ID, "process_date": "20261231", "transaktionsgrund": grund
    })
}

/// Nothing a spawn writes exists: no stream, no outbox entry.
async fn assert_untouched(state: &CommandsApiState) {
    let streams = state.store.list_streams(None).await.expect("list streams");
    assert!(streams.is_empty(), "a rehearsal wrote streams: {streams:?}");
    assert_eq!(
        state.store.len().await.expect("outbox length"),
        0,
        "a rehearsal queued an outbox entry"
    );
}

async fn rehearse_as_lfg(
    state: &CommandsApiState,
    command: &str,
    body: &serde_json::Value,
) -> Probe {
    let rolle = zulassen(state, command, None).expect("rehearsable as LFG");
    rehearse(state, command, rolle, body)
        .await
        .expect("the rehearsal runs")
}

#[tokio::test]
async fn a_passing_rehearsal_writes_nothing_and_names_every_segment() {
    let state = lfg_state().await;
    let probe = rehearse_as_lfg(&state, "geli.lieferende.anmelden", &abmeldung("ZT5")).await;

    assert_eq!(probe.urteil, Urteil::Besteht, "{probe:#?}");
    assert_eq!(probe.stufe, None);
    assert_eq!(probe.grund, None);
    assert_eq!(probe.marktrolle, "LFG");
    assert_eq!(probe.nachrichten.len(), 1);
    let n = &probe.nachrichten[0];
    assert_eq!(n.nachrichtentyp, "UTILMD");
    assert_eq!(n.pruefidentifikator, Some(44004));
    assert_eq!(n.empfaenger, GNB_MP_ID);
    assert!(
        n.formatversion
            .as_deref()
            .is_some_and(|f| f.starts_with("FV"))
    );
    assert!(
        n.wire.contains("UNH+") && n.wire.contains("STS+7++ZT5'"),
        "{}",
        n.wire
    );

    assert_eq!(n.segmente.first().map(|s| s.tag.as_str()), Some("UNH"));
    assert_eq!(n.segmente.last().map(|s| s.tag.as_str()), Some("UNT"));
    for s in &n.segmente {
        assert!(
            s.name.as_deref().is_some_and(|name| !name.is_empty()),
            "segment {} {} has no MIG name",
            s.position,
            s.text
        );
        assert!(s.befunde.is_empty(), "{s:#?}");
    }
    let sts = n.segmente.iter().find(|s| s.tag == "STS").expect("an STS");
    assert_eq!(sts.text, "STS+7++ZT5'");
    assert_eq!(sts.elemente, ["7", "", "ZT5"]);
    assert_eq!(sts.gruppe.as_deref(), Some("SG4"));

    assert_untouched(&state).await;
}

#[tokio::test]
async fn a_failing_rehearsal_reports_the_doors_own_refusal_and_writes_nothing() {
    // `E03` is a Grund the 44004 column does not admit.
    let body = abmeldung("E03");

    let real_state = lfg_state().await;
    let real = dispatch_command(&real_state, "geli.lieferende.anmelden", &body).await;
    let Err(refused) = real else {
        panic!("the real door accepts {body}");
    };
    let real_text = crate::api::mcp_server::dispatch_error_to_string(refused);
    assert_untouched(&real_state).await;

    let state = lfg_state().await;
    let probe = rehearse_as_lfg(&state, "geli.lieferende.anmelden", &body).await;
    assert_eq!(probe.urteil, Urteil::Abgelehnt);
    assert_eq!(probe.grund.as_deref(), Some(real_text.as_str()));
    // The door built the message; its profile refuses it, and the finding
    // sits on the segment and the data element it names.
    assert_eq!(probe.stufe, Some(Stufe::Pruefung));
    let sts = probe.nachrichten[0]
        .segmente
        .iter()
        .find(|s| s.tag == "STS")
        .expect("an STS");
    assert_eq!(sts.name.as_deref(), Some("Transaktionsgrund"));
    assert!(
        sts.befunde.iter().any(|b| b.element == Some(2)
            && b.regel
                .as_deref()
                .is_some_and(|r| r.starts_with("AHB-44004"))),
        "{sts:#?}"
    );
    assert_untouched(&state).await;
}

#[tokio::test]
async fn a_door_refusal_is_the_rehearsals_verdict() {
    // No Transaktionsgrund: the door refuses before a message exists.
    let body = serde_json::json!({ "malo_id": MALO_ID, "process_date": "20261231" });

    let real_state = lfg_state().await;
    let Err(refused) = dispatch_command(&real_state, "geli.lieferende.anmelden", &body).await
    else {
        panic!("the real door accepts {body}");
    };
    let real_text = crate::api::mcp_server::dispatch_error_to_string(refused);

    let state = lfg_state().await;
    let probe = rehearse_as_lfg(&state, "geli.lieferende.anmelden", &body).await;
    assert_eq!(probe.urteil, Urteil::Abgelehnt);
    assert_eq!(probe.stufe, Some(Stufe::Tuer));
    assert_eq!(probe.grund.as_deref(), Some(real_text.as_str()));
    assert!(probe.nachrichten.is_empty());
    assert_untouched(&state).await;
}

#[tokio::test]
async fn a_rolle_the_deployment_does_not_declare_is_refused_naming_it() {
    let state = lfg_state().await;
    let refused =
        zulassen(&state, "gpke.lieferende.anmelden", None).expect_err("LF is not declared");
    assert!(
        matches!(&refused, ProbeRefusal::RolleNichtDeklariert { rolle, deklariert }
            if rolle == "LF" && deklariert == &["LFG".to_owned()]),
        "{refused:?}"
    );
    assert!(
        refused.sentence().contains("Marktrolle LF"),
        "{}",
        refused.sentence()
    );
    assert_eq!(refused.code(), "role_not_configured");
}

#[tokio::test]
async fn a_command_without_a_pre_send_check_is_not_rehearsed() {
    let state = lfg_state().await;
    let refused =
        zulassen(&state, "geli.lieferbeginn.bestaetigen", None).expect_err("no gated door");
    assert!(
        matches!(refused, ProbeRefusal::NotRehearsable(_)),
        "{refused:?}"
    );
    assert!(matches!(
        zulassen(&state, "no.such.command", None),
        Err(ProbeRefusal::UnknownCommand(_))
    ));
    // `rehearse` holds the same line on its own.
    assert!(matches!(
        rehearse(
            &state,
            "geli.lieferbeginn.bestaetigen",
            "GNB".to_owned(),
            &abmeldung("ZT5")
        )
        .await,
        Err(ProbeRefusal::NotRehearsable(_))
    ));
}

/// Every rehearsable command is registered: a rename leaves no stale entry.
#[test]
fn every_rehearsable_command_is_registered() {
    for c in REHEARSABLE {
        assert!(
            COMMAND_REGISTRY.iter().any(|d| d.name == *c),
            "{c} is not a registered command"
        );
    }
}

/// A door that reached its spawn inside a rehearsal is refused before the
/// write — the second line behind [`REHEARSABLE`].
#[tokio::test]
async fn spawn_process_refuses_inside_a_rehearsal() {
    let state = lfg_state().await;
    let cmd = GeliGasLfAnmeldungCommand::InitiateAnmeldung {
        pid: Pruefidentifikator::new(44004).expect("PID"),
        sender: MarktpartnerCode::new(LF_MP_ID),
        receiver: MarktpartnerCode::new(GNB_MP_ID),
        malo_id: MaLo::new(MALO_ID),
        process_date: "20261231".to_owned(),
        transaktionsgrund: Some("ZT5".to_owned()),
        bilanzkreis: None,
        stammdaten: mako_engine::payload::Stammdaten::default(),
        received_at: time::OffsetDateTime::now_utc(),
    };
    let spawned = PROBE
        .scope(std::cell::RefCell::new(None), async {
            spawn_process::<GeliGasLfAnmeldungWorkflow>(
                &state.store,
                state.tenant_id,
                WorkflowId::new(
                    mako_geli_gas::lf_anmeldung::WORKFLOW_NAME,
                    latest_format_version(),
                ),
                cmd,
                &[],
                &[MALO_ID],
            )
            .await
        })
        .await;
    assert!(spawned.is_err(), "spawn_process wrote inside a rehearsal");
    assert_untouched(&state).await;
}

/// The HTTP door: `200` with the verdict, `422` naming an undeclared Rolle,
/// and nothing written either way.
#[tokio::test]
async fn the_http_route_answers_the_verdict_and_names_an_undeclared_rolle() {
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    let state = Arc::new(lfg_state().await);
    let app = super::super::router(Arc::clone(&state));
    let post = |command: &str, body: serde_json::Value| {
        axum::http::Request::post(format!("/api/v1/probe/{command}"))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("request")
    };

    let res = app
        .clone()
        .oneshot(post(
            "geli.lieferende.anmelden",
            serde_json::json!({ "payload": abmeldung("ZT5") }),
        ))
        .await
        .expect("answer");
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.expect("body").to_bytes())
            .expect("json");
    assert_eq!(body["urteil"], "besteht", "{body}");
    assert_eq!(
        body["nachrichten"][0]["segmente"][0]["name"],
        "Nachrichten-Kopfsegment"
    );

    let res = app
        .clone()
        .oneshot(post(
            "gpke.lieferende.anmelden",
            serde_json::json!({ "payload": { "malo_id": MALO_ID } }),
        ))
        .await
        .expect("answer");
    assert_eq!(res.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value =
        serde_json::from_slice(&res.into_body().collect().await.expect("body").to_bytes())
            .expect("json");
    assert_eq!(body["code"], "role_not_configured", "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("Marktrolle LF")),
        "{body}"
    );

    let res = app
        .oneshot(post(
            "geli.lieferende.anmelden",
            serde_json::json!({ "payload": abmeldung("ZT5"), "unknown": 1 }),
        ))
        .await
        .expect("answer");
    assert!(
        res.status().is_client_error(),
        "an unknown field is refused"
    );

    assert_untouched(&state).await;
}
