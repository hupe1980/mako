//! Axum router and startup logic for `edmd`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

// Quality scoring and Gas conversion are provided by the `metering` crate.
// The inline `compute_quality` has been replaced with a call to `metering::score_intervals`.
// Tests for the Hampel filter logic live in crates/metering/src/quality.rs.

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::{Claims, OidcVerifier};
use rubo4e::current::{
    Energiemenge, Lastgang, Medium, Menge, Mengeneinheit, Messart, Messwertstatus,
    Sparte as Bo4eSparte, Zeitraum, Zeitreihe, Zeitreihenwert,
};
use rubo4e::identifiers::ObisCode;
use rust_decimal::Decimal;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::{
    handler::{HandlerState, handle_webhook},
    iceberg::query::OlapEngine,
    pg::{PgTimeSeriesRepository, PgTyp2Repository},
    smgw::{
        get_smgw_compliance, get_smgw_session, list_smgw_sessions, post_smgw_compliance_scan,
        put_smgw_session,
    },
};
use mako_edm::{
    domain::{
        BillingPeriodQuery, IngestionSource, MeterRead, QualityFlag, Sparte as EdmSparte,
        TimeSeriesQuery,
    },
    repository::{TimeSeriesRepository, Typ2Repository},
};

mod archive;
mod billing;
mod confirmations;
mod convert;
mod forecast;
mod gas_quality;
mod gdpr;
mod ingest;
mod iot;
mod jahresablesung;
mod lastgang;
mod quality;
mod reading_orders;
mod sharing;
mod substitute;
mod virtual_meter;

