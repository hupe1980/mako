//! Axum router and startup logic for `edmd`.

use std::collections::BTreeMap;
use std::sync::Arc;

// Quality scoring and Gas conversion are provided by the `metering` crate.
// The inline `compute_quality` has been replaced with a call to `metering::score_intervals`.
// Tests for the Hampel filter logic live in the external `metering` crate.

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
use tokio_util::sync::CancellationToken;

use crate::domain::{
    BillingPeriodQuery, IngestionSource, MeterRead, QualityFlag, Sparte as EdmSparte,
    TimeSeriesQuery, batch_period,
    repository::{TimeSeriesRepository, Typ2Repository},
};
use crate::{
    handler::{HandlerState, handle_webhook},
    smgw::{
        get_smgw_compliance, get_smgw_session, list_smgw_sessions, post_smgw_compliance_scan,
        put_smgw_session,
    },
    store::{MeterStoreTimeSeriesRepository, MeterStoreTyp2Repository, build_stores},
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
pub(crate) mod quality_alert;
mod reading_orders;
mod sharing;
mod substitute;
mod virtual_meter;
mod zsg;

// Path-preserving re-exports: every submodule item stays reachable under
// `crate::server::…` / `edmd::server::…`.
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
pub use zsg::*;

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
        // The canonical projected series, in one direction, through
        // `domain::register`. `/lastgang` is the BO4E export and returns one
        // object per register; folding that back into one series *is* the
        // projection, so it is served made rather than left to each consumer.
        .route("/api/v1/energy/{malo_id}", get(lastgang::get_energy_series))
        // ── Zählerstandsgang (BK6-24-174 „Datenübermittlung ZSG") ─────────────
        // What an iMSys actually measures (§ 2 Satz 1 Nr. 27 MsbG). The
        // differencing into a Lastgang is the MSB's job, and edmd is the MSB.
        .route(
            "/api/v1/zaehlerstandsgang/{malo_id}",
            post(zsg::post_zaehlerstandsgang).get(zsg::get_zaehlerstandsgang),
        )
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
        // N7: Jahresablesung campaign scheduler (§ 40b Abs. 1 EnWG)
        .route(
            "/api/v1/reading-orders/campaign",
            post(jahresablesung_campaign),
        )
        .route(
            "/api/v1/compliance/jahresablesung/{year}",
            get(jahresablesung_compliance),
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
        // § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) bitemporal corrections: audit-trail preserving retroactive corrections.
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
        // Delivery surveillance — the measuring points that stopped delivering.
        // The V-rules can only judge data that arrived; this is the other half.
        .route(
            "/api/v1/surveillance/delivery",
            get(crate::surveillance::get_delivery_surveillance),
        )
        .route(
            "/api/v1/surveillance/delivery/scan",
            post(crate::surveillance::post_delivery_surveillance_scan),
        )
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
        // Analytical SQL — runs read-only SQL across both tiers in meterstore's
        // DataFusion session, returning JSON rows or an Arrow IPC stream. External
        // Iceberg clients (DuckDB/Spark/Trino) connect to meterstore's own catalog
        // facade, not an edmd-hosted one.
        .route("/api/v1/query/sql", post(post_sql_query))
        // ── §14a SMGW session registry (§ 25 MsbG / BSI TR-03109) ────────────
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
        // `/health/live`, `/health/ready` and the generic `/metrics` are the
        // runner's (`mako_service::run`), which probes the pool for real. This
        // is edmd's own domain gauge, mounted beside them.
        .route("/edmd/metrics", get(metrics))
        .with_state(state)
}

// ── REST handlers ─────────────────────────────────────────────────────────────

/// `GET /edmd/metrics` — edmd's own Prometheus gauges.
///
/// The runner mounts the generic `/metrics` (request counters) and both health
/// probes; this adds the domain numbers. No authentication — restrict network
/// access at the ingress layer.
async fn metrics(State(state): State<HandlerState>) -> impl IntoResponse {
    let mut out = String::with_capacity(512);
    let pool = state.repo.pool();

    // The readings count is meterstore's concern (the hot table is
    // `meter_reads_versions`, the resolved `meter_reads` is a DataFusion-only
    // relation) — there is no `meter_reads` table in edmd's Postgres to count, so
    // this gauge is dropped rather than left querying a non-existent relation.
    let billing_periods: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meter_billing_periods")
        .fetch_one(state.repo.pool())
        .await
        .unwrap_or(0);
    let pool_size = pool.size();
    let pool_idle = pool.num_idle();

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

// ── Read windows ──────────────────────────────────────────────────────────────

/// How far back a read reaches when the caller names no `from`.
///
/// Every time-series endpoint used to default to `OffsetDateTime::UNIX_EPOCH`,
/// so `GET /api/v1/lastgang/{malo_id}` with no parameters asked for **every
/// interval ever stored** for that MaLo across both tiers. At quarter-hour
/// resolution a decade is 350 000 rows, materialised into a `Vec<MeterRead>` and
/// then into BO4E JSON — one unparameterised request from a dashboard is a
/// tenant-wide outage. A month is the window an interactive caller almost always
/// means; anything longer is asked for explicitly.
pub(crate) const DEFAULT_READ_WINDOW: time::Duration = time::Duration::days(31);

/// The longest window a single request may ask for.
///
/// Two years covers a Jahresabrechnung with its comparison year, which is the
/// widest legitimate interactive read. Bulk history is what the Arrow IPC
/// negotiation, `POST /api/v1/query/sql` and the Iceberg REST catalog are for —
/// all three stream rather than materialise.
pub(crate) const MAX_READ_WINDOW: time::Duration = time::Duration::days(732);

/// Parse a `?from=` / `?to=` pair into a bounded UTC window.
///
/// A malformed timestamp is a `400`, not a default. Parsing a bound with
/// `.ok()` and falling back silently makes `?from=last-tuesday` return the whole
/// history, which looks like a successful answer to the question the caller
/// asked.
// A whole `axum::Response` in the `Err` arm trips `clippy::result_large_err`,
// so the refusal travels as its message and is rendered by `into_response`.
pub(crate) struct WindowRefusal(String);

impl WindowRefusal {
    /// The `400` this refusal renders as.
    pub(crate) fn into_response(self) -> axum::response::Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": self.0 })),
        )
            .into_response()
    }
}

