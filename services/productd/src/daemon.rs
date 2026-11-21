//! `productd` — Product & Tariff Catalog.
//!
//! Single source of truth for all retail products the LF sells to end customers.
//! All commercial pricing is defined here — `billingd` reads product definitions
//! from `productd` and calculates invoices from them.
//!
//! ## Product Categories
//!
//! | Category | Description | Billing template |
//! |---|---|---|
//! | `STROM` | Electricity SLP/RLM, Eintarif/Zweitarif/Mehrtarif | `calculate_strom` |
//! | `GAS` | Natural gas SLP/RLM with Brennwertkorrektur | `calculate_gas` |
//! | `WAERME` | District heat / Fernwärme | `calculate_waerme` |
//! | `WASSER` | Drinking water + gesplittete Abwassergebühr (municipal) | `calculate_wasser` |
//! | `SOLAR` | Mieterstrom §21 Abs. 3 EEG, §42b EnWG Gemeinschaftliche Gebäudeversorgung | `calculate_solar` |
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
//! ## Pricing schema (`data.tarifpreise`)
//!
//! Products store prices as BO4E Tarifpreisblatt JSONB.
//! `billingd` reads `preistyp` strings (case-insensitive) to extract rates.
//! Example product for a Strom SLP Eintarif:
//! ```json
//! {
//!   "tarifpreise": [
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
//! ## Authentication and authorization
//!
//! Every route below takes a verified OIDC bearer and a Cedar decision from
//! `policies/productd.cedar`, except the two § 41c EnWG comparison-feed routes,
//! which take no `Claims` at all: § 41c Abs. 5 Satz 2 EnWG obliges a
//! Stromlieferant to let comparison instruments use its offer-relevant
//! information free of charge in open data formats, and an obligation
//! discharged only behind a bearer token is not discharged. The other
//! categories are published by choice. Those two serve
//! `product_status = 'PUBLISHED'` rows of the `FEED_CATEGORIES` allowlist and
//! nothing else, which `tests/it/authorization_guard.rs` pins.
//!
//! Reading the catalogue and the market price series needs only a token of the
//! tenant; writing a Tarifpreisblatt, importing an EPEX day or working an
//! Angebot needs a market role (LF, MSB, ESA, ADMIN).
//!
//! Port: `:9080`

use crate::{config, handlers, mcp_server};
use anyhow::Context as _;
use axum::{Extension, Router};
use mako_service::subscriptions::Subscriptions;
use mako_service::{Daemon, ServiceContext};
use std::sync::Arc;
use tracing::info;
use utoipa::OpenApi as _;
use utoipa_axum::{router::OpenApiRouter, routes};

/// The `productd` daemon. `mako_service::run` owns the lifecycle (tracing, tuned
/// pool, real DB-ping readiness, graceful shutdown); this only supplies the
/// migrations and the domain router (product/tariff catalog + MCP server) plus
/// the Angebot auto-expiry worker.
pub struct Productd;

impl Daemon for Productd {
    type Config = config::ProductdConfig;
    const NAME: &'static str = "productd";

    async fn migrate(_cfg: &Self::Config, pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run productd migrations")?;
        // The accepted Angebot's contract request and the product
        // announcements commit with their business write here.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        mako_service::subscriptions::ensure_schema(pool)
            .await
            .context("ensure webhook subscription schema")
    }

    fn validate(cfg: &Self::Config) -> Vec<mako_service::Befund> {
        cfg.validate()
    }

    fn policy(cfg: &Self::Config) -> mako_service::policy::PolicyReport {
        cfg.policy_report()
    }

