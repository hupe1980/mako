//! The `billingd` daemon — the [`Daemon`] impl `mako_service::run` starts.
//!
//! Pure calculation service.  Pulls product definitions from `productd`,
//! consumption from `edmd`; grid pass-through arrives with the request.
//! Outputs canonical BO4E `Rechnung` objects and emits
//! `de.billing.rechnung.erstellt` CloudEvents consumed by `accountingd`.
//!
//! ## Errors
//!
//! Every route answers failures with one envelope carrying a stable code —
//! `{"error":{"code":"PERIOD_ALREADY_BILLED","message":…,"record_id":…}}`. See
//! [`crate::error::BillingError`].
//!
//! ## Design: user-defined pricing
//!
//! All commercial rates (Arbeitspreis, Grundpreis, etc.) are defined by the
//! operator in `productd` — the engine contains zero hardcoded prices.
//! Statutory rates (Stromsteuer, Energiesteuer Gas, BEHG) are configured in
//! `billingd.toml` under `[rates]` and can be overridden per-product.
//!
//! ## Supported product categories
//!
//! | Category | Calculator | Key regulatory refs |
//! |---|---|---|
//! | `STROM` | `calculate_strom` | §41a EnWG (dynamic), §14a Modul 1/3 |
//! | `GAS` | `calculate_gas` | §25 Nr. 4 MessEV (Brennwertkorrektur), §2 EnergieStG, BEHG |
//! | `WAERME` | `calculate_waerme` | EnWG Fernwärme |
//! | `WASSER` | `calculate_wasser` | §12 Abs. 2 Nr. 1 UStG Trinkwasser 7 %, gesplittete Abwassergebühr |
//! | `SOLAR` | `calculate_solar` | §21 Abs. 3 EEG (Mieterstrom), §42b EnWG (GGV) |
//! | `EEG` | `calculate_eeg` | §19 Abs. 1 EEG (Zahlungsanspruch), §20 (Marktprämie), §21 Abs. 1 (Einspeisevergütung), §53 |
//! | `EINSPEISUNG` | `calculate_einspeisung` | Direktvermarktung, Marktwert |
//! | `WAERMEPUMPE` | `calculate_strom` + §14a | §14a EnWG Modul 1/3 |
//! | `WALLBOX` | `calculate_strom` + §14a | §14a EnWG Modul 1/3 |
//! | `HEMS` | `calculate_hems` | Platform + event billing |
//! | `EMOBILITY` | `calculate_emobility` | CPO/EMSP service billing |
//! | `ENERGIEDIENSTLEISTUNG` | `calculate_energiedienstleistung` | MSB, EMS, maintenance |
//! | `SHARING` | `EnergyShareProvider` | §42c EnWG Energiegemeinschaft credit |
//!
//! Port: `:9280`
//!
//! ## Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/billing/{malo_id}/calculate` | Calculate + persist + emit CloudEvent |
//! | `POST` | `/api/v1/billing/{malo_id}/preview` | Dry-run (no persist) |
//! | `POST` | `/api/v1/billing/{id}/correction` | Stornorechnung; releases the period for re-billing |
//! | `GET` | `/api/v1/billing/{id}/ubl` | PEPPOL BIS Billing 3.0 UBL 2.1 XML (EN 16931) |
//! | `POST` | `/api/v1/billing/sammelrechnung/{rv_id}` | B2B consolidated Sammelrechnung |
//! | `POST` | `/api/v1/billing/ggv/{ggv_id}` | §42b EnWG GGV multi-tenant community solar billing |
//! | `POST` | `/api/v1/billing/vpp/{vpp_id}` | §41e VPP dispatch settlement (Gutschrift) |
//! | `POST` | `/api/v1/billing/{id}/submit-b2g` | XRechnung B2G submission (§4a EGovG i.V.m. ERechV) |
//! | `GET` | `/api/v1/billing` | List records (`?malo_id=&lf_mp_id=&outcome=`) |
//! | `GET` | `/api/v1/billing/{id}` | Fetch single record |
//! | `GET` | `/api/v1/billing/{id}/xrechnung` | ZUGFeRD 2.3 / XRechnung 3.0 CII XML |
//! | `GET` | `/api/v1/billing/{id}/pdf` | ZUGFeRD PDF/A-3: the page with the CII XML embedded |
//! | `POST` | `/api/v1/billing/{id}/versenden` | Issue it to the customer — recorded in `outputd` for 8 years and queued for delivery |
//! | `POST` | `/api/v1/billing/{malo_id}/tarifwechsel` | Combined invoice across a mid-period price change |
//! | `GET` | `/api/v1/billing/review-queue` | Analyst work list (REVIEW + HELD) |
//! | `POST` | `/api/v1/billing/{id}/release` | Release a HELD record for dispatch |
//! | `POST` | `/api/v1/billing/{id}/reviewed` | Record the review of a REVIEW record (sends nothing) |
//! | `POST` | `/api/v1/webhooks/vpp-dispatch` | `de.vpp.dispatch.confirmed` auto-settlement |
//! | `GET` | `/health/live` | Liveness |
//! | `GET` | `/health/ready` | Readiness |

