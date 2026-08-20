#![allow(clippy::result_large_err)] // the error *is* the HTTP response

//! Axum router and daemon wiring for `invoicd`.
//!
//! | Method | Path | Cedar action |
//! |---|---|---|
//! | `POST` | `/webhook` | — (HMAC from `marktd`) |
//! | `GET` | `/api/v1/receipts` | `read-receipt` |
//! | `GET` | `/api/v1/receipts/{id}` | `read-receipt` |
//! | `GET` | `/api/v1/receipts/{id}/rechnung` | `read-receipt` |
//! | `POST` | `/api/v1/receipts/{id}/confirm-payment` | `write-receipt` |
//! | `POST` | `/api/v1/receipts/{id}/dispatch-answer` | `write-receipt` |
//! | `POST` | `/api/v1/receipts/{id}/resolve-dispute` | `write-receipt` |
//! | `GET` | `/api/v1/disputes` | `read-disputes` |
//! | `GET` | `/api/v1/overdue-remadv` | `read-overdue-remadv` |
//! | `GET` | `/api/v1/zahlungsstatus/{malo_id}` | `read-receipt` |
//! | `POST` | `/api/v1/selbstausstellen` | `dispatch-selbstausstellen` |
//! | `GET` | `/invoicd/metrics` | — (internal) |
//!
//! `/health/*` and the generic `/metrics` are the runner's and are not mounted
//! here.

use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use mako_service::{Daemon, ServiceContext};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;

use crate::{
    config::Config,
    handler::{HandlerState, handle_webhook},
    pg,
    routing::route_for,
};
use mako_markt::{makod_client::MakodClient, marktd_client::MarktdClient};

// ── Daemon ────────────────────────────────────────────────────────────────────

/// The `invoicd` daemon.
pub struct Invoicd;

impl Daemon for Invoicd {
    type Config = Config;
    const NAME: &'static str = "invoicd";

    async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run invoicd migrations")
    }

    async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
        build(cfg, ctx).await
    }
}

// ── Authorization ─────────────────────────────────────────────────────────────

/// Authorise `action` for the caller against this deployment's tenant.
///
/// Cedar is deny-by-default, so an action named here and missing from
/// `policies/invoicd.cedar` is a permanent 403 no configuration can lift.
/// `tests/cedar_actions.rs` pins the two together.
fn authorize(
    cedar: &CedarEnforcer,
    claims: &Claims,
    action: &str,
    tenant: &str,
) -> Result<(), Response> {
    cedar
        .check(&claims.principal(), action, tenant)
        .map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        })
}

/// Every Cedar action this router checks — the list `tests/cedar_actions.rs`
/// compares against the policy file.
pub const CEDAR_ACTIONS: &[&str] = &[
    "read-receipt",
    "read-disputes",
    "read-overdue-remadv",
    "write-receipt",
    "dispatch-selbstausstellen",
    "use-mcp",
];

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the domain router.
pub fn router(state: HandlerState) -> Router {
    Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/api/v1/receipts", get(list_receipts))
        .route("/api/v1/receipts/{id}", get(get_receipt))
        .route("/api/v1/receipts/{id}/rechnung", get(get_rechnung))
        .route(
            "/api/v1/receipts/{id}/confirm-payment",
            post(confirm_payment),
        )
        .route(
            "/api/v1/receipts/{id}/dispatch-answer",
            post(dispatch_answer),
        )
        .route(
            "/api/v1/receipts/{id}/resolve-dispute",
            post(resolve_dispute),
        )
        .route("/api/v1/disputes", get(list_disputes))
        .route("/api/v1/overdue-remadv", get(list_overdue_remadv))
        .route("/api/v1/zahlungsstatus/{malo_id}", get(get_zahlungsstatus))
        .route(
            "/api/v1/selbstausstellen",
            post(crate::selbstausstellen::post_selbstausstellen),
        )
        .route("/invoicd/metrics", get(metrics))
        .with_state(state)
}

