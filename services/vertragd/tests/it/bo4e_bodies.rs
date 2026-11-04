//! The BO4E bodies vertragd holds — `Person`, `Zahlungsinformation`,
//! `Preisgarantie` — are typed `Bo4e<T>` and refuse a key BO4E does not define.
//!
//! An untyped `serde_json::Value` body has no field to refuse, so a misspelt
//! key is read as absent: `ibn` for `iban` stores a Zahlungsinformation without
//! an IBAN and answers `200`. Each route here must answer that with a `422`
//! naming the key.
//!
//! The refusal tests run on a lazy pool with nothing behind it, so a request
//! that got past the body would fail on the database instead: a `422` proves
//! the body alone decided it. The PostgreSQL test proves the valid documents
//! are still stored, and that a refused one leaves the stored one untouched.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use uuid::Uuid;

/// The tenant of `demos/o2c/vertragd.toml`.
const TENANT: &str = "9912345000005";

fn demo_config() -> vertragd::config::VertragdConfig {
    toml::from_str(include_str!("../../../../demos/o2c/vertragd.toml"))
        .expect("the demo configuration parses")
}

async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn put(base: &str, path: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let resp = reqwest::Client::new()
        .put(format!("{base}/api/v1/{path}"))
        .json(body)
        .send()
        .await
        .expect("the request is answered");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or_default())
}

fn person() -> serde_json::Value {
    serde_json::json!({ "anrede": "FRAU", "vorname": "Erika", "nachname": "Mustermann" })
}

fn zahlungsinformation() -> serde_json::Value {
    serde_json::json!({
        "zahlungsart": "SEPA_LASTSCHRIFT",
        "iban": "DE02120300000000202051",
        "kontoinhaber": "Erika Mustermann"
    })
}

fn preisgarantie() -> serde_json::Value {
    serde_json::json!({
        "preisgarantietyp": "ALLE_PREISBESTANDTEILE_BRUTTO",
        "zeitlicheGueltigkeit": { "startdatum": "2026-01-01", "enddatum": "2027-12-31" }
    })
}

/// `doc` with one key renamed — the misspelling a caller makes.
fn misspelt(mut doc: serde_json::Value, from: &str, to: &str) -> serde_json::Value {
    let obj = doc.as_object_mut().expect("an object");
    let v = obj.remove(from).expect("the key to misspell");
    obj.insert(to.to_owned(), v);
    doc
}

fn assert_unknown_field(route: &str, status: u16, body: &serde_json::Value, path: &str) {
    assert_eq!(status, 422, "{route} → {body}");
    assert_eq!(body["code"], "bo4e.unknown_field", "{route} → {body}");
    assert_eq!(body["paths"], serde_json::json!([path]), "{route} → {body}");
}

#[tokio::test]
async fn a_misspelt_key_is_refused_on_every_bo4e_put() {
    let app = mako_service::build_for_test::<vertragd::Vertragd>(demo_config())
        .await
        .expect("the router builds");
    let base = serve(app).await;
    let id = Uuid::new_v4();

    for (route, path, body, key) in [
        (
            "PUT person",
            format!("kunden/{id}/person"),
            misspelt(person(), "vorname", "vornam"),
            "vornam",
        ),
        (
            "PUT zahlungsinformation",
            format!("kunden/{id}/zahlungsinformation"),
            misspelt(zahlungsinformation(), "iban", "ibn"),
            "ibn",
        ),
        (
            "PUT preisgarantie",
            format!("vertraege/{id}/preisgarantie"),
            misspelt(
                preisgarantie(),
                "zeitlicheGueltigkeit",
                "zeitlicheGeltigkeit",
            ),
            "zeitlicheGeltigkeit",
        ),
    ] {
        let (status, json) = put(&base, &path, &body).await;
        assert_unknown_field(route, status, &json, key);
    }

    // At any depth: the period's `enddatum` is what `preisgarantie_bis` is
    // derived from, so a misspelt one would store a guarantee with no end.
    let mut nested = preisgarantie();
    nested["zeitlicheGueltigkeit"] = misspelt(
        nested["zeitlicheGueltigkeit"].take(),
        "enddatum",
        "endDatum",
    );
    let (status, json) = put(&base, &format!("vertraege/{id}/preisgarantie"), &nested).await;
    assert_unknown_field(
        "PUT preisgarantie (nested)",
        status,
        &json,
        "zeitlicheGueltigkeit.endDatum",
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_bo4e_documents_are_stored_and_a_misspelt_one_changes_nothing() {
    let Some((url, _pg)) = crate::dispatch_integration::pg_container().await else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let mut cfg = demo_config();
    cfg.database.url.clone_from(&url);
    let app = mako_service::build_for_test_on::<vertragd::Vertragd>(cfg, pool.clone())
        .await
        .expect("the router builds on the database");
    let base = serve(app).await;

    let kunde = crate::dispatch_integration::make_kunde(&pool, TENANT).await;
    let vertrag = vertragd::pg::insert_versorgungsvertrag(
        &pool,
        kunde,
        TENANT,
        TENANT,
        &crate::dispatch_integration::vertrag_input("ERP-BO4E-BODIES"),
    )
    .await
    .expect("a contract")
    .id;

    // Person.
    let (status, json) = put(&base, &format!("kunden/{kunde}/person"), &person()).await;
    assert_eq!(status, 204, "{json}");
    let (status, json) = put(
        &base,
        &format!("kunden/{kunde}/person"),
        &misspelt(person(), "nachname", "nachnahme"),
    )
    .await;
    assert_unknown_field("PUT person", status, &json, "nachnahme");
    let stored: serde_json::Value = sqlx::query_scalar("SELECT person FROM kunden WHERE id = $1")
        .bind(kunde)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored["nachname"], "Mustermann", "{stored}");
    assert_eq!(
        stored["_typ"], "PERSON",
        "the canonical round-trip is stored"
    );

    // Zahlungsinformation — and the IBAN check still runs after the gate.
    let (status, json) = put(
        &base,
        &format!("kunden/{kunde}/zahlungsinformation"),
        &zahlungsinformation(),
    )
    .await;
    assert_eq!(status, 200, "{json}");
    assert_eq!(
        json["zahlungsinformation"]["iban"], "DE02120300000000202051",
        "{json}"
    );
    let (status, json) = put(
        &base,
        &format!("kunden/{kunde}/zahlungsinformation"),
        &misspelt(zahlungsinformation(), "iban", "ibn"),
    )
    .await;
    assert_unknown_field("PUT zahlungsinformation", status, &json, "ibn");
    let mut bad_iban = zahlungsinformation();
    bad_iban["iban"] = "DE00000000000000000000".into();
    let (status, json) = put(
        &base,
        &format!("kunden/{kunde}/zahlungsinformation"),
        &bad_iban,
    )
    .await;
    assert_eq!(status, 422, "an IBAN failing mod-97: {json}");

    // Preisgarantie — `preisgarantie_bis` is derived from the stored document.
    let (status, json) = put(
        &base,
        &format!("vertraege/{vertrag}/preisgarantie"),
        &preisgarantie(),
    )
    .await;
    assert_eq!(status, 204, "{json}");
    let bis: Option<time::Date> =
        sqlx::query_scalar("SELECT preisgarantie_bis FROM versorgungsvertraege WHERE id = $1")
            .bind(vertrag)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bis, Some(time::macros::date!(2027 - 12 - 31)));
}
