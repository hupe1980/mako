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

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use einsd::{config, handlers, mcp_server, pg};
use mako_service::{health::health_routes, load_config};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("einsd");

    let cfg: config::EinsdConfig = load_config("einsd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    let ct = mako_service::shutdown::token();
    let mcp_state = std::sync::Arc::new(mcp_server::EinsdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
    });

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

    // Background worker: auto-settle all unsettled active plants on the 2nd of each month.
    //
    // Triggered once on startup (in case the previous month's settlement was missed)
    // and then every 24 h, settling any plants that were missed.
    //
    // EEG Vergütung must be paid monthly per §23 EEG 2023.  The NB is responsible
    // for initiating payment within 30 days of the billing month end.
    let auto_pool = pool.clone();
    let auto_cfg = Arc::clone(&cfg);
    tokio::spawn(async move {
        // Wait for startup before first run.
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        loop {
            let now = time::OffsetDateTime::now_utc();
            // Auto-settle the previous month on the 2nd of each month (or on first startup).
            let prev_month_year = if now.month() as u8 == 1 {
                now.year() - 1
            } else {
                now.year()
            };
            let prev_month = if now.month() as u8 == 1 {
                12i16
            } else {
                now.month() as i16 - 1
            };

            // Resolve EPEX price for previous month.
            let epex = pg::fetch_epex_price(&auto_pool, prev_month_year as i16, prev_month)
                .await
                .ok()
                .flatten();

            let plants = pg::list_unsettled(
                &auto_pool,
                &auto_cfg.tenant,
                prev_month_year as i16,
                prev_month,
            )
            .await
            .unwrap_or_default();

            if !plants.is_empty() {
                tracing::info!(
                    year = prev_month_year,
                    month = prev_month,
                    unsettled = plants.len(),
                    "auto-settle worker: settling unsettled plants"
                );
                for anlage in &plants {
                    let kwh = handlers::fetch_einspeisemenge_from_edmd(
                        &auto_cfg,
                        &anlage.malo_id,
                        prev_month_year as i16,
                        prev_month,
                    )
                    .await;
                    let input = einsd::pg::build_settle_input(
                        &auto_cfg.tenant,
                        anlage,
                        prev_month_year as i16,
                        prev_month,
                        einsd::pg::SettleOverrides {
                            einspeisemenge_kwh: kwh,
                            epex_avg_ct_kwh: epex,
                            managementpraemie_ct_override: None,
                            einspeisemanagement_kwh: None,
                            negative_price_quarter_hours: None,
                            correction_of: None,
                            jahresmarktwert_ct_kwh: None,
                        },
                    );
                    if let Ok(result) = pg::run_settlement(&auto_pool, input).await
                        && (result.status == "calculated" || result.status == "foerderung_beendet")
                    {
                        let ce_type = match anlage.settlement_model.as_str() {
                            "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG" => {
                                "de.eeg.marktpraemie.berechnet"
                            }
                            _ => "de.eeg.verguetung.berechnet",
                        };
                        handlers::emit_settlement_ce(
                            &auto_cfg,
                            ce_type,
                            &anlage.tr_id,
                            &anlage.malo_id,
                            &result,
                            prev_month_year as i16,
                            prev_month,
                        )
                        .await;
                    }
                }
            }

            // Run again in ~23 h (drift-proof; avoids DST edge at midnight).
            tokio::time::sleep(tokio::time::Duration::from_secs(82_800)).await;
        }
    });

    let app = Router::new()
        .merge(mcp_server::router(mcp_state, ct.clone()))
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
        // ── MaStR registration confirmation ────────────────────────────────────
        .route(
            "/api/v1/anlagen/:tr_id/mastr-registrierung",
            post(handlers::post_mastr_registrierung),
        )
        // ── Zusammenlegung (§24 EEG 2023) ─────────────────────────────────────
        .route(
            "/api/v1/anlagen/:tr_id/zusammenlegen",
            post(handlers::post_zusammenlegen),
        )
        // ── §21b EEG 2023 — Veräußerungsform switch ───────────────────────────
        .route(
            "/api/v1/anlagen/:tr_id/switch-veraeusserungsform",
            post(handlers::post_switch_veraeusserungsform),
        )
        // ── §22 MessZV — Correction settlement ────────────────────────────────
        .route(
            "/api/v1/anlagen/:tr_id/settlements/:year/:month/correction",
            post(handlers::post_correction_settle),
        )
        // ── Batch settlement ───────────────────────────────────────────────────
        .route(
            "/api/v1/settle/:year/:month",
            post(handlers::post_batch_settle),
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
    mako_service::shutdown::serve(listener, app, ct).await
}
