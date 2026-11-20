//! The sales edge against real PostgreSQL: the § 14a module at intake, the
//! Netzbetreiber's Beendigung der Zuordnung written back, an Ersatzversorgung
//! opened from its 55013, and the Kündigung beim Vorversorger holding the
//! Anmeldung until its answer.
//!
//! ```bash
//! just test-vertragd-db
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::Arc;

use reqwest::StatusCode;
use sqlx::PgPool;
use time::macros::date;
use uuid::Uuid;
use vertragd::pg;

use super::dispatch_integration::{PgContainer, make_kunde, owed, pg_container, vertrag_input};

const SCHEMA: &str = include_str!("../../migrations/0001_schema.sql");
const TENANT: &str = "9900357000004";
/// The Netzbetreiber `vertrag_input` names.
const NB: &str = "9900000000001";
const MALO: &str = "51238696012";
const VORVERSORGER: &str = "9900357000011";

async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    Some((pool, container))
}

fn app(pool: &PgPool, policy: &str) -> axum::Router {
    let cfg: vertragd::config::VertragdConfig = serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://unused" },
        "tenant": TENANT,
        "lf_mp_id": TENANT,
        "processd_url": "http://processd.invalid",
        "accountingd_url": "http://accountingd.invalid",
        "edmd_url": "http://edmd.invalid",
        "ersatzversorgung_produkt": "STROM-ERSATZ-2026",
        "allow_insecure_no_auth": true,
        "policy": { "nach_kuendigungsablehnung": policy },
    }))
    .expect("config");
    let enforcer = mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
        "../../policies/vertragd.cedar"
    ))
    .expect("policies");
    vertragd::handlers::router(
        Arc::new(vertragd::handlers::Ctx {
            pool: pool.clone(),
            cfg: Arc::new(cfg),
            http: reqwest::Client::new(),
        }),
        Arc::new(enforcer),
    )
    // The operator routes as `build` mounts them: a disabled verifier answers
    // every request as the dev principal.
    .layer(axum::Extension(mako_service::oidc::OidcVerifier::disabled(
        TENANT,
    )))
}

async fn post(
    pool: &PgPool,
    policy: &str,
    path: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app(pool, policy);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let r = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(body)
        .send()
        .await
        .unwrap();
    let status = r.status();
    (status, r.json().await.unwrap_or(serde_json::Value::Null))
}

async fn events_of(pool: &PgPool, ce_type: &str) -> Vec<serde_json::Value> {
    sqlx::query_scalar("SELECT envelope FROM event_outbox WHERE ce_type = $1 ORDER BY seq")
        .bind(ce_type)
        .fetch_all(pool)
        .await
        .unwrap()
}

fn ce(id: &str, ce_type: &str, subject: &str, data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "specversion": "1.0",
        "id": id,
        "source": "urn:mako:makod:tenant:test",
        "type": ce_type,
        "subject": subject,
        "data": data,
    })
}

/// A Strom contract, its component confirmed and supplying.
async fn laufender_vertrag(pool: &PgPool, erp: &str) -> Uuid {
    let kunde = make_kunde(pool, TENANT).await;
    let v = pg::insert_versorgungsvertrag(pool, kunde, TENANT, TENANT, &vertrag_input(erp))
        .await
        .expect("create");
    sqlx::query("UPDATE vertragskomponenten SET status = 'BESTAETIGT' WHERE vertrag_id = $1")
        .bind(v.id)
        .execute(pool)
        .await
        .unwrap();
    v.id
}

