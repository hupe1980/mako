//! `sperrd` — Sperrung execution tracking daemon.
//!
//! Tracks remote disconnection/reconnection orders (Sperrung/Entsperrung) and
//! auto-dispatches IFTSTA 21039 (field confirmation) via `makod` when the
//! field-service team reports execution.
//!
//! Without `sperrd`, a missed IFTSTA 21039 leaves the Sperrung permanently
//! unresolved in the LF system — a GPKE protocol violation under BK6-22-024.
//!
//! Port: `:8780`
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST`  | `/api/v1/sperr-orders` | Register a new Sperrung order |
//! | `GET`   | `/api/v1/sperr-orders` | List orders (`?status=&malo_id=`) |
//! | `GET`   | `/api/v1/sperr-orders/{id}` | Fetch single order |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/execute` | Report field execution → auto-dispatch IFTSTA 21039 |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/fail` | Report field failure → operator escalation |
//! | `GET`   | `/health` | Liveness check |
//! | `GET`   | `/health/ready` | Readiness check |

use anyhow::Context as _;
use axum::{Extension, Router, routing::get};
use mako_markt::makod_client::MakodClient;
use mako_service::{health::health_routes, load_config};
use secrecy::SecretString;
use sperrd::{config, handlers, mcp_server};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("sperrd");

    let cfg: config::SperrdConfig = load_config("sperrd").context("load config")?;

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    let makod = Arc::new(MakodClient::new(
        &cfg.makod_url,
        SecretString::from(cfg.makod_api_key.clone()),
    ));

    // Schema must be applied manually — see migrations/0001_initial.sql for DDL.

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        .route("/api/v1/sperr-orders/stats", get(handlers::get_stats))
        .route(
            "/api/v1/sperr-orders",
            get(handlers::list_orders).post(handlers::create_order),
        )
        .route("/api/v1/sperr-orders/:id", get(handlers::get_order))
        .route(
            "/api/v1/sperr-orders/:id/execute",
            axum::routing::put(handlers::execute_order),
        )
        .route(
            "/api/v1/sperr-orders/:id/fail",
            axum::routing::put(handlers::fail_order),
        )
        .route(
            "/api/v1/sperr-orders/:id/cancel",
            axum::routing::put(handlers::cancel_order),
        )
        .layer(Extension(makod))
        .layer(Extension(config::Tenant(cfg.tenant.clone())))
        .layer(Extension(pool.clone()));

    // ── MCP server ────────────────────────────────────────────────────────────
    let mcp_state = std::sync::Arc::new(mcp_server::SperrdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
    });
    let ct = mako_service::shutdown::token();
    let app = app.merge(mcp_server::router(mcp_state, ct.clone()));

    let addr = format!("0.0.0.0:{}", cfg.port.unwrap_or(8780));
    info!(%addr, "sperrd starting");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    mako_service::shutdown::serve(listener, app, ct).await
}
