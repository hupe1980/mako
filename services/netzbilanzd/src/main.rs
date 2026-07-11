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
use axum::{Extension, Router, routing::get};
use mako_markt::makod_client::MakodClient;
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

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    let makod = Arc::new(MakodClient::new(
        &cfg.makod_url,
        SecretString::from(cfg.makod_api_key.clone()),
    ));

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run migrations")?;

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        .nest("/api/v1/billing", billing_routes())
        .layer(Extension(makod))
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
        .route("/run", axum::routing::post(handlers::run_billing))
        .route("/drafts", get(handlers::list_drafts))
        .route("/drafts/:id", get(handlers::get_draft))
        .route(
            "/drafts/:id/dispatch",
            axum::routing::put(handlers::dispatch_draft),
        )
        .route(
            "/drafts/:id/reject",
            axum::routing::put(handlers::reject_draft),
        )
}