async fn lieferende(pool: &PgPool, vertrag_id: Uuid) -> Option<time::Date> {
    sqlx::query_scalar("SELECT lieferende FROM vertragskomponenten WHERE vertrag_id = $1")
        .bind(vertrag_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ── § 14a at intake ───────────────────────────────────────────────────────────

/// The module set a contract states is recorded from its Lieferbeginn; a set
/// BK8-22/010-A does not offer writes no contract at all.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_sect14a_module_set_travels_with_the_contract() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let kunde = make_kunde(&pool, TENANT).await;
    let mut input = serde_json::to_value(vertrag_body("C-14A")).unwrap();
    input["komponenten"][0]["sect14a"] = serde_json::json!({ "module": ["MODUL_3", "MODUL_1"] });
    let (status, _) = post(
        &pool,
        "halten",
        &format!("/api/v1/kunden/{kunde}/vertraege"),
        &input,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (module, ab): (Vec<String>, time::Date) =
        sqlx::query_as("SELECT module, gueltig_von FROM komponenten_sect14a")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(module, vec!["MODUL_1", "MODUL_3"]);
    assert_eq!(ab, date!(2026 - 10 - 01));

    input["erp_contract_id"] = "C-14A-2".into();
    input["komponenten"][0]["sect14a"] = serde_json::json!({ "module": ["MODUL_1", "MODUL_2"] });
    let (status, body) = post(
        &pool,
        "halten",
        &format!("/api/v1/kunden/{kunde}/vertraege"),
        &input,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "sect14a_kombination_unzulaessig");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM versorgungsvertraege")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "the refused contract is not written");
}

/// `vertrag_input` as the contract route's JSON body.
fn vertrag_body(erp: &str) -> serde_json::Value {
    serde_json::json!({
        "kundentyp": "B2C",
        "vertragsbeginn": "2026-10-01",
        "erp_contract_id": erp,
        "komponenten": [{
            "sparte": "STROM",
            "malo_id": MALO,
            "nb_mp_id": NB,
            "product_code": "STROM-BASIS-2026",
            "lieferbeginn": "2026-10-01",
            "lieferbeginn_anlass": "WECHSEL",
            "enfg_grundlage": "ZG1",
        }],
    })
}

// ── The Netzbetreiber ends the Zuordnung (55037) ──────────────────────────────

fn beendigung(id: &str, nb: &str, datum: Option<&str>) -> serde_json::Value {
    ce(
        id,
        mako_events::mako::PROCESS_INITIATED,
        "11111111-1111-4111-8111-111111111111",
        serde_json::json!({
            "pid": 55037, "malo_id": MALO, "grid_operator": nb,
            "transaktionsgrund": "E03", "process_date": datum,
        }),
    )
}

/// SR-086: the Zuordnungsende becomes the Lieferende of the component the
/// sender is the Netzbetreiber of — and nothing else's.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_zuordnungsende_is_written_back_from_its_netzbetreiber_only() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let v = laufender_vertrag(&pool, "C-LOSS").await;

    let fremd = beendigung("ce-z-1", "9900000000002", Some("20270331"));
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &fremd).await.0,
        StatusCode::OK
    );
    assert_eq!(lieferende(&pool, v).await, None, "another NB ends nothing");

    let ohne_datum = beendigung("ce-z-2", NB, None);
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &ohne_datum).await.0,
        StatusCode::OK
    );
    assert_eq!(lieferende(&pool, v).await, None, "no date is guessed");

    let eigene = beendigung("ce-z-3", NB, Some("20270331"));
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &eigene).await.0,
        StatusCode::OK
    );
    assert_eq!(lieferende(&pool, v).await, Some(date!(2027 - 03 - 31)));
    let ev = events_of(&pool, mako_events::vertrag::ZUORDNUNG_BEENDET).await;
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["data"]["wechsel"], true);
    assert_eq!(ev[0]["data"]["zuordnungsende"], "2027-03-31");
    assert_eq!(ev[0]["data"]["erp_contract_id"], "C-LOSS");
    assert_eq!(
        owed(&pool, TENANT, "ABLESUNG_ENDE").await,
        1,
        "the Schlussablesung is ordered"
    );

    // A later Zuordnungsende does not extend a supply that already ends.
    let spaeter = beendigung("ce-z-4", NB, Some("20270630"));
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &spaeter).await.0,
        StatusCode::OK
    );
    assert_eq!(lieferende(&pool, v).await, Some(date!(2027 - 03 - 31)));
}

