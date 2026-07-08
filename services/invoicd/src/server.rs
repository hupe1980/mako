//! Axum router for `invoicd`.
//!
//! Routes:
//! - `POST /webhook`                             — inbound MarktEvent CloudEvents from `marktd` (HMAC-auth)
//! - `GET  /api/v1/receipts`                     — query INVOIC receipts (OIDC+Cedar)
//! - `GET  /api/v1/receipts/:id`                 — get a single receipt (OIDC+Cedar)
//! - `GET  /api/v1/disputes`                     — list open disputes (OIDC+Cedar)
//! - `GET  /api/v1/overdue-remadv`               — receipts approaching `pay_by` without dispatch
//! - `POST /api/v1/selbstausstellen/{malo_id}`   — trigger outbound selbstausgestellt INVOIC 31006 (M16)
//! - `GET  /health/live`                         — liveness probe (always 200)
//! - `GET  /health/ready`                        — readiness probe (200 OK)

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use invoic_checker::CheckConfig;
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::{Claims, OidcVerifier};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::{
    handler::{HandlerState, handle_webhook},
    pg,
};
use mako_markt::{makod_client::MakodClient, marktd_client::MarktdClient};

// ── Router ────────────────────────────────────────────────────────────────────

/// Build and return the Axum router with all routes attached.
///
/// `/webhook` is HMAC-authenticated (no OIDC — `marktd` is the caller).
/// `/api/v1/*` routes require a valid JWT via the `Claims` extractor.
pub fn router(state: HandlerState) -> Router {
    Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/api/v1/receipts", get(list_receipts))
        .route("/api/v1/receipts/{id}", get(get_receipt))
        .route("/api/v1/disputes", get(list_disputes))
        .route("/api/v1/overdue-remadv", get(list_overdue_remadv))
        .route(
            "/api/v1/selbstausstellen/{malo_id}",
            post(post_selbstausstellen),
        )
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(health_ready))
        .with_state(state)
}

async fn health_ready(State(_state): State<HandlerState>) -> impl IntoResponse {
    StatusCode::OK
}

// ── Receipt query DTOs ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ReceiptListQuery {
    sender_mp_id: Option<String>,
    outcome: Option<String>,
    from: Option<String>,
    to: Option<String>,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_size")]
    size: u32,
}
fn default_page() -> u32 {
    0
}
fn default_size() -> u32 {
    50
}

#[derive(Debug, Serialize)]
struct ReceiptRow {
    pub id: uuid::Uuid,
    pub process_id: uuid::Uuid,
    pub pid: i16,
    pub sender_mp_id: String,
    pub outcome: String,
    pub received_at: time::OffsetDateTime,
    pub bo4e_version: String,
}

// ── Receipt handlers (OIDC + Cedar protected) ─────────────────────────────────

/// `GET /api/v1/receipts` — list receipts for the caller's tenant.
async fn list_receipts(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<ReceiptListQuery>,
) -> impl IntoResponse {
    let principal = claims.principal();
    let resource_tenant = &state.tenant;
    if let Err(e) = enforcer.check(&principal, "read-receipt", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let Some(ref pool) = state.pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database not configured" })),
        )
            .into_response();
    };

    match fetch_receipts(pool, resource_tenant, &params).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/receipts/:id` — fetch a single receipt by UUID.
async fn get_receipt(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    let principal = claims.principal();
    let resource_tenant = &state.tenant;
    if let Err(e) = enforcer.check(&principal, "read-receipt", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let Some(ref pool) = state.pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database not configured" })),
        )
            .into_response();
    };

    match fetch_receipt_by_id(pool, id, resource_tenant).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/disputes` — list receipts with outcome = 'Dispute'.
