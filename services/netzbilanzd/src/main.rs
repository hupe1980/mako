//! `netzbilanzd` — NNE/KA/MMM billing daemon.
//!
//! Generates INVOIC 31001/31002/31005 invoices (NB → LF) from meter readings
//! and tariff data.  Integrates with `marktd` (tariffs) and `edmd` (billing data).
//!
//! Port: `:8680`
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/billing/run` | Generate invoice drafts for a billing period |
//! | `GET`  | `/api/v1/billing/drafts` | List drafts (`?status=&malo_id=&limit=`) |
//! | `GET`  | `/api/v1/billing/drafts/{id}` | Fetch single draft |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/dispatch` | Validate + dispatch via `makod` |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/reject` | Reject with reason |
//! | `GET`  | `/health` | Liveness check |
//! | `GET`  | `/health/ready` | Readiness check |

mod billing;
mod config;
mod handlers;
mod pg;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_markt::makod_client::MakodClient;
use mako_markt::marktd_client::MarktdClient;
use mako_service::{health::health_routes, load_config};
use secrecy::SecretString;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

pub use config::NetzbilanzConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cfg: NetzbilanzConfig = load_config("netzbilanzd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    let makod = Arc::new(MakodClient::new(
        &cfg.makod_url,
        SecretString::from(cfg.makod_api_key.clone()),
    ));

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build HTTP client")?;

    let marktd = Arc::new(MarktdClient::new(
        &cfg.marktd_url,
        secrecy::SecretString::from(cfg.marktd_api_key.clone()),
        http_client,
    ));

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        .nest("/api/v1/billing", billing_routes())
        // N4: Kostenblatt REST API (Redispatch 2.0, BK6-20-061)
        .route(
            "/api/v1/redispatch/kostenblatt",
            get(handlers::list_kostenblatt_handler),
        )
        .route(
            "/api/v1/redispatch/kostenblatt/:activation_id",
            put(handlers::put_kostenblatt).get(handlers::get_kostenblatt),
        )
        .route(
            "/api/v1/redispatch/kostenblatt/submit/:year/:month",
            post(handlers::post_submit_kostenblatt),
        )
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(makod))
        .layer(Extension(marktd))
        .layer(Extension(pool));

    let addr = format!("0.0.0.0:{}", cfg.port.unwrap_or(8680));
    info!(%addr, "netzbilanzd starting");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    axum::serve(listener, app).await.context("serve")
}

fn billing_routes() -> Router {
    Router::new()
        .route("/run", post(handlers::run_billing))
        // N6: MMM auto-run — auto-fetches profil_kwh from edmd (§40 StromNZV)
        .route("/mmm-run/:malo_id", post(handlers::post_mmm_auto_run))
        .route("/drafts", get(handlers::list_drafts))
        .route("/drafts/:id", get(handlers::get_draft))
        .route("/drafts/:id/dispatch", put(handlers::dispatch_draft))
        .route("/drafts/:id/reject", put(handlers::reject_draft))
}
