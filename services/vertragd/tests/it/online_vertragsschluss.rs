//! The online Vertragsschluss end to end: the order with its Bestellnachweis,
//! the schema's § 312j Abs. 4 BGB `CHECK`, the operator's acceptance and the
//! consumer's Widerruf.
//!
//! The router runs as built, against a fake `productd` serving one frozen
//! offer. The tests without a database need none — the refusal they check
//! happens before the first query; the rest are `#[ignore]`d and run by
//! `just test-vertragd-db`.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::dispatch_integration::{PgContainer, pg_container};

const TENANT: &str = "9900357000004";
const MALO: &str = "51238696012";
const ANGEBOT_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PRODUCT_VERSION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

const SCHEMA: &str = include_str!("../../migrations/0001_schema.sql");

async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA).execute(&pool).await.expect("schema");
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    Some((pool, container))
}

/// The frozen offer the fake `productd` serves.
fn angebot(id: Uuid, gueltig_bis: OffsetDateTime) -> Value {
    json!({
        "id": id,
        "lf_mp_id": TENANT,
        "product_code": "STROM-OEKO-24",
        "product_version": PRODUCT_VERSION,
        "sparte": "STROM",
        "malo_id": MALO,
        "nb_mp_id": "9900000000001",
        "verbrauch_kwh": "3500",
        "gewuenschter_lieferbeginn": "2027-03-01",
        // Tomorrow, so the Widerrufsfrist's bound — not the offer's — is what a Lieferbeginn
        // inside the Widerrufsfrist meets.
        "lieferbeginn_fruehestens": mako_fristen::heute().next_day().unwrap().to_string(),
        "preise": [
            { "art": "ARBEITSPREIS_GESAMT_BRUTTO", "bezeichnung": "Arbeitspreis",
              "einheit": "ct/kWh", "wert": "34.90", "quelle": "productd" }
        ],
        "jahreskosten_brutto_eur": "1350.00",
        "abschlag_monat_brutto_eur": "112.50",
        "gueltig_bis": gueltig_bis
            .format(&time::format_description::well_known::Rfc3339).unwrap(),
        "content_sha256": ANGEBOT_SHA,
        "created_at": "2026-11-15T19:00:00+01:00",
    })
}