use crate::{billing_runs, clients, config, mcp_server};
use anyhow::Context as _;
use axum::{Extension, Router};
use mako_markt::marktd_client::MarktdClient;
use mako_service::{Daemon, ServiceContext};
use secrecy::SecretString;
use std::sync::Arc;

/// The `billingd` daemon. `mako_service::run` owns the lifecycle (tracing, tuned
/// pool with `application_name`, real DB-ping readiness, graceful shutdown); this
/// supplies the migrations plus the domain router, its Extension layers, the
/// OIDC verifier, the MCP server, and every background worker (outbox drain +
/// §40b scheduled billing runs).
pub struct Billingd;

impl Daemon for Billingd {
    type Config = config::BillingdConfig;
    const NAME: &'static str = "billingd";

    async fn migrate(_cfg: &Self::Config, pool: &sqlx::PgPool) -> anyhow::Result<()> {
        // Run migrations (currently a single 0001_schema.sql).
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run migrations")?;

        // Transactional outbox: the business write and its CloudEvent commit in
        // one transaction; a background worker delivers the persisted events with
        // at-least-once semantics. Ensure the table exists before either path runs.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure outbox schema")?;
        // Webhook endpoints and deliveries over that outbox.
        mako_service::subscriptions::ensure_schema(pool)
            .await
            .context("ensure webhook subscription schema")?;
        Ok(())
    }