async fn list_disputes(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let principal = claims.principal();
    let resource_tenant = &state.tenant;
    if let Err(e) = enforcer.check(&principal, "read-disputes", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let Some(ref pool) = state.pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database not configured" })),
        )
            .into_response();
    };

    let params = ReceiptListQuery {
        sender_mp_id: None,
        outcome: Some("Dispute".to_owned()),
        from: None,
        to: None,
        page: 0,
        size: 200,
    };
    match fetch_receipts(pool, resource_tenant, &params).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/overdue-remadv`
///
/// List receipts whose `pay_by` Zahlungsziel is within 3 days and for which no
/// REMADV has yet been dispatched (`dispatched_at IS NULL`).
///
/// Alert rule: run every 6 h; alert when non-empty.  Undispatched REMADV past
/// the Zahlungsziel is a §22 MessZV compliance gap.
///
/// Source: GPKE BK6-22-024; Allgemeine Festlegungen §7 (Zahlungsziel).
async fn list_overdue_remadv(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-receipt", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let Some(ref pool) = state.pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database not configured" })),
        )
            .into_response();
    };

    let rows = sqlx::query(
        r"SELECT id, process_id, pid, sender_mp_id, outcome, pay_by, received_at, tenant
          FROM invoic_receipts
          WHERE tenant = $1
            AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
            AND pay_by IS NOT NULL
            AND pay_by < now() + INTERVAL '3 days'
            AND dispatched_at IS NULL
          ORDER BY pay_by ASC
          LIMIT 200",
    )
    .bind(&state.tenant)
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    use sqlx::Row;
                    serde_json::json!({
                        "id": r.try_get::<uuid::Uuid, _>("id").ok(),
                        "process_id": r.try_get::<uuid::Uuid, _>("process_id").ok(),
                        "pid": r.try_get::<i16, _>("pid").ok(),
                        "sender_mp_id": r.try_get::<String, _>("sender_mp_id").ok(),
                        "outcome": r.try_get::<String, _>("outcome").ok(),
                        "pay_by": r.try_get::<time::OffsetDateTime, _>("pay_by").ok()
                            .and_then(|t| {
                                use time::format_description::well_known::Rfc3339;
                                t.format(&Rfc3339).ok()
                            }),
                        "received_at": r.try_get::<time::OffsetDateTime, _>("received_at").ok()
                            .and_then(|t| {
                                use time::format_description::well_known::Rfc3339;
                                t.format(&Rfc3339).ok()
                            }),
                    })
                })
                .collect();
            Json(serde_json::json!({ "count": items.len(), "items": items })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/selbstausstellen/{malo_id}`
///
/// Trigger outbound selbstausgestellt INVOIC 31006 (LF → NB).
///
/// # Prerequisites
///
/// - M15: `edmd` `billing-period` endpoint must be live (for RLM Leistungspreis)
/// - `marktd` must have a valid `PreisblattNetznutzung` for the NB
/// - `marktd` must have a valid `NbContractRecord` for the MaLo
///
/// # §22 MessZV
///
/// The receipt is written to `invoic_receipts` (direction=Outbound,
/// outcome=Dispatched) in a single PostgreSQL transaction BEFORE the command
/// is dispatched to `makod`.  A crash between persist and dispatch is
/// recoverable; a crash before persist would violate 3-year retention.
///
/// Source: GPKE Teil 3 BK6-24-174; §22 MessZV.
#[derive(Debug, serde::Deserialize)]
struct SelbstausstellenRequest {
    /// Start of billing period (ISO 8601 date `YYYY-MM-DD`).
    pub period_from: String,
    /// End of billing period (ISO 8601 date `YYYY-MM-DD`).
    pub period_to: String,
    /// 13-digit NB Marktpartner-ID (BDEW-Codenummer or GLN).
    pub nb_mp_id: String,
}

