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
//! | `WASSER` | Drinking water + gesplittete Abwassergebühr (municipal) | `calculate_wasser` |
//! | `SOLAR` | Mieterstrom §42b, §42a Gemeinschaftliche Gebäudeversorgung | `calculate_solar` |
//! | `EEG` | Feed-in settlement: Vergütung, Marktprämie, Managementprämie | `calculate_eeg` |
//! | `EINSPEISUNG` | Non-EEG Direktvermarktung settlement | `calculate_einspeisung` |
//! | `WAERMEPUMPE` | Heat pump electricity supply with §14a Modul 1/3 | `calculate_strom` |
//! | `WALLBOX` | EV home charging with §14a Modul 1/3 | `calculate_strom` |
//! | `HEMS` | Home Energy Management System platform + events | `calculate_hems` |
//! | `EMOBILITY` | CPO/EMSP charging services | `calculate_emobility` |
//! | `ENERGIEDIENSTLEISTUNG` | MSB, EMS, smart meter, maintenance | `calculate_energiedienstleistung` |
//! | `BUNDLE` | Composite: references component product codes | per-component |
//! | `SHARING` | §42c EnWG Energy Sharing | `calculate_strom` + share allocation |
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
use mako_service::{Daemon, ServiceContext};
use std::sync::Arc;
use tarifbd::{config, handlers, mcp_server};
use tracing::info;

/// The `tarifbd` daemon. `mako_service::run` owns the lifecycle (tracing, tuned
/// pool, real DB-ping readiness, graceful shutdown); this only supplies the
/// migrations and the domain router (product/tariff catalog + MCP server) plus
/// the Angebot auto-expiry worker.
struct Tarifbd;

impl Daemon for Tarifbd {
    type Config = config::TarifbdConfig;
    const NAME: &'static str = "tarifbd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run tarifbd migrations")?;
        Ok(())
    }

    async fn build(cfg: Arc<config::TarifbdConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        let pool = ctx.pool().clone();

        // ── OIDC/JWT authentication ───────────────────────────────────────────
        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.tenant,
            ctx.shutdown.clone(),
        )
        .await
        .context("OIDC setup")?;

        // ── MCP server state ──────────────────────────────────────────────────
        let mcp_state = Arc::new(mcp_server::TarifbdMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
        });

        let app = Router::new()
            .merge(mcp_server::router(mcp_state, ctx.shutdown.clone()))
            // ── Product CRUD ────────────────────────────────────────────────
            .route(
                "/api/v1/products/:lf_mp_id/:product_code",
                put(handlers::put_product)
                    .get(handlers::get_product)
                    .delete(handlers::delete_product),
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
                get(handlers::get_customer_product_handler)
                    .put(handlers::put_customer_product_handler),
            )
            .route(
                "/api/v1/customer/:malo_id/product/history",
                get(handlers::get_customer_product_history_handler),
            )
            // ── EPEX Spot prices ──────────────────────────────────────────────────
            .route("/api/v1/epex-prices/:date", put(handlers::put_epex_prices))
            .route(
                "/api/v1/epex-prices/:date/quarter-hourly",
                get(handlers::get_epex_prices_quarter_hourly),
            )
            .route(
                "/api/v1/epex-prices/:year/:month/average",
                get(handlers::get_epex_monthly_average),
            )
            // ── nEHS certificate prices (BEHG CO₂, auctioned since 2026) ──────────
            .route("/api/v1/nehs-prices/:date", put(handlers::put_nehs_price))
            .route(
                "/api/v1/nehs-prices/latest",
                get(handlers::get_nehs_price_latest),
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
                "/api/v1/angebote/:id/comparison",
                get(handlers::get_angebot_comparison),
            )
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
            // ── Comparison portal feed (public, ETag-cached) ──────────────────────
            // GET /api/v1/comparison-feed?sparte=STROM&verbrauch_kwh=3500&limit=100
            // No auth required — returns public product data only.
            // Responses cached 5 minutes (Cache-Control: public, max-age=300).
            .route(
                "/api/v1/comparison-feed",
                get(handlers::get_comparison_feed),
            )
            // GET /api/v1/comparison-feed/bo4e — §42d EnWG: full BO4E Tarifinfo array
            // for direct schema-validated import by Verivox / Check24 / BNetzA MTS.
            .route(
                "/api/v1/comparison-feed/bo4e",
                get(handlers::get_comparison_feed_bo4e),
            )
            .layer(Extension(oidc))
            .layer(Extension(Arc::clone(&cfg)))
            .layer(Extension(pool.clone()));

        info!("tarifbd starting");

        // ── Background: auto-expire stale Angebote ───────────────────────────
        // Runs daily; marks ANGELEGT/VERSANDT Angebote past gueltig_bis as
        // ABGELAUFEN. Without this, expired quotations accumulate in the
        // VERSANDT state and sales staff waste time on dead leads.
        {
            let pool_bg = pool.clone();
            let shutdown = ctx.shutdown.clone();
            tokio::spawn(async move {
                // Initial 60 s grace after startup, then sweep every 23 h.
                let first = tokio::time::sleep(tokio::time::Duration::from_secs(60));
                tokio::select! {
                    _ = first => {}
                    _ = shutdown.cancelled() => return,
                }
                let mut interval =
                    tokio::time::interval(tokio::time::Duration::from_secs(23 * 3600));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await; // consume the immediate first tick
                loop {
                    match tarifbd::pg::expire_stale_angebote(&pool_bg).await {
                        Ok(n) if n > 0 => {
                            tracing::info!(expired = n, "tarifbd: auto-expired stale Angebote")
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "tarifbd: expire_stale_angebote failed")
                        }
                    }
                    tokio::select! {
                        _ = interval.tick() => {}
                        _ = shutdown.cancelled() => break,
                    }
                }
            });
        }

        Ok(app)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Tarifbd>().await
}