    fn validate(cfg: &config::BillingdConfig) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        let mut out = cfg.validate();
        if let Err(e) = cfg.check_auth_posture() {
            out.push(Befund::aus_fehler("oidc", e).bereich(Bereich::Zugriff));
        }
        out.extend(cfg.subscriptions.befunde());
        out
    }

    fn policy(cfg: &config::BillingdConfig) -> mako_service::policy::PolicyReport {
        cfg.policy_report()
    }

    async fn build(
        cfg: Arc<config::BillingdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "allow_insecure_no_auth is set — HTTP API authentication is degraded (dev mode)"
            );
        }

        let pool = ctx.pool().clone();

        // ── Cedar ABAC ────────────────────────────────────────────────────────────
        // Authentication says *who* is calling; this says what they may do.
        // billingd enabled the `cedar` feature and enforced nothing, so any
        // authenticated caller could Storno an invoice the customer had already
        // received or release an invoice the risk gate was holding back — the
        // gate's whole purpose. `einsd`, which likewise issues documents that
        // create payment obligations, has had a policy from the start.
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/billingd.cedar"
            ))
            .context("billingd.cedar must parse at startup")?,
        );

        let productd = Arc::new(clients::ProductdClient::new(
            &cfg.productd_url,
            cfg.productd_api_key.clone(),
        ));
        let edmd = Arc::new(clients::EdmdClient::new(
            &cfg.edmd_url,
            cfg.edmd_api_key.clone(),
        ));
        let marktd = Arc::new(MarktdClient::new(
            &cfg.marktd_url,
            SecretString::from(cfg.marktd_api_key.clone().unwrap_or_default()),
            ctx.http.clone(),
        ));
        let vertragd = Arc::new(clients::VertragdClient::new(
            cfg.vertragd_url
                .as_deref()
                .unwrap_or("http://localhost:9780"),
            cfg.vertragd_api_key.clone(),
        ));
        let outputd = Arc::new(clients::OutputdClient::new(
            cfg.outputd_url
                .as_deref()
                .unwrap_or("http://localhost:9880"),
            cfg.outputd_api_key.clone(),
        ));
        // Deliberately not defaulted to a localhost URL. Every other client
        // here answers a question billingd can also be told the answer to in
        // the request; this one supplies the § 40 Abs. 1 EnWG advance
        // itemisation, and an unreachable default would let the §40b sweep
        // treat "accountingd is not deployed" and "this customer paid no
        // advances" as the same fact.
        let accountingd = cfg.accountingd_url.as_deref().map(|url| {
            Arc::new(clients::AccountingdClient::new(
                url,
                cfg.accountingd_api_key.clone(),
            ))
        });

        // One bundle instead of six extensions. Every handler needs some subset
        // of these, and threading them individually is what pushed nearly every
        // function in the service past clippy's argument limit.
        let deps = Arc::new(clients::BillingDeps {
            cfg: Arc::clone(&cfg),
            productd,
            edmd,
            marktd,
            vertragd,
            outputd,
            accountingd,
        });

        // ── Webhook subscriptions over the outbox ────────────────────────────────
        // The configured ERP URL is the endpoint `default`, signed with
        // `erp_hmac_secret`; integrators register further endpoints through
        // `/api/v1/webhooks/endpoints` and read `/api/v1/events`.
        let subs = subscriptions(&cfg, pool.clone());
        subs.spawn(ctx.shutdown.clone());

        // ── §40b EnWG scheduled billing runs (config-gated) ──────────────────────
        billing_runs::spawn_billing_run_worker(
            Arc::clone(&deps),
            pool.clone(),
            ctx.shutdown.clone(),
        );

        // ── OIDC/JWT authentication ───────────────────────────────────────────────
        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.tenant,
            ctx.shutdown.clone(),
        )
        .await
        .context("OIDC setup")?;

        let (api, doc) = crate::openapi::api(&subs).split_for_parts();
        let app = Router::new()
            // Readiness and operator policy: GET /admin/einrichtung (Cedar
            // `read-config`) and GET /api/v1/policy (Cedar `read-policy`), read-only.
            .merge(mako_service::einrichtung::router(Arc::clone(
                &ctx.einrichtung,
            )))
            .merge(api)
            .merge(mako_service::openapi::serve(doc))
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.tenant.clone(),
            )))
            .layer(Extension(oidc.clone()))
            .layer(Extension(Arc::clone(&cedar)))
            .layer(Extension(Arc::clone(&deps)))
            .layer(Extension(pool.clone()));

        // ── MCP server ────────────────────────────────────────────────────────────
        let mcp_state = std::sync::Arc::new(mcp_server::BillingdMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            // The same verifier and the same policy as the REST surface: a
            // JWT is verified and checked for `use-mcp`. Built with
            // `from_auth_config` instead, `/mcp` runs in dev mode whenever
            // `[mcp]` carries no key — serving the tenant's billing records to
            // any caller.
            auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                &cfg.mcp,
                oidc.clone(),
                Some(Arc::clone(&cedar)),
                &cfg.tenant,
            ),
            deps: Arc::clone(&deps),
            policy: Arc::new(cfg.policy_report()),
            subs,
        });
        Ok(app.merge(mcp_server::router(mcp_state, ctx.shutdown.clone())))
    }
}

/// The webhook module over billingd's outbox: the configured ERP URL is the
/// endpoint `default`, signed with `erp_hmac_secret`.
#[must_use]
pub fn subscriptions(
    cfg: &config::BillingdConfig,
    pool: sqlx::PgPool,
) -> mako_service::subscriptions::Subscriptions {
    mako_service::subscriptions::Subscriptions::new(
        pool,
        Billingd::NAME,
        &cfg.tenant,
        cfg.subscriptions.clone(),
    )
    .with_default_endpoint(
        cfg.erp_webhook_url.clone(),
        cfg.erp_hmac_secret.clone().map(SecretString::from),
    )
}
