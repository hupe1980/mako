//! The second round of the ESA invoice check, end to end through the webhook.
//!
//! A refused invoice is answered by the MSB with a COMDIS 29001 claiming it is
//! correct; the ESA owes another answer under `E_0266` (EBD 4.3 Kap. 8.27.3,
//! S. 634), whose Prüfschritt 1 — „Konnte der MSB alle Einwände des
//! Rechnungsempfängers entkräften?" — is a person's judgement. The walk holds
//! there until one is recorded, and the recorded judgement decides it.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::IntoResponse as _;
use rubo4e::current::{
    Adresse, Betrag, Geschaeftspartner, Menge, Mengeneinheit, Preis, Rechnung, Rechnungsposition,
    Steuerbetrag, Zeitraum, ZusatzAttribut,
};
use rust_decimal::Decimal;
use secrecy::SecretString;
use uuid::Uuid;

use invoicd::handler::{HandlerState, erneut_pruefen, handle_webhook};
use invoicd::pg::feststellungen::{Feststellungen, feststellen, gehaltene_rechnung};

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

const TENANT: &str = "9900111000002";
const MSB: &str = "9900357000004";

async fn pg_pool() -> Option<(sqlx::PgPool, PgContainer)> {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;

    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply invoicd schema");
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    Some((pool, container))
}

/// marktd and makod in one: the order is known, no offer is on record, and
/// every command makod is sent is kept.
async fn peer(commands: Arc<Mutex<Vec<serde_json::Value>>>) -> String {
    use axum::routing::{get, post};
    let app = axum::Router::new()
        .route(
            "/api/v1/esa/subscriptions/{r}",
            get(|| async { axum::Json(serde_json::json!({ "messprodukt": "9991000000044" })) }),
        )
        .route(
            "/api/v1/commands",
            post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let commands = Arc::clone(&commands);
                async move {
                    let command = body["command"].clone();
                    commands.lock().unwrap().push(body);
                    axum::Json(serde_json::json!({
                        "process_id": Uuid::new_v4(),
                        "command": command,
                        "idempotency_key": null,
                    }))
                }
            }),
        )
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn betrag(s: &str) -> Option<Betrag> {
    Some(Betrag {
        wert: Some(d(s)),
        ..Betrag::default()
    })
}

fn standard() -> Option<Vec<ZusatzAttribut>> {
    Some(vec![ZusatzAttribut {
        name: Some(invoic_checker::ustg::STEUERKATEGORIE_ATTRIBUT.to_owned()),
        wert: Some(serde_json::Value::from("S")),
        ..ZusatzAttribut::default()
    }])
}

fn partei(name: &str) -> Option<Box<Geschaeftspartner>> {
    Some(Box::new(Geschaeftspartner {
        organisationsname: Some(name.to_owned()),
        umsatzsteuer_id: Some("DE999999999".to_owned()),
        adresse: Some(Adresse {
            ort: Some("Köln".to_owned()),
            postleitzahl: Some("50667".to_owned()),
            ..Adresse::default()
        }),
        ..Geschaeftspartner::default()
    }))
}

/// An ESA invoice stating every § 14 Abs. 4 UStG Angabe: one position, 10 EUR
/// at 19 %.
fn rechnung() -> Rechnung {
    let heute = mako_fristen::heute();
    let von = heute - time::Duration::days(40);
    let bis = heute - time::Duration::days(10);
    let zeitraum = Some(Zeitraum {
        startdatum: Some(von),
        enddatum: Some(bis),
        ..Zeitraum::default()
    });
    Rechnung {
        rechnungsnummer: Some("RE-ESA-0001".to_owned()),
        rechnungsersteller: partei("Messstellenbetrieb GmbH"),
        rechnungsempfaenger: partei("Energieservice AG"),
        rechnungsdatum: Some((heute - time::Duration::days(5)).midnight().assume_utc()),
        faelligkeitsdatum: Some((heute + time::Duration::days(40)).midnight().assume_utc()),
        rechnungsperiode: zeitraum.clone(),
        rechnungspositionen: Some(vec![Rechnungsposition {
            positionsnummer: Some(1),
            artikel_id: Some("9990001100002".to_owned()),
            positions_menge: Some(Menge {
                wert: Some(d("1")),
                einheit: Some(Mengeneinheit::Stueck),
                ..Menge::default()
            }),
            einzelpreis: Some(Preis {
                wert: Some(d("10")),
                ..Preis::default()
            }),
            gesamtpreis: betrag("10"),
            lieferungszeitraum: zeitraum,
            steuerbetrag: Some(Steuerbetrag {
                steuersatz: Some(d("19")),
                zusatz_attribute: standard(),
                ..Steuerbetrag::default()
            }),
            ..Rechnungsposition::default()
        }]),
        steuerbetraege: Some(vec![Steuerbetrag {
            steuersatz: Some(d("19")),
            basiswert: Some(d("10")),
            steuerwert: Some(d("1.90")),
            zusatz_attribute: standard(),
            ..Steuerbetrag::default()
        }]),
        gesamtnetto: betrag("10"),
        gesamtsteuer: betrag("1.90"),
        gesamtbrutto: betrag("11.90"),
        zu_zahlen: betrag("11.90"),
        ..Rechnung::default()
    }
}

