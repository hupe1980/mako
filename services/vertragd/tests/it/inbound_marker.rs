//! The inbound idempotency marker commits with the work it marks.
//!
//! `received_events` answers a redelivery with 200 and does nothing. A marker
//! committed before its work therefore turns any failure after it into a
//! permanent one: `makod` retries, the retry is deduplicated, and the
//! Vertragskomponente never reaches `BESTAETIGT` while the market considers
//! supply started. Each test fails the work once, then redelivers the same
//! CloudEvent and expects the work done.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::Arc;

use reqwest::StatusCode;
use sqlx::PgPool;
use vertragd::pg;

use super::dispatch_integration::{PgContainer, make_kunde, pg_container, vertrag_input};

const SCHEMA: &str = include_str!("../../migrations/0001_schema.sql");
const TENANT: &str = "9800000000023";

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

fn app(pool: &PgPool) -> axum::Router {
    let cfg: vertragd::config::VertragdConfig = serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://unused" },
        "tenant": TENANT,
        "lf_mp_id": TENANT,
        "processd_url": "http://processd.invalid",
        "accountingd_url": "http://accountingd.invalid",
        "edmd_url": "http://edmd.invalid",
        "allow_insecure_no_auth": true,
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
}

async fn post(pool: &PgPool, path: &str, body: &serde_json::Value) -> StatusCode {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app(pool);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(body)
        .send()
        .await
        .unwrap()
        .status()
}

async fn sql(pool: &PgPool, stmt: &'static str) {
    sqlx::raw_sql(stmt).execute(pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_confirmation_that_fails_once_is_applied_on_redelivery() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let kunde = make_kunde(&pool, TENANT).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, TENANT, TENANT, &vertrag_input("C-IN"))
            .await
            .expect("create");
    sqlx::query("UPDATE vertragskomponenten SET mako_process_id = 'P-1' WHERE vertrag_id = $1")
        .bind(created.id)
        .execute(&pool)
        .await
        .unwrap();
    let ce = serde_json::json!({
        "specversion": "1.0",
        "id": "ce-confirm-1",
        "source": "/makod/test",
        "type": mako_events::mako::PROCESS_COMPLETED,
        "data": { "process_id": "P-1", "accepted": true, "malo_id": "51238696012" },
    });

    // The work after the marker fails: the follow-up obligations cannot be
    // enqueued.
    sql(
        &pool,
        "ALTER TABLE event_outbox RENAME TO event_outbox_away",
    )
    .await;
    assert!(post(&pool, "/api/v1/events", &ce).await.is_server_error());
    sql(
        &pool,
        "ALTER TABLE event_outbox_away RENAME TO event_outbox",
    )
    .await;

    // makod redelivers the same event.
    assert_eq!(post(&pool, "/api/v1/events", &ce).await, StatusCode::OK);
    let status: String =
        sqlx::query_scalar("SELECT status FROM vertragskomponenten WHERE vertrag_id = $1")
            .bind(created.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "BESTAETIGT",
        "the redelivery must apply the confirmation, not be deduplicated away"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_accepted_angebot_that_fails_once_is_contracted_on_redelivery() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let angebot_id = uuid::Uuid::new_v4();
    let ce = serde_json::json!({
        "specversion": "1.0",
        "id": "ce-angebot-1",
        "source": "/productd/test",
        "type": mako_events::tarif::ANGEBOT_ANGENOMMEN,
        "subject": angebot_id.to_string(),
        "data": {
            "angebot_id": angebot_id.to_string(),
            "angebotsnummer": "A-1",
            "interessent_name": "Muster GmbH",
            "lieferbeginn": "2027-01-01",
            "laufzeit_monate": 12,
        },
    });

    sql(
        &pool,
        "ALTER TABLE rahmenvertraege RENAME TO rahmenvertraege_away",
    )
    .await;
    assert!(
        post(&pool, "/api/v1/webhooks/angebot", &ce)
            .await
            .is_server_error()
    );
    sql(
        &pool,
        "ALTER TABLE rahmenvertraege_away RENAME TO rahmenvertraege",
    )
    .await;

    assert_eq!(
        post(&pool, "/api/v1/webhooks/angebot", &ce).await,
        StatusCode::CREATED,
        "the redelivery must contract the Angebot, not be deduplicated away"
    );
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rahmenvertraege WHERE erp_rahmenvertrag_id = $1")
            .bind(angebot_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 1);

    // A third delivery is a replay.
    assert_eq!(
        post(&pool, "/api/v1/webhooks/angebot", &ce).await,
        StatusCode::OK
    );
}