// ── Metrics ───────────────────────────────────────────────────────────────────

/// `GET /invoicd/metrics` — receipt counts, disputes, overdue answers, and the
/// two queues an operator has to watch.
///
/// **Every gauge is tenant-scoped.** Three of them were not: the totals counted
/// every tenant in the database, so a shared deployment published one
/// operator's invoice volume on another's dashboard, and any single-tenant
/// alert threshold was meaningless.
///
/// Unauthenticated by design — restrict network access at the ingress.
async fn metrics(State(state): State<HandlerState>) -> Response {
    let pool = &state.pool;
    let tenant = &state.tenant;

    let scalar = |sql: &'static str| async move {
        sqlx::query_scalar::<_, i64>(sql)
            .bind(tenant)
            .fetch_one(pool)
            .await
            .unwrap_or(0)
    };
    let (total, disputes, overdue, undelivered, dead_letters) = tokio::join!(
        scalar("SELECT COUNT(*) FROM invoic_receipts WHERE tenant = $1"),
        scalar("SELECT COUNT(*) FROM invoic_receipts WHERE tenant = $1 AND outcome = 'Dispute'"),
        scalar(
            "SELECT COUNT(*) FROM invoic_receipts WHERE tenant = $1 \
             AND pay_by < now() + INTERVAL '3 days' AND dispatched_at IS NULL"
        ),
        scalar(
            "SELECT COUNT(*) FROM invoic_receipts WHERE tenant = $1 \
             AND erp_notified_at IS NULL AND erp_attempts >= 5"
        ),
        scalar("SELECT COUNT(*) FROM invoic_dlq WHERE tenant = $1 AND resolved_at IS NULL"),
    );

    let mut out = String::with_capacity(1024);
    for (name, help, value) in [
        (
            "invoicd_receipts_total",
            "INVOIC receipts persisted (§ 147 AO / GoBD).",
            total,
        ),
        (
            "invoicd_disputes_total",
            "Receipts with a Dispute outcome.",
            disputes,
        ),
        (
            "invoicd_overdue_remadv_total",
            "Receipts within 3 days of the Zahlungsziel whose answer never went out.",
            overdue,
        ),
        (
            "invoicd_erp_dead_lettered_total",
            "Receipts the ERP webhook never accepted. Non-zero means the ERP is not hearing about settled invoices.",
            undelivered,
        ),
        (
            "invoicd_dlq_open_total",
            "INVOICs that could not become receipts. Non-zero is an unprocessed Buchungsbeleg.",
            dead_letters,
        ),
    ] {
        out.push_str(&format!(
            "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"
        ));
    }

    let by_pid: Vec<(i16, String, i64)> = sqlx::query_as(
        r"SELECT pid, outcome, COUNT(*) FROM invoic_receipts WHERE tenant = $1 GROUP BY pid, outcome",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if !by_pid.is_empty() {
        out.push_str(
            "# HELP invoicd_receipts_by_pid_outcome Receipts by PID and outcome.\n\
             # TYPE invoicd_receipts_by_pid_outcome gauge\n",
        );
        for (pid, outcome, count) in by_pid {
            out.push_str(&format!(
                "invoicd_receipts_by_pid_outcome{{pid=\"{pid}\",outcome=\"{outcome}\"}} {count}\n"
            ));
        }
    }

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
        .into_response()
}

// ── Reads ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ReceiptListQuery {
    sender_mp_id: Option<String>,
    outcome: Option<String>,
    from: Option<String>,
    to: Option<String>,
    #[serde(default)]
    page: u32,
    #[serde(default = "default_size")]
    size: u32,
}

fn default_size() -> u32 {
    50
}