// ── Ersatzversorgung (55013) ──────────────────────────────────────────────────

fn eog(id: &str) -> serde_json::Value {
    ce(
        id,
        mako_events::mako::PROCESS_COMPLETED,
        "22222222-2222-4222-8222-222222222222",
        serde_json::json!({
            "pid": 55013, "malo_id": MALO, "new_supplier": TENANT, "grid_operator": NB,
            "process_date": "20261201", "eog_art": "ERSATZVERSORGUNG", "haushaltskunde": true,
            "accepted": true,
            "kunde_name": { "namensformat": "Z01", "teile": ["Mustermann", "Erika"] },
        }),
    )
}

/// SR-091: an accepted Ersatzversorgung opens one contract, three months long
/// and never registered, however often its outcome is delivered.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_ersatzversorgung_opens_one_contract() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &eog("ce-e-1"))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &eog("ce-e-2"))
            .await
            .0,
        StatusCode::OK
    );

    let vertraege: Vec<(String, time::Date, Option<time::Date>)> = sqlx::query_as(
        "SELECT v.vertragsart, k.lieferbeginn, k.lieferende
           FROM versorgungsvertraege v JOIN vertragskomponenten k ON k.vertrag_id = v.id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        vertraege,
        vec![(
            "ERSATZVERSORGUNG".to_owned(),
            date!(2026 - 12 - 01),
            Some(date!(2027 - 03 - 01))
        )]
    );
    let ev = events_of(&pool, mako_events::vertrag::ERSATZVERSORGUNG_BEGONNEN).await;
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["data"]["kunde_name"], "Erika Mustermann");
    assert_eq!(ev[0]["data"]["quelle"], "55013");
    assert_eq!(
        owed(&pool, TENANT, "LIEFERBEGINN").await,
        0,
        "the NB assigned it"
    );
}

// ── Kündigung beim Vorversorger ───────────────────────────────────────────────

/// A contract asking for the Kündigung; its process id set as processd would.
async fn mit_kuendigung(pool: &PgPool, erp: &str) -> (Uuid, Uuid) {
    let kunde = make_kunde(pool, TENANT).await;
    let mut input = vertrag_input(erp);
    input.komponenten[0].vorversorger = Some(vertragd::kuendigung_vorversorger::Vorversorger {
        mp_id: VORVERSORGER.to_owned(),
        kundennummer: None,
        vollmacht: Some(vertragd::kuendigung_vorversorger::Vollmacht {
            erteilt_am: date!(2026 - 09 - 01),
            referenz: "sha256:ab".to_owned(),
        }),
    });
    input.komponenten[0].kuendigung_beim_vorversorger = Some(
        vertragd::kuendigung_vorversorger::KuendigungBeimVorversorger {
            zum: date!(2026 - 10 - 31),
        },
    );
    let v = pg::insert_versorgungsvertrag(pool, kunde, TENANT, TENANT, &input)
        .await
        .expect("create");
    let komp = v.komponenten[0].id;
    sqlx::query("UPDATE vertragskomponenten SET kuendigung_process_id = $2 WHERE id = $1")
        .bind(komp)
        .bind(format!("K-{erp}"))
        .execute(pool)
        .await
        .unwrap();
    (v.id, komp)
}

fn antwort(id: &str, process: &str, accepted: bool, code: &str) -> serde_json::Value {
    ce(
        id,
        mako_events::mako::PROCESS_COMPLETED,
        process,
        serde_json::json!({
            "pid": if accepted { 55017 } else { 55018 }, "malo_id": MALO,
            "accepted": accepted, "outcome": if accepted { "accepted" } else { "rejected" },
            "antwortcode": code, "codeliste": "E_0614",
        }),
    )
}

