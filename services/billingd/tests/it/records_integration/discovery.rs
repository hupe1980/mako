//! Discovery evidence must survive a failed roster, an interrupted worker and concurrent retries.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::{Json, Router, routing::get};
use billingd::{billing_runs::sweep, clients::*, discovery::UnbilledPeriods};
use serde_json::{Value, json};
use time::{Duration, macros::date};
use tokio::sync::Notify;
use uuid::Uuid;

use super::{MALO, TENANT, test_pool};

fn deps(source: &str) -> Arc<BillingDeps> {
    let mut cfg: billingd::config::BillingdConfig =
        toml::from_str(include_str!("../../../../../deploy/demo/billingd.toml")).unwrap();
    cfg.tenant = TENANT.into();
    cfg.billing_runs.abrechnungsinformation = false;
    cfg.billing_runs.jahresrechnung = false;
    Arc::new(BillingDeps {
        cfg: Arc::new(cfg),
        productd: Arc::new(ProductdClient::new(source, None)),
        edmd: Arc::new(EdmdClient::new(source, None)),
        marktd: Arc::new(mako_markt::marktd_client::MarktdClient::new(
            source,
            secrecy::SecretString::from(String::new()),
            reqwest::Client::new(),
        )),
        vertragd: Arc::new(VertragdClient::new(source, None)),
        outputd: Arc::new(OutputdClient::new(source, None)),
        accountingd: None,
    })
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn discovery_retains_the_gap_until_the_roster_and_issuer_outcomes_are_recorded() {
    let (pool, _container) = test_pool("discovery")
        .await
        .expect("PostgreSQL is required");
    let today = date!(2026 - 11 - 25);
    let first = today - Duration::days(200);
    let key = || Some(Uuid::new_v4());
    let body = Arc::new(Mutex::new(json!({})));
    let fetched = Arc::new(AtomicUsize::new(0));
    let wait = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let app = Router::new().route(
        "/api/v1/vertraege/billing-candidates",
        get({
            let (body, fetched, wait, entered) =
                (body.clone(), fetched.clone(), wait.clone(), entered.clone());
            move |axum::extract::Query(q): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >| {
                let (body, fetched, wait, entered) =
                    (body.clone(), fetched.clone(), wait.clone(), entered.clone());
                async move {
                    assert!(q.contains_key("ended_from") && q.contains_key("ended_through"));
                    if fetched.fetch_add(1, Ordering::SeqCst) == 0 {
                        entered.notify_one();
                        wait.notified().await;
                    }
                    let mut response = body.lock().unwrap().clone();
                    if response.get("ended_from").is_none() {
                        response["ended_from"] = json!(q["ended_from"]);
                    }
                    response["ended_through"] = json!(q["ended_through"]);
                    Json(response)
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let deps = deps(&format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let run = {
        let deps = deps.clone();
        let pool = pool.clone();
        tokio::spawn(async move { sweep(&deps, &pool, first, None, None).await })
    };
    entered.notified().await;
    let read = || UnbilledPeriods::read(&pool, TENANT, true, None);
    assert_eq!(read().await.unwrap().discovery.unwrap().status, "running");
    assert!(
        sweep(&deps, &pool, first, None, key())
            .await
            .unwrap()
            .is_none()
    );
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    wait.notify_one();
    let failed_id = key();
    assert!(sweep(&deps, &pool, first, None, failed_id).await.is_err());
    let failed = read().await.unwrap().discovery.unwrap();
    assert_eq!(
        failed.error_code.as_deref(),
        Some("candidate_source_unavailable")
    );
    assert_eq!(failed.candidate_count, None);
    let prior_error: String = sqlx::query_scalar(
        "SELECT error_code FROM billing_discovery WHERE tenant=$1 AND origin='scheduled'",
    )
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(prior_error, "sweep_interrupted");
    let tries = fetched.load(Ordering::SeqCst);
    assert!(sweep(&deps, &pool, today, None, failed_id).await.is_err());
    assert_eq!(fetched.load(Ordering::SeqCst), tries);

    for invalid in [
        json!({"count":1,"candidates":[]}),
        json!({"count":0,"candidates":[],"ended_from":"2000-01-01"}),
    ] {
        *body.lock().unwrap() = invalid;
        assert!(sweep(&deps, &pool, today, None, key()).await.is_err());
        assert_eq!(
            read().await.unwrap().discovery.unwrap().candidate_count,
            None
        );
    }
    *body.lock().unwrap() = json!({"count":0,"candidates":[]});
    let empty_id = key();
    assert_eq!(
        sweep(&deps, &pool, today, None, empty_id).await.unwrap(),
        empty_id
    );
    let empty = read().await.unwrap().discovery.unwrap();
    assert_eq!(empty.candidate_count, Some(0));
    assert_eq!(empty.ended_through, first);
    assert!(empty.last_failure_at.is_some());
    let tries = fetched.load(Ordering::SeqCst);
    assert_eq!(
        sweep(&deps, &pool, today, None, empty_id).await.unwrap(),
        empty_id
    );
    assert_eq!(fetched.load(Ordering::SeqCst), tries);
    assert!(
        UnbilledPeriods::read(&pool, "another-tenant", true, None)
            .await
            .unwrap()
            .discovery
            .is_none()
    );
    let roster: Vec<Value> = ["9900257000001","9900357000003"].iter().map(|lf|json!({"malo_id":MALO,"lf_mp_id":lf,"sparte":"STROM","abrechnungszyklus":"JAEHRLICH","vertragsbeginn":first,"lieferbeginn":first,"lieferende":first+Duration::DAY,"beendet":true})).collect();
    for (field, value) in [
        ("abrechnungszyklus", json!("UNKNOWN")),
        ("lieferende", json!(first)),
        ("lieferende", Value::Null),
        ("lieferbeginn", json!(first + Duration::days(3))),
    ] {
        let mut row = roster[0].clone();
        row[field] = value;
        *body.lock().unwrap() = json!({"count":1,"candidates":[row]});
        assert!(sweep(&deps, &pool, today, None, key()).await.is_err());
        assert_eq!(
            read()
                .await
                .unwrap()
                .discovery
                .unwrap()
                .error_code
                .as_deref(),
            Some("candidate_source_unavailable")
        );
    }
    *body.lock().unwrap() = json!({"count":2,"candidates":roster});
    sqlx::query(
        "ALTER TABLE unbilled_periods ADD CONSTRAINT fixture_block_write CHECK (false) NOT VALID",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(sweep(&deps, &pool, today, None, key()).await.is_err());
    let failed = read().await.unwrap().discovery.unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("evidence_write_failed"));
    let issuer_errors: Vec<i32> = sqlx::query_scalar(
        "SELECT errors FROM billing_discovery_issuers WHERE tenant=$1 AND run_id=$2",
    )
    .bind(TENANT)
    .bind(failed.run_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(issuer_errors, [1, 1]);
    sqlx::query("ALTER TABLE unbilled_periods DROP CONSTRAINT fixture_block_write")
        .execute(&pool)
        .await
        .unwrap();
    sweep(&deps, &pool, today, None, key()).await.unwrap();
    let result = read().await.unwrap();
    let completed = result.discovery.unwrap();
    assert_eq!(completed.candidate_count, Some(2));
    assert_eq!(completed.ended_from, first.next_day().unwrap());
    assert_eq!(completed.ended_from, failed.ended_from);
    assert_eq!(result.count, 2);
    assert!(
        result
            .periods
            .iter()
            .all(|p| p.final_invoice && p.reason_code == "ABSCHLAEGE_UNBEKANNT")
    );
    assert_ne!(result.periods[0].lf_mp_id, result.periods[1].lf_mp_id);
    server.abort();
}

#[tokio::test]
async fn the_contract_price_modules_and_recipient_require_the_same_issuer_and_period() {
    let from = date!(2026 - 09 - 01);
    let to = date!(2026 - 09 - 30);
    let issuer = "9900357000003";
    let captured = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback(get({
        let captured = captured.clone();
        move |uri: axum::http::Uri,
              axum::extract::Query(q): axum::extract::Query<
            std::collections::HashMap<String, String>,
        >| {
            let captured = captured.clone();
            async move {
                assert_eq!(q["from"], "2026-09-01");
                assert_eq!(q["to"], "2026-09-30");
                assert_eq!(q["lf_mp_id"], "9900357000003");
                captured.lock().unwrap().push(uri.path().to_owned());
                axum::http::StatusCode::CONFLICT
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let client = VertragdClient::new(&url, None);
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for error in [
        client
            .get_product_slices(MALO, issuer, from, to)
            .await
            .unwrap_err(),
        client
            .get_sect14a(MALO, issuer, from, to)
            .await
            .unwrap_err(),
        client
            .get_supply_contract(MALO, issuer, from, to)
            .await
            .unwrap_err(),
    ] {
        assert_eq!(error.code(), "AMBIGUOUS_SUPPLY_PERIOD");
    }
    let advances = AccountingdClient::new(&url, None);
    assert!(
        advances
            .get_abschlaege(MALO, issuer, from, to)
            .await
            .is_err()
    );
    assert_eq!(
        *captured.lock().unwrap(),
        [
            format!("/api/v1/malo/{MALO}/produkte"),
            format!("/api/v1/malo/{MALO}/sect14a"),
            format!("/api/v1/vertraege/by-malo/{MALO}"),
            format!("/api/v1/accounts/{MALO}/abschlaege"),
        ]
    );
    server.abort();
}
