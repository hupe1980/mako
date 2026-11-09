//! Every processd request body is a typed struct read through
//! `mako_service::Json`.
//!
//! An untyped `serde_json::Value` body has no fields, so the extractor's
//! unknown-key refusal has nothing to refuse against, and `axum::Json` has no
//! such refusal at all: a misspelt `malo_id` would be read as an absent one.
//! For each route below an unknown key and a missing mandatory field are a
//! `422`, and a well-formed body gets past the extractor — for the supply
//! routes all the way to `makod`, whose received command is checked.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// A `marktd` that accepts any subscription.
async fn marktd() -> String {
    let app = axum::Router::new().fallback(|| async { axum::http::StatusCode::OK });
    serve(app).await
}

/// A `makod` that accepts every command and records what it was sent.
async fn makod(received: Arc<Mutex<Vec<Value>>>) -> String {
    let app = axum::Router::new().route(
        "/api/v1/commands",
        axum::routing::post(move |axum::Json(cmd): axum::Json<Value>| {
            let received = Arc::clone(&received);
            async move {
                let command = cmd["command"].clone();
                received.lock().unwrap().push(cmd);
                axum::Json(json!({
                    "process_id": uuid::Uuid::new_v4(),
                    "command": command,
                    "idempotency_key": null,
                }))
            }
        }),
    );
    serve(app).await
}

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// processd with the demo configuration, a stand-in `makod` and a pool with no
/// database behind it.
async fn processd(received: Arc<Mutex<Vec<Value>>>) -> String {
    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../demos/nb-stp/processd.toml"))
            .expect("the demo configuration parses");
    cfg["marktd"]["url"] = toml::Value::String(marktd().await);
    cfg["makod"]["url"] = toml::Value::String(makod(received).await);
    let app = mako_service::build_for_test::<processd::Processd>(
        cfg.try_into()
            .expect("the demo configuration is a processd one"),
    )
    .await
    .expect("the router builds");
    serve(app).await
}

async fn send(method: reqwest::Method, url: String, body: &Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .request(method, url)
        .json(body)
        .send()
        .await
        .expect("answered");
    let status = resp.status().as_u16();
    let body = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

/// `body` with `key` added (`Some`) or removed (`None`).
fn with(body: &Value, key: &str, value: Option<Value>) -> Value {
    let mut body = body.clone();
    let obj = body.as_object_mut().expect("an object");
    match value {
        Some(v) => obj.insert(key.to_owned(), v),
        None => obj.remove(key),
    };
    body
}

/// An unknown key is refused naming it, and each mandatory field is refused
/// when left out.
async fn assert_refusals(method: reqwest::Method, url: &str, valid: &Value, mandatory: &[&str]) {
    let typo = with(valid, "malo_idd", Some(json!("51238696012")));
    let (status, body) = send(method.clone(), url.to_owned(), &typo).await;
    assert_eq!(status, 422, "{url}: unknown key: {body}");
    assert_eq!(body["code"], "unknown_field", "{url}: {body}");
    assert_eq!(body["paths"], json!(["malo_idd"]), "{url}: {body}");

    for field in mandatory {
        let missing = with(valid, field, None);
        let (status, body) = send(method.clone(), url.to_owned(), &missing).await;
        assert_eq!(status, 422, "{url}: without {field}: {body}");
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|d| d.contains(&format!("missing field `{field}`"))),
            "{url}: without {field}: {body}"
        );
    }
}

#[tokio::test]
async fn start_supply_body_is_typed() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let base = processd(Arc::clone(&received)).await;
    let url = format!("{base}/api/v1/start-supply");
    let valid = json!({
        "malo_id": "51238696012",
        "lieferbeginn_datum": "2099-01-01",
        "bilanzkreis": "11XBK-STD------Y",
    });
    assert_refusals(
        reqwest::Method::POST,
        &url,
        &valid,
        &["malo_id", "lieferbeginn_datum"],
    )
    .await;
    assert!(
        received.lock().unwrap().is_empty(),
        "a refusal reached makod"
    );

    let (status, body) = send(reqwest::Method::POST, url, &valid).await;
    assert_eq!(status, 202, "{body}");
    let sent = received.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["command"], "gpke.lieferbeginn.anmelden");
    assert_eq!(sent[0]["payload"]["malo_id"], "51238696012");
    assert_eq!(sent[0]["payload"]["bilanzkreis"], "11XBK-STD------Y");
}