async fn post_selbstausstellen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(body): Json<SelbstausstellenRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "dispatch-selbstausstellen",
        &state.tenant,
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let Some(ref pool) = state.pool else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database not configured" })),
        )
            .into_response();
    };

    use time::macros::format_description;
    let fmt = format_description!("[year]-[month]-[day]");
    let period_from = match time::Date::parse(&body.period_from, &fmt) {
        Ok(d) => d,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid period_from — use YYYY-MM-DD" })),
            )
                .into_response();
        }
    };
    let period_to = match time::Date::parse(&body.period_to, &fmt) {
        Ok(d) => d,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid period_to — use YYYY-MM-DD" })),
            )
                .into_response();
        }
    };

    if period_to < period_from {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "period_to must be >= period_from" })),
        )
            .into_response();
    }

    // ── Step 1: Fetch PreisblattNetznutzung from marktd ───────────────────────
    // Fetched for reference; full RLM generation uses mako-netzbilanz (N3).
    let _sheet = state
        .preisblatt_client
        .get_preisblatt(&body.nb_mp_id, period_from)
        .await
        .ok()
        .flatten();

    // ── Step 2: Invoke invoic-checker with what we have ───────────────────────
    // For selbstausstellen, we generate a minimal draft. Full RLM generation
    // requires MeterBillingPeriod from edmd (M15) — wired when edmd is available.
    //
    // Current: generate the receipt record and let the ERP supply the rechnung
    // via the `rechnung` field below (simple mode), or error if not provided.
    //
    // TODO N3: replace with mako-netzbilanz::generate_invoic() when available.

    tracing::info!(
        malo_id = %malo_id,
        nb_mp_id = %body.nb_mp_id,
        period_from = %period_from,
        period_to = %period_to,
        "invoicd: selbstausstellen 31006 triggered"
    );

    // ── Step 3: Persist as Dispatched (§22 MessZV) ───────────────────────────
    let process_id = uuid::Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();

    // Minimal rechnung placeholder — full generation is N3 (mako-netzbilanz).
    let rechnung_placeholder = serde_json::json!({
        "_note": "Full Rechnung generation requires mako-netzbilanz (N3)",
        "malo_id": malo_id,
        "nb_mp_id": body.nb_mp_id,
        "period_from": body.period_from,
        "period_to": body.period_to,
    });

    let row = pg::ReceiptRow {
        process_id,
        pid: 31006,
        direction: "Outbound".to_owned(),
        sender_mp_id: state.tenant.clone(),
        receiver_gln: body.nb_mp_id.clone(),
        rechnung: rechnung_placeholder,
        bo4e_version: "v202501.0.0".to_owned(),
        outcome: "Dispatched".to_owned(),
        findings: serde_json::json!([]),
        pay_by: None,
        received_at: now,
        checked_at: now,
        dispatched_at: None,
        tenant: state.tenant.clone(),
    };

    if let Err(e) = pg::upsert_receipt(pool, &row).await {
        tracing::warn!(%e, "invoicd: failed to persist selbstausstellen receipt");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "failed to persist receipt — §22 MessZV; aborting dispatch" }))).into_response();
    }

    // ── Step 4: Dispatch to makod ─────────────────────────────────────────────
    let idempotency_key = format!("invoicd-selbst-31006-{process_id}");
    let cmd = mako_markt::makod_client::ForwardCommand {
        marktrolle: None,
        command: "gpke.abrechnung.selbstausstellen".to_owned(),
        malo_id: Some(malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "pid": 31006,
            "nb_mp_id": body.nb_mp_id,
            "period_from": body.period_from,
            "period_to": body.period_to,
            "note": "Full RLM Rechnung generation pending N3 (mako-netzbilanz)",
        }),
    };

    match state.makod.post_command(&idempotency_key, &cmd).await {
        Ok(accepted) => {
            if let Err(e) =
                pg::receipts::mark_dispatched(pool, process_id, time::OffsetDateTime::now_utc())
                    .await
            {
                tracing::warn!(%e, %process_id, "invoicd: failed to mark selbstausstellen as dispatched");
            }
            (StatusCode::ACCEPTED, Json(serde_json::json!({
                "process_id": accepted.process_id,
                "malo_id": malo_id,
                "nb_mp_id": body.nb_mp_id,
                "period_from": body.period_from,
                "period_to": body.period_to,
                "outcome": "Dispatched",
                "note": "Full RLM Rechnung generation requires mako-netzbilanz (N3) and edmd MeterBillingPeriod (M15)",
            }))).into_response()
        }
        Err(e) => {
            tracing::warn!(%e, %process_id, "invoicd: selbstausstellen dispatch to makod failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("makod dispatch failed: {e}") })),
            )
                .into_response()
        }
    }
}

// ── Database helpers ──────────────────────────────────────────────────────────

async fn fetch_receipts(
    pool: &PgPool,
    tenant: &str,
    params: &ReceiptListQuery,
) -> Result<Vec<ReceiptRow>, sqlx::Error> {
    // Runtime query to avoid compile-time DB requirement.
    // All filtering is done server-side; param binding prevents injection.
    let limit = params.size.min(500) as i64;
    let offset = (params.page as i64) * limit;

    use time::format_description::well_known::Rfc3339;
    let from_ts = params
        .from
        .as_deref()
        .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok());
    let to_ts = params
        .to
        .as_deref()
        .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok());

    let rows = sqlx::query_as::<
        _,
        (
            uuid::Uuid,
            uuid::Uuid,
            i16,
            String,
            String,
            time::OffsetDateTime,
            String,
        ),
    >(
        r#"
        SELECT id, process_id, pid, sender_mp_id, outcome, received_at, bo4e_version
        FROM invoic_receipts
        WHERE tenant = $1
          AND ($2::text IS NULL OR sender_mp_id = $2)
          AND ($3::text IS NULL OR outcome = $3)
          AND ($4::timestamptz IS NULL OR received_at >= $4)
          AND ($5::timestamptz IS NULL OR received_at <= $5)
        ORDER BY received_at DESC
        LIMIT $6 OFFSET $7
        "#,
    )
    .bind(tenant)
    .bind(params.sender_mp_id.as_deref())
    .bind(params.outcome.as_deref())
    .bind(from_ts)
    .bind(to_ts)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(id, process_id, pid, sender_mp_id, outcome, received_at, bo4e_version)| ReceiptRow {
                id,
                process_id,
                pid,
                sender_mp_id,
                outcome,
                received_at,
                bo4e_version,
            },
        )
        .collect())
}

