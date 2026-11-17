//! productd's events reach a webhook endpoint registered through the API,
//! signed so the receiver verifies them with the secret it was shown once.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::Mutex;

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

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

/// What the receiver saw: the verified events, and any it refused.
#[derive(Default)]
struct Seen {
    verified: Vec<serde_json::Value>,
    refused: Vec<String>,
}

/// A receiver on loopback that verifies each delivery with `secret`.
async fn receiver(secret: Arc<Mutex<String>>, seen: Arc<Mutex<Seen>>) -> String {
    let app = axum::Router::new().fallback(axum::routing::post(
        move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let (secret, seen) = (Arc::clone(&secret), Arc::clone(&seen));
            async move {
                let key = mako_service::webhook::signing_key(&secret.lock().await);
                let mut seen = seen.lock().await;
                match mako_service::webhook::verify_request(Some(&key), &headers, &body) {
                    Ok(_) => {
                        seen.verified
                            .push(serde_json::from_slice(&body).expect("a JSON CloudEvent"));
                        axum::http::StatusCode::NO_CONTENT
                    }
                    Err(e) => {
                        seen.refused.push(e.to_string());
                        axum::http::StatusCode::UNAUTHORIZED
                    }
                }
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/hook")
}

/// productd on `url`'s database, admitting the loopback receiver.
async fn serve(url: &str, pool: &PgPool) -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/productd.toml");
    let mut cfg =
        mako_service::load_config_file::<productd::config::ProductdConfig>("productd", &path)
            .expect("the demo configuration loads");
    cfg.database.url = url.to_owned();
    cfg.subscriptions.allow_loopback = true;
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

/// An endpoint registered through `POST /api/v1/webhooks/endpoints` receives
/// the `de.tarif.product.updated` a product write commits, and the receiver
/// verifies its signature with the secret the registration showed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_productd_endpoint_receives_a_verifying_delivery() {
    let Some((url, _pg)) = pg().await else {
        return;
    };
    let pool = PgPool::connect(&url).await.expect("connect");
    let base = serve(&url, &pool).await;
    let secret = Arc::new(Mutex::new(String::new()));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let hook = receiver(Arc::clone(&secret), Arc::clone(&seen)).await;
    let http = reqwest::Client::new();

    let created = http
        .post(format!("{base}/api/v1/webhooks/endpoints"))
        .json(&serde_json::json!({
            "name": "erp-test",
            "url": hook,
            "event_types": ["de.tarif.*"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201, "{}", created.text().await.unwrap());
    let created: serde_json::Value = created.json().await.unwrap();
    let shown = created["secret"].as_str().expect("the secret, shown once");
    assert!(shown.starts_with("whsec_"), "{shown}");
    *secret.lock().await = shown.to_owned();

    let put = http
        .put(format!("{base}/api/v1/products/9900357000004/HOOK-STROM"))
        .json(&serde_json::json!({
            "category": "STROM",
            "name": "Webhook-Strom",
            "sparte": "STROM",
            "register_count": "Eintarif",
            "kundentyp": "Haushalt",
            "dyn_source": null,
            "valid_from": "2026-01-01",
            "valid_to": null,
            "data": {},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), 200, "{}", put.text().await.unwrap());

    // `build_for_test_on` cancels the daemon's shutdown token once the router
    // is built, so its worker is gone: a worker on the same tables drains.
    mako_service::subscriptions::Subscriptions::new(
        pool.clone(),
        "productd",
        "9900357000004",
        mako_service::subscriptions::SubscriptionsConfig {
            allow_loopback: true,
            ..Default::default()
        },
    )
    .drain()
    .await;
    {
        let seen = seen.lock().await;
        assert!(seen.refused.is_empty(), "{:?}", seen.refused);
        let ev = seen
            .verified
            .first()
            .expect("a verified delivery reached the endpoint");
        assert_eq!(ev["type"], mako_events::tarif::PRODUCT_UPDATED);
        assert_eq!(ev["subject"], "HOOK-STROM");
    }

    // The delivery log records it.
    let id = created["endpoint"]["id"].as_str().expect("the endpoint id");
    let log: serde_json::Value = http
        .get(format!(
            "{base}/api/v1/webhooks/endpoints/{id}/deliveries?status=delivered"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(log["items"].as_array().map(Vec::len), Some(1), "{log}");
}
