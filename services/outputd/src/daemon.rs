//! The [`Outputd`] daemon: what `mako_service::run` starts.

use std::sync::Arc;

use crate::{config, handlers};
use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use mako_service::{Daemon, ServiceContext};

/// The `outputd` daemon. `mako_service::run` owns the lifecycle; this supplies
/// migrations, the router, and the OIDC verifier.
pub struct Outputd;

impl Daemon for Outputd {
    type Config = config::OutputdConfig;
    const NAME: &'static str = "outputd";

    async fn migrate(_cfg: &Self::Config, pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run outputd migrations")?;
        // Every pushed delivery is an outbox command committed with its
        // document.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")
    }

    fn validate(cfg: &config::OutputdConfig) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        let mut out = Vec::new();
        if cfg.allow_insecure_no_auth {
            return out;
        }
        // Fail closed: an unauthenticated outputd lets anyone publish the
        // layout every customer document renders with — and render arbitrary
        // content under the operator's Briefkopf.
        if cfg.oidc.is_none() {
            out.push(
                Befund::blockiert(
                    "oidc",
                    "refusing to start without [oidc]: template publishing and rendering \
                     would accept unauthenticated requests. Configure [oidc] or set \
                     allow_insecure_no_auth = true (dev only).",
                )
                .bereich(Bereich::Zugriff),
            );
        }
        // The outbound side of the same posture: a relay push carries the
        // customer's document itself — the invoice or Mahnung base64-encoded,
        // with their name, e-mail address, MaLo and Kundennummer beside it. The
        // credential is what signs it, so a URL without one publishes all of
        // that to whoever answers at that address. One finding per relay.
        for key in cfg.delivery.unsigned_relays() {
            out.push(
                Befund::blockiert(
                    format!("delivery.{key}"),
                    format!(
                        "[delivery] configures a relay URL without its credential `{key}` — \
                         customer documents would be POSTed unsigned and unauthenticated. \
                         Configure it, or set allow_insecure_no_auth = true (dev only)."
                    ),
                )
                .bereich(Bereich::Zugriff),
            );
        }
        out
    }

    async fn build(cfg: Arc<config::OutputdConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "allow_insecure_no_auth is set — HTTP API authentication is degraded and \
                 document pushes to a credential-less relay go out unsigned (dev mode)"
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

        // ── Document delivery ─────────────────────────────────────────────────
        // A relay worker per configured channel drains the outbox commands
        // `issue` committed with each document.
        crate::delivery::worker::spawn(ctx.pool().clone(), Arc::clone(&cfg), ctx.shutdown.clone());

        Ok(Router::new()
            .route("/api/v1/render/{kind}", post(handlers::post_render))
            // ── Issued documents and their delivery ───────────────────────────
            //
            // `issue/` is a real path segment, not decoration: axum matches a
            // capture by position, so `POST /documents/{kind}` and
            // `GET /documents/{document_id}` are the *same* place under two
            // names, and registering both panics the router at startup. They
            // are also different resources — one names a document kind, the
            // other one stored document — so the verb belongs in the path.
            .route(
                "/api/v1/documents/issue/{kind}",
                post(handlers::post_document),
            )
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
            // Read-only: GET /admin/einrichtung (Cedar `read-config`) and
            // GET /api/v1/policy (Cedar `read-policy`), from the boot report.
            .merge(mako_service::einrichtung::router(ctx.einrichtung.clone()))
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.tenant.clone(),
            )))
            .layer(Extension(oidc))
            .layer(Extension(cedar))
            .layer(Extension(cfg.clone()))
            .layer(Extension(ctx.pool().clone())))
    }
}