/// Largest receipt page a caller may request.
const MAX_PAGE: u32 = 500;

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ReceiptSummary {
    id: uuid::Uuid,
    process_id: uuid::Uuid,
    pid: i16,
    sender_mp_id: String,
    outcome: String,
    invoice_ref: Option<String>,
    rechnungsnummer: Option<String>,
    #[serde(serialize_with = "ser_ts")]
    received_at: time::OffsetDateTime,
    bo4e_version: String,
}

/// RFC 3339 on the wire — never `time`'s derived component array, which
/// round-trips only through `time` itself (`xtask check-wire-timestamps`).
fn ser_ts<S: serde::Serializer>(t: &time::OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&t.format(&Rfc3339).unwrap_or_default())
}

fn fmt_ts(t: Option<time::OffsetDateTime>) -> Option<String> {
    t.and_then(|t| t.format(&Rfc3339).ok())
}

fn db_error(e: sqlx::Error) -> Response {
    tracing::warn!(error = %e, "invoicd: query failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": "query failed" })),
    )
        .into_response()
}

/// `GET /api/v1/receipts` — list receipts for this tenant.
async fn list_receipts(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<ReceiptListQuery>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "read-receipt", &state.tenant) {
        return r;
    }
    match fetch_receipts(&state.pool, &state.tenant, &params).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/disputes` — the open exception queue.
async fn list_disputes(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "read-disputes", &state.tenant) {
        return r;
    }
    let params = ReceiptListQuery {
        sender_mp_id: None,
        outcome: Some("Dispute".to_owned()),
        from: None,
        to: None,
        page: 0,
        size: 200,
    };
    match fetch_receipts(&state.pool, &state.tenant, &params).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/receipts/{id}`
async fn get_receipt(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "read-receipt", &state.tenant) {
        return r;
    }
    let row = sqlx::query_as::<_, ReceiptSummary>(
        r"SELECT id, process_id, pid, sender_mp_id, outcome, invoice_ref, rechnungsnummer,
                 received_at, bo4e_version
          FROM invoic_receipts WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some(r)) => Json(r).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/receipts/{id}/rechnung` — the invoice exactly as received.
async fn get_rechnung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "read-receipt", &state.tenant) {
        return r;
    }
    let row: Result<Option<(serde_json::Value, String, i16)>, _> = sqlx::query_as(
        "SELECT rechnung, bo4e_version, pid FROM invoic_receipts WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some((rechnung, bo4e_version, pid))) => Json(
            serde_json::json!({ "rechnung": rechnung, "bo4e_version": bo4e_version, "pid": pid }),
        )
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/overdue-remadv`
///
/// Receipts whose Zahlungsziel is within three days and whose answer never went
/// out. An undispatched answer past the Zahlungsziel is both a market-process
/// breach and a § 147 AO gap, so this is the alert query: run it every 6 h and
/// alert when it is non-empty.
async fn list_overdue_remadv(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "read-overdue-remadv", &state.tenant) {
        return r;
    }
    let rows: Result<Vec<ReceiptSummary>, _> = sqlx::query_as(
        r"SELECT id, process_id, pid, sender_mp_id, outcome, invoice_ref, rechnungsnummer,
                 received_at, bo4e_version
          FROM invoic_receipts
          WHERE tenant = $1
            AND pay_by IS NOT NULL
            AND pay_by < now() + INTERVAL '3 days'
            AND dispatched_at IS NULL
          ORDER BY pay_by ASC
          LIMIT 200",
    )
    .bind(&state.tenant)
    .fetch_all(&state.pool)
    .await;
    match rows {
        Ok(items) => {
            Json(serde_json::json!({ "count": items.len(), "items": items })).into_response()
        }
        Err(e) => db_error(e),
    }
}

