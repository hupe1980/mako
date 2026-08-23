//! Router assembly and daemon wiring for `portald`.

use std::sync::Arc;

use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_service::{Daemon, ServiceContext};

use crate::{
    clients::{PortalClients, UpstreamClient},
    config::PortaldConfig,
    handlers, mcp_server,
};

/// The `portald` daemon — stateless, so no pool and no migrations.
pub struct Portald;

impl Daemon for Portald {
    type Config = PortaldConfig;
    const NAME: &'static str = "portald";

    async fn build(cfg: Arc<PortaldConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        let mut cfg = (*cfg).clone();
        cfg.resolve_env_refs()?;

        // Every portal route serves one customer's consumption, ledger and
        // invoices, and `vertragd` is the only thing that knows which customer.
        // Starting without it is refused unless the operator asked for it by
        // name — an omitted URL is a mistake, not a request to serve everyone's
        // data to everyone.
        if cfg.vertragd_url.is_none() && !cfg.allow_insecure_no_auth {
            anyhow::bail!(
                "no `vertragd_url` configured. Every portal route resolves customer \
                 ownership through vertragd, so without it portald cannot tell one \
                 customer's Lastgang, Kontoauszug and invoices from another's. Set \
                 `vertragd_url`, or set `allow_insecure_no_auth = true` to accept a \
                 deployment that serves them unauthorised."
            );
        }
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

        let mcp_state = Arc::new(mcp_server::PortaldMcpState {
            clients: Arc::clone(&clients),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
        });

        Ok(router()
            .merge(mcp_server::router(mcp_state, ctx.shutdown.clone()))
            .layer(Extension(cfg))
            .layer(Extension(clients)))
    }
}

/// The portal routes.
///
/// Every one of them is customer-scoped and authorises through
/// [`crate::auth::authorize`]; the infra routes (`/health/*`, `/metrics`) are
/// the runner's and are not mounted here.
pub fn router() -> Router {
    Router::new()
        // ── Read model ───────────────────────────────────────────────────
        .route(
            "/api/v1/portal/{malo_id}/dashboard",
            get(handlers::get_dashboard),
        )
        .route(
            "/api/v1/portal/{malo_id}/lastgang",
            get(handlers::get_lastgang),
        )
        .route(
            "/api/v1/portal/{malo_id}/invoices",
            get(handlers::get_invoices),
        )
        .route(
            "/api/v1/portal/{malo_id}/dokumente",
            get(handlers::get_dokumente),
        )
        .route(
            "/api/v1/portal/{malo_id}/dokumente/{document_id}",
            get(handlers::get_dokument),
        )
        .route(
            "/api/v1/portal/{malo_id}/invoices/{record_id}/download",
            get(handlers::get_portal_invoice_download),
        )
        .route(
            "/api/v1/portal/{malo_id}/balance",
            get(handlers::get_balance),
        )
        .route(
            "/api/v1/portal/{malo_id}/kontoauszug",
            get(handlers::get_kontoauszug),
        )
        .route(
            "/api/v1/portal/{malo_id}/vorauszahlung",
            get(handlers::get_portal_vorauszahlung),
        )
        .route(
            "/api/v1/portal/{malo_id}/eeg",
            get(handlers::get_eeg_status),
        )
        .route(
            "/api/v1/portal/{malo_id}/versorgung",
            get(handlers::get_versorgung),
        )
        // ── Contract + self-service writes (§ 41 EnWG customer rights) ───
        .route(
            "/api/v1/portal/{malo_id}/vertrag",
            get(handlers::get_portal_vertrag),
        )
        .route(
            "/api/v1/portal/{malo_id}/kuendigungsfrist",
            get(handlers::get_portal_kuendigungsfrist),
        )
        .route(
            "/api/v1/portal/{malo_id}/tarifwechsel",
            post(handlers::post_portal_tarifwechsel),
        )
        .route(
            "/api/v1/portal/{malo_id}/kuendigen",
            post(handlers::post_portal_kuendigen),
        )
        .route(
            "/api/v1/portal/{malo_id}/kontakt",
            put(handlers::put_portal_kontakt),
        )
        .route(
            "/api/v1/portal/{malo_id}/sepa",
            put(handlers::put_portal_sepa),
        )
}
