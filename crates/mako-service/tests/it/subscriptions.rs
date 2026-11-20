//! Real-PostgreSQL suite for `mako_service::subscriptions`, driven through the
//! module's routes as a service mounts them (the reference mount below) and a
//! scriptable receiver. `#[ignore]`d (Docker required):
//!
//! ```bash
//! just test-outbox-db
//! ```

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mako_service::CloudEvent;
use mako_service::outbox::{self, OutboxConfig};
use mako_service::subscriptions::{self, Partner, Subscriptions, SubscriptionsConfig};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use testcontainers::ImageExt as _;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tower::ServiceExt as _;
use utoipa_axum::router::OpenApiRouter;

const TENANT: &str = "t1";

async fn pg_pool() -> sqlx::PgPool {
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("start postgres");
    let port = container.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    Box::leak(Box::new(container));
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("connect");
    outbox::ensure_schema(&pool).await.expect("outbox schema");
    subscriptions::ensure_schema(&pool)
        .await
        .expect("subscription schema");
    pool
}

// ── Receiver ────────────────────────────────────────────────────────────────

/// One request as received: webhook-id, webhook-timestamp, webhook-signature, body.
type Got = (String, String, String, Vec<u8>);

#[derive(Default)]
struct Receiver {
    /// Event ids answered 500.
    failing: Mutex<HashSet<String>>,
    /// Answer every request with this status instead.
    status: Mutex<Option<u16>>,
    /// (webhook-id, webhook-timestamp, webhook-signature, body), in arrival order.
    got: Mutex<Vec<Got>>,
}

impl Receiver {
    fn ids(&self) -> Vec<String> {
        self.got
            .lock()
            .unwrap()
            .iter()
            .map(|g| g.0.clone())
            .collect()
    }
}

async fn receiver() -> (Arc<Receiver>, String) {
    let rx = Arc::new(Receiver::default());
    let state = Arc::clone(&rx);
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let rx = Arc::clone(&state);
                async move {
                    let h = |n: &str| {
                        headers
                            .get(n)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default()
                            .to_owned()
                    };
                    let id = h("webhook-id");
                    rx.got.lock().unwrap().push((
                        id.clone(),
                        h("webhook-timestamp"),
                        h("webhook-signature"),
                        body.to_vec(),
                    ));
                    if let Some(s) = *rx.status.lock().unwrap() {
                        return StatusCode::from_u16(s).unwrap();
                    }
                    if rx.failing.lock().unwrap().contains(&id) {
                        StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        StatusCode::OK
                    }
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (rx, format!("http://127.0.0.1:{}/hook", addr.port()))
}

// ── The reference mount ─────────────────────────────────────────────────────

fn module(pool: &sqlx::PgPool) -> Subscriptions {
    Subscriptions::new(
        pool.clone(),
        "testd",
        TENANT,
        SubscriptionsConfig {
            retention_days: 30,
            allow_loopback: true,
            endpoints: Vec::new(),
        },
    )
    .with_policy(OutboxConfig {
        max_attempts: 3,
        backoff: vec![Duration::ZERO],
        lease: Duration::ZERO,
        ..OutboxConfig::default()
    })
}

const POLICY: &str = r#"permit(principal, action in [Action::"manage-subscription", Action::"read-events"], resource)
    when { context.principal_tenant == context.resource_tenant };"#;

/// The service's `/api/v1` router with the module merged in, behind the
/// layers a daemon supplies; a request carrying `x-partner` is marked as that
/// partner's, as a service's partner authentication would.
fn app(subs: &Subscriptions, policy: &str) -> (axum::Router, utoipa::openapi::OpenApi) {
    let (router, doc) = OpenApiRouter::<()>::new()
        .merge(subs.router())
        .split_for_parts();
    let cedar = mako_service::cedar::CedarEnforcer::from_policy_str(policy).unwrap();
    let app = router
        .layer(axum::middleware::from_fn(
            |mut req: Request<Body>, next: axum::middleware::Next| async move {
                if let Some(p) = req.headers().get("x-partner").and_then(|v| v.to_str().ok()) {
                    let p = Partner(p.to_owned());
                    req.extensions_mut().insert(p);
                }
                next.run(req).await
            },
        ))
        .layer(axum::Extension(Arc::new(cedar)))
        .layer(axum::Extension(mako_service::oidc::OidcVerifier::disabled(
            TENANT,
        )));
    (app, doc)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, value)
}

