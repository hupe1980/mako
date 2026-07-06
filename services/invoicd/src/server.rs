//! Axum router for `invoicd`.
//!
//! Routes:
//! - `POST /webhook`      — inbound MdmEvent CloudEvents from `mdmd`
//! - `GET  /health/live`  — liveness probe (always 200)
//! - `GET  /health/ready` — readiness probe (200 when tariff store seeded)
//! - `PUT  /admin/tariff` — seed a tariff entry into the in-process store
//! - `GET  /metrics`      — plain-text Prometheus-compatible counters (future)

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use invoic_checker::CheckConfig;
use secrecy::SecretString;
use tokio::net::TcpListener;

use crate::{
    handler::{HandlerState, handle_webhook},
    makod_client::MakodClient,
    tariff_store::TariffStoreHandle,
};

/// Build and return the Axum router with all routes attached.
pub fn router(state: HandlerState) -> Router {
    Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(health_ready))
        .route("/admin/tariff", put(admin_put_tariff))
        .with_state(state)
}

/// `PUT /admin/tariff` — seed a [`TariffStoreEntry`] into the in-process store.
///
/// Accepts a JSON body matching [`TariffStoreEntry`] (publisher_gln, valid_from,
/// charge_category, unit_price).  Returns `204 No Content` on success.
///
/// Operators use this endpoint to pre-populate the tariff store at startup or
/// when PRICAT data changes, since PRICAT 27003 events do not embed tariff
/// details in the `ProcessCompleted` outbox payload.
async fn admin_put_tariff(
    State(state): State<HandlerState>,
    Json(entry): Json<invoic_checker::TariffEntry>,
) -> impl IntoResponse {
    let gln = entry.publisher_gln.clone();
    let valid_from = entry.valid_from.clone();
    {
        let mut store = state.tariff_store.0.write().await;
        store.insert(entry);
    }
    tracing::info!(publisher_gln = %gln, valid_from = %valid_from, "invoicd: tariff entry seeded via admin API");
    StatusCode::NO_CONTENT
}

/// `GET /health/ready` — 200 OK once the server is up and accepting requests.
///
/// Future: check that tariff store has at least one entry and the `makod`
/// connection is reachable.
async fn health_ready(State(_state): State<HandlerState>) -> impl IntoResponse {
    StatusCode::OK
}

/// Configuration for [`run`].
pub struct RunConfig {
    pub listen: SocketAddr,
    pub makod_url: String,
    pub mdmd_url: String,
    pub subscriber_id: String,
    pub webhook_url: String,
    pub webhook_secret: Option<SecretString>,
    pub inbound_secret: Option<SecretString>,
    pub check_config: CheckConfig,
    pub auto_dispute_threshold_eur_cents: i64,
}

/// Bind, register subscription with `mdmd`, and serve forever.
pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    let tariff_store = TariffStoreHandle::new();
    let makod = MakodClient::new(&cfg.makod_url);

    let state = HandlerState {
        tariff_store,
        makod,
        check_config: Arc::new(cfg.check_config),
        inbound_secret: Arc::new(cfg.inbound_secret),
        auto_dispute_threshold_eur_cents: cfg.auto_dispute_threshold_eur_cents,
    };

    // Register subscription with mdmd (idempotent PUT).
    register_subscription(
        &cfg.mdmd_url,
        &cfg.subscriber_id,
        &cfg.webhook_url,
        cfg.webhook_secret.as_ref(),
    )
    .await;

    let app = router(state);
    let listener = TcpListener::bind(cfg.listen).await?;

    tracing::info!(
        listen = %cfg.listen,
        makod_url = %cfg.makod_url,
        mdmd_url = %cfg.mdmd_url,
        "invoicd: listening"
    );

    axum::serve(listener, app).await?;
    Ok(())
}

/// `PUT /api/v1/subscriptions/{subscriber_id}` on `mdmd`.
///
/// Uses an idempotent PUT so restarts are safe — if the subscription already
/// exists with the same URL and secret, the call is a no-op.
async fn register_subscription(
    mdmd_url: &str,
    subscriber_id: &str,
    webhook_url: &str,
    webhook_secret: Option<&SecretString>,
) {
    use reqwest::Client;
    use secrecy::ExposeSecret;
    use serde_json::json;

    let body = json!({
        "webhook_url":    webhook_url,
        "webhook_secret": webhook_secret.map(|s| s.expose_secret()),
        "roles":          serde_json::Value::Array(vec![]),  // match all roles
        "event_types":    ["de.mako.process.initiated"],
        "active":         true,
    });

    let url = format!("{mdmd_url}/api/v1/subscriptions/{subscriber_id}");

    match Client::new().put(&url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(subscriber_id, "invoicd: subscription registered with mdmd");
        }
        Ok(resp) => {
            tracing::warn!(
                subscriber_id,
                status = resp.status().as_u16(),
                "invoicd: subscription registration returned non-2xx — events may not be delivered"
            );
        }
        Err(err) => {
            tracing::warn!(
                %err,
                subscriber_id,
                "invoicd: could not register subscription with mdmd — events may not be delivered"
            );
        }
    }
}