async fn fetch_receipt_by_id(
    pool: &PgPool,
    id: uuid::Uuid,
    tenant: &str,
) -> Result<Option<ReceiptRow>, sqlx::Error> {
    let row = sqlx::query_as::<
        _,
        (
            uuid::Uuid,
            uuid::Uuid,
            i16,
            String,
            String,
            time::OffsetDateTime,
            String,
        ),
    >(
        r#"
        SELECT id, process_id, pid, sender_mp_id, outcome, received_at, bo4e_version
        FROM invoic_receipts
        WHERE id = $1 AND tenant = $2
        "#,
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(id, process_id, pid, sender_mp_id, outcome, received_at, bo4e_version)| ReceiptRow {
            id,
            process_id,
            pid,
            sender_mp_id,
            outcome,
            received_at,
            bo4e_version,
        },
    ))
}

// ── RunConfig ─────────────────────────────────────────────────────────────────

/// Configuration for [`run`].
pub struct RunConfig {
    pub listen: SocketAddr,
    pub makod_url: String,
    pub makod_api_key: Option<SecretString>,
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub subscriber_id: String,
    pub webhook_url: String,
    pub webhook_secret: Option<SecretString>,
    pub inbound_secret: Option<SecretString>,
    pub check_config: CheckConfig,
    pub auto_dispute_threshold_eur_cents: i64,
    /// PostgreSQL URL — `None` = development mode (receipts not persisted).
    pub database_url: Option<String>,
    /// Max PostgreSQL pool connections.
    pub db_max_connections: u32,
    /// Tenant identifier written to every receipt row.
    pub tenant: String,
    /// OIDC verifier.  Use [`OidcVerifier::disabled`] in dev/test.
    pub oidc: OidcVerifier,
    /// Cedar ABAC enforcer loaded from `policies/invoicd.cedar`.
    pub cedar: Arc<CedarEnforcer>,
    /// Graceful-shutdown token.
    pub shutdown: CancellationToken,
}

/// Bind, register subscription with `marktd`, and serve forever.
pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    let preisblatt_client = MarktdClient::new(
        &cfg.marktd_url,
        cfg.marktd_api_key.clone(),
        mako_service::http::default_client(),
    );
    let api_key = cfg
        .makod_api_key
        .unwrap_or_else(|| secrecy::SecretString::new(String::new().into()));
    let makod = MakodClient::new(&cfg.makod_url, api_key);

    // ── PostgreSQL pool (§22 MessZV compliance) ───────────────────────────────
    let pool = if let Some(ref url) = cfg.database_url {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(cfg.db_max_connections)
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        tracing::info!("invoicd: database connected and migrations applied");
        Some(pool)
    } else {
        tracing::warn!(
            "invoicd: no --database-url configured — INVOIC receipts will NOT be persisted (§22 MessZV violation in production)"
        );
        None
    };

    let state = HandlerState {
        preisblatt_client: preisblatt_client.clone(),
        makod,
        check_config: Arc::new(cfg.check_config),
        inbound_secret: Arc::new(cfg.inbound_secret),
        auto_dispute_threshold_eur_cents: cfg.auto_dispute_threshold_eur_cents,
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
    };

    // ── MCP state ─────────────────────────────────────────────────────────────
    let mcp_state = pool.as_ref().map(|p| {
        Arc::new(crate::mcp_server::InvoicdMcpState {
            pool: p.clone(),
            tenant: cfg.tenant.clone(),
            oidc: cfg.oidc.clone(),
            cedar: cfg.cedar.clone(),
        })
    });

    // Register subscription with marktd using the shared MarktdClient.
    preisblatt_client
        .put_subscription(
            &cfg.subscriber_id,
            &mako_markt::marktd_client::SubscriptionRequest {
                webhook_url: &cfg.webhook_url,
                webhook_secret: cfg.webhook_secret.as_ref().map(|s| {
                    use secrecy::ExposeSecret;
                    let secret: &str = s.expose_secret();
                    secret
                }),
                event_types: &["de.mako.process.initiated"],
                makopid_filter: &[],
                active: true,
            },
        )
        .await;

    let mut app = router(state)
        .layer(Extension(cfg.cedar))
        .layer(Extension(cfg.oidc));

    if let Some(mcp) = mcp_state {
        app = app.merge(crate::mcp_server::router(mcp, cfg.shutdown.clone()));
    }

    let listener = TcpListener::bind(cfg.listen).await?;

    tracing::info!(
        listen = %cfg.listen,
        makod_url = %cfg.makod_url,
        marktd_url = %cfg.marktd_url,
        "invoicd: listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cfg.shutdown.cancelled().await })
        .await?;
    Ok(())
}