/// `GET /api/v1/zahlungsstatus/{malo_id}` — payment lifecycle per MaLo.
async fn get_zahlungsstatus(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "read-receipt", &state.tenant) {
        return r;
    }
    type Row = (
        uuid::Uuid,
        uuid::Uuid,
        i16,
        String,
        String,
        Option<time::OffsetDateTime>,
        Option<time::OffsetDateTime>,
        Option<time::OffsetDateTime>,
        time::OffsetDateTime,
    );
    let rows: Result<Vec<Row>, _> = sqlx::query_as(
        r"SELECT id, process_id, pid, sender_mp_id, outcome, pay_by,
                 dispatched_at, payment_confirmed_at, received_at
          FROM invoic_receipts
          WHERE tenant = $1 AND malo_id = $2
          ORDER BY received_at DESC
          LIMIT 100",
    )
    .bind(&state.tenant)
    .bind(&malo_id)
    .fetch_all(&state.pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => return db_error(e),
    };
    let now = time::OffsetDateTime::now_utc();
    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |(
                id,
                process_id,
                pid,
                sender_mp_id,
                outcome,
                pay_by,
                dispatched,
                confirmed,
                received,
            )| {
                let status = if confirmed.is_some() {
                    "settled"
                } else if dispatched.is_none() {
                    "undispatched"
                } else if pay_by.is_some_and(|d| d < now) {
                    "overdue"
                } else {
                    "pending"
                };
                serde_json::json!({
                    "id":                   id,
                    "process_id":           process_id,
                    "pid":                  pid,
                    "sender_mp_id":         sender_mp_id,
                    "outcome":              outcome,
                    "zahlungsstatus":       status,
                    "pay_by":               fmt_ts(pay_by),
                    "dispatched_at":        fmt_ts(dispatched),
                    "payment_confirmed_at": fmt_ts(confirmed),
                    "received_at":          fmt_ts(Some(received)),
                })
            },
        )
        .collect();

    let count = |s: &str| items.iter().filter(|i| i["zahlungsstatus"] == s).count();
    Json(serde_json::json!({
        "malo_id":            malo_id,
        "settled_count":      count("settled"),
        "pending_count":      count("pending"),
        "overdue_count":      count("overdue"),
        "undispatched_count": count("undispatched"),
        "items":              items,
    }))
    .into_response()
}

// ── Writes ────────────────────────────────────────────────────────────────────

/// `POST /api/v1/receipts/{id}/confirm-payment`
///
/// The ERP reports that the money moved. Closes the § 147 AO payment trail and
/// stops the receipt appearing in the overdue view.
async fn confirm_payment(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "write-receipt", &state.tenant) {
        return r;
    }
    match pg::receipts::confirm_payment(&state.pool, id, &state.tenant).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "receipt not found or already confirmed" })),
        )
            .into_response(),
        Err(e) => db_error(e),
    }
}

#[derive(Deserialize)]
struct ResolveDisputeBody {
    note: Option<String>,
}

/// `POST /api/v1/receipts/{id}/resolve-dispute`
///
/// Record that a dispute was settled out of band (a corrected invoice, a phone
/// call, a COMDIS). Moves `Dispute` → `Resolved` with the operator's note.
async fn resolve_dispute(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    body: Option<Json<ResolveDisputeBody>>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "write-receipt", &state.tenant) {
        return r;
    }
    let note = body.as_ref().and_then(|b| b.note.as_deref());
    match pg::receipts::resolve_dispute(&state.pool, id, &state.tenant, note).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "receipt not found or not in Dispute state" })),
        )
            .into_response(),
        Err(e) => db_error(e),
    }
}