async fn create(app: &axum::Router, name: &str, url: &str, partner: Option<&str>) -> Value {
    let headers: Vec<(&str, &str)> = partner.map(|p| ("x-partner", p)).into_iter().collect();
    let (status, _, body) = call(
        app,
        "POST",
        "/api/v1/webhooks/endpoints",
        &headers,
        Some(json!({ "name": name, "url": url })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn emit(pool: &sqlx::PgPool, ty: &str, subject: &str, partner: Option<&str>) -> String {
    let ce = CloudEvent::new(
        mako_service::source("testd", TENANT),
        ty,
        subject,
        json!({ "n": subject }),
    )
    .extension_opt(subscriptions::PARTNER_EXTENSION, partner);
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.commit().await.unwrap();
    ce.id
}

async fn delivery_state(pool: &sqlx::PgPool, event_id: &str) -> (Option<i16>, bool, bool) {
    sqlx::query_as(
        "SELECT last_status, delivered_at IS NOT NULL, dead_lettered_at IS NOT NULL
           FROM webhook_deliveries WHERE event_id = $1",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ── Endpoints ───────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs Docker"]
async fn the_secret_is_shown_once() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let created = create(&app, "erp", "https://erp.example.com/hook", None).await;
    let secret = created["secret"].as_str().unwrap();
    assert!(secret.starts_with("whsec_"), "{created}");
    let id = created["endpoint"]["id"].as_str().unwrap();
    let (status, headers, one) = call(
        &app,
        "GET",
        &format!("/api/v1/webhooks/endpoints/{id}"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["etag"], "\"1\"");
    let (_, _, list) = call(&app, "GET", "/api/v1/webhooks/endpoints", &[], None).await;
    for body in [one.to_string(), list.to_string()] {
        assert!(!body.contains(secret), "the secret is shown again: {body}");
        assert!(!body.contains("whsec_"), "{body}");
    }
    let (status, _, refused) = call(
        &app,
        "POST",
        "/api/v1/webhooks/endpoints",
        &[],
        Some(json!({ "name": "erp", "url": "https://erp.example.com/other" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "endpoint_name_taken");
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn the_config_endpoint_cannot_be_written() {
    let pool = pg_pool().await;
    let subs =
        module(&pool).with_default_endpoint(Some("http://erp-adapter:8080/events".into()), None);
    subs.sync_config().await.unwrap();
    let (app, _) = app(&subs, POLICY);
    let (_, _, list) = call(&app, "GET", "/api/v1/webhooks/endpoints", &[], None).await;
    let default = &list["items"][0];
    assert_eq!(default["name"], "default");
    assert_eq!(default["owner"], "config");
    let id = default["id"].as_str().unwrap();
    let uri = format!("/api/v1/webhooks/endpoints/{id}");
    let (status, _, put) = call(
        &app,
        "PUT",
        &uri,
        &[("if-match", "\"1\"")],
        Some(json!({ "url": "https://evil.example.com/", "active": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(put["code"], "endpoint_config_owned");
    let (status, _, del) = call(&app, "DELETE", &uri, &[("if-match", "\"1\"")], None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(del["code"], "endpoint_config_owned");
    let (status, _, reserved) = call(
        &app,
        "POST",
        "/api/v1/webhooks/endpoints",
        &[],
        Some(json!({ "name": "default", "url": "https://erp.example.com/" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reserved["code"], "endpoint_name_reserved");
}

/// A `[[subscriptions.endpoints]]` entry is config-owned like `default`: it
/// receives the events its filter matches, signed with its configured secret,
/// is refused for writes, and is deactivated once the config drops it.
#[tokio::test]
#[ignore = "needs Docker"]
async fn a_config_endpoint_receives_its_filter_and_leaves_with_the_config() {
    let pool = pg_pool().await;
    let (rx, url) = receiver().await;
    let cfg: SubscriptionsConfig = toml::from_str(&format!(
        r#"
        [[endpoints]]
        name = "makod"
        url = "{url}"
        secret = "in-cluster-secret"
        event_types = ["de.mako.markt.malo.*"]
        extensions = {{ marktsparte = "strom" }}
        "#
    ))
    .expect("the section parses");
    assert!(cfg.befunde().is_empty());
    let subs = Subscriptions::new(pool.clone(), "testd", TENANT, cfg).with_policy(OutboxConfig {
        backoff: vec![Duration::ZERO],
        lease: Duration::ZERO,
        ..OutboxConfig::default()
    });
    subs.sync_config().await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let src = mako_service::source("testd", TENANT);
    let ce = |ty: &str, sparte: &str| {
        CloudEvent::new(src.clone(), ty, "51238696012", json!({})).extension("marktsparte", sparte)
    };
    let strom = ce("de.mako.markt.malo.updated", "strom");
    let gas = ce("de.mako.markt.malo.updated", "gas");
    let other = ce("de.mako.markt.partner.updated", "strom");
    for ev in [&strom, &gas, &other] {
        outbox::enqueue(&mut conn, ev).await.unwrap();
    }
    drop(conn);
    subs.drain().await;
    assert_eq!(
        rx.ids(),
        std::slice::from_ref(&strom.id),
        "only the Strom MaLo event matches"
    );
    let got = rx.got.lock().unwrap()[0].clone();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("webhook-id", got.0.parse().unwrap());
    headers.insert("webhook-timestamp", got.1.parse().unwrap());
    headers.insert("webhook-signature", got.2.parse().unwrap());
    mako_service::webhook::verify_request(Some(b"in-cluster-secret"), &headers, &got.3)
        .expect("signed with the configured secret");

    let (app, _) = app(&subs, POLICY);
    let (_, _, list) = call(&app, "GET", "/api/v1/webhooks/endpoints", &[], None).await;
    assert_eq!(list["items"][0]["name"], "makod");
    assert_eq!(list["items"][0]["owner"], "config");
    let uri = format!(
        "/api/v1/webhooks/endpoints/{}",
        list["items"][0]["id"].as_str().unwrap()
    );
    let (status, _, del) = call(&app, "DELETE", &uri, &[("if-match", "\"1\"")], None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(del["code"], "endpoint_config_owned");

    let gone = Subscriptions::new(
        pool.clone(),
        "testd",
        TENANT,
        SubscriptionsConfig::default(),
    );
    gone.sync_config().await.unwrap();
    let (active, reason): (bool, Option<String>) = sqlx::query_as(
        "SELECT active, deactivated_reason FROM webhook_endpoints WHERE name = 'makod'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!active);
    assert_eq!(reason.as_deref(), Some("config_removed"));
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_literal_loopback_is_refused_at_registration() {
    let pool = pg_pool().await;
    let subs = Subscriptions::new(
        pool.clone(),
        "testd",
        TENANT,
        SubscriptionsConfig::default(),
    );
    let (app, _) = app(&subs, POLICY);
    for url in [
        "https://127.0.0.1/hook",
        "https://10.1.2.3/",
        "http://erp.example.com/",
    ] {
        let (status, _, body) = call(
            &app,
            "POST",
            "/api/v1/webhooks/endpoints",
            &[],
            Some(json!({ "name": "x", "url": url })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{url}");
        assert_eq!(body["code"], "endpoint_url_not_allowed", "{url}");
    }
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_write_without_the_action_is_forbidden() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let only_feed = r#"permit(principal, action == Action::"read-events", resource);"#;
    let (app, _) = app(&subs, only_feed);
    let (status, _, _) = call(&app, "GET", "/api/v1/webhooks/endpoints", &[], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, "GET", "/api/v1/events", &[], None).await;
    assert_eq!(status, StatusCode::OK);
}

// ── Delivery ────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_rotation_keeps_at_most_two_secrets() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (rx, url) = receiver().await;
    let created = create(&app, "erp", &url, None).await;
    let first = created["secret"].as_str().unwrap().to_owned();
    let id = created["endpoint"]["id"].as_str().unwrap().to_owned();
    let rotate_uri = format!("/api/v1/webhooks/endpoints/{id}/rotate-secret");
    let rotate = |hours: u32| {
        call(
            &app,
            "POST",
            &rotate_uri,
            &[],
            Some(json!({ "overlap_hours": hours })),
        )
    };
    let (status, _, too_long) = rotate(169).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(too_long["code"], "overlap_too_long");
    let (_, _, second) = rotate(24).await;
    let (_, _, third) = rotate(24).await;
    let second = second["secret"].as_str().unwrap().to_owned();
    let third = third["secret"].as_str().unwrap().to_owned();

    emit(&pool, "de.mako.vertrag.kuendigung", "V-1", None).await;
    subs.drain().await;
    let got = rx.got.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    let (wid, ts, sig, body) = &got[0];
    assert_eq!(
        sig.split(' ').count(),
        2,
        "two active secrets give two signatures: {sig}"
    );
    let ts: i64 = ts.parse().unwrap();
    let verifies = |s: &str| {
        mako_service::webhook::verify_signature(
            &mako_service::webhook::signing_key(s),
            wid,
            ts,
            body,
            sig,
        )
    };
    assert!(verifies(&third) && verifies(&second));
    assert!(!verifies(&first), "a third secret was still signing");
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn an_earlier_event_for_the_same_subject_holds_the_next() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (rx, url) = receiver().await;
    create(&app, "erp", &url, None).await;
    let a1 = emit(&pool, "de.mako.vertrag.aktiv", "A", None).await;
    let a2 = emit(&pool, "de.mako.vertrag.gekuendigt", "A", None).await;
    let b1 = emit(&pool, "de.mako.vertrag.aktiv", "B", None).await;
    rx.failing.lock().unwrap().insert(a1.clone());

    subs.fan_out_once().await.unwrap();
    subs.deliver_once().await.unwrap();
    let first: HashSet<String> = rx.ids().into_iter().collect();
    assert_eq!(
        first,
        HashSet::from([a1.clone(), b1.clone()]),
        "A2 waits, B1 does not"
    );

    subs.deliver_once().await.unwrap();
    assert_eq!(
        rx.ids().iter().filter(|i| **i == a2).count(),
        0,
        "A2 waits while A1 retries"
    );

    rx.failing.lock().unwrap().clear();
    subs.drain().await;
    let order = rx.ids();
    let last_a1 = order.iter().rposition(|i| *i == a1).unwrap();
    let a2_at = order.iter().position(|i| *i == a2).expect("A2 delivered");
    assert!(last_a1 < a2_at, "{order:?}");
    assert!(delivery_state(&pool, &a2).await.1);
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn other_subjects_do_not_wait() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (rx, url) = receiver().await;
    create(&app, "erp", &url, None).await;
    let a1 = emit(&pool, "de.mako.vertrag.aktiv", "A", None).await;
    rx.failing.lock().unwrap().insert(a1.clone());
    // Dead-letter A1: its spent budget stops it blocking A.
    subs.drain().await;
    let (_, delivered, dead) = delivery_state(&pool, &a1).await;
    assert!(!delivered && dead);
    let a2 = emit(&pool, "de.mako.vertrag.gekuendigt", "A", None).await;
    let b1 = emit(&pool, "de.mako.vertrag.aktiv", "B", None).await;
    subs.drain().await;
    assert!(
        delivery_state(&pool, &a2).await.1,
        "a dead letter does not block its key"
    );
    assert!(delivery_state(&pool, &b1).await.1);
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_410_deactivates_the_endpoint() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (rx, url) = receiver().await;
    let created = create(&app, "erp", &url, None).await;
    *rx.status.lock().unwrap() = Some(410);
    let e = emit(&pool, "de.mako.vertrag.aktiv", "A", None).await;
    subs.drain().await;
    assert_eq!(delivery_state(&pool, &e).await, (Some(410), false, true));
    let id = created["endpoint"]["id"].as_str().unwrap();
    let (_, _, endpoint) = call(
        &app,
        "GET",
        &format!("/api/v1/webhooks/endpoints/{id}"),
        &[],
        None,
    )
    .await;
    assert_eq!(endpoint["active"], false);
    assert_eq!(endpoint["deactivated_reason"], "gone");
    emit(&pool, "de.mako.vertrag.aktiv", "B", None).await;
    subs.drain().await;
    assert_eq!(
        rx.ids().len(),
        1,
        "nothing is delivered to a deactivated endpoint"
    );
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_crash_before_commit_refans_the_event() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (_rx, url) = receiver().await;
    create(&app, "erp", &url, None).await;
    let e = emit(&pool, "de.mako.vertrag.aktiv", "A", None).await;
    // Another replica claimed the event and dies before committing.
    let mut dying = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM event_outbox FOR UPDATE")
        .fetch_all(&mut *dying)
        .await
        .unwrap();
    assert_eq!(
        subs.fan_out_once().await.unwrap(),
        0,
        "a claimed event is skipped"
    );
    drop(dying);
    assert_eq!(subs.fan_out_once().await.unwrap(), 1);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE event_id = $1")
            .bind(&e)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(subs.fan_out_once().await.unwrap(), 0, "fanned once");
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_partner_endpoint_receives_only_its_partners_events() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (rx1, url1) = receiver().await;
    let (rx_op, url_op) = receiver().await;
    let p1 = create(&app, "p1", &url1, Some("P1")).await;
    create(&app, "operator", &url_op, None).await;
    let mine = emit(
        &pool,
        "de.mako.vertrag.bestellung.eingegangen",
        "B-1",
        Some("P1"),
    )
    .await;
    emit(
        &pool,
        "de.mako.vertrag.bestellung.eingegangen",
        "B-2",
        Some("P2"),
    )
    .await;
    emit(&pool, "de.mako.vertrag.aktiv", "V-3", None).await;
    subs.drain().await;
    assert_eq!(
        rx1.ids(),
        [mine],
        "a partner endpoint receives its partner's events only"
    );
    assert_eq!(
        rx_op.ids().len(),
        3,
        "the operator's endpoint receives every event"
    );

    let id = p1["endpoint"]["id"].as_str().unwrap();
    let (status, _, _) = call(
        &app,
        "GET",
        &format!("/api/v1/webhooks/endpoints/{id}"),
        &[("x-partner", "P2")],
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a partner cannot read another's endpoint"
    );
    let (_, _, list) = call(
        &app,
        "GET",
        "/api/v1/webhooks/endpoints",
        &[("x-partner", "P2")],
        None,
    )
    .await;
    assert_eq!(list["items"], json!([]));
    let (_, _, feed) = call(&app, "GET", "/api/v1/events", &[("x-partner", "P1")], None).await;
    assert_eq!(feed["items"].as_array().unwrap().len(), 1, "{feed}");
}

// ── Log, replay, feed ───────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_window_replay_backfills_events_without_a_row() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (rx_a, url_a) = receiver().await;
    create(&app, "a", &url_a, None).await;
    let e1 = emit(&pool, "de.mako.vertrag.aktiv", "A", None).await;
    let e2 = emit(&pool, "de.mako.vertrag.aktiv", "B", None).await;
    subs.drain().await;
    assert_eq!(rx_a.ids().len(), 2);

    // An endpoint registered after the events has no delivery for them.
    let (rx_b, url_b) = receiver().await;
    let b = create(&app, "b", &url_b, None).await;
    let id = b["endpoint"]["id"].as_str().unwrap();
    let now = time::OffsetDateTime::now_utc();
    let fmt = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let (status, _, replayed) = call(
        &app,
        "POST",
        &format!("/api/v1/webhooks/endpoints/{id}/replay"),
        &[],
        Some(json!({ "from": fmt(now - time::Duration::hours(1)), "to": fmt(now + time::Duration::hours(1)) })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["queued"], 2);
    subs.drain().await;
    assert_eq!(rx_b.ids(), [e1.clone(), e2], "backfilled in commit order");

    // One delivery again, from the log.
    let (_, _, log) = call(
        &app,
        "GET",
        &format!("/api/v1/webhooks/endpoints/{id}/deliveries?status=delivered&limit=1"),
        &[],
        None,
    )
    .await;
    assert_eq!(log["items"].as_array().unwrap().len(), 1, "{log}");
    assert!(log["next_cursor"].is_string(), "{log}");
    let delivery = log["items"][0]["id"].as_str().unwrap();
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/webhooks/deliveries/{delivery}/replay"),
        &[],
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    subs.drain().await;
    assert_eq!(rx_b.ids().len(), 3);
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_replay_outside_retention_names_the_oldest_instant() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let created = create(&app, "erp", "https://erp.example.com/hook", None).await;
    let id = created["endpoint"]["id"].as_str().unwrap();
    let (status, _, refused) = call(
        &app,
        "POST",
        &format!("/api/v1/webhooks/endpoints/{id}/replay"),
        &[],
        Some(json!({ "from": "2020-01-01T00:00:00Z", "to": "2020-01-02T00:00:00Z" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused["code"], "outside_retention");
    let oldest = refused["oldest_replayable"].as_str().unwrap();
    let oldest =
        time::OffsetDateTime::parse(oldest, &time::format_description::well_known::Rfc3339)
            .unwrap();
    let expected = time::OffsetDateTime::now_utc() - time::Duration::days(30);
    assert!(
        (oldest - expected).abs() < time::Duration::minutes(1),
        "{oldest}"
    );
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn the_feed_pages_by_seq() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let mut emitted = Vec::new();
    for i in 0..5 {
        emitted.push(emit(&pool, "de.mako.vertrag.aktiv", &format!("V-{i}"), None).await);
    }
    emit(&pool, "de.mako.markt.malo.updated", "M-1", None).await;
    let mut seen = Vec::new();
    let mut uri = "/api/v1/events?type=de.mako.vertrag.*&limit=2".to_owned();
    loop {
        let (status, _, page) = call(&app, "GET", &uri, &[], None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        for item in page["items"].as_array().unwrap() {
            seen.push(item["id"].as_str().unwrap().to_owned());
        }
        match page["next_cursor"].as_str() {
            Some(c) => uri = format!("/api/v1/events?type=de.mako.vertrag.*&limit=2&cursor={c}"),
            None => break,
        }
    }
    assert_eq!(seen, emitted, "every event once, in commit order");
    let (_, _, first) = call(
        &app,
        "GET",
        "/api/v1/events?type=de.mako.vertrag.*&limit=2",
        &[],
        None,
    )
    .await;
    let cursor = first["next_cursor"].as_str().unwrap();
    let (status, _, foreign) = call(
        &app,
        "GET",
        &format!("/api/v1/events?type=de.mako.markt.*&cursor={cursor}"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(foreign["code"], "cursor_invalid");
    let (status, _, too_large) = call(&app, "GET", "/api/v1/events?limit=500", &[], None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(too_large["code"], "limit_too_large");
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn a_test_delivery_is_signed_like_a_real_one() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let (rx, url) = receiver().await;
    let created = create(&app, "erp", &url, None).await;
    let secret = created["secret"].as_str().unwrap();
    let id = created["endpoint"]["id"].as_str().unwrap();
    let uri = format!("/api/v1/webhooks/endpoints/{id}/test");
    let (status, _, form) = call(&app, "POST", &uri, &[], None).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(form["code"], "json_required");
    let (status, _, result) = call(&app, "POST", &uri, &[], Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["delivered"], true);
    assert_eq!(result["status"], 200);
    let got = rx.got.lock().unwrap().clone();
    let (wid, ts, sig, body) = &got[0];
    let event: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(event["type"], mako_events::mako::WEBHOOK_TEST);
    assert!(mako_service::webhook::verify_signature(
        &mako_service::webhook::signing_key(secret),
        wid,
        ts.parse().unwrap(),
        body,
        sig,
    ));
}

#[tokio::test]
#[ignore = "needs Docker"]
async fn an_endpoint_is_replaced_and_deleted_under_if_match() {
    let pool = pg_pool().await;
    let subs = module(&pool);
    let (app, _) = app(&subs, POLICY);
    let created = create(&app, "erp", "https://erp.example.com/hook", None).await;
    let id = created["endpoint"]["id"].as_str().unwrap();
    let uri = format!("/api/v1/webhooks/endpoints/{id}");
    let body = json!({ "url": "https://erp.example.com/v2", "event_types": ["de.mako.vertrag.*"],
                       "extensions": { "marktsparte": "strom" }, "active": true });
    let (status, _, missing) = call(&app, "PUT", &uri, &[], Some(body.clone())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(missing["code"], "if_match_required");
    let (status, _, stale) = call(
        &app,
        "PUT",
        &uri,
        &[("if-match", "\"7\"")],
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(stale["code"], "endpoint_version_mismatch");
    let (status, headers, put) =
        call(&app, "PUT", &uri, &[("if-match", "\"1\"")], Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{put}");
    assert_eq!(headers["etag"], "\"2\"");
    assert_eq!(put["url"], "https://erp.example.com/v2");
    assert_eq!(put["extensions"]["marktsparte"], "strom");
    let (status, _, _) = call(
        &app,
        "PUT",
        &uri,
        &[("if-match", "\"2\"")],
        Some(json!({ "url": "https://erp.example.com/v3", "active": true, "secret": "x" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a key the body does not declare"
    );
    let (status, _, _) = call(&app, "DELETE", &uri, &[("if-match", "\"1\"")], None).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    let (status, _, _) = call(&app, "DELETE", &uri, &[("if-match", "\"2\"")], None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, gone) = call(&app, "GET", &uri, &[], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(gone["code"], "endpoint_not_found");
    assert_eq!(
        subs.prune().await.unwrap(),
        0,
        "nothing has left the retention window"
    );
}