/// A fake `productd` answering `GET /api/v1/endkunden-angebote/{id}` with
/// [`angebot`]; its base URL.
async fn fake_productd(gueltig_bis: OffsetDateTime) -> String {
    use axum::{extract::Path, routing::get};
    let app = axum::Router::new().route(
        "/api/v1/endkunden-angebote/{id}",
        get(move |Path(id): Path<Uuid>| async move { axum::Json(angebot(id, gueltig_bis)) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

fn config(policy: &str, productd: &str) -> vertragd::config::VertragdConfig {
    toml::from_str(&format!(
        r#"
        tenant       = "{TENANT}"
        lf_mp_id     = "{TENANT}"
        processd_url = "http://127.0.0.1:1"
        accountingd_url = "http://127.0.0.1:1"
        edmd_url     = "http://127.0.0.1:1"
        outputd_url  = "http://127.0.0.1:1"
        productd_url = "{productd}"
        allow_insecure_no_auth = true

        [database]
        url = "postgres://unused"

        [absender]
        name = "Stadtwerke Musterstadt GmbH"
        email = "service@stadtwerke.example"

        [policy]
        {policy}
        "#
    ))
    .expect("the test configuration parses")
}

/// The router as `build` mounts it, served on a free port; its base URL.
async fn serve(pool: PgPool, cfg: vertragd::config::VertragdConfig) -> String {
    let enforcer = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
            "../../policies/vertragd.cedar"
        ))
        .unwrap(),
    );
    let ctx = Arc::new(vertragd::handlers::Ctx {
        pool,
        cfg: Arc::new(cfg),
        http: reqwest::Client::new(),
    });
    let app = vertragd::handlers::router(ctx, enforcer).layer(axum::Extension(
        mako_service::oidc::OidcVerifier::disabled(TENANT),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

fn lazy_pool() -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(200))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .unwrap()
}

/// A complete consumer order accepting the fake offer.
fn order(angebot_id: Uuid, dokumente: &[(&str, String)]) -> Value {
    json!({
        "bestellreferenz": "WEB-2026-000123",
        "angebot_ref": { "id": angebot_id, "sha256": ANGEBOT_SHA },
        "kunde": {
            "kundentyp": "B2C",
            "geschaeftspartner": {
                "_typ": "GESCHAEFTSPARTNER",
                "vorname": "Erika", "nachname": "Mustermann",
                "adresse": { "strasse": "Hauptstraße", "hausnummer": "1",
                             "postleitzahl": "50667", "ort": "Köln", "landescode": "DE" }
            },
            "email": "erika@example.org",
            "malo_id": MALO
        },
        "verbraucher": true,
        "gewuenschter_lieferbeginn": "2027-03-01",
        "anmeldung": { "lieferbeginn_anlass": "WECHSEL", "enfg_grundlage": "ZG1" },
        "kuendigungsfrist_monate": 1,
        "angezeigte_informationen": {
            "wesentliche_eigenschaften": "Ökostrom aus Wasserkraft",
            "gesamtpreis": "1.350,00 EUR/Jahr",
            "personalisierter_preis": false,
            "zusaetzliche_kosten": "keine",
            "gesamtpreis_pro_abrechnungszeitraum": "112,50 EUR/Monat",
            "laufzeit_kuendigung": "unbefristet, Kündigungsfrist 1 Monat",
            "mindestdauer": "keine",
            "sha256": "cd".repeat(32)
        },
        "button_beschriftung": "zahlungspflichtig bestellen",
        "dokumente": dokumente.iter().map(|(art, sha)| json!({ "art": art, "sha256": sha }))
            .collect::<Vec<_>>(),
        "belehrung_angezeigt": true,
        "wertersatz_info_angezeigt": true,
        "lieferung_in_widerrufsfrist_verlangt": { "verlangt": true, "text_sha256": "06".repeat(32) },
        "bestellt_am": "2026-11-15T19:02:11+01:00",
        "kanal": "WEBSITE"
    })
}

/// Register the four operator documents; their `(art, sha256)` with the
/// offer's Preisblatt.
async fn rechtsdokumente(http: &reqwest::Client, base: &str) -> Vec<(&'static str, String)> {
    let mut out = vec![("PREISBLATT", PRODUCT_VERSION.to_owned())];
    for art in [
        "AGB",
        "WIDERRUFSBELEHRUNG",
        "MUSTER_WIDERRUFSFORMULAR",
        "DATENSCHUTZ",
    ] {
        let r: Value = http
            .put(format!("{base}/api/v1/rechtsdokumente/{art}"))
            .json(&json!({ "text": format!("{art} der Stadtwerke, Fassung 1"), "gueltig_ab": "2026-11-01" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        out.push((art, r["sha256"].as_str().unwrap().to_owned()));
    }
    out
}

async fn post_order(
    http: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    body: &Value,
) -> (u16, Value) {
    let mut req = http.post(format!("{base}/api/v1/bestellungen")).json(body);
    if let Some(k) = key {
        req = req.header("Idempotency-Key", k);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn owed(pool: &PgPool, kind: &str) -> i64 {
    super::dispatch_integration::owed(pool, TENANT, kind).await
}

async fn events(pool: &PgPool, ce_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM event_outbox WHERE kind <> 'command' AND ce_type = $1")
        .bind(ce_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ── Without a database ────────────────────────────────────────────────────────

/// `[policy] vertragsschluss` has no default — the route refuses until the
/// operator decides.
#[tokio::test]
async fn an_unset_vertragsschluss_policy_refuses_the_route() {
    let base = serve(lazy_pool(), config("", "http://127.0.0.1:1")).await;
    let (status, body) = post_order(
        &reqwest::Client::new(),
        &base,
        Some("k1"),
        &order(Uuid::nil(), &[]),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["code"], "policy_nicht_gesetzt");
}

/// The IETF `Idempotency-Key` draft: a missing key is a client error.
#[tokio::test]
async fn an_order_without_an_idempotency_key_is_refused() {
    let base = serve(
        lazy_pool(),
        config(
            r#"vertragsschluss = "mit_bestellung""#,
            "http://127.0.0.1:1",
        ),
    )
    .await;
    let (status, body) = post_order(
        &reqwest::Client::new(),
        &base,
        None,
        &order(Uuid::nil(), &[]),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "idempotency_key_missing");
}

/// The generic create route has no way to say `ONLINE` — the field is
/// unknown to its body, so `Json` refuses it before any write.
#[tokio::test]
async fn the_generic_route_cannot_create_an_online_consumer_contract() {
    let base = serve(lazy_pool(), config("", "http://127.0.0.1:1")).await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/v1/kunden/{}/vertraege", Uuid::new_v4()))
        .json(&json!({
            "kundentyp": "B2C", "verbraucher": true, "abschlussweg": "ONLINE",
            "vertragsbeginn": "2027-03-01", "komponenten": []
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 422);
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("abschlussweg"),
        "the refusal names the field: {text}"
    );
}

// ── Schema ────────────────────────────────────────────────────────────────────

/// An `ONLINE` consumer contract without a Bestellnachweis is refused
/// by the schema, whatever writes it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_consumer_online_contract_without_bestellnachweis_is_refused() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let kunde = super::dispatch_integration::make_kunde(&pool, TENANT).await;
    let err = sqlx::query(
        "INSERT INTO versorgungsvertraege
           (kunden_id, tenant, vertragsbeginn, kundentyp, verbraucher, abschlussweg)
         VALUES ($1, $2, DATE '2027-03-01', 'B2C', true, 'ONLINE')",
    )
    .bind(kunde)
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect_err("§ 312j Abs. 4 BGB: no consumer online contract without its record");
    assert!(
        err.to_string().contains("vv_online_verbraucher_nachweis"),
        "the CHECK refuses it by name: {err}"
    );
    // The same row for a business customer, or concluded otherwise, stands.
    for (verbraucher, weg) in [(false, "ONLINE"), (true, "SONSTIG")] {
        sqlx::query(
            "INSERT INTO versorgungsvertraege
               (kunden_id, tenant, vertragsbeginn, kundentyp, verbraucher, abschlussweg)
             VALUES ($1, $2, DATE '2027-03-01', 'B2C', $3, $4)",
        )
        .bind(kunde)
        .bind(TENANT)
        .bind(verbraucher)
        .bind(weg)
        .execute(&pool)
        .await
        .unwrap();
    }
}

// ── The order ─────────────────────────────────────────────────────────────────

/// A complete order concludes the
/// contract with its Nachweis in one transaction — the Anmeldung, the two
/// documents and the two events owed beside it — and its hash verifies. The
/// record is append-only. A replay returns the first answer; the same key
/// with another body is refused.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_complete_order_concludes_the_contract_with_its_nachweis() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let docs = rechtsdokumente(&http, &base).await;
    let body = order(Uuid::new_v4(), &docs);

    let (status, first) = post_order(&http, &base, Some("k-1"), &body).await;
    assert_eq!(status, 201, "{first}");
    assert_eq!(first["status"], "ANGENOMMEN");
    let vertrag_id: Uuid = first["vertrag_id"].as_str().unwrap().parse().unwrap();
    let (weg, nachweis): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT abschlussweg, bestellnachweis_id FROM versorgungsvertraege WHERE id = $1",
    )
    .bind(vertrag_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(weg, "ONLINE");
    assert!(nachweis.is_some());
    assert_eq!(
        owed(&pool, "LIEFERBEGINN").await,
        1,
        "the Anmeldung is queued"
    );
    assert_eq!(
        owed(&pool, "DOKUMENT").await,
        2,
        "Zusammenfassung and Bestätigung"
    );
    assert_eq!(
        events(&pool, "de.mako.vertrag.bestellung.eingegangen").await,
        1
    );
    assert_eq!(
        events(&pool, "de.mako.vertrag.bestellung.angenommen").await,
        1
    );
    let subject: String = sqlx::query_scalar(
        "SELECT envelope->>'subject' FROM event_outbox WHERE ce_type = 'de.mako.vertrag.bestellung.angenommen'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(subject, "WEB-2026-000123", "the partner's own reference");

    // The Nachweis: the stored hash is the hash of the stored bytes.
    let bestellung = first["bestellung_id"].as_str().unwrap();
    let n: Value = http
        .get(format!("{base}/api/v1/bestellungen/{bestellung}/nachweis"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(n["sha256"], n["sha256_neu_berechnet"]);
    assert_eq!(n["sha256"], first["bestellnachweis"]["sha256"]);
    let err = sqlx::query("UPDATE bestellnachweise SET button_beschriftung = 'Weiter'")
        .execute(&pool)
        .await
        .expect_err("the Bestellnachweis is append-only");
    assert!(err.to_string().contains("append-only"), "{err}");

    // Replay: the first answer, nothing new.
    let (status, again) = post_order(&http, &base, Some("k-1"), &body).await;
    assert_eq!((status, &again), (201, &first));
    assert_eq!(
        count(&pool, "SELECT count(*) FROM versorgungsvertraege").await,
        1
    );

    // Same key, another body.
    let mut other = body.clone();
    other["bestellreferenz"] = json!("WEB-2026-000124");
    let (status, refused) = post_order(&http, &base, Some("k-1"), &other).await;
    assert_eq!(status, 422, "{refused}");
    assert_eq!(refused["code"], "idempotency_key_reused");
}

/// One order replayed a hundred times concurrently yields one
/// Bestellung, one contract and one Anmeldung.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_hundred_concurrent_replays_conclude_one_contract() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let body = order(Uuid::new_v4(), &rechtsdokumente(&http, &base).await);
    let mut runs = Vec::new();
    for _ in 0..100 {
        let (http, base, body) = (http.clone(), base.clone(), body.clone());
        runs.push(tokio::spawn(async move {
            post_order(&http, &base, Some("k-race"), &body).await
        }));
    }
    for r in runs {
        let (status, body) = r.await.unwrap();
        assert!(
            status == 201 || (status == 409 && body["code"] == "idempotency_request_in_flight"),
            "{status} {body}"
        );
    }
    assert_eq!(count(&pool, "SELECT count(*) FROM bestellungen").await, 1);
    assert_eq!(
        count(&pool, "SELECT count(*) FROM versorgungsvertraege").await,
        1
    );
    assert_eq!(owed(&pool, "LIEFERBEGINN").await, 1);
}

/// A refused order stores nothing and names why.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_refused_order_stores_nothing_and_names_why() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let docs = rechtsdokumente(&http, &base).await;

    let mut unbekannt = docs.clone();
    unbekannt[1].1 = "ff".repeat(32);
    let (status, body) = post_order(
        &http,
        &base,
        Some("k-a"),
        &order(Uuid::new_v4(), &unbekannt),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (422, Some("dokument_unbekannt")),
        "{body}"
    );

    let mut weiter = order(Uuid::new_v4(), &docs);
    weiter["button_beschriftung"] = json!("Weiter");
    let (status, body) = post_order(&http, &base, Some("k-b"), &weiter).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (422, Some("bestellbutton_nicht_zulaessig"))
    );

    let mut leer = order(Uuid::new_v4(), &docs);
    leer["angezeigte_informationen"]["gesamtpreis"] = Value::Null;
    let (status, body) = post_order(&http, &base, Some("k-c"), &leer).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (422, Some("bestellnachweis_unvollstaendig"))
    );
    assert_eq!(
        body["fehlend"],
        json!(["angezeigte_informationen.gesamtpreis"])
    );

    // Without the express request the Lieferbeginn waits for the end
    // of the Widerrufsfrist — 2027-03-01 is far enough, so ask for sooner.
    let mut ohne = order(Uuid::new_v4(), &docs);
    ohne["lieferung_in_widerrufsfrist_verlangt"] = json!({ "verlangt": false });
    ohne["gewuenschter_lieferbeginn"] = json!("2027-03-01");
    let (status, _) = post_order(&http, &base, Some("k-d"), &ohne).await;
    assert_eq!(status, 201, "a Lieferbeginn after the Frist is admitted");

    assert_eq!(
        count(&pool, "SELECT count(*) FROM bestellnachweise").await,
        1
    );
}

/// A registered document resolves by the hash an order cites, to the
/// bytes it was registered with; registering the same bytes again is a
/// replay, not a second version.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_registered_agb_resolves_by_its_hash() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let base = serve(pool.clone(), config("", "http://127.0.0.1:1")).await;
    let http = reqwest::Client::new();
    let text = "AGB der Stadtwerke, Fassung 1";
    let put = |status: u16| {
        let http = http.clone();
        let base = base.clone();
        async move {
            let resp = http
                .put(format!("{base}/api/v1/rechtsdokumente/AGB"))
                .json(&json!({ "text": text, "gueltig_ab": "2026-11-01" }))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), status);
            resp.json::<Value>().await.unwrap()
        }
    };
    let first = put(201).await;
    let again = put(200).await;
    assert_eq!(first["sha256"], again["sha256"]);
    let sha = first["sha256"].as_str().unwrap();
    let resp = http
        .get(format!("{base}/api/v1/rechtsdokumente/{sha}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.text().await.unwrap(), text);
    assert_eq!(
        count(&pool, "SELECT count(*) FROM rechtsdokumente").await,
        1
    );
    let unknown = http
        .get(format!("{base}/api/v1/rechtsdokumente/{}", "ff".repeat(32)))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status().as_u16(), 404);
}

/// § 357a Abs. 2 Satz 1 Nr. 1 BGB: without the consumer's express
/// request, supply cannot start inside the Widerrufsfrist; the answer names
/// the day after it ends. With the request the same date is admitted.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn without_the_request_supply_cannot_start_inside_the_widerrufsfrist() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let docs = rechtsdokumente(&http, &base).await;
    let heute = mako_fristen::heute();
    let frist = mako_fristen::vorlauf::widerrufsfrist(
        heute,
        Some(heute),
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    // Inside the Widerrufsfrist, and no earlier than the Anmeldung's own lead
    // time allows — the order is held to both.
    let innerhalb = mako_fristen::vorlauf::fruehester_lieferbeginn(
        mako_fristen::Sparte::Strom,
        true,
        heute,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    assert!(
        innerhalb <= frist.ende,
        "the lead time ends inside the Widerrufsfrist"
    );
    let innerhalb = innerhalb.to_string();

    let mut ohne = order(Uuid::new_v4(), &docs);
    ohne["lieferung_in_widerrufsfrist_verlangt"] = json!({ "verlangt": false });
    ohne["gewuenschter_lieferbeginn"] = json!(innerhalb);
    let (status, body) = post_order(&http, &base, Some("k-ohne"), &ohne).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (422, Some("lieferbeginn_zu_frueh")),
        "{body}"
    );
    assert_eq!(
        body["fruehestens"],
        frist.ende.next_day().unwrap().to_string()
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM bestellnachweise").await,
        0
    );

    let mut mit = order(Uuid::new_v4(), &docs);
    mit["gewuenschter_lieferbeginn"] = json!(innerhalb);
    let (status, body) = post_order(&http, &base, Some("k-mit"), &mit).await;
    assert_eq!(status, 201, "the express request admits it: {body}");
}

/// Under `mit_annahme` the contract exists only after the operator's
/// acceptance, and an order refused closes with its event.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn under_mit_annahme_the_contract_exists_only_after_acceptance() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_annahme""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let docs = rechtsdokumente(&http, &base).await;
    let (status, b) = post_order(&http, &base, Some("k-1"), &order(Uuid::new_v4(), &docs)).await;
    assert_eq!(status, 202, "{b}");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM versorgungsvertraege").await,
        0
    );
    let id = b["bestellung_id"].as_str().unwrap();
    let r: Value = http
        .post(format!("{base}/api/v1/bestellungen/{id}/annehmen"))
        .json(&json!({ "bonitaet_ref": "SCHUFA-4711" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["status"], "ANGENOMMEN", "{r}");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM versorgungsvertraege").await,
        1
    );
    assert_eq!(owed(&pool, "LIEFERBEGINN").await, 1);

    let (_, b2) = post_order(&http, &base, Some("k-2"), &order(Uuid::new_v4(), &docs)).await;
    let id2 = b2["bestellung_id"].as_str().unwrap();
    let resp = http
        .post(format!("{base}/api/v1/bestellungen/{id2}/ablehnen"))
        .json(&json!({ "grund": "Bonität" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        events(&pool, "de.mako.vertrag.bestellung.abgelehnt").await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM versorgungsvertraege").await,
        1
    );
}

// ── Widerruf ──────────────────────────────────────────────────────────────────

/// Conclude one order; the contract id.
async fn concluded(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    bestellt_am: OffsetDateTime,
) -> Uuid {
    let mut body = order(Uuid::new_v4(), &rechtsdokumente(http, base).await);
    body["bestellt_am"] = json!(
        bestellt_am
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    );
    let (status, r) = post_order(http, base, Some(key), &body).await;
    assert_eq!(status, 201, "{r}");
    r["vertrag_id"].as_str().unwrap().parse().unwrap()
}

/// The instant `tage` days ago.
fn vor_tagen(tage: i64) -> OffsetDateTime {
    OffsetDateTime::now_utc() - time::Duration::days(tage)
}

/// Move a contract's Vertragsschluss to `am`, the instant its order was placed
/// at; the Frist the function computes for it. The Belehrung is the order's
/// own day, so both ends of the Frist's start move together.
async fn zurueckdatieren(
    pool: &PgPool,
    vertrag: Uuid,
    am: OffsetDateTime,
) -> mako_fristen::vorlauf::Widerrufsfrist {
    sqlx::query("UPDATE versorgungsvertraege SET vertragsschluss_am = $2 WHERE id = $1")
        .bind(vertrag)
        .bind(am)
        .execute(pool)
        .await
        .unwrap();
    let schluss = mako_fristen::berlin_date(am);
    mako_fristen::vorlauf::widerrufsfrist(
        schluss,
        Some(schluss),
        mako_fristen::HolidayCalendar::BdewMaKo,
    )
}

fn widerruf(abgesendet: OffsetDateTime) -> Value {
    json!({
        "name": "Erika Mustermann",
        "vertrag": "VV-2026-00000001",
        "bestaetigung_an": "erika@example.org",
        "kanal": "WIDERRUFSFUNKTION",
        "abgesendet_am": abgesendet.format(&time::format_description::well_known::Rfc3339).unwrap(),
    })
}

async fn post_widerruf(http: &reqwest::Client, url: String, body: &Value) -> (u16, Value) {
    let resp = http.post(url).json(body).send().await.unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

/// Sent on the last day of the Frist
/// (§ 355 Abs. 1 Satz 5 BGB: the sending decides), before Lieferbeginn — the
/// Anmeldung is withdrawn on the Stornierung path, the contract is
/// `WIDERRUFEN`, the Eingangsbestätigung and the event are owed. A second
/// Widerruf is answered with the first.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_widerruf_sent_on_the_last_day_before_lieferbeginn_withdraws_the_anmeldung() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let am = vor_tagen(40);
    let vertrag = concluded(&http, &base, "k-1", am).await;
    let frist = zurueckdatieren(&pool, vertrag, am).await;
    let letzter_tag = mako_fristen::berlin_at(frist.ende, time::macros::time!(21:30));

    let (status, r) = post_widerruf(
        &http,
        format!("{base}/api/v1/vertraege/{vertrag}/widerruf"),
        &widerruf(letzter_tag),
    )
    .await;
    assert_eq!(status, 201, "{r}");
    assert_eq!(r["rechtzeitig"], true);
    assert_eq!(r["wirkung"], "STORNIERT");
    assert_eq!(
        r["wertersatz_geschuldet"],
        Value::Null,
        "nothing was delivered"
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM versorgungsvertraege WHERE id = $1")
            .bind(vertrag)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "WIDERRUFEN");
    assert_eq!(
        owed(&pool, "LIEFERBEGINN").await,
        0,
        "the Anmeldung is parked"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM event_outbox WHERE event_id LIKE '%DOKUMENT:WIDERRUFSBESTAETIGUNG:%'").await,
        1
    );
    assert_eq!(
        events(&pool, "de.mako.vertrag.widerruf.eingegangen").await,
        1
    );

    let (status, again) = post_widerruf(
        &http,
        format!("{base}/api/v1/vertraege/{vertrag}/widerruf"),
        &widerruf(letzter_tag),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(again["widerruf_id"], r["widerruf_id"]);
}

/// One day late is recorded, its receipt confirmed, and refused with
/// the dates.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_late_widerruf_is_refused_with_the_dates() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let am = vor_tagen(40);
    let vertrag = concluded(&http, &base, "k-1", am).await;
    let frist = zurueckdatieren(&pool, vertrag, am).await;
    let zu_spaet =
        mako_fristen::berlin_at(frist.ende.next_day().unwrap(), time::macros::time!(09:00));
    let (status, r) = post_widerruf(
        &http,
        format!("{base}/api/v1/vertraege/{vertrag}/widerruf"),
        &widerruf(zu_spaet),
    )
    .await;
    assert_eq!(status, 422, "{r}");
    assert_eq!(r["code"], "widerrufsfrist_abgelaufen");
    assert_eq!(r["frist"]["ende"], frist.ende.to_string());
    let status: String =
        sqlx::query_scalar("SELECT status FROM versorgungsvertraege WHERE id = $1")
            .bind(vertrag)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(status, "WIDERRUFEN");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM widerrufe WHERE NOT rechtzeitig"
        )
        .await,
        1
    );
}

/// After Lieferbeginn supply ends on the
/// Lieferende path, and Wertersatz is owed because the record holds the
/// express request and the Nr. 3 information.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn after_lieferbeginn_supply_ends_on_the_lieferende_path() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let am = vor_tagen(5);
    let vertrag = concluded(&http, &base, "k-1", am).await;
    zurueckdatieren(&pool, vertrag, am).await;
    // Supply has begun.
    sqlx::query("UPDATE versorgungsvertraege SET status = 'AKTIV' WHERE id = $1")
        .bind(vertrag)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE vertragskomponenten SET status = 'AKTIV', lieferbeginn = heute() - 2 WHERE vertrag_id = $1",
    )
    .bind(vertrag)
    .execute(&pool)
    .await
    .unwrap();
    let (status, r) = post_widerruf(
        &http,
        format!("{base}/api/v1/vertraege/{vertrag}/widerruf"),
        &widerruf(OffsetDateTime::now_utc() - time::Duration::minutes(5)),
    )
    .await;
    assert_eq!(status, 201, "{r}");
    assert_eq!(r["wirkung"], "LIEFERENDE");
    assert_eq!(r["wertersatz_geschuldet"], true);
    assert_eq!(owed(&pool, "LIEFERENDE").await, 1);
}

/// A Widerruf of an order not yet accepted closes it `WIDERRUFEN` —
/// no contract, no Anmeldung.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unaccepted_bestellung_closes_widerrufen() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_annahme""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let docs = rechtsdokumente(&http, &base).await;
    let (_, b) = post_order(&http, &base, Some("k-1"), &order(Uuid::new_v4(), &docs)).await;
    let id = b["bestellung_id"].as_str().unwrap();
    let (status, r) = post_widerruf(
        &http,
        format!("{base}/api/v1/bestellungen/{id}/widerruf"),
        &widerruf(OffsetDateTime::now_utc() - time::Duration::minutes(1)),
    )
    .await;
    assert_eq!(status, 201, "{r}");
    assert_eq!(r["wirkung"], "BESTELLUNG_GESCHLOSSEN");
    let status: String = sqlx::query_scalar("SELECT status FROM bestellungen WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "WIDERRUFEN");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM versorgungsvertraege").await,
        0
    );
}

