//! `sperrd` — Sperr-/Entsperrauftrag execution queue (Netzbetreiber role).
//!
//! The grid operator's work queue for the physical acts GPKE orders it to
//! perform: an ORDERS **17115 Sperrauftrag** or **17117 Entsperrauftrag** from a
//! Lieferant becomes a job for the field team, and the outcome goes back as
//! **IFTSTA 21039** (Auftragsstatus Sperren/Entsperren).
//!
//! Without the IFTSTA the Lieferant's `gpke-sperrung-lf` process never reaches a
//! terminal state and GPKE gives them no way to find out what happened but to
//! ask — so dispatching it is the service's whole reason to exist, and the one
//! state it will not let fall silently on the floor (see `worker`).
//!
//! Port: `:8780`
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST`  | `/webhook` | Market ingest: `de.mako.process.initiated` for ORDERS 17115/17117 |
//! | `POST`  | `/api/v1/sperr-orders` | Create an order by hand |
//! | `GET`   | `/api/v1/sperr-orders` | The queue (`?status=&malo_id=&due=true`) |
//! | `GET`   | `/api/v1/sperr-orders/stats` | Counters, incl. outstanding IFTSTA |
//! | `GET`   | `/api/v1/sperr-orders/{id}` | One order |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/execute` | Carried out → IFTSTA `Z14` |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/fail` | Not carried out → IFTSTA `Z13` |
//! | `PUT`   | `/api/v1/sperr-orders/{id}/cancel` | Withdraw a pending order |
//! | `GET`   | `/health/live`, `/health/ready` | Probes |

use std::sync::Arc;

use anyhow::Context as _;
use axum::{Extension, Router, routing::get};
use mako_markt::makod_client::MakodClient;
use mako_service::{Daemon, ServiceContext};
use sperrd::{config, handlers, mcp_server, worker};

/// The `sperrd` daemon. `mako_service::run` owns the lifecycle (tracing, tuned
/// pool, real DB-ping readiness, graceful shutdown); this supplies the
/// migrations, the domain router, the MCP server and the IFTSTA retry worker.
struct Sperrd;

impl Daemon for Sperrd {
    type Config = config::SperrdConfig;
    const NAME: &'static str = "sperrd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run sperrd migrations")?;
        // Transactional outbox for the de.sperr.* events.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure outbox schema")?;
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

        // ── Cedar ABAC ────────────────────────────────────────────────────
        // Authentication says *who* is calling; this says what they may do.
        // sperrd enabled the `cedar` feature and enforced nothing, so every
        // route took `_claims: Claims` and discarded it — a valid token from any
        // tenant could order a disconnection in this operator's name.
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/sperrd.cedar"
            ))
            .context("sperrd.cedar must parse at startup")?,
        );

        let makod = Arc::new(MakodClient::new(&cfg.makod_url, cfg.makod_api_key.clone()));

        let app = Router::new()
            // Market ingest — HMAC-authenticated, not OIDC.
            .route("/webhook", axum::routing::post(handlers::ingest_webhook))
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
            .layer(Extension(Arc::clone(&makod)))
            .layer(Extension(Arc::clone(&cedar)))
            .layer(Extension(config::Tenant(cfg.tenant.clone())))
            .layer(Extension(cfg.inbound_hmac_secret.clone()))
            .layer(Extension(ctx.pool().clone()))
            .layer(Extension(oidc));

        // ── IFTSTA 21039 retry worker ─────────────────────────────────────
        // A terminal order whose IFTSTA never went out leaves the Lieferant
        // waiting indefinitely. This drains that queue.
        tokio::spawn(worker::run(
            ctx.pool().clone(),
            Arc::clone(&makod),
            cfg.tenant.clone(),
            ctx.shutdown.clone(),
        ));

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
