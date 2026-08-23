//! `outputd` — Customer-Communications daemon.
//!
//! Port: `:9880`
//!
//! Extracted from `billingd` 2026-08-10: the document engine renders what other
//! services computed and owns nothing about *what* a document says — see
//! `outputd::document` for the layering and `document::mahnung` for why the
//! view contracts live with the renderer. `outputd::delivery` owns the other
//! half: the documents actually issued, the channels they go out on (portal
//! inbox, e-mail, print spool, ERP) and the per-document evidence that they
//! arrived.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/render/{kind}` | Render a view with the current or a pinned template; `X-Mako-Template-Hash` names the layout used. Stores nothing |
//! | `POST` | `/api/v1/documents/{kind}` | Render, **record** and queue for delivery; idempotent on `subject_ref` |
//! | `GET` | `/api/v1/documents` | A customer's documents (`?malo_id=` or `?kunden_nr=`, `&kind=`) — the portal inbox |
//! | `GET` | `/api/v1/documents/{id}` | One document with every delivery track |
//! | `GET` | `/api/v1/documents/{id}/content` | The bytes as issued (§ 147 AO reproduction, never a re-render) |
//! | `POST` | `/api/v1/deliveries/{id}/read` | The customer opened it in the portal |
//! | `POST` | `/api/v1/deliveries/{id}/status` | A channel reports arrival or a bounce |
//! | `GET` | `/api/v1/spool` | What a print service collects |
//! | `GET` | `/api/v1/templates` | Every template this tenant published (`?kind=&limit=`) |
//! | `POST` | `/api/v1/templates` | Prove a template, then store it forever |
//! | `POST` | `/api/v1/templates/preview` | Render a candidate against the kind's specimen; stores nothing |
//! | `GET` | `/api/v1/templates/reference/{kind}` | The reference layout mako ships (INVOICE, MAHNUNG) |
//! | `GET`/`PUT` | `/api/v1/templates/{kind}/current` | Which template is rolled out |
//! | `GET` | `/api/v1/templates/by-hash/{hash}` | Resolve the layout an issued document used |
//! | `GET` | `/health` · `/health/ready` | Liveness · readiness |

use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use mako_service::{Daemon, ServiceContext};
use outputd::{config, handlers};

/// The `outputd` daemon. `mako_service::run` owns the lifecycle; this supplies
/// migrations, the router, and the OIDC verifier.
struct Outputd;

impl Daemon for Outputd {
    type Config = config::OutputdConfig;
    const NAME: &'static str = "outputd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run outputd migrations")?;
        Ok(())
    }

    async fn build(cfg: Arc<config::OutputdConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        // Fail closed: an unauthenticated outputd lets anyone publish the
        // layout every customer document renders with — and render arbitrary
        // content under the operator's Briefkopf.
        if !cfg.allow_insecure_no_auth && cfg.oidc.is_none() {
            anyhow::bail!(
                "refusing to start without [oidc]: template publishing and \
                 rendering would accept unauthenticated requests. Configure \
                 [oidc] or set allow_insecure_no_auth = true (dev only)."
            );
        }
        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "allow_insecure_no_auth is set — HTTP API authentication is degraded (dev mode)"
            );
        }

        // ── Cedar ABAC ────────────────────────────────────────────────────────
        // Authentication says *who* is calling; this says what they may do.
        // outputd enabled the `cedar` feature and enforced nothing, so any
        // authenticated caller could roll out the layout every invoice and
        // Mahnung of the tenant renders with, or render arbitrary content under
        // the operator's Briefkopf. A template is not one document — it is the
        // shape of all of them.
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/outputd.cedar"
            ))
            .context("outputd.cedar must parse at startup")?,
        );

        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.tenant,
            ctx.shutdown.clone(),
        )
        .await
        .context("OIDC setup")?;

        // ── Document delivery worker ──────────────────────────────────────────
        // Drains the queued deliveries with backoff and a ceiling. Replicas are
        // safe: the claim is `FOR UPDATE SKIP LOCKED`.
        tokio::spawn(outputd::delivery::worker::run(
            ctx.pool().clone(),
            Arc::clone(&cfg),
            ctx.http.clone(),
            ctx.shutdown.clone(),
        ));

        Ok(Router::new()
            .route("/api/v1/render/{kind}", post(handlers::post_render))
            // ── Issued documents and their delivery ───────────────────────────
            .route("/api/v1/documents/{kind}", post(handlers::post_document))
            .route("/api/v1/documents", get(handlers::list_documents))
            .route(
                "/api/v1/documents/{document_id}",
                get(handlers::get_document),
            )
            .route(
                "/api/v1/documents/{document_id}/content",
                get(handlers::get_document_content),
            )
            .route(
                "/api/v1/deliveries/{delivery_id}/read",
                post(handlers::post_delivery_read),
            )
            .route(
                "/api/v1/deliveries/{delivery_id}/status",
                post(handlers::post_delivery_status),
            )
            .route("/api/v1/spool", get(handlers::get_spool))
            .route(
                "/api/v1/templates",
                get(handlers::list_templates).post(handlers::post_template),
            )
            .route(
                "/api/v1/templates/preview",
                post(handlers::post_template_preview),
            )
            .route(
                "/api/v1/templates/reference/{kind}",
                get(handlers::get_reference_template),
            )
            .route(
                "/api/v1/templates/{kind}/current",
                get(handlers::get_current_template).put(handlers::put_current_template),
            )
            .route(
                "/api/v1/templates/by-hash/{hash}",
                get(handlers::get_template_by_hash),
            )
            .layer(Extension(oidc))
            .layer(Extension(cedar))
            .layer(Extension(cfg.clone()))
            .layer(Extension(ctx.pool().clone())))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Outputd>().await
}