pub(crate) fn read_window(
    from: Option<&str>,
    to: Option<&str>,
) -> Result<(OffsetDateTime, OffsetDateTime), WindowRefusal> {
    use time::format_description::well_known::Rfc3339;

    let refuse = WindowRefusal;
    let parse = |name: &str, raw: &str| {
        OffsetDateTime::parse(raw, &Rfc3339).map_err(|_| {
            refuse(format!(
                "invalid `{name}` timestamp {raw:?}; expected RFC 3339"
            ))
        })
    };

    let to = match to {
        Some(raw) => parse("to", raw)?,
        None => OffsetDateTime::now_utc(),
    };
    let from = match from {
        Some(raw) => parse("from", raw)?,
        None => to - DEFAULT_READ_WINDOW,
    };

    if from >= to {
        return Err(refuse(format!(
            "`from` ({from}) must be before `to` ({to})"
        )));
    }
    if to - from > MAX_READ_WINDOW {
        return Err(refuse(format!(
            "requested window is {} days; the maximum for a materialised read is {}. \
             Use `Accept: application/vnd.apache.arrow.stream`, POST /api/v1/query/sql, \
             or the Iceberg REST catalog under /api/v1/iceberg for bulk history.",
            (to - from).whole_days(),
            MAX_READ_WINDOW.whole_days(),
        )));
    }
    Ok((from, to))
}

