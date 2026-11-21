//! An accepted Angebot owes `vertragd` a contract, and that obligation commits
//! with the acceptance.
//!
//! The customer's acceptance is binding the moment it answers 200. If the
//! `de.mako.tarif.angebot.angenommen` that makes `vertragd` create the
//! Rahmenvertrag is a POST attempted once after the commit, a `vertragd`
//! outage leaves an accepted Angebot with no contract and nothing that retries
//! it — and the customer's own retry is a 409, because the Angebot is decided.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use productd::pg;
use sqlx::PgPool;

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

const TENANT: &str = "stadtwerke-musterstadt";
/// The Lieferant `deploy/demo/productd.toml` sells under.
const LF: &str = "9900357000028";

async fn pg() -> Option<(String, PgContainer)> {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    Some((
        format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres"),
        container,
    ))
}

/// productd on `url`'s database, its ERP webhook pointing at `webhook`.
async fn serve(url: &str, pool: &PgPool, webhook: &str) -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/productd.toml");
    let mut cfg =
        mako_service::load_config_file::<productd::config::ProductdConfig>("productd", &path)
            .expect("the demo configuration loads");
    cfg.database.url = url.to_owned();
    cfg.erp_webhook_url = Some(webhook.to_owned());
    let app = mako_service::build_for_test_on::<productd::Productd>(cfg, pool.clone())
        .await
        .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn priced_angebot(pool: &PgPool) -> uuid::Uuid {
    let req = pg::CreateAngebotRequest {
        lf_mp_id: Some(LF.to_owned()),
        kunden_id: None,
        interessent_name: Some("ACME GmbH".to_owned()),
        contact_email: None,
        contact_phone: None,
        gueltig_bis: Some("2099-12-31".to_owned()),
        lieferbeginn: None,
        laufzeit_monate: Some(12),
        positionen: vec![],
        varianten: None,
        wiederverkaeufer_13b: false,
        erp_angebot_id: None,
        notizen: None,
    };
    pg::insert_angebot(
        pool,
        TENANT,
        LF,
        "ANG-2026-00042",
        &req,
        &serde_json::json!([]),
        &serde_json::json!([]),
        // Any non-empty document counts as priced.
        &serde_json::json!({ "_typ": "ANGEBOT" }),
        None,
        None,
        time::macros::date!(2099 - 12 - 31),
        None,
    )
    .await
    .expect("insert angebot")
}

/// With `vertragd` unreachable the acceptance stands and its contract request
/// survives; when `vertragd` answers, the request is delivered and the
/// Rahmenvertrag it names is linked back onto the Angebot.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_accepted_angebot_is_contracted_after_vertragd_returns() {
    let Some((url, _pg)) = pg().await else {
        return;
    };
    let pool = PgPool::connect(&url).await.expect("connect");
    // Nothing listens on port 9.
    let base = serve(&url, &pool, "http://127.0.0.1:9/api/v1/webhooks/angebot").await;
    let id = priced_angebot(&pool).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/api/v1/angebote/{id}/annehmen"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the customer's acceptance stands");

    let owed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox
          WHERE ce_type = $1 AND delivered_at IS NULL AND envelope->>'subject' = $2",
    )
    .bind(mako_events::tarif::ANGEBOT_ANGENOMMEN)
    .bind(id.to_string())
    .fetch_one(&pool)
    .await
    .expect("the contract request is on record");
    assert_eq!(owed, 1, "the contract request survives the outage");

    // vertragd returns and creates the Rahmenvertrag.
    let rahmenvertrag = uuid::Uuid::new_v4();
    let app = axum::Router::new().route(
        "/api/v1/webhooks/angebot",
        axum::routing::post(move || async move {
            (
                axum::http::StatusCode::CREATED,
                axum::Json(serde_json::json!({ "rahmenvertrag_id": rahmenvertrag })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let vertragd = format!(
        "http://{}/api/v1/webhooks/angebot",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    sqlx::query("UPDATE event_outbox SET next_attempt_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    productd::daemon::angebot_worker(pool.clone(), vertragd, None)
        .flush_once()
        .await
        .expect("flush");

    let linked: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT rahmenvertrag_id FROM angebote WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        linked,
        Some(rahmenvertrag),
        "the Angebot names its contract"
    );
}

/// A contract price (`VERTRAG`) is not a catalogue product, so
/// a B2B Angebot naming its code is refused, the position named.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_contract_price_is_not_offered_in_an_angebot() {
    let Some((url, _pg)) = pg().await else {
        return;
    };
    let pool = PgPool::connect(&url).await.expect("connect");
    let base = serve(&url, &pool, "http://127.0.0.1:9/api/v1/webhooks/angebot").await;
    sqlx::query(
        "INSERT INTO products (lf_mp_id, product_code, category, name, sparte, data, bo4e_version,
                               product_status, tenant, abrechnung)
         VALUES ($1, 'P~0123456789ab', 'STROM', 'P (Vertriebspreis)', 'STROM',
                 '{\"tarifpreise\": []}', 'v202607.0.0', 'VERTRAG', $2, '{}')",
    )
    .bind(LF)
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("contract price");

    let resp = reqwest::Client::new()
        .post(format!("{base}/api/v1/angebote"))
        .json(&serde_json::json!({
            "lf_mp_id": LF,
            "interessent_name": "ACME GmbH",
            "positionen": [{
                "product_code": "P~0123456789ab",
                "sparte": "STROM",
                "jahresverbrauch_kwh": "3500",
            }],
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status, 422, "{body}");
    assert_eq!(
        body["unpreisbare_positionen"][0]["grund"], "PRODUKT_UNBEKANNT",
        "{body}"
    );
}