/// A 44001 body as a Lieferant's full contract states it.
fn gas_anmeldung() -> Value {
    let partei = json!({
        "vorname": "Erika",
        "nachname": "Mustermann",
        "anschrift": {
            "strasse": "Hauptstraße", "hausnummer": "1",
            "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
        }
    });
    json!({
        "malo_id": "51238696012",
        "process_date": "20990101",
        "transaktionsgrund": "E03",
        "bilanzkreis": "9870000000006",
        "abrechnungsintervall_monate": 12,
        "haushaltskunde": true,
        "netznutzungsvertrag": "LIEFERANT_NB",
        "netznutzung_zahler": "LIEFERANT",
        "gemeinderabatt_prozent": "0",
        "kunde": partei,
        "ablesekarte": partei,
    })
}

#[tokio::test]
async fn start_supply_gas_body_is_typed() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let base = processd(Arc::clone(&received)).await;
    let url = format!("{base}/api/v1/start-supply-gas");
    let valid = gas_anmeldung();
    assert_refusals(
        reqwest::Method::POST,
        &url,
        &valid,
        &["malo_id", "process_date"],
    )
    .await;

    // A date the Mindestvorlauffrist cannot be checked against is refused,
    // not forwarded unchecked.
    let bad_date = with(&valid, "process_date", Some(json!("2099-01-01")));
    let (status, body) = send(reqwest::Method::POST, url.clone(), &bad_date).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["code"], "INVALID_DATE", "{body}");

    // Every contract fact the 44001 column makes Muss is named when left out.
    for key in [
        "transaktionsgrund",
        "abrechnungsintervall_monate",
        "haushaltskunde",
        "netznutzungsvertrag",
        "netznutzung_zahler",
        "gemeinderabatt_prozent",
        "kunde",
        "ablesekarte",
    ] {
        let (status, body) =
            send(reqwest::Method::POST, url.clone(), &with(&valid, key, None)).await;
        assert_eq!(status, 422, "without {key}: {body}");
        assert_eq!(body["code"], "ANMELDEDATEN_UNVOLLSTAENDIG", "{body}");
        assert!(
            body["fehlend"].as_array().is_some_and(|f| f
                .iter()
                .any(|x| x.as_str().is_some_and(|x| x.starts_with(key)))),
            "without {key}: {body}"
        );
    }
    // A value the column cannot carry is named too.
    let (status, body) = send(
        reqwest::Method::POST,
        url.clone(),
        &with(&valid, "abrechnungsintervall_monate", Some(json!(2))),
    )
    .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["code"], "ANMELDEDATEN_UNGUELTIG", "{body}");
    assert!(
        received.lock().unwrap().is_empty(),
        "a refusal reached makod"
    );

    let (status, body) = send(reqwest::Method::POST, url, &valid).await;
    assert_eq!(status, 202, "{body}");
    let sent = received.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["marktrolle"], "LFG");
    // makod's own body type, field for field: it refuses any other key.
    let forwarded: mako_markt::commands::GeliLieferbeginnAnmelden =
        mako_service::json_from_value(sent[0]["payload"].clone())
            .expect("the body makod's geli.lieferbeginn.anmelden reads");
    assert_eq!(forwarded.malo_id, "51238696012");
    assert_eq!(forwarded.transaktionsgrund, "E03");
    assert_eq!(forwarded.bilanzkreis, "9870000000006");
    assert!(forwarded.maengel().is_empty());
}

