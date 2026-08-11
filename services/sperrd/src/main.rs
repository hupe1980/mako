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

use std::sync::Arc;

use anyhow::Context as _;
use axum::{Extension, Router, routing::get};
use mako_markt::makod_client::MakodClient;
use mako_service::{Daemon, ServiceContext};
use secrecy::SecretString;
use sperrd::{config, handlers, mcp_server};

/// The `sperrd` daemon. `mako_service::run` owns the lifecycle (tracing, tuned
/// pool, real DB-ping readiness, graceful shutdown); this only supplies the
/// migrations and the domain router + MCP server.
struct Sperrd;

impl Daemon for Sperrd {
    type Config = config::SperrdConfig;
    const NAME: &'static str = "sperrd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run sperrd migrations")?;
        Ok(())
    }

    async fn build(cfg: Arc<config::SperrdConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        // Every mutating route here has a physical effect: `create` schedules a
        // disconnection, `execute` and `fail` each put a real IFTSTA 21039 on
        // the market. Serving them unauthenticated has to be asked for by name,
        // never reached by leaving a section out of the config.
        if cfg.oidc.is_none() && !cfg.allow_insecure_no_auth {
            anyhow::bail!(
                "no [oidc] section configured. The Sperrung routes create and confirm \
                 physical disconnections and dispatch IFTSTA 21039 into the market, so \
                 they are not served without token verification. Configure [oidc], or \
                 set allow_insecure_no_auth = true to accept an unauthenticated \
                 deployment."
            );
        }
        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "sperrd: allow_insecure_no_auth is set — any caller that can open a socket \
                 can order and confirm a Sperrung in this tenant's name"
            );
        }
        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.tenant,
            ctx.shutdown.clone(),
        )
        .await
        .context("OIDC setup")?;

        let makod = Arc::new(MakodClient::new(
            &cfg.makod_url,
            SecretString::from(cfg.makod_api_key.clone()),
        ));

        let app = Router::new()
            .route("/api/v1/sperr-orders/stats", get(handlers::get_stats))
            .route(
                "/api/v1/sperr-orders",
                get(handlers::list_orders).post(handlers::create_order),
            )
            .route("/api/v1/sperr-orders/{id}", get(handlers::get_order))
            .route(
                "/api/v1/sperr-orders/{id}/execute",
                axum::routing::put(handlers::execute_order),
            )
            .route(
                "/api/v1/sperr-orders/{id}/fail",
                axum::routing::put(handlers::fail_order),
            )
            .route(
                "/api/v1/sperr-orders/{id}/cancel",
                axum::routing::put(handlers::cancel_order),
            )
            .layer(Extension(makod))
            .layer(Extension(config::Tenant(cfg.tenant.clone())))
            .layer(Extension(ctx.pool().clone()))
            .layer(Extension(oidc));

        let mcp_state = Arc::new(mcp_server::SperrdMcpState {
            pool: ctx.pool().clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
        });
        Ok(app.merge(mcp_server::router(mcp_state, ctx.shutdown.clone())))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Sperrd>().await
}