async fn webhook(state: &HandlerState, process_id: Uuid, data: serde_json::Value) {
    let body = serde_json::to_vec(&serde_json::json!({
        "specversion": "1.0",
        "id": Uuid::new_v4(),
        "source": "/marktd/test",
        "type": mako_events::mako::PROCESS_INITIATED,
        "subject": process_id,
        "data": data,
    }))
    .unwrap();
    let status = handle_webhook(
        State(state.clone()),
        axum::http::HeaderMap::new(),
        axum::body::Bytes::from(body),
    )
    .await
    .into_response()
    .status();
    assert!(status.is_success(), "{status}");
}

async fn receipt(pool: &sqlx::PgPool, process_id: Uuid) -> (Uuid, String, serde_json::Value) {
    sqlx::query_as("SELECT id, outcome, offen FROM invoic_receipts WHERE process_id = $1")
        .bind(process_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn schritte(offen: &serde_json::Value) -> Vec<u64> {
    offen
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["pruefschritt"].as_u64())
        .collect()
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_comdis_opens_the_second_round_and_a_judgement_decides_it() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let commands = Arc::new(Mutex::new(Vec::new()));
    let peer = peer(Arc::clone(&commands)).await;
    let key = SecretString::from("test");
    let state = HandlerState {
        marktd: mako_markt::marktd_client::MarktdClient::new(
            &peer,
            key.clone(),
            reqwest::Client::new(),
        ),
        makod: mako_markt::makod_client::MakodClient::new(peer.clone(), key),
        check_config: Arc::new(invoic_checker::CheckConfig::default()),
        inbound_secret: Arc::new(None),
        auto_dispute_threshold_raw: 0,
        pool: pool.clone(),
        tenant: TENANT.to_owned(),
        marktrolle: invoicd::config::EmpfaengerRolle::Esa,
        erp_webhook_url: None,
        erp_hmac_secret: None,
        http_client: reqwest::Client::new(),
    };
    let route = invoicd::routing::route_for(31009).expect("31009 is answered");
    let letzter = || {
        commands.lock().unwrap().last().unwrap()["command"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // Round one: the invoice arrives and the walk holds at 310 and 320.
    let process_id = Uuid::new_v4();
    webhook(
        &state,
        process_id,
        serde_json::json!({
            "pid": 31009,
            "invoice_ref": "RE-ESA-0001",
            "sender_mp_id": MSB,
            "receiver_gln": TENANT,
            "bestellung_ref": "ORDERS-17007-1",
            "rechnungstyp": "KON",
            "rechnung": serde_json::to_value(rechnung()).unwrap(),
        }),
    )
    .await;
    let (id, outcome, offen) = receipt(&pool, process_id).await;
    assert_eq!(outcome, "Offen");
    assert_eq!(schritte(&offen), vec![310, 320], "{offen}");

    // The ESA states the service was not performed: the walk refuses (A10).
    let mut f = Feststellungen::default();
    f.leistung_erbracht.insert(1, false);
    f.preis_wie_angebot.insert(1, true);
    assert!(
        feststellen(
            &pool,
            TENANT,
            id,
            &f,
            "keine Werte erhalten",
            "esa",
            time::OffsetDateTime::now_utc()
        )
        .await
        .unwrap()
    );
    let held = gehaltene_rechnung(&pool, TENANT, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(erneut_pruefen(&state, &held).await.unwrap(), "Dispute");
    assert_eq!(letzter(), route.reject, "the refusal goes out");

    // The MSB insists with a COMDIS 29001: the second round holds at
    // Prüfschritt 1, nothing is sent.
    let gesendet = commands.lock().unwrap().len();
    webhook(
        &state,
        process_id,
        serde_json::json!({
            "pid": 29001,
            "invoice_ref": "RE-ESA-0001",
            "sender_mp_id": MSB,
            "comdis_ref": "COMDIS-1",
        }),
    )
    .await;
    let (_, outcome, offen) = receipt(&pool, process_id).await;
    assert_eq!(outcome, "Offen");
    assert_eq!(schritte(&offen), vec![1], "{offen}");
    assert_eq!(
        commands.lock().unwrap().len(),
        gesendet,
        "nothing is sent while open"
    );

    // The ESA judges the objection rebutted — the values have arrived after
    // all — and the second round accepts.
    let mut f = Feststellungen {
        einwaende_entkraeftet: Some(true),
        ..Feststellungen::default()
    };
    f.leistung_erbracht.insert(1, true);
    assert!(
        feststellen(
            &pool,
            TENANT,
            id,
            &f,
            "Werte nachgeliefert",
            "esa",
            time::OffsetDateTime::now_utc()
        )
        .await
        .unwrap()
    );
    let held = gehaltene_rechnung(&pool, TENANT, id)
        .await
        .unwrap()
        .unwrap();
    let outcome = erneut_pruefen(&state, &held).await.unwrap();
    assert!(outcome == "Ok" || outcome == "Warn", "{outcome}");
    assert_eq!(letzter(), route.accept, "the Zahlungsavis goes out");
    let (runde, erste): (i16, Option<serde_json::Value>) = sqlx::query_as(
        "SELECT runde, erste_runde_antwort FROM invoic_receipts WHERE process_id = $1",
    )
    .bind(process_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(runde, 2);
    assert!(erste.is_some(), "the first round's answer stays on record");
}

/// The judgement „nicht entkräftet" refuses with `E_0266`'s `A25`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn objections_not_rebutted_refuse_with_a25() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let commands = Arc::new(Mutex::new(Vec::new()));
    let peer = peer(Arc::clone(&commands)).await;
    let key = SecretString::from("test");
    let state = HandlerState {
        marktd: mako_markt::marktd_client::MarktdClient::new(
            &peer,
            key.clone(),
            reqwest::Client::new(),
        ),
        makod: mako_markt::makod_client::MakodClient::new(peer.clone(), key),
        check_config: Arc::new(invoic_checker::CheckConfig::default()),
        inbound_secret: Arc::new(None),
        auto_dispute_threshold_raw: 0,
        pool: pool.clone(),
        tenant: TENANT.to_owned(),
        marktrolle: invoicd::config::EmpfaengerRolle::Esa,
        erp_webhook_url: None,
        erp_hmac_secret: None,
        http_client: reqwest::Client::new(),
    };
    let process_id = Uuid::new_v4();
    let mut f = Feststellungen::default();
    f.leistung_erbracht.insert(1, false);
    f.preis_wie_angebot.insert(1, true);
    webhook(
        &state,
        process_id,
        serde_json::json!({
            "pid": 31009, "invoice_ref": "RE-ESA-0001", "sender_mp_id": MSB,
            "receiver_gln": TENANT, "bestellung_ref": "ORDERS-17007-1", "rechnungstyp": "KON",
            "rechnung": serde_json::to_value(rechnung()).unwrap(),
        }),
    )
    .await;
    let (id, _, _) = receipt(&pool, process_id).await;
    feststellen(
        &pool,
        TENANT,
        id,
        &f,
        "keine Werte",
        "esa",
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let held = gehaltene_rechnung(&pool, TENANT, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(erneut_pruefen(&state, &held).await.unwrap(), "Dispute");
    webhook(
        &state,
        process_id,
        serde_json::json!({ "pid": 29001, "invoice_ref": "RE-ESA-0001", "sender_mp_id": MSB, "comdis_ref": "COMDIS-1" }),
    )
    .await;
    let f = Feststellungen {
        einwaende_entkraeftet: Some(false),
        ..Feststellungen::default()
    };
    feststellen(
        &pool,
        TENANT,
        id,
        &f,
        "COMDIS belegt nichts",
        "esa",
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let held = gehaltene_rechnung(&pool, TENANT, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(erneut_pruefen(&state, &held).await.unwrap(), "Dispute");
    let last = commands.lock().unwrap().last().unwrap().clone();
    assert_eq!(last["payload"]["antwort_code"], "A25", "{last}");
    assert_eq!(last["payload"]["antwort_codeliste"], "E_0266", "{last}");
}
