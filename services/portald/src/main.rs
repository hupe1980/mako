//! `portald` — Customer portal read-model gateway.
//!
//! Aggregates Lastgang (edmd), invoices (billingd), account balance (accountingd),
//! VersorgungsStatus (marktd), and EEG settlement status (einsd) into a single
//! customer-facing REST + Server-Sent Events API.
//!
//! ## Authentication
//!
//! All portal endpoints require a valid OIDC JWT bearer token issued by the
//! configured `oidc_issuer`.  When `oidc_issuer` is absent in config,
//! authentication is skipped (dev/test mode only).
//!
//! The JWT `sub` claim must match the `malo_id` path parameter **or** the
//! operator's ERP must inject a claim that maps customers to MaLo IDs.  The
//! exact claim-to-MaLo mapping is operator-configurable (default: `sub == malo_id`).
//!
//! ## Port
//!
//! `:9480`


use portald::{clients, config, handlers, mcp_server};
use std::sync::Arc;

use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use handlers::PortalClients;
use mako_service::health::health_routes;
use mako_service::load_config;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{clients::UpstreamClient, config::PortaldConfig};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg: PortaldConfig = load_config("portald")?;
    let port = cfg.port;

    // Build upstream clients.
    let clients = Arc::new(PortalClients {
        edmd: cfg
            .edmd_url
            .as_deref()
            .map(|u| Arc::new(UpstreamClient::new("edmd", u, cfg.edmd_api_key.clone()))),
        billingd: cfg.billingd_url.as_deref().map(|u| {
            Arc::new(UpstreamClient::new(
                "billingd",
                u,
                cfg.billingd_api_key.clone(),
            ))
        }),
        accountingd: cfg.accountingd_url.as_deref().map(|u| {
            Arc::new(UpstreamClient::new(
                "accountingd",
                u,
                cfg.accountingd_api_key.clone(),
            ))
        }),
        einsd: cfg
            .einsd_url
            .as_deref()
            .map(|u| Arc::new(UpstreamClient::new("einsd", u, cfg.einsd_api_key.clone()))),
        marktd: cfg
            .marktd_url
            .as_deref()
            .map(|u| Arc::new(UpstreamClient::new("marktd", u, cfg.marktd_api_key.clone()))),
        // Write-capable vertragd client for self-service portal write API (L3).
        vertragd: cfg.vertragd_url.as_deref().map(|u| {
            Arc::new(UpstreamClient::new(
                "vertragd",
                u,
                cfg.vertragd_api_key.clone(),
            ))
        }),
    });

    let cfg = Arc::new(cfg);

    let shutdown = CancellationToken::new();
    let mcp_state = std::sync::Arc::new(mcp_server::PortaldMcpState {
        clients: Arc::clone(&clients),
    });

    let app = Router::new()
        // Dashboard — aggregated customer snapshot.
        .route(
            "/api/v1/portal/:malo_id/dashboard",
            get(handlers::get_dashboard),
        )
        // Energy consumption.
        .route(
            "/api/v1/portal/:malo_id/lastgang",
            get(handlers::get_lastgang),
        )
        // Billing & invoices.
        .route(
            "/api/v1/portal/:malo_id/invoices",
            get(handlers::get_invoices),
        )
        // Customer account ledger.
        .route(
            "/api/v1/portal/:malo_id/balance",
            get(handlers::get_balance),
        )
        .route(
            "/api/v1/portal/:malo_id/kontoauszug",
            get(handlers::get_kontoauszug),
        )
        // EEG plant status.
        .route("/api/v1/portal/:malo_id/eeg", get(handlers::get_eeg_status))
        // Supply status.
        .route(
            "/api/v1/portal/:malo_id/versorgung",
            get(handlers::get_versorgung),
        )
        // Real-time SSE stream.
        .route("/api/v1/portal/:malo_id/events", get(handlers::sse_events))
        // ── Self-service write API (L3 — §41 EnWG) ───────────────────────────
        // Contract view — prerequisite for Tarifwechsel / Kündigung UI.
        .route(
            "/api/v1/portal/:malo_id/vertrag",
            get(handlers::get_portal_vertrag),
        )
        // §41 Abs. 1 EnWG — Tarifwechsel (minimum 14 days notice)
        .route(
            "/api/v1/portal/:malo_id/tarifwechsel",
            post(handlers::post_portal_tarifwechsel),
        )
        // §41 Abs. 3 EnWG — Kündigung (minimum 14 days, end-of-month billing boundary)
        .route(
            "/api/v1/portal/:malo_id/kuendigen",
            post(handlers::post_portal_kuendigen),
        )
        // GDPR Art. 16 — contact data update (Geschaeftspartner + SEPA)
        .route(
            "/api/v1/portal/:malo_id/kontakt",
            put(handlers::put_portal_kontakt),
        )
        // Document download — ZUGFeRD 2.3 / XRechnung 3.0 CII XML
        .route(
            "/api/v1/portal/:malo_id/invoices/:record_id/download",
            get(handlers::get_portal_invoice_download),
        )
        // MCP server.
        .merge(mcp_server::router(Arc::clone(&mcp_state), shutdown.clone()))
        // Health endpoints.
        .merge(health_routes(|| async { true }))
        .layer(Extension(cfg))
        .layer(Extension(clients));

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();
    tracing::info!(port, "portald listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