/// `POST /api/v1/receipts/{id}/dispatch-answer`
///
/// Re-send the market answer for a receipt whose automatic dispatch failed.
///
/// Both the routing key and the command come from the receipt:
///
/// - **The routing key** is the EDIFACT message reference, which is what
///   `makod` correlates the answer by.
/// - **The command** is the answering PID's, from [`crate::routing`].
///
/// Getting either wrong fails silently — `makod` accepts the command and only
/// the correlation misses — so neither is defaulted.
async fn dispatch_answer(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    if let Err(r) = authorize(&cedar, &claims, "write-receipt", &state.tenant) {
        return r;
    }
    let target = match pg::receipts::dispatch_target(&state.pool, id, &state.tenant).await {
        Ok(Some(t)) => t,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return db_error(e),
    };

    if target.already_dispatched {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "receipt already dispatched" })),
        )
            .into_response();
    }
    let Some(invoice_ref) = target.invoice_ref else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "receipt has no INVOIC message reference — the answer cannot be routed"
            })),
        )
            .into_response();
    };
    let Some(route) = route_for(target.pid as u32) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("PID {} is not answered by this service", target.pid)
            })),
        )
            .into_response();
    };

    let dispute = target.outcome == "Dispute";
    let (command, payload) = if dispute {
        (
            route.reject,
            serde_json::json!({
                "invoice_ref": invoice_ref,
                "ablehnungsgrund": "Erneute Übermittlung der Ablehnung (Operator)",
            }),
        )
    } else {
        (
            route.accept,
            serde_json::json!({ "invoice_ref": invoice_ref }),
        )
    };

    // A distinct salt from the automatic dispatch: this is a second command for
    // the same process, and reusing the key would make it indistinguishable
    // from the attempt that failed.
    let key = uuid::Uuid::new_v5(&target.process_id, b"manual-dispatch").to_string();
    let cmd = mako_markt::makod_client::ForwardCommand {
        marktrolle: None,
        command: command.to_owned(),
        malo_id: None,
        melo_id: None,
        payload,
    };
    match state.makod.post_command(&key, &cmd).await {
        Ok(_) => {
            if let Err(e) = pg::receipts::mark_dispatched(
                &state.pool,
                target.process_id,
                time::OffsetDateTime::now_utc(),
            )
            .await
            {
                tracing::warn!(%e, process_id = %target.process_id, "invoicd: manual dispatch not recorded");
            }
            Json(serde_json::json!({
                "dispatched":  true,
                "process_id":  target.process_id,
                "pid":         target.pid,
                "command":     command,
                "invoice_ref": invoice_ref,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("makod dispatch failed: {e}") })),
        )
            .into_response(),
    }
}

// ── Queries ───────────────────────────────────────────────────────────────────

async fn fetch_receipts(
    pool: &PgPool,
    tenant: &str,
    params: &ReceiptListQuery,
) -> Result<Vec<ReceiptSummary>, sqlx::Error> {
    let limit = i64::from(params.size.clamp(1, MAX_PAGE));
    let offset = i64::from(params.page) * limit;
    let from = params
        .from
        .as_deref()
        .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok());
    let to = params
        .to
        .as_deref()
        .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok());

    sqlx::query_as::<_, ReceiptSummary>(
        r"SELECT id, process_id, pid, sender_mp_id, outcome, invoice_ref, rechnungsnummer,
                 received_at, bo4e_version
          FROM invoic_receipts
          WHERE tenant = $1
            AND ($2::text IS NULL OR sender_mp_id = $2)
            AND ($3::text IS NULL OR outcome = $3)
            AND ($4::timestamptz IS NULL OR received_at >= $4)
            AND ($5::timestamptz IS NULL OR received_at <= $5)
          ORDER BY received_at DESC
          LIMIT $6 OFFSET $7",
    )
    .bind(tenant)
    .bind(params.sender_mp_id.as_deref())
    .bind(params.outcome.as_deref())
    .bind(from)
    .bind(to)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

// ── Assembly ──────────────────────────────────────────────────────────────────

