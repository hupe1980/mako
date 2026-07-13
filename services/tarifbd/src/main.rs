//! `tarifbd` — Product & Tariff Catalog.
//!
//! Single source of truth for all retail products the LF sells to end customers.
//! All commercial pricing is defined here — `billingd` reads product definitions
//! from `tarifbd` and calculates invoices from them.
//!
//! ## Product Categories
//!
//! | Category | Description | Billing template |
//! |---|---|---|
//! | `STROM` | Electricity SLP/RLM, Eintarif/Zweitarif/Mehrtarif | `calculate_strom` |
//! | `GAS` | Natural gas SLP/RLM with Brennwertkorrektur | `calculate_gas` |
//! | `WAERME` | District heat / Fernwärme | `calculate_waerme` |
//! | `SOLAR` | Mieterstrom §42b, §42a Gemeinschaftliche Gebäudeversorgung | `calculate_solar` |
//! | `EEG` | Feed-in settlement: Vergütung, Marktprämie, Managementprämie | `calculate_eeg` |
//! | `EINSPEISUNG` | Non-EEG Direktvermarktung settlement | `calculate_einspeisung` |
//! | `WAERMEPUMPE` | Heat pump electricity supply with §14a Modul 1/3 | `calculate_strom` |
//! | `WALLBOX` | EV home charging with §14a Modul 1/3 | `calculate_strom` |
//! | `HEMS` | Home Energy Management System platform + events | `calculate_hems` |
//! | `EMOBILITY` | CPO/EMSP charging services | `calculate_emobility` |
//! | `ENERGIEDIENSTLEISTUNG` | MSB, EMS, smart meter, maintenance | `calculate_energiedienstleistung` |
//! | `BUNDLE` | Composite: references component product codes | per-component |
//!
//! ## Pricing schema (`data.tarifpreispositionen`)
//!
//! Products store prices as BO4E Tarifpreisblatt JSONB.
//! `billingd` reads `preistyp` strings (case-insensitive) to extract rates.
//! Example product for a Strom SLP Eintarif:
//! ```json
//! {
//!   "tarifpreispositionen": [
//!     { "preistyp": "grundpreis",   "preisstaffeln": [{ "preis": { "wert": "20.50", "einheit": "CT" } }] },
//!     { "preistyp": "arbeitspreis", "preisstaffeln": [{ "preis": { "wert": "31.20", "einheit": "CT" } }] }
//!   ]
//! }
//! ```
//! For regulatory overrides (e.g. Stromsteuerbefreiung §9 StromStG):
//! ```json
//! { "stromsteuer_ct_per_kwh_override": "0" }
//! ```
//!
//! Port: `:9080`

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_service::{health::health_routes, load_config};
use sqlx::PgPool;
use std::sync::Arc;
use tarifbd::{config, handlers, pg};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cfg: config::TarifbdConfig = load_config("tarifbd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        // ── Product CRUD ──────────────────────────────────────────────────────
        .route(
            "/api/v1/products/:lf_mp_id/:product_code",
            put(handlers::put_product).get(handlers::get_product),
        )
        .route(
            "/api/v1/products/:lf_mp_id/:product_code/history",
            get(handlers::get_product_history),
        )
        // ── Energiemix sub-resource (§42 EnWG) ───────────────────────────────
        .route(
            "/api/v1/products/:lf_mp_id/:product_code/energiemix",
            put(handlers::put_energiemix)
                .get(handlers::get_energiemix)
                .delete(handlers::delete_energiemix_handler),
        )
        .route(
            "/api/v1/products/:lf_mp_id",
            get(handlers::list_products_handler),
        )
        // ── Customer → product assignment ─────────────────────────────────────
        .route(
            "/api/v1/customer/:malo_id/product",
            get(handlers::get_customer_product_handler).put(handlers::put_customer_product_handler),
        )
        // ── EPEX Spot prices ──────────────────────────────────────────────────
        .route("/api/v1/epex-prices/:date", put(handlers::put_epex_prices))
        .route(
            "/api/v1/epex-prices/:date/hourly",
            get(handlers::get_epex_prices_hourly),
        )
        .route(
            "/api/v1/epex-prices/:year/:month/average",
            get(handlers::get_epex_monthly_average),
        )
        // ── Angebot (B2B Quotation, L4) ───────────────────────────────────────
        .route(
            "/api/v1/angebote",
            get(handlers::list_angebote_handler).post(handlers::post_angebot),
        )
        .route(
            "/api/v1/angebote/expire",
            post(handlers::post_expire_angebote),
        )
        .route("/api/v1/angebote/:id", get(handlers::get_angebot_handler))
        .route(
            "/api/v1/angebote/:id/versenden",
            post(handlers::post_angebot_versenden),
        )
        .route(
            "/api/v1/angebote/:id/annehmen",
            post(handlers::post_angebot_annehmen),
        )
        .route(
            "/api/v1/angebote/:id/ablehnen",
            post(handlers::post_angebot_ablehnen),
        )
        // ── Angebot editing (before VERSANDT) ────────────────────────────────
        .route(
            "/api/v1/angebote/:id",
            axum::routing::put(handlers::put_angebot),
        )
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(pool.clone()));

    let port = cfg.port.unwrap_or(9080);
    let addr = format!("0.0.0.0:{port}");
    info!(%addr, "tarifbd starting");

    // ── Background: auto-expire stale Angebote ───────────────────────────────
    // Runs daily; marks ANGELEGT/VERSANDT Angebote past gueltig_bis as ABGELAUFEN.
    // Without this, expired quotations accumulate in the VERSANDT state and
    // sales staff waste time on dead leads.
    {
        let pool_bg = pool.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            loop {
                match tarifbd::pg::expire_stale_angebote(&pool_bg).await {
                    Ok(n) if n > 0 => {
                        tracing::info!(expired = n, "tarifbd: auto-expired stale Angebote")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "tarifbd: expire_stale_angebote failed"),
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    axum::serve(listener, app).await.context("serve")
}
