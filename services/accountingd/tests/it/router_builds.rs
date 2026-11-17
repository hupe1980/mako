//! The router builds the way `mako_service::run` mounts it.
//!
//! The runner's infra routes are merged in exactly as at boot — so a route axum
//! refuses, a merge collision with the runner's `/metrics` or `/health/*`, or a
//! `build` that panics fails a test instead of the container start. This
//! daemon's `build` connects a ledger store of its own from `[database] url`,
//! so it starts against a real PostgreSQL (`just test-accountingd-db`).

use testcontainers::{ContainerAsync, ImageExt as _, runners::AsyncRunner as _};
use testcontainers_modules::postgres::Postgres;

/// A throwaway PostgreSQL and its URL, or `None` when Docker is unavailable.
async fn postgres() -> Option<(ContainerAsync<Postgres>, String, sqlx::PgPool)> {
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    #[expect(
        clippy::disallowed_methods,
        reason = "the test connects straight to its throwaway container; pool sizing is a deployment concern"
    )]
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    Some((pg, url, pool))
}

/// Build the router from `cfg` with its `[database] url` pointed at `url`.
async fn build(mut cfg: toml::Value, url: String, pool: sqlx::PgPool) -> axum::Router {
    cfg["database"]["url"] = toml::Value::String(url);
    // One role in the throwaway container: it migrates and serves.
    if let Some(db) = cfg["database"].as_table_mut() {
        db.remove("migration_url");
    }
    let cfg = cfg
        .try_into()
        .expect("the configuration is an accountingd one");
    mako_service::build_for_test_on::<accountingd::Accountingd>(cfg, pool)
        .await
        .expect("the router builds")
}

/// The router `just dev` and `just demo` start, over a throwaway PostgreSQL, with
/// a pool on the same database and the container that must outlive both.
async fn dev_app() -> Option<(axum::Router, sqlx::PgPool, ContainerAsync<Postgres>)> {
    let (pg, url, pool) = postgres().await?;
    let cfg = toml::from_str(include_str!("../../../../deploy/demo/accountingd.toml"))
        .expect("the demo configuration parses");
    let app = build(cfg, url, pool.clone()).await;
    Some((app, pool, pg))
}

async fn call(
    app: axum::Router,
    request: axum::http::Request<axum::body::Body>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt as _;
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, body)
}

fn seal_request(period: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::post(format!("/api/v1/periods/{period}/seal"))
        .body(axum::body::Body::empty())
        .expect("request")
}

/// A ledger store that fails underneath a Festschreibung is this service's
/// fault: a `500` with an `error_id` the log line carries, never a `422` that
/// tells the caller its period was wrong. The store's own text — table and
/// schema names — stays in the log.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_ledger_store_failure_under_a_seal_answers_500() {
    let Some((app, pool, _pg)) = dev_app().await else {
        return;
    };
    sqlx::query("DROP TABLE ledger.seals CASCADE")
        .execute(&pool)
        .await
        .expect("drop the table the seal writes");

    let (status, body) = call(app, seal_request("2020-01")).await;
    assert_eq!(
        status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "{body}"
    );
    assert!(body["error_id"].is_string(), "{body}");
    assert!(!body.to_string().contains("seals"), "{body}");
}

/// The refusal half stays a refusal: a month the calendar does not define is
/// the caller's to correct, so it answers `422` with the ledger's code.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_period_the_ledger_refuses_answers_422() {
    let Some((app, _pool, _pg)) = dev_app().await else {
        return;
    };
    let (status, body) = call(app, seal_request("1999-01")).await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert!(body.get("error_id").is_none(), "{body}");
    assert_eq!(body["code"], "ledger.311", "{body}");
}

/// An entry the ledger does not hold is a `404`; a store that cannot be read
/// is a `500` with an `error_id` — not the same `404`, which would tell an
/// auditor the entry is absent from books nobody could read.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_entry_proof_tells_absence_from_a_store_failure() {
    let Some((app, pool, _pg)) = dev_app().await else {
        return;
    };
    let unknown = "/api/v1/entries/6f1c0c5e-2b7a-4c1e-9d3f-0a1b2c3d4e5f/proof";
    let get = || {
        axum::http::Request::get(unknown)
            .body(axum::body::Body::empty())
            .expect("request")
    };
    let (status, body) = call(app.clone(), get()).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{body}");

    sqlx::query("DROP TABLE ledger.entries CASCADE")
        .execute(&pool)
        .await
        .expect("drop the table the proof reads");
    let (status, body) = call(app, get()).await;
    assert_eq!(
        status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "{body}"
    );
    assert!(body["error_id"].is_string(), "{body}");
}

/// A Zahlungszuordnung the ledger refuses is the caller's to correct, so it
/// answers `422` with the reason, not a `500` that sends an operator to the
/// log. The refusal here is a Kontokorrent the ledger cannot name: an empty
/// `lf_mp_id` leaves an empty segment in the account path.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_clearing_the_ledger_refuses_answers_422() {
    let Some((app, pool, _pg)) = dev_app().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO accounts (malo_id, lf_mp_id, tenant) VALUES ('51238696012', '', '9900357000004')",
    )
    .execute(&pool)
    .await
    .expect("an account under an empty lf_mp_id");

    let (status, body) = call(
        app,
        axum::http::Request::post("/api/v1/accounts/51238696012/clear?lf_mp_id=")
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert!(body.get("error_id").is_none(), "{body}");
}

/// `deploy/demo/accountingd.toml` — what `just dev` and `just demo` start — builds a router.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_demo_config_builds_a_router() {
    let Some((_pg, url, pool)) = postgres().await else {
        return;
    };
    let cfg = toml::from_str(include_str!("../../../../deploy/demo/accountingd.toml"))
        .expect("the demo configuration parses");
    let _app = build(cfg, url, pool).await;
}

/// A payment to match carries the payer's IBAN, so it is posted as a body: no
/// access log, trace span or log line — at any level — records the IBAN, and
/// the query-string form is no route at all.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_payment_match_logs_no_iban() {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt as _;
    const IBAN: &str = "DE89370400440532013000";

    #[derive(Clone)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("buffer").extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let Some((app, _pool, _pg)) = dev_app().await else {
        return;
    };
    let buf = Buf(Arc::new(Mutex::new(Vec::new())));
    let writer = buf.clone();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("trace"))
        .with(tracing_subscriber::fmt::layer().with_writer(move || writer.clone()));
    let _guard = tracing::subscriber::set_default(subscriber);
    // Callsites another test of this binary hit with no subscriber cached
    // `Interest::never`; recompute it for this one. A thread-local subscriber
    // sees only its own thread, which is why `test-accountingd-db` runs this
    // suite with one test thread.
    tracing::callsite::rebuild_interest_cache();

    let (status, body) = call(
        app.clone(),
        axum::http::Request::post("/api/v1/payments/match")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "amount_ct": 12_500, "reference": "Rechnung 4711", "iban": IBAN,
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let log = String::from_utf8(buf.0.lock().expect("buffer").clone()).expect("utf-8");
    // The IBAN rung ran — by its hash — and was traced.
    assert!(log.contains("iban_hash"), "{log}");
    assert!(!log.contains(IBAN), "{log}");

    let (status, _) = call(
        app,
        axum::http::Request::get(format!(
            "/api/v1/payments/match?amount_ct=12500&reference=x&iban={IBAN}"
        ))
        .body(axum::body::Body::empty())
        .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::METHOD_NOT_ALLOWED);
}
