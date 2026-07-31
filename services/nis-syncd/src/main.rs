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

use std::sync::Arc;

use axum::{Extension, Router};
use mako_service::{Daemon, ServiceContext};
use nis_syncd::{config, handlers, mcp_server, sync};

/// The stateless `nis-syncd` daemon — no database (`ServiceConfig::database` is
/// `None`), so `mako_service::run` connects no pool and readiness is just process
/// liveness. It only relays NIS exports to `marktd`.
struct NisSyncd;

impl Daemon for NisSyncd {
    type Config = config::NisSyncdConfig;
    const NAME: &'static str = "nis-syncd";

    async fn build(
        cfg: Arc<config::NisSyncdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        let marktd = Arc::new(mako_markt::marktd_client::MarktdClient::new(
            &cfg.marktd_url,
            secrecy::SecretString::from(cfg.marktd_api_key.clone()),
            ctx.http.clone(),
        ));

        // Shared cache of the most recent sync report (HTTP handler + MCP).
        let last_report: sync::LastSyncReport = Arc::new(tokio::sync::RwLock::new(None));

        let mcp_state = Arc::new(mcp_server::NisSyncdMcpState {
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.nb_mp_id),
            nb_mp_id: cfg.nb_mp_id.clone(),
            service_base_url: format!("http://0.0.0.0:{}", cfg.port.unwrap_or(9680)),
            http_client: ctx.http.clone(),
            marktd_api_key: cfg.marktd_api_key.clone(),
            marktd: Arc::clone(&marktd),
            last_report: Arc::clone(&last_report),
        });

        let hcfg = handlers::HandlerConfig {
            sync_concurrency: cfg.sync_concurrency,
            max_batch_size: cfg.max_batch_size,
        };

        Ok(Router::new()
            .merge(mcp_server::router(
                Arc::clone(&mcp_state),
                ctx.shutdown.clone(),
            ))
            .route(
                "/api/v1/grid/sync",
                axum::routing::post(handlers::sync_grid),
            )
            .layer(Extension(marktd))
            .layer(Extension(cfg.nb_mp_id.clone()))
            .layer(Extension(handlers::DriftWebhook {
                url: cfg.drift_webhook_url.clone(),
                secret: cfg.drift_webhook_secret.clone(),
            }))
            .layer(Extension(hcfg))
            .layer(Extension(last_report)))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<NisSyncd>().await
}
