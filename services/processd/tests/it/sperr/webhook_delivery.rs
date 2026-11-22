//! A de.mako.sperr.* notice reaches the webhook endpoint `default` — `[sperr]
//! erp_webhook_url` — through the shared subscription module processd mounts.
//!
//! Run with `just test-processd-db`, or:
//! `cargo test -p processd --test it -- webhook_delivery:: --include-ignored`
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::{Arc, Mutex};

use processd::sperr::model::OrderType;
use processd::sperr::pg::{self, CreateOrderRequest, Treffpunkt};
use sqlx::PgPool;
use tower::ServiceExt as _;

const TENANT: &str = mako_service::oidc::demo::TENANT;
const SECRET: &str = "dev-sperr-hmac-secret";

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;
type Received = Arc<Mutex<Vec<(axum::http::HeaderMap, Vec<u8>)>>>;

/// A throwaway `postgres:17-alpine` and its URL; `None` when Docker is
/// unavailable.
async fn postgres() -> Option<(PgPool, String, PgContainer)> {
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
    let pool = PgPool::connect(&url).await.ok()?;
    Some((pool, url, container))
}

async fn stand_in(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// An ERP that records every delivery it is sent.
async fn erp() -> (Received, String) {
    let got: Received = Arc::default();
    let state = Arc::clone(&got);
    let url = stand_in(axum::Router::new().route(
        "/events",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let got = Arc::clone(&state);
                async move {
                    got.lock().unwrap().push((headers, body.to_vec()));
                    axum::http::StatusCode::OK
                }
            },
        ),
    ))
    .await;
    (got, format!("{url}/events"))
}

fn order() -> CreateOrderRequest {
    CreateOrderRequest {
        malo_id: "51238696012".to_owned(),
        lf_mp_id: "9900012345678".to_owned(),
        order_type: OrderType::Sperrung,
        process_id: Some(format!("proc-{}", uuid::Uuid::new_v4().simple())),
        ausfuehrung_am: Some(time::macros::date!(2026 - 09 - 01)),
        fruehestens_am: None,
        arbeitszeit: None,
        treffpunkt: Treffpunkt {
            hinweis: None,
            strasse: None,
            plz: None,
            ort: None,
            land: None,
        },
        hinweis: None,
        external_ref: None,
    }
}

async fn call(app: &axum::Router, method: &str, uri: &str) -> (u16, serde_json::Value) {
    let res = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status().as_u16();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

/// processd's migration creates the subscription tables, its build writes the
/// endpoint `default` from `[sperr]`, a cancellation through the REST route
/// enqueues `de.mako.sperr.storniert`, and the subscription worker delivers it
/// signed with `[sperr] erp_hmac_secret`, after the order's
/// `de.mako.sperr.auftrag.eingegangen`. A row enqueued for a named target —
/// the §38 EnWG notice — is its own worker's and never reaches `default`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_sperr_notice_reaches_the_default_endpoint() {
    let Some((pool, url, _pg)) = postgres().await else {
        return;
    };
    let (got, erp_url) = erp().await;
    let marktd =
        stand_in(axum::Router::new().fallback(|| async { axum::http::StatusCode::OK })).await;

    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../../deploy/demo/processd.toml")).expect("parses");
    cfg["database"]["url"] = toml::Value::String(url);
    if let Some(db) = cfg["database"].as_table_mut() {
        db.remove("migration_url");
    }
    cfg["marktd"]["url"] = toml::Value::String(marktd);
    let mut sperr = toml::Table::new();
    sperr.insert("erp_webhook_url".into(), erp_url.clone().into());
    sperr.insert("erp_hmac_secret".into(), SECRET.into());
    cfg.as_table_mut()
        .unwrap()
        .insert("sperr".into(), sperr.into());
    let app = mako_service::build_for_test_on::<processd::Processd>(
        cfg.try_into().expect("a processd configuration"),
        pool.clone(),
    )
    .await
    .expect("the router builds");

    // The endpoint `default` is written in the background; the REST list
    // shows it once it is.
    let mut listed = serde_json::Value::Null;
    for _ in 0..50 {
        let (status, body) = call(&app, "GET", "/api/v1/webhooks/endpoints").await;
        assert_eq!(status, 200, "{body}");
        if body["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            listed = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(listed["items"][0]["name"], "default", "{listed}");
    assert_eq!(listed["items"][0]["url"], erp_url.as_str(), "{listed}");

    let id = pg::create_order_pg(&pool, TENANT, &order())
        .await
        .unwrap()
        .unwrap();
    let (status, body) = call(&app, "PUT", &format!("/api/v1/sperr-orders/{id}/cancel")).await;
    assert_eq!(status, 204, "{body}");

    // A §38 EnWG notice for its own target, beside it on the outbox.
    let notice = mako_service::cloudevent::CloudEvent::new(
        "processd/test",
        "de.mako.eog.expiring",
        "51238696012",
        serde_json::json!({}),
    );
    let mut conn = pool.acquire().await.unwrap();
    mako_service::outbox::enqueue_to(&mut conn, processd::eog_module::EOG_NOTIFY_TARGET, &notice)
        .await
        .unwrap();
    drop(conn);

    // `build_for_test_on` stops the workers `build` spawned once it returns;
    // this is the worker `build` starts, over the same tables.
    let subs = mako_service::subscriptions::Subscriptions::new(
        pool.clone(),
        "processd",
        TENANT,
        mako_service::subscriptions::SubscriptionsConfig::default(),
    );
    subs.drain().await;

    let got = got.lock().unwrap().clone();
    let types: Vec<String> = got
        .iter()
        .map(|(_, b)| {
            serde_json::from_slice::<serde_json::Value>(b).unwrap()["type"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(
        types,
        [
            mako_events::sperr::AUFTRAG_EINGEGANGEN,
            mako_events::sperr::STORNIERT
        ],
        "the order's two notices, in commit order, and not the targeted row"
    );
    let (headers, body) = &got[1];
    mako_service::webhook::verify_request(Some(SECRET.as_bytes()), headers, body)
        .expect("signed with [sperr] erp_hmac_secret");
    let event: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(event["type"], mako_events::sperr::STORNIERT, "{event}");
    assert_eq!(event["data"]["order_id"], id.to_string(), "{event}");
    assert_eq!(
        headers.get("webhook-id").and_then(|v| v.to_str().ok()),
        event["id"].as_str(),
        "the receiver deduplicates on the CloudEvent id"
    );

    let fanned: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT fanned_out_at FROM event_outbox WHERE event_id = $1")
            .bind(&notice.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(fanned.is_none(), "a targeted row is not fanned out");
}