// Path-preserving re-exports: everything that used to live directly in
// `server.rs` stays reachable under `crate::server::…` / `edmd::server::…`.
pub(crate) use archive::*;
pub(crate) use billing::*;
pub(crate) use confirmations::*;
pub(crate) use convert::*;
pub(crate) use forecast::*;
pub(crate) use gas_quality::*;
pub(crate) use gdpr::*;
pub use ingest::*;
pub(crate) use iot::*;
pub use jahresablesung::*;
pub(crate) use lastgang::*;
pub use quality::*;
pub(crate) use reading_orders::*;
pub(crate) use sharing::*;
pub use substitute::*;
pub(crate) use virtual_meter::*;

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: HandlerState) -> Router {
    Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/api/v1/deliveries/{malo_id}", get(get_deliveries))
        .route(
            "/api/v1/imbalance/{malo_id}/{year}/{month}",
            get(get_imbalance),
        )
        .route("/api/v1/billing-period/{malo_id}", get(get_billing_period))
        // Collection endpoint for mabis-syncd MaLo discovery.
        // mabis-syncd calls: GET /api/v1/billing-periods?from=YYYY-MM-DD&to=YYYY-MM-DD&tenant=...
        .route("/api/v1/billing-periods", get(list_billing_periods))
        .route("/api/v1/lastgang/{malo_id}", get(get_lastgang))
        .route("/api/v1/zeitreihe/{malo_id}", get(get_zeitreihe))
        // ESA "Werte nach Typ 2" — a deliberately separate read path from the
        // billing endpoints above. It reads `esa_typ2_reads` only, never
        // `meter_reads`, so Typ-2 data is unreachable from any billing query.
        .route("/api/v1/esa/typ2/{malo_id}", get(get_esa_typ2))
        // ── Ablesesteuerung ───────────────────────────────────────────────────
        .route(
            "/api/v1/reading-orders",
            post(create_reading_order).get(list_reading_orders),
        )
        .route("/api/v1/reading-orders/{id}", get(get_reading_order))
        .route(
            "/api/v1/reading-orders/{id}/complete",
            put(complete_reading_order),
        )
        .route(
            "/api/v1/reading-orders/{id}/cancel",
            put(cancel_reading_order),
        )
        .route("/api/v1/reading-orders/{id}/fail", put(fail_reading_order))
        // N7: Jahresablesung campaign scheduler (§40 Abs. 2 EnWG)
        .route(
            "/api/v1/reading-orders/campaign",
            post(jahresablesung_campaign),
        )
        .route(
            "/api/v1/compliance/jahresablesung/{year}",
            get(jahresablesung_compliance),
        )
        .route(
            "/api/v1/gdpr/erasure/{malo_id}/archive-plan",
            post(plan_gdpr_archive_erasure),
        )
        .route(
            "/api/v1/gdpr/erasure/{malo_id}/archive-complete",
            post(complete_gdpr_archive_erasure),
        )
        // iMSys / SMGW direct push — bypasses EDIFACT for RLM/iMSys customers.
        // §41a EnWG dynamic tariffs require sub-hourly resolution;
        // MSCONS round-trip adds 15–60 min latency.
        .route(
            "/api/v1/meter-reads/rlm/{malo_id}",
            post(post_direct_reads_rlm),
        )
        .route(
            "/api/v1/meter-reads/gas/{malo_id}",
            post(post_direct_reads_gas),
        )
        // retroactive quality scoring for existing meter_reads (MSCONS or direct push).
        .route(
            "/api/v1/quality-score/{malo_id}",
            post(post_quality_rescore),
        )
        // § 60 Abs. 6 MsbG bitemporal corrections: audit-trail preserving retroactive corrections.
        .route("/api/v1/corrections/{malo_id}", post(post_corrections))
        // Bulk ingestion: batched direct-push reads (performance path for large MSCONS deliveries)
        .route("/api/v1/meter-reads/{malo_id}/bulk", post(post_bulk_reads))
        // § 60 Abs. 2 MsbG auto-substitute: fill gaps using prior-period average method
        .route(
            "/api/v1/meter-reads/{malo_id}/substitute",
            post(post_substitute_values),
        )
        // resampled Lastgang — down-sample to hourly / daily / monthly buckets
        .route(
            "/api/v1/lastgang/{malo_id}/resampled",
            get(get_lastgang_resampled),
        )
        // Virtual meter — compute derived time series from AggregationRule
        .route(
            "/api/v1/virtual/{virtual_malo_id}/lastgang",
            get(get_virtual_lastgang),
        )
        .route(
            "/api/v1/virtual",
            get(list_virtual_meters).post(create_virtual_meter),
        )
        .route(
            "/api/v1/virtual/{virtual_malo_id}",
            get(get_virtual_meter).delete(delete_virtual_meter),
        )
        // Quality assessments — per-batch quality history
        .route(
            "/api/v1/quality-assessments/{malo_id}",
            get(list_quality_assessments),
        )
        // Annual forecast (§ 60 Abs. 2 MsbG Jahresprognose)
        .route("/api/v1/forecast/{malo_id}", get(get_annual_forecast))
        // Summenzeitreihe — MABIS-ready monthly aggregated series
        .route(
            "/api/v1/summenzeitreihe/{malo_id}",
            get(get_summenzeitreihe),
        )
        // Gas quality data (PID 13007 Gasbeschaffenheitsdaten)
        .route("/api/v1/gas-quality/{malo_id}", get(get_gas_quality))
        // §22 EnWG Verlustenergie — indicative grid-loss balance
        .route("/api/v1/netzverlust", get(get_netzverlust))
        // § 60 Abs. 2 MsbG — estimated-reading confirmation obligations
        .route("/api/v1/confirmations", get(list_confirmations))
        // Iceberg/S3 archive endpoints
        .route("/api/v1/archive/status", get(get_archive_status))
        .route("/api/v1/archive/olap/{malo_id}", get(get_archive_olap))
        .route("/api/v1/archive/portfolio", get(get_archive_portfolio))
        .route(
            "/api/v1/archive/timeseries/{malo_id}",
            get(get_archive_timeseries),
        )
        // §42c Energy Sharing VZW quarter-hour allocation
        .route("/api/v1/sharing/readiness", get(get_sharing_readiness))
        .route("/api/v1/meter-reads/iot/{malo_id}", post(post_iot_reads))
        .route(
            "/api/v1/sharing/{community_id}/allocation",
            get(get_sharing_allocation),
        )
        // GDPR §17 DSGVO right to erasure — mark a MaLo for deletion from
        // hot PostgreSQL storage. Cold Iceberg deletion is scheduled asynchronously.
        .route(
            "/api/v1/gdpr/erasure/{malo_id}",
            axum::routing::delete(post_gdpr_erasure),
        )
        // P2: Iceberg REST catalog — enables DuckDB/Snowflake/Databricks to query
        // the cold Iceberg archive directly without going through edmd REST.
        // DuckDB: ATTACH 'rest+http://edmd:8380' AS mako (TYPE ICEBERG);
        // Spec: Apache Iceberg REST Catalog specification (ICEBERG-89).
        .route("/api/v1/iceberg/v1/config", get(iceberg_rest_config))
        .route(
            "/api/v1/iceberg/v1/namespaces",
            get(iceberg_list_namespaces),
        )
        .route(
            "/api/v1/iceberg/v1/namespaces/{namespace}/tables",
            get(iceberg_list_tables),
        )
        .route(
            "/api/v1/iceberg/v1/namespaces/{namespace}/tables/{table}",
            get(iceberg_load_table),
        )
        // P2: DataFusion SQL endpoint — runs analytical SQL over both hot
        // (PostgreSQL via custom UDF) and cold (Iceberg/Parquet via DataFusion)
        // tier. Returns results as Arrow IPC or JSON.
        .route("/api/v1/query/sql", post(post_sql_query))
        // ── §14a SMGW session registry (MsbG §21c / BSI TR-03109) ────────────
        // `compliance` is a static segment and takes priority over {malo_id} in Axum 0.8.
        .route("/api/v1/smgw", get(list_smgw_sessions))
        .route("/api/v1/smgw/compliance", get(get_smgw_compliance))
        .route(
            "/api/v1/smgw/compliance/scan",
            axum::routing::post(post_smgw_compliance_scan),
        )
        .route(
            "/api/v1/smgw/{malo_id}",
            get(get_smgw_session).put(put_smgw_session),
        )
        .route("/metrics", get(metrics))
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(health_ready))
        .with_state(state)
}

