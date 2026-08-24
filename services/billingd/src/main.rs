//! `billingd` — Energy Billing Engine.
//!
//! Pure calculation service.  Pulls product definitions from `productd`,
//! consumption from `edmd`, and grid pass-through from `marktd`.
//! Outputs canonical BO4E `Rechnung` objects and emits
//! `de.billing.rechnung.erstellt` CloudEvents consumed by `accountingd`.
//!
//! ## Errors
//!
//! Every route answers failures with one envelope carrying a stable code —
//! `{"error":{"code":"PERIOD_ALREADY_BILLED","message":…,"record_id":…}}`. See
//! [`billingd::error::BillingError`].
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
//! | `POST` | `/api/v1/webhooks/vpp-dispatch` | `de.vpp.dispatch.confirmed` auto-settlement |
//! | `GET` | `/health/live` | Liveness |
//! | `GET` | `/health/ready` | Readiness |

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use billingd::{billing_runs, clients, config, handlers, mcp_server};
use mako_markt::marktd_client::MarktdClient;
use mako_service::{Daemon, ServiceContext};
use secrecy::SecretString;
use std::sync::Arc;

/// The `billingd` daemon. `mako_service::run` owns the lifecycle (tracing, tuned
/// pool with `application_name`, real DB-ping readiness, graceful shutdown); this
/// supplies the migrations plus the domain router, its Extension layers, the
/// OIDC verifier, the MCP server, and every background worker (outbox drain +
/// §40b scheduled billing runs).
struct Billingd;

impl Daemon for Billingd {
    type Config = config::BillingdConfig;
    const NAME: &'static str = "billingd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
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
        Ok(())
    }

    async fn build(
        cfg: Arc<config::BillingdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        // Fail closed: without `[oidc]` every billing endpoint (calculate,
        // correction, VPP contract mutation) accepts any caller. That posture
        // must be requested by name via `allow_insecure_no_auth`.
        if !cfg.allow_insecure_no_auth && cfg.oidc.is_none() {
            anyhow::bail!(
                "refusing to start without [oidc]: the billing API would accept \
                 unauthenticated calculate/correction/mutation requests. \
                 Configure [oidc] or set allow_insecure_no_auth = true (dev only)."
            );
        }
        // The VPP auto-billing webhook mutates billing state on inbound events;
        // running it without HMAC verification is only allowed by name.
        if !cfg.allow_insecure_no_auth
            && cfg.vpp_auto_billing
            && cfg.inbound_webhook_secret.is_none()
        {
            anyhow::bail!(
                "refusing to start: vpp_auto_billing is enabled but inbound_webhook_secret \
                 is not set — unsigned webhooks could trigger billing. Configure \
                 inbound_webhook_secret or set allow_insecure_no_auth = true (dev only)."
            );
        }
        cfg.validate()?;
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

        // ── Outbox drain worker (config-gated on the ERP webhook) ────────────────
        // Only runs when there is somewhere to deliver to; signs with the same
        // `erp_hmac_secret` the enqueued events are meant to be signed with.
        if let Some(ref url) = cfg.erp_webhook_url {
            tokio::spawn(
                mako_service::outbox::OutboxWorker::new(
                    pool.clone(),
                    url.clone(),
                    cfg.erp_hmac_secret.clone().map(Into::into),
                )
                .run(ctx.shutdown.clone()),
            );
        }

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

        let app = Router::new()
            .route(
                "/api/v1/billing/{malo_id}/calculate",
                post(handlers::post_calculate),
            )
            .route("/api/v1/billing", get(handlers::list_records))
            // Risk gate: analyst work list + release of HELD records
            .route(
                "/api/v1/billing/review-queue",
                get(handlers::get_review_queue),
            )
            .route("/api/v1/billing/{id}/release", post(handlers::post_release))
            .route("/api/v1/billing/{id}", get(handlers::get_record))
            .route(
                "/api/v1/billing/{id}/xrechnung",
                get(handlers::get_xrechnung),
            )
            // The ZUGFeRD document: the page and the XML in one file, both
            // from the record's stored EN 16931 model. billingd renders and
            // proves the payload; outputd renders the page and the PDF/A-3
            // carrier with the operator's template (templates live there).
            .route("/api/v1/billing/{id}/pdf", get(handlers::get_invoice_pdf))
            // Issue the document to the customer: render, record for 8 years,
            // and queue it on their channels. `GET /pdf` shows an invoice;
            // this one sends it.
            .route(
                "/api/v1/billing/{id}/versenden",
                post(handlers::post_invoice_versenden),
            )
            .route(
                "/api/v1/billing/{malo_id}/preview",
                post(handlers::post_preview),
            )
            // L8: Korrekturrechnung / Stornorechnung (§ 147 AO / GoBD audit trail)
            .route(
                "/api/v1/billing/{id}/correction",
                post(handlers::post_correction),
            )
            // Tarifwechsel: combined invoice for price change mid-period (§41 EnWG)
            .route(
                "/api/v1/billing/{malo_id}/tarifwechsel",
                post(handlers::post_tarifwechsel),
            )
            // L2: B2B Sammelrechnung for Rahmenvertrag with rechnungsstellung=SAMMEL
            .route(
                "/api/v1/billing/sammelrechnung/{rahmenvertrag_id}",
                post(handlers::post_sammelrechnung),
            )
            // B1: §42b EnWG GGV community solar multi-tenant proportional billing
            .route(
                "/api/v1/billing/ggv/{ggv_id}",
                post(handlers::post_ggv_billing),
            )
            // §41e VPP dispatch settlement (Gutschrift) — de.vpp.settlement.berechnet
            .route(
                "/api/v1/billing/vpp/{vpp_id}",
                post(handlers::post_vpp_billing),
            )
            // §41e auto-settlement webhook (de.vpp.dispatch.confirmed, HMAC-signed)
            .route(
                "/api/v1/webhooks/vpp-dispatch",
                post(handlers::post_vpp_webhook),
            )
            // XRechnung B2G submission (§4a EGovG i.V.m. ERechV — mandatory since 27.11.2020)
            .route(
                "/api/v1/billing/{id}/submit-b2g",
                post(handlers::post_submit_b2g),
            )
            // PEPPOL BIS Billing 3.0 UBL 2.1 — EN 16931's other permitted syntax
            .route(
                "/api/v1/billing/{id}/ubl",
                axum::routing::get(handlers::get_ubl),
            )
            .layer(Extension(oidc))
            .layer(Extension(cedar))
            .layer(Extension(Arc::clone(&deps)))
            .layer(Extension(pool.clone()));

        // ── MCP server ────────────────────────────────────────────────────────────
        let mcp_state = std::sync::Arc::new(mcp_server::BillingdMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
            deps: Arc::clone(&deps),
        });
        Ok(app.merge(mcp_server::router(mcp_state, ctx.shutdown.clone())))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Billingd>().await
}