/// The Kündigung runs first and the Anmeldung waits for its answer; a
/// Vertragsbindung refusal under `halten` keeps it until a person releases it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_vertragsbindung_refusal_holds_the_anmeldung_until_released() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let (v, komp) = mit_kuendigung(&pool, "BIND").await;
    assert_eq!(owed(&pool, TENANT, "KUENDIGUNG_VORVERSORGER").await, 1);
    assert_eq!(owed(&pool, TENANT, "LIEFERBEGINN").await, 0);

    let refusal = antwort("ce-k-1", "K-BIND", false, "A06");
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &refusal).await.0,
        StatusCode::OK
    );
    assert_eq!(owed(&pool, TENANT, "LIEFERBEGINN").await, 0, "held");
    let ev = events_of(
        &pool,
        mako_events::vertrag::KUENDIGUNG_VORVERSORGER_ABGELEHNT,
    )
    .await;
    assert_eq!(ev[0]["data"]["klasse"], "vertragsbindung");
    assert_eq!(ev[0]["data"]["reaktion"], "gehalten");

    let path =
        format!("/api/v1/vertraege/{v}/komponenten/{komp}/kuendigung-vorversorger/freigeben");
    let (status, body) = post(&pool, "halten", &path, &serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(owed(&pool, TENANT, "LIEFERBEGINN").await, 1, "released");
    let (status, body) = post(&pool, "halten", &path, &serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "kuendigung_nicht_gehalten");
}

/// „Kein Vertrag mehr" leaves nothing to terminate, and `anmelden` sends the
/// Anmeldung even on a Vertragsbindung refusal.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_class_and_the_policy_decide_the_reaction() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    mit_kuendigung(&pool, "KEIN").await;
    let kein = antwort("ce-k-2", "K-KEIN", false, "A04");
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &kein).await.0,
        StatusCode::OK
    );
    assert_eq!(owed(&pool, TENANT, "LIEFERBEGINN").await, 1);

    mit_kuendigung(&pool, "ANM").await;
    let bindung = antwort("ce-k-3", "K-ANM", false, "A06");
    assert_eq!(
        post(&pool, "anmelden", "/api/v1/events", &bindung).await.0,
        StatusCode::OK
    );
    assert_eq!(owed(&pool, TENANT, "LIEFERBEGINN").await, 2);

    mit_kuendigung(&pool, "VOLL").await;
    let vollmacht = antwort("ce-k-4", "K-VOLL", false, "A08");
    assert_eq!(
        post(&pool, "anmelden", "/api/v1/events", &vollmacht)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        owed(&pool, TENANT, "LIEFERBEGINN").await,
        2,
        "a Vollmacht refusal holds"
    );
}

/// A new supplier's 55016 is announced for the running contract with its
/// date, weeks before the 55037 — and changes nothing on the contract.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_new_suppliers_kuendigung_is_announced_and_changes_nothing() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let v = laufender_vertrag(&pool, "C-WIN").await;
    let kuendigung = ce(
        "ce-n-1",
        mako_events::mako::PROCESS_INITIATED,
        "33333333-3333-4333-8333-333333333333",
        serde_json::json!({
            "pid": 55016, "malo_id": MALO, "sender": "9900357000011",
            "receiver": TENANT, "process_date": "20270131",
        }),
    );
    assert_eq!(
        post(&pool, "halten", "/api/v1/events", &kuendigung).await.0,
        StatusCode::OK
    );
    let ev = events_of(
        &pool,
        mako_events::vertrag::KUENDIGUNG_NEULIEFERANT_EINGEGANGEN,
    )
    .await;
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["data"]["neulieferant"], "9900357000011");
    assert_eq!(ev[0]["data"]["kuendigung_zum"], "2027-01-31");
    assert_eq!(ev[0]["data"]["erp_contract_id"], "C-WIN");
    assert_eq!(
        lieferende(&pool, v).await,
        None,
        "the 55037 ends supply, not this"
    );
}