/// The order answer names the Kunde, a second order for that Kunde concludes
/// under the same one, and an order naming a Kunde this deployment does not
/// hold is refused before anything is stored.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_second_order_for_the_same_kunde_makes_no_second_kunde() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let productd = fake_productd(OffsetDateTime::now_utc() + time::Duration::days(14)).await;
    let base = serve(
        pool.clone(),
        config(r#"vertragsschluss = "mit_bestellung""#, &productd),
    )
    .await;
    let http = reqwest::Client::new();
    let docs = rechtsdokumente(&http, &base).await;

    let (status, first) =
        post_order(&http, &base, Some("k-a"), &order(Uuid::new_v4(), &docs)).await;
    assert_eq!(status, 201, "{first}");
    let kunden_id = first["kunden_id"]
        .as_str()
        .expect("the answer names the Kunde");

    let mut second = order(Uuid::new_v4(), &docs);
    second["bestellreferenz"] = json!("WEB-2026-000124");
    second["kunde"]["kunden_id"] = json!(kunden_id);
    let (status, answer) = post_order(&http, &base, Some("k-b"), &second).await;
    assert_eq!(status, 201, "{answer}");
    assert_eq!(answer["kunden_id"], kunden_id);
    assert_ne!(answer["vertrag_id"], first["vertrag_id"]);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM kunden").await, 1);

    let mut unknown = order(Uuid::new_v4(), &docs);
    unknown["bestellreferenz"] = json!("WEB-2026-000125");
    unknown["kunde"]["kunden_id"] = json!(Uuid::new_v4());
    let (status, refusal) = post_order(&http, &base, Some("k-c"), &unknown).await;
    assert_eq!(status, 422, "{refusal}");
    assert_eq!(refusal["code"], "kunde_unbekannt");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM versorgungsvertraege").await,
        2
    );
}