// ── REST handlers ─────────────────────────────────────────────────────────────

/// `GET /health/ready` — confirms the database connection is alive.
/// Returns 503 when the pool cannot reach PostgreSQL.
async fn health_ready(State(state): State<HandlerState>) -> impl IntoResponse {
    match sqlx::query("SELECT 1").execute(state.repo.pool()).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::warn!(error = %e, "edmd: readiness probe: DB unreachable");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// `GET /metrics` — Prometheus-compatible operational metrics.
/// No authentication required; restrict network access at the ingress layer.
async fn metrics(State(state): State<HandlerState>) -> impl IntoResponse {
    let mut out = String::with_capacity(512);
    let pool = state.repo.pool();

    let meter_reads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meter_reads")
        .fetch_one(state.repo.pool())
        .await
        .unwrap_or(0);
    let billing_periods: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meter_billing_periods")
        .fetch_one(state.repo.pool())
        .await
        .unwrap_or(0);
    let pool_size = pool.size();
    let pool_idle = pool.num_idle();

    out.push_str("# HELP edmd_meter_reads_total Total meter read entries stored.\n");
    out.push_str("# TYPE edmd_meter_reads_total gauge\n");
    out.push_str(&format!("edmd_meter_reads_total {meter_reads}\n"));
    out.push_str("# HELP edmd_billing_periods_total Pre-aggregated MeterBillingPeriod records.\n");
    out.push_str("# TYPE edmd_billing_periods_total gauge\n");
    out.push_str(&format!("edmd_billing_periods_total {billing_periods}\n"));
    out.push_str("# HELP edmd_db_pool_size Current PostgreSQL connection pool size.\n");
    out.push_str("# TYPE edmd_db_pool_size gauge\n");
    out.push_str(&format!("edmd_db_pool_size {pool_size}\n"));
    out.push_str("# HELP edmd_db_pool_idle Idle PostgreSQL connections.\n");
    out.push_str("# TYPE edmd_db_pool_idle gauge\n");
    out.push_str(&format!("edmd_db_pool_idle {pool_idle}\n"));

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
}

/// Query params shared by virtual meter and other new endpoints.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct SimpleTimeParams {
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
}

// ── RunConfig + startup ───────────────────────────────────────────────────────

