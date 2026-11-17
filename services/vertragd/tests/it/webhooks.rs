//! vertragd's `de.vertrag.*` CloudEvents reach the webhook endpoint `default`
//! — the configured `erp_webhook_url` — through the shared subscription module,
//! signed with `erp_hmac_secret` per Standard Webhooks.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use base64::Engine as _;
use tokio::sync::mpsc;

const TENANT: &str = "9900357000004";

fn config(db: &str, receiver: &str, secret: &str) -> vertragd::config::VertragdConfig {
    let mut cfg: vertragd::config::VertragdConfig =
        toml::from_str(include_str!("../../../../deploy/demo/vertragd.toml"))
            .expect("the demo configuration parses");
    cfg.database.url = db.to_owned();
    cfg.erp_webhook_url = Some(receiver.to_owned());
    cfg.erp_hmac_secret = Some(secret.to_owned());
    cfg
}

/// A receiver that hands every delivery to the test.
async fn receiver() -> (String, mpsc::Receiver<(HeaderMap, Bytes)>) {
    let (tx, rx) = mpsc::channel(16);
    let tx = Arc::new(tx);
    let app = axum::Router::new().fallback(axum::routing::post(
        move |headers: HeaderMap, body: Bytes| {
            let tx = Arc::clone(&tx);
            async move {
                let _ = tx.send((headers, body)).await;
                StatusCode::NO_CONTENT
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{addr}/events"), rx)
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_contract_event_reaches_the_default_endpoint_signed() {
    let Some((url, _pg)) = crate::dispatch_integration::pg_container().await else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let (target, mut deliveries) = receiver().await;
    let secret = format!(
        "whsec_{}",
        base64::engine::general_purpose::STANDARD.encode(b"vertragd-erp-signing-key")
    );

    let app = mako_service::build_for_test_on::<vertragd::Vertragd>(
        config(&url, &target, &secret),
        pool.clone(),
    )
    .await
    .expect("the router builds on the database");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    // `build_for_test_on` stops the workers it spawned once the router is
    // built; the module is started here exactly as `build` configures it.
    let shutdown = tokio_util::sync::CancellationToken::new();
    vertragd::daemon::subscriptions(&config(&url, &target, &secret), pool.clone())
        .start(shutdown.clone())
        .await
        .expect("the subscriptions start");

    let kunde = crate::dispatch_integration::make_kunde(&pool, TENANT).await;
    let vertrag = vertragd::pg::insert_versorgungsvertrag(
        &pool,
        kunde,
        TENANT,
        TENANT,
        &crate::dispatch_integration::vertrag_input("ERP-WEBHOOK"),
    )
    .await
    .expect("a contract")
    .id;
    let status = reqwest::Client::new()
        .put(format!("{base}/api/v1/vertraege/{vertrag}/preisgarantie"))
        .json(&serde_json::json!({
            "preisgarantietyp": "ALLE_PREISBESTANDTEILE_BRUTTO",
            "zeitlicheGueltigkeit": { "startdatum": "2026-01-01", "enddatum": "2027-12-31" }
        }))
        .send()
        .await
        .expect("answered")
        .status();
    assert_eq!(status, 204);

    let (headers, body) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let (headers, body) = deliveries.recv().await.expect("the receiver runs");
            let event: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
            if event["type"] == "de.vertrag.preisgarantie-hinterlegt" {
                return (headers, body);
            }
        }
    })
    .await
    .expect("the event is delivered to the endpoint `default`");

    let key = mako_service::webhook::signing_key(&secret);
    let id = mako_service::webhook::verify_request(Some(&key), &headers, &body)
        .expect("the delivery verifies with the configured secret")
        .expect("the delivery carries webhook-id");
    let event: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(
        event["id"].as_str(),
        Some(id.0.as_str()),
        "webhook-id is the CloudEvent id"
    );
    assert_eq!(event["subject"], vertrag.to_string());
    shutdown.cancel();
}
