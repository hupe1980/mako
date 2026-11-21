//! Router assembly and daemon wiring for `portald`.

use std::sync::Arc;

use axum::{Extension, Router, routing::get};
use mako_service::{Daemon, ServiceContext};

use crate::{
    clients::{PortalClients, UpstreamClient},
    config::PortaldConfig,
};

/// The `portald` daemon — stateless, so no pool and no migrations.
pub struct Portald;

impl Daemon for Portald {
    type Config = PortaldConfig;
    const NAME: &'static str = "portald";

    /// Every portal route serves one customer's consumption, ledger and
    /// invoices, and `vertragd` is the only thing that knows which customer.
    /// Starting without it is refused unless the operator asked for it by name
    /// — an omitted URL is a mistake, not a request to serve everyone's data to
    /// everyone.
    fn validate(cfg: &PortaldConfig) -> Vec<mako_service::Befund> {
        let mut out = Vec::new();
        // An `env:` reference resolves at build; its value is checked there.
        if !cfg.lf_mp_id.starts_with("env:")
            && let Err(e) = rubo4e::identifiers::MarktpartnerId::new(cfg.lf_mp_id.trim())
        {
            out.push(mako_service::Befund::blockiert(
                "lf_mp_id",
                format!(
                    "lf_mp_id {:?} is not an MP-ID ({e}) — set the Lieferant's 13-digit \
                     BDEW- or DVGW-Codenummer, as in accountingd",
                    cfg.lf_mp_id
                ),
            ));
        }
        if cfg.vertragd_url.is_none() && !cfg.allow_insecure_no_auth {
            out.push(
                mako_service::Befund::blockiert(
                    "vertragd_url",
                    "no `vertragd_url` configured. Every portal route resolves customer \
                     ownership through vertragd, so without it portald cannot tell one \
                     customer's Lastgang, Kontoauszug and invoices from another's. Set \
                     `vertragd_url`, or set `allow_insecure_no_auth = true` to accept a \
                     deployment that serves them unauthorised.",
                )
                .bereich(mako_service::befund::Bereich::Zugriff),
            );
        }
        out
    }

    async fn build(cfg: Arc<PortaldConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        let mut cfg = (*cfg).clone();
        cfg.resolve_env_refs()?;
        rubo4e::identifiers::MarktpartnerId::new(cfg.lf_mp_id.trim())
            .map_err(|e| anyhow::anyhow!("lf_mp_id {:?} is not an MP-ID: {e}", cfg.lf_mp_id))?;

        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "portald: allow_insecure_no_auth is set — every caller can read every \
                 customer's consumption, account statement and invoices in this tenant"
            );
        }

        let cfg = Arc::new(cfg);
        let up = |name: &'static str, url: Option<&String>, key: Option<&String>| {
            url.map(|u| {
                Arc::new(UpstreamClient::new(
                    name,
                    u,
                    key.cloned().map(secrecy::SecretString::from),
                    ctx.http.clone(),
                ))
            })
        };
        let clients = Arc::new(PortalClients {
            edmd: up("edmd", cfg.edmd_url.as_ref(), cfg.edmd_api_key.as_ref()),
            billingd: up(
                "billingd",
                cfg.billingd_url.as_ref(),
                cfg.billingd_api_key.as_ref(),
            ),
            accountingd: up(
                "accountingd",
                cfg.accountingd_url.as_ref(),
                cfg.accountingd_api_key.as_ref(),
            ),
            einsd: up("einsd", cfg.einsd_url.as_ref(), cfg.einsd_api_key.as_ref()),
            marktd: up(
                "marktd",
                cfg.marktd_url.as_ref(),
                cfg.marktd_api_key.as_ref(),
            ),
            vertragd: up(
                "vertragd",
                cfg.vertragd_url.as_ref(),
                cfg.vertragd_api_key.as_ref(),
            ),
            outputd: up(
                "outputd",
                cfg.outputd_url.as_ref(),
                cfg.outputd_api_key.as_ref(),
            ),
            auth_client: ctx.http.clone(),
        });

        Ok(router().layer(Extension(cfg)).layer(Extension(clients)))
    }
}

/// The portal routes.
///
/// Every `/api/v1` route is customer-scoped, authorises through
/// [`crate::auth::authorize`] and is mounted by [`crate::openapi::api_router`],
/// which also yields the document served at `GET /api/v1/openapi.json`.
///
/// `GET /nachrechnen/{token}` is mounted here as a plain route, outside that
/// document: it is a public HTML page for a person reading a printed invoice,
/// not a JSON API, and the token in its path is its only credential (see
/// [`crate::nachrechnen`]). The infra routes (`/health/*`, `/metrics`) are the
/// runner's and are not mounted here.
pub fn router() -> Router {
    let (api, doc) = crate::openapi::api_router();
    api.merge(mako_service::openapi::serve(doc))
        // ── Public, token-addressed HTML: the one route without a customer
        // session and outside the API document (see `crate::nachrechnen`).
        .route("/nachrechnen/{token}", get(crate::nachrechnen::get_page))
        .layer(Extension(std::sync::Arc::new(
            crate::nachrechnen::Limiter::default(),
        )))
}
