//! `billingd` — Energy Billing Engine.
//!
//! Pure calculation service.  Pulls product definitions from `tarifbd`,
//! consumption from `edmd`, and grid pass-through from `marktd`.
//! Outputs canonical BO4E `Rechnung` objects and emits
//! `de.billing.rechnung.erstellt` CloudEvents consumed by `accountingd`.
//!
//! ## Design: user-defined pricing
//!
//! All commercial rates (Arbeitspreis, Grundpreis, etc.) are defined by the
//! operator in `tarifbd` — the engine contains zero hardcoded prices.
//! Statutory rates (Stromsteuer, Energiesteuer Gas, BEHG) are configured in
//! `billingd.toml` under `[rates]` and can be overridden per-product.
//!
//! ## Supported product categories
//!
//! | Category | Calculator | Key regulatory refs |
//! |---|---|---|
//! | `STROM` | `calculate_strom` | §41a EnWG (dynamic), §14a Modul 1/3 |
//! | `GAS` | `calculate_gas` | §10 GasGVV (Brennwertkorrektur), §2 EnergieStG, BEHG |
//! | `WAERME` | `calculate_waerme` | EnWG Fernwärme |
//! | `SOLAR` | `calculate_solar` | §42b EEG (Mieterstrom), §42a EEG (GGV) |
//! | `EEG` | `calculate_eeg` | §21 EEG (Vergütung), §38 EEG (Marktprämie), §53 EEG |
//! | `EINSPEISUNG` | `calculate_einspeisung` | Direktvermarktung, Marktwert |
//! | `WAERMEPUMPE` | `calculate_strom` + §14a | §14a EnWG Modul 1/3 |
//! | `WALLBOX` | `calculate_strom` + §14a | §14a EnWG Modul 1/3 |
//! | `HEMS` | `calculate_hems` | Platform + event billing |
//! | `EMOBILITY` | `calculate_emobility` | CPO/EMSP service billing |
//! | `ENERGIEDIENSTLEISTUNG` | `calculate_energiedienstleistung` | MSB, EMS, maintenance |
//! | `BUNDLE` | composite | Per-component recursion via tarifbd |
//!
//! Port: `:9280`
//!
//! ## Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/billing/{malo_id}/calculate` | Calculate + persist + emit CloudEvent |
//! | `POST` | `/api/v1/billing/{malo_id}/preview` | Dry-run (no persist) |
//! | `POST` | `/api/v1/billing/{id}/correction` | Korrekturrechnung / Stornorechnung (\u00a722 Me\u00dfZV) |
//! | `POST` | `/api/v1/billing/sammelrechnung/{rv_id}` | B2B consolidated Sammelrechnung |
//! | `POST` | `/api/v1/billing/ggv/{ggv_id}` | \u00a742a GGV multi-tenant community solar billing |
//! | `POST` | `/api/v1/billing/vpp/{vpp_id}` | VPP aggregation settlement (RED III Art. 17) |
//! | `POST` | `/api/v1/billing/{id}/submit-b2g` | XRechnung B2G submission (\u00a727 EGovG 01.01.2027) |
//! | `GET` | `/api/v1/billing` | List records (`?malo_id=&lf_mp_id=&outcome=`) |
//! | `GET` | `/api/v1/billing/{id}` | Fetch single record |
//! | `GET` | `/api/v1/billing/{id}/xrechnung` | ZUGFeRD 2.3 / XRechnung 3.0 CII XML |
//! | `GET` | `/api/v1/billing/{id}/ubl` | PEPPOL BIS Billing 3.0 UBL 2.1 XML (EN16931) |
//! | `GET` | `/health` | Liveness |
//! | `GET` | `/health/ready` | Readiness |

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use billingd::{clients, config, handlers, mcp_server};
use mako_markt::marktd_client::MarktdClient;
use mako_service::{health::health_routes, load_config};
use secrecy::SecretString;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("billingd");

    let cfg: config::BillingdConfig = load_config("billingd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    let tarifbd = Arc::new(clients::TarifbdClient::new(&cfg.tarifbd_url));
    let edmd = Arc::new(clients::EdmdClient::new(
        &cfg.edmd_url,
        cfg.edmd_api_key.clone(),
    ));
    let marktd = Arc::new(MarktdClient::new(
        &cfg.marktd_url,
        SecretString::from(cfg.marktd_api_key.clone().unwrap_or_default()),
        mako_service::http::default_client(),
    ));
    let vertragd = Arc::new(clients::VertragdClient::new(
        cfg.vertragd_url
            .as_deref()
            .unwrap_or("http://localhost:9780"),
    ));

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        .route(
            "/api/v1/billing/:malo_id/calculate",
            post(handlers::post_calculate),
        )
        .route("/api/v1/billing", get(handlers::list_records))
        .route("/api/v1/billing/:id", get(handlers::get_record))
        .route(
            "/api/v1/billing/:id/xrechnung",
            get(handlers::get_xrechnung),
        )
        .route(
            "/api/v1/billing/:malo_id/preview",
            post(handlers::post_preview),
        )
        // L8: Korrekturrechnung / Stornorechnung (§22 MessZV audit trail)
        .route(
            "/api/v1/billing/:id/correction",
            post(handlers::post_correction),
        )
        // L2: B2B Sammelrechnung for Rahmenvertrag with rechnungsstellung=SAMMEL
        .route(
            "/api/v1/billing/sammelrechnung/:rahmenvertrag_id",
            post(handlers::post_sammelrechnung),
        )
        // B1: §42a GGV community solar multi-tenant proportional billing
        .route(
            "/api/v1/billing/ggv/:ggv_id",
            post(handlers::post_ggv_billing),
        )
        // B12: VPP aggregation billing (RED III Article 17) — de.vpp.settlement.berechnet
        .route(
            "/api/v1/billing/vpp/:vpp_id",
            post(handlers::post_vpp_billing),
        )
        // B10: XRechnung B2G submission (\u00a727 EGovG — mandatory from 01.01.2027)
        .route(
            "/api/v1/billing/:id/submit-b2g",
            post(handlers::post_submit_b2g),
        )
        // B11: PEPPOL BIS Billing 3.0 UBL 2.1 XML (EN16931 — mandatory from 01.01.2028)
        .route(
            "/api/v1/billing/:id/ubl",
            axum::routing::get(handlers::get_ubl),
        )
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(tarifbd))
        .layer(Extension(edmd))
        .layer(Extension(marktd))
        .layer(Extension(vertragd))
        .layer(Extension(pool.clone()));

    let port = cfg.port.unwrap_or(9280);
    let addr = format!("0.0.0.0:{port}");
    info!(%addr, "billingd starting");

    // ── MCP server ────────────────────────────────────────────────────────────
    let mcp_state = std::sync::Arc::new(mcp_server::BillingdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
        self_url: format!("http://localhost:{port}"),
        seller_name: cfg
            .seller_name
            .clone()
            .unwrap_or_else(|| cfg.tenant.clone()),
        seller_vat_id: cfg.seller_vat_id.clone(),
    });
    let ct = mako_service::shutdown::token();
    let app = app.merge(mcp_server::router(mcp_state, ct.clone()));

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    mako_service::shutdown::serve(listener, app, ct).await
}