pub struct RunConfig {
    pub listen: SocketAddr,
    pub database_url: SecretString,
    pub marktd_url: String,
    pub marktd_api_key: secrecy::SecretString,
    pub subscriber_id: String,
    pub webhook_url: String,
    pub webhook_secret: Option<SecretString>,
    pub inbound_secret: Option<SecretString>,
    pub db_pool_size: u32,
    /// Tenant identifier — used as Cedar resource_tenant.
    pub tenant: String,
    /// OIDC verifier.  Use [`OidcVerifier::disabled`] in dev/test.
    pub oidc: OidcVerifier,
    /// Cedar ABAC enforcer.
    pub cedar: Arc<CedarEnforcer>,
    /// MCP server auth config (API-key fallback + optional per-named-key identity).
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// Graceful-shutdown token.
    pub shutdown: CancellationToken,
    /// Resolved archive config (env vars already substituted, disabled when absent).
    pub archive: Option<crate::config::ArchiveConfig>,
    /// ERP webhook URL for outbound CloudEvents (direct push + quality warnings).
    pub erp_webhook_url: Option<String>,
    /// Request rate limits. Ingest endpoints accept unbounded batches, so an
    /// unthrottled client can saturate the write path for every other tenant.
    pub rate_limit: mako_service::RateLimitConfig,
    /// Kafka ingest consumer (None or `enabled = false` → not started).
    pub kafka_ingest: Option<crate::config::KafkaIngestConfig>,
    /// § 60 Abs. 2 MsbG confirmation loop (overdue escalation worker).
    pub confirmation: crate::config::ConfirmationConfig,
}