    async fn build(
        cfg: Arc<config::ProductdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        cfg.check_auth_posture()?;

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

        // ── Cedar ABAC ────────────────────────────────────────────────────────
        // Authentication says who is calling; this says what they may do. They
        // are separate decisions: comparing the path's `lf_mp_id` with the
        // token's tenant is authentication only, and on its own it lets any
        // token the verifier accepts for the tenant `PUT` a new Arbeitspreis
        // onto a live tariff, which the next `billingd` run bills.
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/productd.cedar"
            ))
            .context("productd.cedar must parse at startup")?,
        );

        // ── Webhook subscriptions on the outbox ──────────────────────────────
        // Every CloudEvent productd commits fans out to the endpoints an
        // operator registers through `/api/v1/webhooks/endpoints`, and to the
        // `default` endpoint the config names.
        let subs = Subscriptions::new(
            pool.clone(),
            Self::NAME,
            &cfg.tenant,
            cfg.subscriptions.clone(),
        )
        .with_default_endpoint(
            cfg.erp_webhook_url.clone(),
            cfg.erp_hmac_secret.clone().map(secrecy::SecretString::from),
        );
        subs.spawn(ctx.shutdown.clone());

        // ── MCP server state ──────────────────────────────────────────────────
        // Gated by the same verifier and the same policy as the REST surface: a
        // JWT is verified and checked for `use-mcp`, and a configured `[mcp]`
        // key stays accepted for agent clients that mint no OIDC token.
        let mcp_state = Arc::new(mcp_server::ProductdMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            lf_mp_id: cfg.lf_mp_id().to_owned(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                &cfg.mcp,
                oidc.clone(),
                Some(Arc::clone(&cedar)),
                &cfg.tenant,
            ),
            policy: Arc::new(ctx.einrichtung.policy.clone()),
            subscriptions: subs.clone(),
        });

        // ── The 2027 BEHG regime: billingd's `[policy] behg_regime` ──────────
        let regime_quelle = match &cfg.billingd_url {
            Some(url) => crate::billingd::BehgRegimeQuelle::billingd(
                url,
                cfg.billingd_api_key.clone(),
                ctx.http.clone(),
            ),
            None => {
                tracing::warn!(
                    "productd: no billingd_url — a Gas quotation for a year § 10 BEHG fixes \
                     no price for is refused BEHG_PREIS_FEHLT"
                );
                crate::billingd::BehgRegimeQuelle::keine()
            }
        };

        let (api, doc) = api(&subs).split_for_parts();
        let app = Router::new()
            .merge(mcp_server::router(mcp_state, ctx.shutdown.clone()))
            // ── Readiness and operator posture: GET /admin/einrichtung (Cedar
            // `read-config`) and GET /api/v1/policy (Cedar `read-policy`) ──────
            .merge(mako_service::einrichtung::router(Arc::clone(
                &ctx.einrichtung,
            )))
            .merge(api)
            .merge(mako_service::openapi::serve(doc))
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.tenant.clone(),
            )))
            .layer(Extension(oidc))
            .layer(Extension(cedar))
            .layer(Extension(Arc::new(regime_quelle)))
            .layer(Extension(Arc::new(
                crate::endkunden_angebot::MarktdQuelle::new(
                    cfg.marktd_url.as_deref(),
                    cfg.marktd_api_key.clone(),
                    ctx.http.clone(),
                ),
            )))
            .layer(Extension(Arc::clone(&cfg)))
            .layer(Extension(pool.clone()));

        info!("productd starting");

        // ── Background: the contract request an accepted Angebot owes ────────
        if let Some(url) = cfg.erp_webhook_url.clone() {
            let secret = cfg.erp_hmac_secret.clone().map(secrecy::SecretString::from);
            tokio::spawn(angebot_worker(pool.clone(), url, secret).run(ctx.shutdown.clone()));
        }

        // ── Background: auto-expire stale Angebote ───────────────────────────
        // Runs daily; marks ANGELEGT/VERSANDT Angebote past gueltig_bis as
        // ABGELAUFEN. Without this, expired quotations accumulate in the
        // VERSANDT state and sales staff waste time on dead leads.
        {
            let pool_bg = pool.clone();
            let tenant = cfg.tenant.clone();
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
                    match crate::pg::expire_stale_angebote(&pool_bg, &tenant).await {
                        Ok(n) if n > 0 => {
                            tracing::info!(expired = n, "productd: auto-expired stale Angebote")
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "productd: expire_stale_angebote failed")
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

/// The worker that delivers the contract request of an accepted Angebot —
/// `de.mako.tarif.angebot.angenommen`, enqueued for [`crate::pg::ANGEBOT_TARGET`] —
/// to the ERP webhook.
///
/// `vertragd` answers it with the Rahmenvertrag it created; the worker links
/// that onto the Angebot in the transaction that marks the delivery done, so
/// the Angebot names its contract exactly when the contract exists. It is a
/// delivery to one receiver whose answer is written back, which a fan-out to
/// subscribers does not carry, so it is not one of the subscription
/// deliveries.
#[must_use]
pub fn angebot_worker(
    pool: sqlx::PgPool,
    webhook_url: impl Into<String>,
    secret: Option<secrecy::SecretString>,
) -> mako_service::outbox::OutboxWorker {
    mako_service::outbox::OutboxWorker::for_target(
        pool,
        crate::pg::ANGEBOT_TARGET,
        webhook_url,
        secret,
    )
    .on_delivered(Arc::new(|conn, delivered| {
        Box::pin(async move {
            if delivered.ce_type != mako_events::tarif::ANGEBOT_ANGENOMMEN {
                return Ok(());
            }
            let angebot = delivered
                .subject
                .as_deref()
                .and_then(|s| s.parse::<uuid::Uuid>().ok());
            let rahmenvertrag = delivered
                .reply
                .as_ref()
                .and_then(|r| r.get("rahmenvertrag_id"))
                .and_then(serde_json::Value::as_str)
                .and_then(|s| s.parse::<uuid::Uuid>().ok());
            let (Some(angebot), Some(rahmenvertrag)) = (angebot, rahmenvertrag) else {
                // A replay answer names no contract: the first delivery
                // already linked it.
                return Ok(());
            };
            crate::pg::link_angebot_rahmenvertrag(conn, angebot, rahmenvertrag).await
        })
    }))
}

/// Every `/api/v1` route productd serves, each with its document entry, and
/// the subscription routes of `subs`.
pub fn api(subs: &Subscriptions) -> OpenApiRouter {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        // ── Product CRUD and the § 42 EnWG Energiemix ─────────────────────────
        .routes(routes!(
            handlers::put_product,
            handlers::get_product,
            handlers::delete_product
        ))
        .routes(routes!(handlers::get_product_history))
        .routes(routes!(
            handlers::put_energiemix,
            handlers::get_energiemix,
            handlers::delete_energiemix_handler
        ))
        .routes(routes!(handlers::list_products_handler))
        // billingd resolves one definition per leg of a period split by a
        // Tarifwechsel in one round trip, not one per leg.
        .routes(routes!(handlers::post_resolve_products))
        // ── EPEX Spot and nEHS (BEHG) prices ──────────────────────────────────
        .routes(routes!(handlers::put_epex_prices))
        .routes(routes!(handlers::get_epex_prices_quarter_hourly))
        .routes(routes!(handlers::get_epex_monthly_average))
        .routes(routes!(handlers::put_nehs_price))
        .routes(routes!(handlers::get_nehs_price_latest))
        // ── Angebot (B2B quotation) ───────────────────────────────────────────
        .routes(routes!(
            handlers::list_angebote_handler,
            handlers::post_angebot
        ))
        .routes(routes!(handlers::post_expire_angebote))
        .routes(routes!(
            handlers::get_angebot_handler,
            handlers::put_angebot
        ))
        .routes(routes!(handlers::get_angebot_comparison))
        .routes(routes!(handlers::post_angebot_versenden))
        .routes(routes!(handlers::post_angebot_annehmen))
        .routes(routes!(handlers::post_angebot_ablehnen))
        // ── Endkunden-Angebot (the frozen B2C offer an online order cites) ────
        .routes(routes!(handlers::post_endkunden_angebot))
        .routes(routes!(handlers::get_endkunden_angebot))
        // ── § 41c Abs. 5 EnWG comparison feed (public, ETag-cached) ───────────
        .routes(routes!(handlers::get_comparison_feed))
        .routes(routes!(handlers::get_comparison_feed_bo4e))
        .merge(subs.router())
}

/// The document's header; the paths come from [`api`].
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "productd — Product and Tariff Catalog",
        description = "The retail products and tariffs a Lieferant sells, the market price series they are billed against, B2B Angebote, the frozen Endkunden-Angebot, the § 41c EnWG comparison feed, and webhook subscriptions on productd's events."
    ),
    tags(
        (name = "products", description = "Products, their Tarifpreisblatt and Energiemix"),
        (name = "marktpreise", description = "EPEX day-ahead and nEHS certificate prices"),
        (name = "angebote", description = "B2B quotations"),
        (name = "endkunden-angebote", description = "Frozen B2C offers"),
        (name = "comparison-feed", description = "§ 41c Abs. 5 EnWG feed, served without a credential"),
        (name = "webhooks", description = "Webhook endpoints, deliveries and the event feed"),
    )
)]
pub struct ApiDoc;