// ── RunConfig + startup ───────────────────────────────────────────────────────

pub struct RunConfig {
    /// The runner's tuned pool, already connected and migrated. It also backs
    /// meterstore's hot tier, so its sizing bounds both.
    pub pool: PgPool,
    /// §14a SMGW/CLS compliance sweep thresholds.
    pub smgw: crate::config::SmgwConfig,
    /// Delivery-surveillance thresholds.
    pub surveillance: crate::config::SurveillanceConfig,
    /// The same URL the pool was built from — meterstore's `SqlCatalog` opens its
    /// own small metadata pool over it (see `store::CATALOG_POOL_MAX`).
    pub database_url: SecretString,
    pub marktd_url: String,
    pub marktd_api_key: secrecy::SecretString,
    pub subscriber_id: String,
    pub webhook_url: String,
    pub webhook_secret: Option<SecretString>,
    pub inbound_secret: Option<SecretString>,
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
    /// Optional secret signing every outbound CloudEvent (Standard Webhooks).
    pub erp_webhook_secret: Option<String>,
    /// Request rate limits. Ingest endpoints accept unbounded batches, so an
    /// unthrottled client can saturate the write path for every other tenant.
    pub rate_limit: mako_service::RateLimitConfig,
    /// Kafka ingest consumer (None or `enabled = false` → not started).
    pub kafka_ingest: Option<crate::config::KafkaIngestConfig>,
    /// § 60 Abs. 2 MsbG confirmation loop (overdue escalation worker).
    pub confirmation: crate::config::ConfirmationConfig,
}

/// Cedar gate for the nested meterstore Iceberg REST catalog.
///
/// The facade exposes table locations and schemas for the tenant's archived meter
/// data, so it is gated by the same `read-archive-olap` action as the archive
/// queries it describes — authenticated by the shared OIDC `Claims` extractor and
/// authorised by Cedar — rather than left open to anything that can reach the
/// port. meterstore's router deliberately carries no auth of its own so the host
/// can wrap exactly this policy around it.
async fn iceberg_catalog_guard(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "read-archive-olap",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    next.run(request).await
}

