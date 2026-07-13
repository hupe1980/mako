//! `nis-syncd` — NIS/GIS grid topology import adapter.
//!
//! Stateless HTTP service that accepts NIS (Network Information System) export
//! data and pushes `malo_grid` records into `marktd` via
//! `PUT /api/v1/malo/{id}/grid`.  No database required.
//!
//! # Purpose
//!
//! Without accurate `malo_grid` records in `marktd`, `processd` NB check 4
//! (`Bilanzierungsgebiet` consistency) falls back to the `malo.bilanzierungsgebiet`
//! column.  When that column is also absent, check 4 is skipped and the
//! Anmeldung **escalates** to the operator instead of being auto-accepted.
//!
//! `nis-syncd` bridges the gap: the NB's NIS/GIS system exports a batch of
//! `{malo_id, bilanzierungsgebiet, netzgebiet, sparte}` tuples, and `nis-syncd`
//! pushes them to `marktd` in a single sync pass.
//!
//! **Result:** `processd` NB STP improves from ~80 % to ≥ 95 %.
//!
//! # Architecture
//!
//! ```text
//! NIS/GIS system (SAP IS-U, Smallworld, GE Smallworld, …)
//!   → POST /api/v1/grid/sync           (batch NIS export)
//! nis-syncd :9680  (stateless)
//!   → PUT marktd /api/v1/malo/{id}/grid  (per MaLo, idempotent)
//! marktd :8180
//!   → processd /api/v1/…               (STP ≥ 95 %)
//! ```
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/grid/sync` | Sync NIS export to `marktd` |
//! | `POST` | `/api/v1/grid/sync?dry_run=true` | Dry-run: compare without writing |
//! | `GET`  | `/health/live` | Liveness probe |
//! | `GET`  | `/health/ready` | Readiness probe |

use anyhow::Context as _;
use axum::{Extension, Router};
use mako_service::{health::health_routes, load_config};
use nis_syncd::{config, handlers, mcp_server};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cfg: config::NisSyncdConfig = load_config("nis-syncd").context("load config")?;

    let marktd = std::sync::Arc::new(mako_markt::marktd_client::MarktdClient::new(
        &cfg.marktd_url,
        secrecy::SecretString::from(cfg.marktd_api_key.clone()),
        reqwest::Client::new(),
    ));

    let shutdown = tokio_util::sync::CancellationToken::new();
    let mcp_state = std::sync::Arc::new(mcp_server::NisSyncdMcpState {
        marktd_api_key: cfg.marktd_api_key.clone(),
        nb_mp_id: cfg.nb_mp_id.clone(),
        service_base_url: format!("http://0.0.0.0:{}", cfg.port.unwrap_or(9680)),
    });

    let app = Router::new()
        .merge(mcp_server::router(
            std::sync::Arc::clone(&mcp_state),
            shutdown.clone(),
        ))
        .merge(health_routes(|| async { true }))
        .route(
            "/api/v1/grid/sync",
            axum::routing::post(handlers::sync_grid),
        )
        .layer(Extension(marktd))
        .layer(Extension(cfg.nb_mp_id.clone()))
        .layer(Extension(cfg.drift_webhook_url.clone()));

    let addr = format!("0.0.0.0:{}", cfg.port.unwrap_or(9680));
    info!(%addr, "nis-syncd starting");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    axum::serve(listener, app).await.context("serve")
}