/// Resolve secrets, build the OIDC verifier and Cedar enforcer, wire the
/// handler state and MCP server, spawn the background workers, and register the
/// `marktd` subscription.
pub async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
    let oidc = mako_service::oidc::OidcConfig::build_verifier(
        cfg.oidc.as_ref(),
        &ctx.http,
        &cfg.identity.tenant,
        ctx.shutdown.clone(),
    )
    .await?;
    let cedar = Arc::new(
        CedarEnforcer::from_policy_str(include_str!("../policies/invoicd.cedar"))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    let resolve = |v: Option<&str>, what: &'static str| -> anyhow::Result<Option<SecretString>> {
        v.map(crate::config::resolve_env_secret)
            .transpose()
            .context(what)
    };
    let makod_api_key = resolve(cfg.makod.api_key.as_deref(), "makod.api_key")?;
    let marktd_api_key =
        crate::config::resolve_env_secret(&cfg.marktd.api_key).context("marktd.api_key")?;
    let inbound_secret = resolve(
        cfg.webhook.inbound_secret.as_deref(),
        "webhook.inbound_secret",
    )?;
    let erp_hmac_secret = resolve(cfg.erp.hmac_secret.as_deref(), "erp.hmac_secret")?;
    let edmd_api_key = resolve(cfg.edmd.api_key.as_deref(), "edmd.api_key")?;

    let tenant = cfg.identity.tenant.clone();
    let pool = ctx.pool().clone();
    let marktd = MarktdClient::new(&cfg.marktd.url, marktd_api_key, ctx.http.clone());
    let makod = MakodClient::new(
        &cfg.makod.url,
        makod_api_key.unwrap_or_else(|| SecretString::new(String::new().into())),
    );

    let state = HandlerState {
        marktd: marktd.clone(),
        makod,
        check_config: Arc::new(cfg.check_config()),
        inbound_secret: Arc::new(inbound_secret.clone()),
        auto_dispute_threshold_raw: cfg.auto_dispute_threshold_raw(),
        pool: pool.clone(),
        tenant: tenant.clone(),
        erp_webhook_url: cfg.erp.webhook_url.clone(),
        erp_hmac_secret: erp_hmac_secret.clone(),
        edmd: cfg.edmd.url.as_deref().map(|url| {
            mako_service::http::Upstream::new("edmd", url, edmd_api_key, ctx.http.clone())
        }),
        http_client: ctx.http.clone(),
    };

    let mcp_state = Arc::new(crate::mcp_server::InvoicdMcpState {
        pool: pool.clone(),
        tenant: tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            oidc.clone(),
            Some(cedar.clone()),
            &tenant,
        ),
    });

    if let Some(erp_url) = cfg.erp.webhook_url.clone() {
        crate::erp_outbox::spawn(
            pool.clone(),
            tenant.clone(),
            erp_url.clone(),
            erp_hmac_secret.clone(),
            ctx.http.clone(),
            ctx.shutdown.clone(),
        );
        crate::payment_overdue::spawn(
            pool.clone(),
            tenant.clone(),
            erp_url,
            erp_hmac_secret,
            ctx.http.clone(),
            ctx.shutdown.clone(),
        );
    } else {
        tracing::warn!(
            "invoicd: no [erp] webhook_url — de.invoic.receipt.* and de.invoic.payment.overdue \
             events are recorded but nothing delivers them; the ERP will not learn that an \
             invoice was settled, disputed or has passed its Zahlungsziel"
        );
    }

    // The PIDs registered as the subscription filter come from the routing
    // table, so a PID added there starts arriving without a second edit — and
    // one removed stops being delivered.
    let pids: Vec<u32> = crate::routing::ROUTES.iter().map(|r| r.pid).collect();
    marktd
        .put_subscription(
            &cfg.subscription.subscriber_id,
            &mako_markt::marktd_client::SubscriptionRequest {
                webhook_url: &cfg.subscription.webhook_url,
                webhook_secret: inbound_secret.as_ref().map(|s| {
                    use secrecy::ExposeSecret as _;
                    s.expose_secret()
                }),
                event_types: &[mako_events::mako::PROCESS_INITIATED],
                makopid_filter: &pids,
                active: true,
            },
        )
        .await;

    Ok(router(state)
        .layer(Extension(cedar))
        .layer(Extension(oidc))
        .merge(crate::mcp_server::router(mcp_state, ctx.shutdown.clone())))
}