/// The Transaktionsgrund is a code the 44001 column admits, read from
/// the profile. A misspelt `e03` is no Einzug that skips the binding
/// Vorlauffrist, and a code of another column is no Transaktionsgrund here.
#[tokio::test]
async fn start_supply_gas_refuses_a_transaktionsgrund_outside_the_column() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let base = processd(Arc::clone(&received)).await;
    let url = format!("{base}/api/v1/start-supply-gas");
    let admitted = processd::server::transaktionsgruende_44001();
    assert_eq!(
        admitted,
        ["E01", "E02", "E03", "ZD2"],
        "UTILMD AHB Gas 1.2 S. 45"
    );
    for code in ["e03", "Z36", ""] {
        let (status, body) = send(
            reqwest::Method::POST,
            url.clone(),
            &with(&gas_anmeldung(), "transaktionsgrund", Some(json!(code))),
        )
        .await;
        assert_eq!(status, 422, "{code:?}: {body}");
        assert_eq!(
            body["code"], "TRANSAKTIONSGRUND_UNZULAESSIG",
            "{code:?}: {body}"
        );
    }
    // `E03` keeps the binding 10-WT Mindestvorlauffrist.
    let tomorrow = mako_fristen::heute() + time::Duration::days(1);
    let kurz = format!(
        "{:04}{:02}{:02}",
        tomorrow.year(),
        u8::from(tomorrow.month()),
        tomorrow.day()
    );
    let (status, body) = send(
        reqwest::Method::POST,
        url.clone(),
        &with(&gas_anmeldung(), "process_date", Some(json!(kurz))),
    )
    .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["code"], "VORLAUFFRIST_VIOLATION", "{body}");
    assert!(
        received.lock().unwrap().is_empty(),
        "a refusal reached makod"
    );
}

#[tokio::test]
async fn end_supply_body_is_typed() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let base = processd(Arc::clone(&received)).await;
    let url = format!("{base}/api/v1/end-supply");
    let valid = json!({ "malo_id": "51238696012", "lieferende_datum": "2099-01-01" });
    assert_refusals(
        reqwest::Method::POST,
        &url,
        &valid,
        &["malo_id", "lieferende_datum"],
    )
    .await;
    assert!(
        received.lock().unwrap().is_empty(),
        "a refusal reached makod"
    );

    let (status, body) = send(reqwest::Method::POST, url, &valid).await;
    assert_eq!(status, 202, "{body}");
    let sent = received.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["command"], "gpke.lieferende.anmelden");
    assert_eq!(sent[0]["marktrolle"], "LF");
    assert_eq!(sent[0]["payload"], valid);
}

/// `geli.lieferende.anmelden` is licensed to `LFG` alone and reads the
/// Marktlokation and `process_date`, so that is what reaches `makod`.
#[tokio::test]
async fn end_supply_gas_body_is_typed() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let base = processd(Arc::clone(&received)).await;
    let url = format!("{base}/api/v1/end-supply-gas");
    let valid = json!({
        "malo_id": "51238696012",
        "process_date": "20990101",
    });
    assert_refusals(
        reqwest::Method::POST,
        &url,
        &valid,
        &["malo_id", "process_date"],
    )
    .await;
    assert!(
        received.lock().unwrap().is_empty(),
        "a refusal reached makod"
    );

    let (status, body) = send(reqwest::Method::POST, url, &valid).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["process_date"], "20990101", "{body}");
    let sent = received.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["command"], "geli.lieferende.anmelden");
    assert_eq!(sent[0]["marktrolle"], "LFG");
    assert_eq!(sent[0]["payload"], valid);
}

/// The identification is refused before the database; a well-formed one gets
/// past the extractor and fails only at the absent database (`500`).
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
#[tokio::test]
async fn neuanlage_identifikation_body_is_typed() {
    let base = processd(Arc::new(Mutex::new(Vec::new()))).await;
    let url = format!(
        "{base}/api/v1/neuanlage/{}/identifikation",
        uuid::Uuid::new_v4()
    );
    let valid = json!({ "malo_id": "51238696012" });
    assert_refusals(reqwest::Method::PUT, &url, &valid, &["malo_id"]).await;

    let (status, body) = send(reqwest::Method::PUT, url.clone(), &json!({ "malo_id": "" })).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["code"], "MISSING_MALO_ID", "{body}");

    let (status, body) = send(reqwest::Method::PUT, url, &valid).await;
    assert_eq!(status, 500, "past the extractor, at the database: {body}");
}