/// Connect to the database, run migrations, register subscription, and serve.
pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    let pool = PgPool::connect_with(
        cfg.database_url
            .expose_secret()
            .parse::<sqlx::postgres::PgConnectOptions>()?,
    )
    .await?;

    // Run database migrations at startup.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("run edmd migrations: {e}"))?;

    // ── Iceberg/S3 archive setup ───────────────────────────────────────────────
    let olap_engine: Option<Arc<OlapEngine>> = if let Some(ref archive_cfg) = cfg.archive {
        if archive_cfg.enabled && !archive_cfg.storage_uri.is_empty() {
            // Build FileIO (iceberg's opendal-backed storage abstraction).
            match crate::iceberg::build_file_io(archive_cfg) {
                Ok(file_io) => {
                    // Spawn archival worker: loads/creates the table via SqlCatalog,
                    // writes Parquet batches to S3, marks rows archived in PostgreSQL.
                    let worker = crate::iceberg::worker::ArchiveWorker::new(
                        pool.clone(),
                        archive_cfg.clone(),
                        file_io,
                        cfg.database_url.expose_secret().to_owned(),
                    );
                    worker.spawn(cfg.shutdown.clone());

                    // Build OLAP engine: loads the table from the SQL catalog and
                    // registers it with DataFusion as an IcebergTableProvider.
                    match crate::iceberg::worker::load_table_for_olap(
                        archive_cfg,
                        cfg.database_url.expose_secret(),
                        pool.clone(),
                        cfg.tenant.clone(),
                    )
                    .await
                    {
                        Ok(engine) => {
                            tracing::info!(
                                storage_uri = %archive_cfg.storage_uri,
                                catalog_schema = %archive_cfg.iceberg_catalog_schema,
                                "edmd: Iceberg OLAP engine ready"
                            );
                            Some(Arc::new(engine))
                        }
                        Err(e) => {
                            // Table may not exist on first run — that's fine.
                            // The worker will create it on next archive cycle.
                            tracing::info!(
                                error = %e,
                                "edmd: Iceberg OLAP engine not yet available \
                                 (table will be created on first archive run)"
                            );
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "edmd: cannot build FileIO — archive disabled");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let mcp_state = Arc::new(crate::mcp_server::EdmdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        marktd_url: cfg.marktd_url.clone(),
        marktd_api_key: cfg.marktd_api_key.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            cfg.oidc.clone(),
            Some(cfg.cedar.clone()),
            &cfg.tenant,
        ),
    });

    let repo = PgTimeSeriesRepository::new(pool.clone());
    let typ2_repo = PgTyp2Repository::new(pool.clone());
    // Clone the webhook URL and tenant before they are moved into HandlerState.
    let smgw_webhook_url = cfg.erp_webhook_url.clone();
    let smgw_tenant = cfg.tenant.clone();
    let state = HandlerState {
        repo,
        typ2_repo,
        inbound_secret: Arc::new(cfg.inbound_secret),
        tenant: cfg.tenant,
        marktd_url: cfg.marktd_url.clone(),
        marktd_api_key: cfg.marktd_api_key.clone(),
        olap_engine,
        erp_webhook_url: cfg.erp_webhook_url,
    };
    // ── Kafka ingest consumer (optional) ─────────────────────────────────────
    // High-throughput intake for head-end systems that stream reading batches
    // instead of pushing per-gateway HTTP. Same validation, same store, same
    // audit trail as the REST paths.
    if let Some(kafka_cfg) = cfg.kafka_ingest.as_ref().filter(|k| k.enabled) {
        crate::kafka_ingest::spawn(
            kafka_cfg.clone(),
            state.repo.clone(),
            state.tenant.clone(),
            cfg.shutdown.clone(),
        );
    }

    {
        use mako_markt::marktd_client::{MarktdClient, SubscriptionRequest};
        use mako_service::http::default_client;
        let marktd = MarktdClient::new(
            &cfg.marktd_url,
            cfg.marktd_api_key.clone(),
            default_client(),
        );
        marktd
            .put_subscription(
                &cfg.subscriber_id,
                &SubscriptionRequest {
                    webhook_url: &cfg.webhook_url,
                    webhook_secret: cfg.webhook_secret.as_ref().map(|s| {
                        use secrecy::ExposeSecret;
                        let secret: &str = s.expose_secret();
                        secret
                    }),
                    // Receive MSCONS completions for meter data storage
                    // + INSRPT initiations for reading-order auto-creation
                    // + Lieferbeginn/Lieferende completions for supply handover readings
                    event_types: &[
                        mako_events::mako::PROCESS_COMPLETED,
                        mako_events::mako::PROCESS_INITIATED,
                    ],
                    makopid_filter: mako_edm::domain::MSCONS_PIDS,
                    active: true,
                },
            )
            .await;
    }

    let pool_arc = Arc::new(pool);

    // Both limiters apply: the keyed one bounds any single caller, the global
    // one bounds their sum.
    let app = mako_service::ServiceBuilder::new()
        .merge(
            router(state)
                .layer(Extension(cfg.cedar))
                .layer(Extension(cfg.oidc))
                .layer(Extension(pool_arc.clone()))
                .merge(crate::mcp_server::router(mcp_state, cfg.shutdown.clone())),
        )
        .with_tenant_rate_limit(cfg.rate_limit.clone())
        .with_rate_limit(cfg.rate_limit)
        .build();

    let listener = TcpListener::bind(cfg.listen).await?;

    tracing::info!(
        listen = %cfg.listen,
        marktd_url = %cfg.marktd_url,
        "edmd: listening"
    );

    // §14a Fernsteuerbarkeit compliance background worker (MsbG §21c, BSI TR-03109-4 §6.3).
    // Daily sweep of all SmgwSessions: checks TLS cert validity, CLS channel §14a
    // Konfigurationsprodukt, and communication faults.
    // Emits `de.messwert.cls.compliance_issue` CloudEvents for every detected issue.
    {
        use crate::smgw::spawn_cls_compliance_worker;
        spawn_cls_compliance_worker(
            pool_arc.clone(),
            smgw_tenant.clone(),
            smgw_webhook_url.clone(),
            30,     // cert_warning_days — warn 30 days before expiry (BSI TR-03109-4 §6.3)
            2,      // comm_fault_threshold_hours — § 60 Abs. 2 MsbG: substitute after 2h silence
            86_400, // interval_secs — sweep daily
            cfg.shutdown.clone(),
        );
    }

    // SMGW certificate-expiry alerting (BSI TR-03109-4 §6.3). Daily sweep of every
    // certificate in `smgw_sessions`, emitting `de.messwert.smgw.cert.expiry_warning`
    // at 90 / 30 / 7 days before `valid_to` (SMGW_CERT_ABLAUFDATUM), once per tier per
    // certificate. An expired cert silently ends §14a Fernsteuerbarkeit.
    {
        use crate::smgw::spawn_smgw_cert_expiry_worker;
        spawn_smgw_cert_expiry_worker(
            pool_arc.clone(),
            smgw_tenant.clone(),
            smgw_webhook_url.clone(),
            86_400, // interval_secs — sweep daily
            cfg.shutdown.clone(),
        );
    }

    // § 60 Abs. 2 MsbG confirmation loop — escalates estimated/substituted
    // intervals that were never replaced by a plausibilised real value.
    if cfg.confirmation.enabled {
        crate::confirmation::spawn_confirmation_worker(
            pool_arc,
            smgw_tenant,
            smgw_webhook_url,
            cfg.confirmation.deadline_weeks,
            86_400, // daily
            cfg.shutdown.clone(),
        );
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cfg.shutdown.cancelled().await })
        .await?;
    Ok(())
}
