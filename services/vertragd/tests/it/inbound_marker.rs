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

/// The contract's § 13 BGB Verbraucher status reaches accountingd with its own
/// command when the NB confirms, as a value or as an explicit `null` — the
/// contract is its one source, so an unrecorded status clears a stale one
/// rather than leaving it in place.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_confirmation_sends_the_contracts_verbraucher_status() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let kunde = make_kunde(&pool, TENANT).await;
    for (n, status) in [Some(false), Some(true), None].into_iter().enumerate() {
        let mut input = vertrag_input(&format!("C-VB-{n}"));
        input.verbraucher = status;
        let created = pg::insert_versorgungsvertrag(&pool, kunde, TENANT, TENANT, &input)
            .await
            .expect("create");
        let process = format!("P-VB-{n}");
        sqlx::query("UPDATE vertragskomponenten SET mako_process_id = $1 WHERE vertrag_id = $2")
            .bind(&process)
            .bind(created.id)
            .execute(&pool)
            .await
            .unwrap();
        let ce = serde_json::json!({
            "specversion": "1.0",
            "id": format!("ce-vb-{n}"),
            "source": "/makod/test",
            "type": mako_events::mako::PROCESS_COMPLETED,
            "data": { "process_id": process, "accepted": true, "malo_id": "51238696012" },
        });
        assert_eq!(post(&pool, "/api/v1/events", &ce).await, StatusCode::OK);
        let body: serde_json::Value = sqlx::query_scalar(
            "SELECT envelope->'body' FROM event_outbox
              WHERE kind = 'command' AND ce_type = 'VERBRAUCHER'
                AND event_id LIKE '%' || $1",
        )
        .bind(created.komponenten[0].id.to_string())
        .fetch_one(&pool)
        .await
        .expect("one Verbraucher command per confirmed component");
        assert_eq!(
            body,
            serde_json::json!({ "verbraucher": status }),
            "{status:?}"
        );
    }
}

/// The grid operator's answer to one component is announced with the
/// references of whoever handed the contract over — and a refusal carries the
/// Antwortcode with the Codeliste that defines it, which is also what the
/// component stores.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_grid_operators_answer_is_announced_with_the_callers_reference() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let kunde = make_kunde(&pool, TENANT).await;
    for (reference, process, data) in [
        (
            "C-OK",
            "P-OK",
            serde_json::json!({ "process_id": "P-OK", "accepted": true, "malo_id": "51238696012" }),
        ),
        (
            "C-NO",
            "P-NO",
            serde_json::json!({
                "process_id": "P-NO", "accepted": false, "outcome": "rejected",
                "antwortcode": "A02", "codeliste": "E_0622", "reason": "abgelehnt",
            }),
        ),
    ] {
        let created =
            pg::insert_versorgungsvertrag(&pool, kunde, TENANT, TENANT, &vertrag_input(reference))
                .await
                .expect("create");
        sqlx::query("UPDATE vertragskomponenten SET mako_process_id = $1 WHERE vertrag_id = $2")
            .bind(process)
            .bind(created.id)
            .execute(&pool)
            .await
            .unwrap();
        let ce = serde_json::json!({
            "specversion": "1.0", "id": format!("ce-{process}"), "source": "/makod/test",
            "type": mako_events::mako::PROCESS_COMPLETED, "data": data,
        });
        assert_eq!(post(&pool, "/api/v1/events", &ce).await, StatusCode::OK);
    }

    let event = |ce_type: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT envelope FROM event_outbox WHERE ce_type = $1",
            )
            .bind(ce_type)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    let confirmed = event(mako_events::vertrag::LIEFERBEGINN_BESTAETIGT).await;
    assert_eq!(confirmed.len(), 1, "{confirmed:?}");
    let data = &confirmed[0]["data"];
    assert_eq!(data["erp_contract_id"], "C-OK");
    assert_eq!(data["kunden_id"], kunde.to_string());
    assert_eq!(data["malo_id"], "51238696012");
    assert!(data["lieferbeginn"].is_string() && data["komp_id"].is_string());
    assert_eq!(confirmed[0]["subject"], data["vertrag_id"]);
    assert!(
        confirmed[0]["dataschema"]
            .as_str()
            .is_some_and(|s| s.ends_with("de.mako.vertrag.lieferbeginn.bestaetigt.json")),
        "{}",
        confirmed[0]
    );

    let refused = event(mako_events::vertrag::LIEFERBEGINN_ABGELEHNT).await;
    assert_eq!(refused.len(), 1, "{refused:?}");
    let data = &refused[0]["data"];
    assert_eq!(data["erp_contract_id"], "C-NO");
    assert_eq!(data["antwortcode"], "A02");
    assert_eq!(data["codeliste"], "E_0622");
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT k.abgelehnt_erc FROM vertragskomponenten k
           JOIN versorgungsvertraege v ON v.id = k.vertrag_id WHERE v.erp_contract_id = 'C-NO'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some("A02"),
        "the component keeps the code"
    );

    // The caller finds its contract by its own reference, whatever became of it.
    let found = pg::list_vertraege_by_referenz(&pool, TENANT, Some("C-NO"), None)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(
        pg::list_vertraege_by_referenz(&pool, "another-tenant", Some("C-NO"), None)
            .await
            .unwrap()
            .is_empty()
    );
    // The portfolio row says what the contract is.
    let portfolio = pg::list_portfolio_by_kunde(&pool, kunde, TENANT)
        .await
        .unwrap();
    assert!(
        portfolio
            .iter()
            .all(|row| row.vertragsart == "SONDERVERTRAG"),
        "{portfolio:?}"
    );
    assert!(!portfolio.is_empty());
}
