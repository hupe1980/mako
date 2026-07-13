//! `einsd` — Einspeiser Registry + EEG Settlement daemon.
//!
//! Manages the lifecycle of decentralised feed-in plants (Einspeiseanlagen)
//! under the EEG (Erneuerbare-Energien-Gesetz) and calculates their monthly
//! feed-in remuneration according to the applicable settlement model:
//!
//! | Model | Regulation | Flow |
//! |---|---|---|
//! | `VERGUETUNG` | §21 EEG 2023 | Fixed tariff NB → Anlagenbetreiber |
//! | `DIREKTVERMARKTUNG` | §20 EEG 2023 | Marktprämie (max(0, AW−EPEX)) NB → ÜNB |
//! | `POST_EEG_SPOT` | post-20yr | Spot market reference value |
//! | `EIGENVERBRAUCH` | §38a EEG | Self-consumption; no settlement |
//!
//! Emits CloudEvents:
//! - `de.eeg.verguetung.berechnet` — VERGUETUNG/POST_EEG_SPOT/EIGENVERBRAUCH settled
//! - `de.eeg.marktpraemie.berechnet` — DIREKTVERMARKTUNG settled
//! - `de.eeg.anlage.foerderung_auslaufend` — `foerderendedatum` within 180 days
//!
//! Port: `:9180`
//!
//! # Endpoints
//!
//! | Method   | Path | Description |
//! |---|---|---|
//! | `POST`   | `/api/v1/anlagen` | Register EEG plant |
//! | `GET`    | `/api/v1/anlagen` | List plants (`?erzeugungsart=&settlement_model=&status=`) |
//! | `GET`    | `/api/v1/anlagen/{tr_id}` | Fetch plant |
//! | `PUT`    | `/api/v1/anlagen/{tr_id}` | Update plant |
//! | `DELETE` | `/api/v1/anlagen/{tr_id}` | Decommission plant |
//! | `GET`    | `/api/v1/anlagen/foerderung-auslaufend` | Plants expiring within 180 days |
//! | `POST`   | `/api/v1/anlagen/{tr_id}/settle/{year}/{month}` | Trigger monthly settlement |
//! | `GET`    | `/api/v1/anlagen/{tr_id}/settlements` | Settlement history |
//! | `PUT`    | `/api/v1/epex-monthly/{year}/{month}` | Import EPEX monthly price |
//! | `GET`    | `/api/v1/epex-monthly/{year}/{month}` | Fetch stored EPEX price |
//! | `GET`    | `/health` | Liveness check |
//! | `GET`    | `/health/ready` | Readiness check |


use einsd::{config, handlers, pg};
use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_service::{health::health_routes, load_config};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cfg: config::EinsdConfig = load_config("einsd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    // Schema must be applied manually — see migrations/0001_initial.sql for DDL.

    // Background worker: emit de.eeg.anlage.foerderung_auslaufend every 6 h.
    let alert_pool = pool.clone();
    let alert_cfg = Arc::clone(&cfg);
    tokio::spawn(async move {
        let interval_secs = alert_cfg.alert_interval_secs.unwrap_or(21_600);
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
            match pg::list_expiring(&alert_pool, &alert_cfg.tenant, 180).await {
                Ok(plants) if !plants.is_empty() => {
                    let today = time::OffsetDateTime::now_utc().date();
                    for plant in &plants {
                        let days_remaining = (plant.foerderendedatum - today).whole_days();
                        tracing::info!(
                            tr_id = %plant.tr_id,
                            foerderendedatum = %plant.foerderendedatum,
                            days_remaining,
                            "foerderung_auslaufend — emitting CloudEvent"
                        );
                        handlers::emit_foerderung_alert_ce(
                            &alert_cfg,
                            &plant.tr_id,
                            &plant.malo_id,
                            plant.foerderendedatum,
                            days_remaining,
                        )
                        .await;
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::error!("alert worker error: {e}"),
            }
        }
    });

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        // ── Anlage CRUD ────────────────────────────────────────────────────────
        .route(
            "/api/v1/anlagen",
            post(handlers::post_anlage).get(handlers::get_anlagen),
        )
        .route(
            "/api/v1/anlagen/foerderung-auslaufend",
            get(handlers::get_foerderung_auslaufend),
        )
        .route(
            "/api/v1/anlagen/:tr_id",
            get(handlers::get_anlage)
                .put(handlers::put_anlage)
                .delete(handlers::delete_anlage),
        )
        // ── Settlement ─────────────────────────────────────────────────────────
        .route(
            "/api/v1/anlagen/:tr_id/settle/:year/:month",
            post(handlers::post_settle),
        )
        .route(
            "/api/v1/anlagen/:tr_id/settlements",
            get(handlers::get_settlements),
        )
        // ── Repowering (§22 EEG 2023) ──────────────────────────────────────────
        .route(
            "/api/v1/anlagen/:tr_id/repowering",
            post(handlers::post_repowering),
        )
        // ── EPEX monthly prices ────────────────────────────────────────────────
        .route(
            "/api/v1/epex-monthly/:year/:month",
            put(handlers::put_epex_price).get(handlers::get_epex_price),
        )
        // ── EEG tariff rate lookup ─────────────────────────────────────────────
        .route(
            "/api/v1/verguetungssatz-lookup",
            post(handlers::post_verguetungssatz_lookup),
        )
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(pool));

    let port = cfg.port.unwrap_or(9180);
    let addr = format!("0.0.0.0:{port}");
    info!(%addr, "einsd starting");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    axum::serve(listener, app).await.context("serve")
}