/// Build edmd's domain router and spawn its background workers.
///
/// The runner (`mako_service::run`) owns everything around this: tracing, the
/// tuned pool (already connected and migrated, handed over in `cfg.pool`), the
/// health probes with a real DB ping, the generic `/metrics`, and graceful
/// shutdown on SIGINT **and SIGTERM**.
pub async fn build(cfg: RunConfig) -> anyhow::Result<Router> {
    let pool = cfg.pool.clone();

    // ── meterstore-backed storage tier ─────────────────────────────────────────
    // The hot PostgreSQL window and the cold Iceberg history are owned by
    // `meterstore`: `build_stores` constructs one `SqlCatalog` over this same
    // Postgres and an OpenDAL S3 warehouse, then builds every table over it as a
    // shared `MeterCatalog`. `meter_reads` is the authoritative store (with a GDPR
    // subject registry); `esa_typ2_reads` is a second, non-authoritative table for
    // the ESA "Werte nach Typ 2" stream. The warehouse URI comes from
    // `[archive].storage_uri` when configured.
    let database_url = cfg.database_url.expose_secret().to_owned();
    let warehouse_uri = cfg
        .archive
        .as_ref()
        .map(|a| a.storage_uri.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "memory://".to_string());

    // meterstore runs in-process, so every tiering knob it needs comes from edmd's
    // `[archive]` config (whole days → `Duration`); an absent section falls back
    // to meterstore's defaults.
    let tiering = cfg
        .archive
        .as_ref()
        .map(|a| crate::store::TieringConfig {
            settlement_lag: time::Duration::days(i64::from(a.settlement_lag_days)),
            partition_step: time::Duration::days(i64::from(a.partition_step_days)),
            archival_step: time::Duration::days(i64::from(a.archival_step_days)),
            cold_file_target_bytes: a.cold_file_target_mib as usize * 1024 * 1024,
        })
        .unwrap_or_default();

    // Object-store credentials for an `s3://` warehouse (empty for file/memory, or
    // when the deployment relies on an instance-role credential chain).
    let warehouse_auth = cfg
        .archive
        .as_ref()
        .map(|a| crate::store::WarehouseAuth {
            region: a.region.clone(),
            endpoint: a.endpoint_url.clone(),
            access_key_id: a.access_key_id.clone(),
            secret_access_key: a.secret_access_key.clone(),
        })
        .unwrap_or_default();

    // All three tables share one Iceberg catalog and one DataFusion session
    // (§15.3): `meter_reads` (authoritative intervals, with the GDPR subject
    // registry), `esa_typ2_reads` (non-authoritative ESA stream) and
    // `meter_readings` (the Zählerstandsgang — register values at instants).
    let (reads_store, typ2_store, zsg_store, reads_cold) = build_stores(
        pool.clone(),
        &database_url,
        &warehouse_uri,
        tiering,
        &warehouse_auth,
    )
    .await?;
    tracing::info!(warehouse = %warehouse_uri, "edmd: meterstore tiers ready");

    // Start meterstore's tiering maintenance loop for each store. Without it,
    // settled intervals never move from the hot PostgreSQL tier into the cold
    // Iceberg tier — the watermark advances only when a cycle archives the windows
    // it covers. Each handle **owns** its loop (dropping it stops the loop), so
    // they are held for the whole process lifetime. Gated on `enabled`: a hot-only
    // dev store (in-memory warehouse) has nowhere to tier to.
    let _maintenance = if cfg.archive.as_ref().is_some_and(|a| a.enabled) {
        let period = time::Duration::seconds(i64::from(
            cfg.archive
                .as_ref()
                .map_or(3_600, |a| a.maintenance_interval_secs),
        ));
        vec![
            reads_store.maintenance().interval(period).spawn(),
            typ2_store.maintenance().interval(period).spawn(),
            // The Zählerstandsgang tiers too: a quarter-hourly ZSG is the same
            // volume as the Lastgang it produces, and settling one while the
            // other grows unbounded in the hot tier is not a decision anyone
            // made.
            zsg_store.maintenance().interval(period).spawn(),
        ]
    } else {
        Vec::new()
    };

    let repo = MeterStoreTimeSeriesRepository::new(reads_store, zsg_store, pool.clone());
    // The surveillance worker reads the resolved series across both tiers, so it
    // needs the store handle, not just the business-table pool.
    let repo_for_surveillance = repo.clone();

    let mcp_state = Arc::new(crate::mcp_server::EdmdMcpState {
        pool: pool.clone(),
        repo: repo.clone(),
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

    let typ2_repo = MeterStoreTyp2Repository::new(typ2_store);
    // The surveillance worker watches the Typ-2 store on its own thresholds.
    let typ2_for_surveillance = typ2_repo.clone();
    // Clone the webhook URL/secret and tenant before they are moved into HandlerState.
    let smgw_webhook_url = cfg.erp_webhook_url.clone();
    let smgw_webhook_secret = cfg.erp_webhook_secret.clone();
    let smgw_tenant = cfg.tenant.clone();
    let state = HandlerState {
        repo,
        typ2_repo,
        inbound_secret: Arc::new(cfg.inbound_secret),
        tenant: cfg.tenant,
        marktd_url: cfg.marktd_url.clone(),
        marktd_api_key: cfg.marktd_api_key.clone(),
        erp_webhook_url: cfg.erp_webhook_url,
        erp_webhook_secret: cfg
            .erp_webhook_secret
            .clone()
            .map(secrecy::SecretString::from),
        cold_tier_enabled: cfg.archive.as_ref().is_some_and(|a| a.enabled),
        smgw: cfg.smgw.clone(),
        surveillance: cfg.surveillance.clone(),
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
            crate::kafka_ingest::QualityAlertTarget {
                webhook_url: state.erp_webhook_url.clone(),
                secret: state.erp_webhook_secret.clone(),
            },
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
                    makopid_filter: crate::domain::MSCONS_PIDS,
                    active: true,
                },
            )
            .await;
    }

    let pool_arc = Arc::new(pool);

    // meterstore's read-only Iceberg REST catalog, mounted so external engines
    // (DuckDB / Spark / Trino / PyIceberg) can read the cold tier directly from
    // object storage — edmd stays in the metadata path only, never the data path.
    // Gated by the same Cedar `read-archive-olap` action as the archive queries it
    // describes; the outer OIDC/Cedar Extension layers below reach it because they
    // wrap the nested routes too.
    let iceberg_facade =
        reads_cold
            .catalog_facade()
            .router()
            .route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                iceberg_catalog_guard,
            ));

    // Both limiters apply: the keyed one bounds any single caller, the global one
    // bounds their sum. Health, tracing and the generic `/metrics` come from the
    // runner, so they are not added again here.
    let app = mako_service::ServiceBuilder::new()
        .merge(
            router(state)
                .nest("/api/v1/iceberg", iceberg_facade)
                .layer(Extension(cfg.cedar))
                .layer(Extension(cfg.oidc))
                .layer(Extension(pool_arc.clone()))
                .merge(crate::mcp_server::router(mcp_state, cfg.shutdown.clone())),
        )
        .with_tenant_rate_limit(&cfg.rate_limit)
        .with_rate_limit(&cfg.rate_limit)
        .build();

    tracing::info!(marktd_url = %cfg.marktd_url, "edmd: domain router built");

    // §14a compliance sweeps (§ 25 MsbG: the GWA is responsible for monitoring
    // and maintenance of the intelligent metering system). Two sweeps, both
    // deduplicated so a standing fault is announced once rather than daily:
    // the CLS/certificate compliance register and the tiered certificate-expiry
    // advance warning.
    if cfg.smgw.enabled {
        crate::smgw::spawn_cls_compliance_worker(
            pool_arc.clone(),
            smgw_tenant.clone(),
            smgw_webhook_url.clone(),
            smgw_webhook_secret.clone(),
            cfg.smgw.cert_warning_days,
            cfg.smgw.comm_fault_threshold_hours,
            cfg.smgw.sweep_interval_secs,
            cfg.shutdown.clone(),
        );
        crate::smgw::spawn_smgw_cert_expiry_worker(
            pool_arc.clone(),
            smgw_tenant.clone(),
            smgw_webhook_url.clone(),
            smgw_webhook_secret.clone(),
            cfg.smgw.sweep_interval_secs,
            cfg.shutdown.clone(),
        );
    }

    // Delivery surveillance — the measuring points that have gone quiet. The
    // V-rules can only judge data that arrived; this is the other half.
    if cfg.surveillance.enabled {
        crate::surveillance::spawn_surveillance_worker(
            repo_for_surveillance,
            Some(typ2_for_surveillance),
            cfg.surveillance.clone(),
            smgw_tenant.clone(),
            smgw_webhook_url.clone(),
            smgw_webhook_secret.clone(),
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
            smgw_webhook_secret,
            cfg.confirmation.deadline_weeks,
            86_400, // daily
            cfg.shutdown.clone(),
        );
    }

    // The maintenance handles own their tiering loops — dropping one stops it —
    // so they are parked on the shutdown token's lifetime rather than on this
    // function's, which returns as soon as the router is built.
    if !_maintenance.is_empty() {
        let shutdown = cfg.shutdown.clone();
        tokio::spawn(async move {
            let _held = _maintenance;
            shutdown.cancelled().await;
            tracing::info!("edmd: meterstore tiering loops stopping");
        });
    }

    Ok(app)
}
