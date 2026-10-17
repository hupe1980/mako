//! processd's one `POST /webhook`, against real PostgreSQL: a single delivery
//! reaches every module whose types it matches.
//!
//! One subscription carries the union of what the decision modules, the
//! process read model and the Sperrauftrag queue read, so `marktd` delivers a
//! `de.mako.process.initiated` once. These pin that the one delivery projects
//! the process **and** hands it on — to the Sperrauftrag queue for an ORDERS
//! 17115, to the NB Anmeldung module for a 55001.

use std::sync::{Arc, Mutex};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

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
    Some((pool, container))
}

const OWN: &str = "9900357000004";

/// A `marktd` that refuses every read with `503` and records what was asked.
async fn unavailable_marktd() -> (String, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let log = Arc::clone(&log);
        async move {
            log.lock().expect("lock").push(uri.path().to_owned());
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

/// processd, built the way the runner builds it, serving on a free port.
async fn processd(pool: sqlx::PgPool, marktd: &str) -> String {
    let cfg: processd::config::Config = serde_json::from_value(serde_json::json!({
        "allow_insecure_no_auth": true,
        "http": {},
        "database": { "url": "postgres://unused/processd" },
        "identity": { "own_mp_id": OWN },
        "makod": { "url": marktd, "api_key": "test" },
        "marktd": { "url": marktd, "api_key": "test" },
    }))
    .expect("the configuration parses");
    let app = mako_service::build_for_test_on::<processd::Processd>(cfg, pool)
        .await
        .expect("processd builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn initiated(process: &str, pid: u32, data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "specversion": "1.0",
        "id": format!("evt-{process}"),
        "source": "makod",
        "type": mako_events::mako::PROCESS_INITIATED,
        "subject": process,
        "time": OffsetDateTime::now_utc().format(&Rfc3339).expect("rfc3339"),
        "makopid": pid,
        "data": data,
    })
}

async fn post(base: &str, event: &serde_json::Value) -> reqwest::StatusCode {
    reqwest::Client::new()
        .post(format!("{base}/webhook"))
        .json(event)
        .send()
        .await
        .expect("webhook reachable")
        .status()
}

async fn projected_pid(pool: &sqlx::PgPool, process: &str) -> Option<i32> {
    sqlx::query_scalar("SELECT pid FROM process_projections WHERE process_id = $1::uuid")
        .bind(process)
        .fetch_optional(pool)
        .await
        .expect("projection read")
}

/// An ORDERS 17115 delivered once opens the projection row and the work order;
/// delivered again, it opens neither a second time.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn one_sperrauftrag_delivery_projects_and_queues() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let (marktd, _) = unavailable_marktd().await;
    let base = processd(pool.clone(), &marktd).await;
    let process = "00000000-0000-4000-8000-0000000017a1";
    let event = initiated(
        process,
        17_115,
        serde_json::json!({
            "malo_id": "51238696012",
            "lf_mp_id": "9900012345678",
            "ausfuehrung_am": "20260901",
        }),
    );

    assert_eq!(post(&base, &event).await, reqwest::StatusCode::CREATED);
    assert_eq!(projected_pid(&pool, process).await, Some(17_115));
    let orders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sperr_orders WHERE process_id = $1")
        .bind(process)
        .fetch_one(&pool)
        .await
        .expect("orders");
    assert_eq!(orders, 1, "one delivery opens one work order");

    // AS4 ReceptionAwareness and the fan-out both redeliver.
    assert_eq!(post(&base, &event).await, reqwest::StatusCode::NO_CONTENT);
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM sperr_orders) + (SELECT COUNT(*) FROM process_projections)",
    )
    .fetch_one(&pool)
    .await
    .expect("counts");
    assert_eq!(
        rows, 2,
        "a redelivery duplicates neither the order nor the projection"
    );
}

/// A GPKE Anmeldung delivered once is projected **and** reaches the NB module.
///
/// `marktd` is down, so the NB module cannot decide and the delivery is not
/// acknowledged — the fan-out will redeliver. The projection was written
/// first and survives, and the NB module is the one that asked `marktd` for
/// the Marktlokation's supply status.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn one_anmeldung_delivery_projects_and_reaches_the_nb_module() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let (marktd, asked) = unavailable_marktd().await;
    let base = processd(pool.clone(), &marktd).await;
    let process = "00000000-0000-4000-8000-0000000550a1";
    let event = initiated(
        process,
        55_001,
        serde_json::json!({
            "malo_id": "51238696012",
            "new_supplier": "9900012345678",
            "grid_operator": OWN,
            "process_date": "2026-11-01",
        }),
    );

    assert_eq!(
        post(&base, &event).await,
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        "an undecidable Anmeldung is not acknowledged"
    );
    assert_eq!(projected_pid(&pool, process).await, Some(55_001));
    assert!(
        asked
            .lock()
            .expect("lock")
            .iter()
            .any(|p| p == "/api/v1/versorgung/51238696012"),
        "the NB module read the supply status: {:?}",
        asked.lock().expect("lock")
    );
}
