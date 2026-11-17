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
//! | `POST` | `/api/v1/receipts/{id}/feststellungen` | `record-feststellung` |
//! | `POST` | `/api/v1/leistungsnachweise` | `record-feststellung` |
//! | `GET` | `/api/v1/disputes` | `read-disputes` |
//! | `GET` | `/api/v1/overdue-remadv` | `read-overdue-remadv` |
//! | `GET` | `/api/v1/zahlungsstatus/{malo_id}` | `read-receipt` |
//! | `GET` | `/invoicd/metrics` | — (internal) |
//!
//! Every `/api/v1` route mounts through `utoipa-axum`'s `OpenApiRouter`
//! ([`api`]), with the webhook routes of `mako_service::subscriptions`
//! (`manage-subscription`, the feed `read-events`); `GET /api/v1/openapi.json`
//! serves the document and `openapi.json` beside the crate is its committed copy.
//!
//! `/health/*` and the generic `/metrics` are the runner's and are not mounted
//! here.

use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use mako_service::subscriptions::{Subscriptions, SubscriptionsConfig};
use mako_service::{ApiError, ApiResult, Json};
use mako_service::{Daemon, ServiceContext};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use utoipa_axum::{router::OpenApiRouter, routes};

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

    async fn migrate(_cfg: &Self::Config, pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run invoicd migrations")?;
        // The `de.invoic.receipt.*` events commit with the receipt's
        // conclusion here and are delivered from it.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        // Webhook endpoints and deliveries over that outbox.
        mako_service::subscriptions::ensure_schema(pool)
            .await
            .context("ensure webhook subscription schema")
    }

    /// Fail closed on both doors — the REST API and the inbound webhook each
    /// reach the receipt store and the market answer — and refuse an
    /// auto-dispute threshold that is no EUR amount and a Zahlungsziel ceiling
    /// of zero days.
    fn validate(cfg: &Config) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        let mut out = Vec::new();
        if let Err(e) = cfg.check_auth_posture() {
            let pfad = if cfg.oidc.is_none() {
                "oidc"
            } else {
                "webhook.inbound_secret"
            };
            out.push(Befund::aus_fehler(pfad, e).bereich(Bereich::Zugriff));
        }
        if let Err(e) = cfg.check.auto_dispute_threshold_raw() {
            out.push(Befund::aus_fehler("check.auto_dispute_threshold_eur", e));
        }
        if cfg.check.max_zahlungsziel_days == Some(0) {
            // `0` would warn on every invoice not due on its own date. Leave
            // the key out for no ceiling.
            out.push(Befund::blockiert(
                "check.max_zahlungsziel_days",
                "check.max_zahlungsziel_days = 0 accepts no payment term at all; \
                 omit the key to check no ceiling",
            ));
        }
        out.extend(cfg.subscriptions.befunde());
        out
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
/// `tests/it/cedar_actions.rs` pins the two together.
fn authorize(cedar: &CedarEnforcer, claims: &Claims, action: &str, tenant: &str) -> ApiResult<()> {
    cedar
        .check(&claims.principal(), action, tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))
}

/// Every Cedar action this router checks — the list `tests/it/cedar_actions.rs`
/// compares against the policy file.
pub const CEDAR_ACTIONS: &[&str] = &[
    "read-receipt",
    "read-disputes",
    "read-overdue-remadv",
    "write-receipt",
    "record-feststellung",
    "use-mcp",
    "read-policy",
    "read-config",
    "manage-subscription",
    "read-events",
];

// ── Router ────────────────────────────────────────────────────────────────────

/// The OpenAPI document's frame: the paths come from the routes.
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "invoicd — INVOIC check and settlement",
        description = "The § 147 AO receipt register of every INVOIC a market partner sent, the answers it was given, the statements that decide a held invoice, and the webhook routes for the events invoicd emits.",
    ),
    tags(
        (name = "receipts", description = "Received invoices, their answers and their payment"),
        (name = "webhooks", description = "Webhook endpoints, deliveries, replay and the event feed"),
    ),
)]
pub struct ApiDoc;

/// Every `/api/v1` route invoicd serves, each with its document entry, and
/// the webhook routes of `subs`.
pub fn api(subs: &Subscriptions) -> OpenApiRouter<HandlerState> {
    OpenApiRouter::with_openapi(<ApiDoc as utoipa::OpenApi>::openapi())
        .routes(routes!(list_receipts))
        .routes(routes!(get_receipt))
        .routes(routes!(get_rechnung))
        .routes(routes!(confirm_payment))
        .routes(routes!(dispatch_answer))
        .routes(routes!(resolve_dispute))
        .routes(routes!(post_feststellungen))
        .routes(routes!(post_leistungsnachweis))
        .routes(routes!(list_disputes))
        .routes(routes!(list_overdue_remadv))
        .routes(routes!(get_zahlungsstatus))
        .merge(subs.router())
}

/// The document `GET /api/v1/openapi.json` serves.
///
/// Built over a lazy pool that never connects: the document depends on the
/// routes, not on a database. The pool needs a Tokio runtime to be created in.
#[must_use]
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .expect("a lazy pool opens no connection");
    let subs = subscriptions(pool, "unused", SubscriptionsConfig::default(), None, None);
    api(&subs).split_for_parts().1
}

/// The webhook module over invoicd's outbox: the ERP URL is the endpoint
/// `default`, signed with its secret.
#[must_use]
pub fn subscriptions(
    pool: PgPool,
    tenant: &str,
    cfg: SubscriptionsConfig,
    erp_url: Option<String>,
    erp_secret: Option<SecretString>,
) -> Subscriptions {
    Subscriptions::new(pool, <Invoicd as Daemon>::NAME, tenant, cfg)
        .with_default_endpoint(erp_url, erp_secret)
}

/// Build the domain router: the inbound webhook, the documented `/api/v1`
/// routes with their document, and the internal metrics.
pub fn router(state: HandlerState, subs: &Subscriptions) -> Router {
    let (api, doc) = api(subs).split_for_parts();
    Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/invoicd/metrics", get(metrics))
        .merge(api)
        .merge(mako_service::openapi::serve(doc))
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
    let (total, disputes, overdue, dead_letters) = tokio::join!(
        scalar("SELECT COUNT(*) FROM invoic_receipts WHERE tenant = $1"),
        scalar("SELECT COUNT(*) FROM invoic_receipts WHERE tenant = $1 AND outcome = 'Dispute'"),
        scalar(
            "SELECT COUNT(*) FROM invoic_receipts WHERE tenant = $1 \
             AND pay_by < now() + INTERVAL '3 days' AND dispatched_at IS NULL"
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

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
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

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
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

/// `GET /api/v1/receipts` — list receipts for this tenant.
#[utoipa::path(
    get,
    path = "/api/v1/receipts",
    tag = "receipts",
    params(
        ReceiptListQuery,
    ),
    responses(
        (status = 200, description = "Receipts, newest first", body = Vec<ReceiptSummary>),
        (status = 403, description = "Cedar `read-receipt` refused", body = ApiError),
    )
)]
async fn list_receipts(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<ReceiptListQuery>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "read-receipt", &state.tenant)?;
    match fetch_receipts(&state.pool, &state.tenant, &params).await {
        Ok(rows) => Ok(Json(rows).into_response()),
        Err(e) => Err(e.into()),
    }
}

/// `GET /api/v1/disputes` — the open exception queue.
#[utoipa::path(
    get,
    path = "/api/v1/disputes",
    tag = "receipts",
    responses(
        (status = 200, description = "Receipts whose outcome is Dispute, newest first", body = Vec<ReceiptSummary>),
        (status = 403, description = "Cedar `read-disputes` refused", body = ApiError),
    )
)]
async fn list_disputes(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "read-disputes", &state.tenant)?;
    let params = ReceiptListQuery {
        sender_mp_id: None,
        outcome: Some("Dispute".to_owned()),
        from: None,
        to: None,
        page: 0,
        size: 200,
    };
    match fetch_receipts(&state.pool, &state.tenant, &params).await {
        Ok(rows) => Ok(Json(rows).into_response()),
        Err(e) => Err(e.into()),
    }
}

/// `GET /api/v1/receipts/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/receipts/{id}",
    tag = "receipts",
    params(
        ("id" = uuid::Uuid, Path, description = "The receipt"),
    ),
    responses(
        (status = 200, description = "The receipt", body = ReceiptSummary),
        (status = 403, description = "Cedar `read-receipt` refused", body = ApiError),
        (status = 404, description = "No such receipt in this tenant", body = ApiError),
    )
)]
async fn get_receipt(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "read-receipt", &state.tenant)?;
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
        Ok(Some(r)) => Ok(Json(r).into_response()),
        Ok(None) => Err(ApiError::NotFound),
        Err(e) => Err(e.into()),
    }
}

/// `GET /api/v1/receipts/{id}/rechnung` — the invoice exactly as received.
#[utoipa::path(
    get,
    path = "/api/v1/receipts/{id}/rechnung",
    tag = "receipts",
    params(
        ("id" = uuid::Uuid, Path, description = "The receipt"),
    ),
    responses(
        (status = 200, description = "The BO4E Rechnung exactly as received, with its BO4E version and PID", body = serde_json::Value),
        (status = 403, description = "Cedar `read-receipt` refused", body = ApiError),
        (status = 404, description = "No such receipt in this tenant", body = ApiError),
    )
)]
async fn get_rechnung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "read-receipt", &state.tenant)?;
    let row: Result<Option<(serde_json::Value, String, i16)>, _> = sqlx::query_as(
        "SELECT rechnung, bo4e_version, pid FROM invoic_receipts WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some((rechnung, bo4e_version, pid))) => Ok(Json(
            serde_json::json!({ "rechnung": rechnung, "bo4e_version": bo4e_version, "pid": pid }),
        )
        .into_response()),
        Ok(None) => Err(ApiError::NotFound),
        Err(e) => Err(e.into()),
    }
}

/// `GET /api/v1/overdue-remadv`
///
/// Receipts whose Zahlungsziel is within three days and whose answer never went
/// out. An undispatched answer past the Zahlungsziel is both a market-process
/// breach and a § 147 AO gap, so this is the alert query: run it every 6 h and
/// alert when it is non-empty.
#[utoipa::path(
    get,
    path = "/api/v1/overdue-remadv",
    tag = "receipts",
    responses(
        (status = 200, description = "`count` and `items`: receipts within three days of their Zahlungsziel whose answer never went out", body = serde_json::Value),
        (status = 403, description = "Cedar `read-overdue-remadv` refused", body = ApiError),
    )
)]
async fn list_overdue_remadv(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "read-overdue-remadv", &state.tenant)?;
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
            Ok(Json(serde_json::json!({ "count": items.len(), "items": items })).into_response())
        }
        Err(e) => Err(e.into()),
    }
}

/// `GET /api/v1/zahlungsstatus/{malo_id}` — payment lifecycle per MaLo.
#[utoipa::path(
    get,
    path = "/api/v1/zahlungsstatus/{malo_id}",
    tag = "receipts",
    params(
        ("malo_id" = String, Path, description = "11-digit Marktlokations-ID"),
    ),
    responses(
        (status = 200, description = "The payment lifecycle of the Marktlokation\'s receipts, with counts per status", body = serde_json::Value),
        (status = 403, description = "Cedar `read-receipt` refused", body = ApiError),
    )
)]
async fn get_zahlungsstatus(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "read-receipt", &state.tenant)?;
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

    let rows = rows?;
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
    Ok(Json(serde_json::json!({
        "malo_id":            malo_id,
        "settled_count":      count("settled"),
        "pending_count":      count("pending"),
        "overdue_count":      count("overdue"),
        "undispatched_count": count("undispatched"),
        "items":              items,
    }))
    .into_response())
}

// ── Writes ────────────────────────────────────────────────────────────────────

/// `POST /api/v1/receipts/{id}/confirm-payment`
///
/// The ERP reports that the money moved. Closes the § 147 AO payment trail and
/// stops the receipt appearing in the overdue view.
#[utoipa::path(
    post,
    path = "/api/v1/receipts/{id}/confirm-payment",
    tag = "receipts",
    params(
        ("id" = uuid::Uuid, Path, description = "The receipt"),
    ),
    responses(
        (status = 204, description = "The payment is recorded"),
        (status = 403, description = "Cedar `write-receipt` refused", body = ApiError),
        (status = 404, description = "No such receipt, or it was confirmed already", body = ApiError),
    )
)]
async fn confirm_payment(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "write-receipt", &state.tenant)?;
    match pg::receipts::confirm_payment(&state.pool, id, &state.tenant).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT.into_response()),
        // The message says which of the two it can be; both are 404.
        Ok(false) => Err(ApiError::NotFound),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct ResolveDisputeBody {
    note: Option<String>,
}

/// `POST /api/v1/receipts/{id}/resolve-dispute`
///
/// Record that a dispute was settled out of band (a corrected invoice, a phone
/// call, a COMDIS). Moves `Dispute` → `Resolved` with the caller as the
/// decider and the operator's note. No market message goes out: the answer
/// REMADV was sent when the dispute was raised.
#[utoipa::path(
    post,
    path = "/api/v1/receipts/{id}/resolve-dispute",
    tag = "receipts",
    params(
        ("id" = uuid::Uuid, Path, description = "The receipt"),
    ),
    request_body(content = ResolveDisputeBody, description = "An optional note on how the dispute was settled"),
    responses(
        (status = 204, description = "Dispute → Resolved, with the caller as decider"),
        (status = 403, description = "Cedar `write-receipt` refused", body = ApiError),
        (status = 404, description = "No such receipt in Dispute", body = ApiError),
    )
)]
async fn resolve_dispute(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    body: Option<Json<ResolveDisputeBody>>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "write-receipt", &state.tenant)?;
    let note = body.as_ref().and_then(|Json(b)| b.note.as_deref());
    match pg::receipts::resolve_dispute(&state.pool, id, &state.tenant, claims.sub(), note).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT.into_response()),
        Ok(false) => Err(ApiError::NotFound),
        Err(e) => Err(e.into()),
    }
}

/// The body of `POST /api/v1/receipts/{id}/feststellungen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeststellungBody {
    feststellungen: pg::feststellungen::Feststellungen,
    /// Why — never empty: the statement replaces a check the tree asks for.
    begruendung: String,
}

/// `POST /api/v1/receipts/{id}/feststellungen`
///
/// State the answers a held invoice's walk could not read — Prüfschritt 310
/// (Leistung erbracht), 320 (Preis wie Angebot), 330 (Steuersatz) per
/// Positionsnummer, and 80 (Preisblatt-Version) — and walk it again in place.
/// A walk that now reaches a verdict is answered and announced as an
/// automatic one would be; one still open names what is missing. `409` when
/// the receipt is not held.
#[utoipa::path(
    post,
    path = "/api/v1/receipts/{id}/feststellungen",
    tag = "receipts",
    params(
        ("id" = uuid::Uuid, Path, description = "The receipt"),
    ),
    request_body(content = serde_json::Value, description = "`FeststellungBody`: the stated Prüfschritt answers and the Begründung"),
    responses(
        (status = 200, description = "The receipt walked again; `outcome` is its result", body = serde_json::Value),
        (status = 403, description = "Cedar `record-feststellung` refused", body = ApiError),
        (status = 409, description = "The receipt is not held for a statement", body = ApiError),
        (status = 422, description = "No Begründung, or no Prüfschritt stated", body = ApiError),
        (status = 502, description = "The walk could not run", body = ApiError),
    )
)]
async fn post_feststellungen(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<FeststellungBody>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "record-feststellung", &state.tenant)?;
    if body.begruendung.trim().is_empty() {
        return Err(ApiError::unprocessable("begruendung is required"));
    }
    if body.feststellungen.ist_leer() {
        return Err(ApiError::unprocessable(
            "feststellungen states no Prüfschritt",
        ));
    }
    let recorded = pg::feststellungen::feststellen(
        &state.pool,
        &state.tenant,
        id,
        &body.feststellungen,
        &body.begruendung,
        claims.sub(),
        time::OffsetDateTime::now_utc(),
    )
    .await?;
    if !recorded {
        return Err(ApiError::conflict("receipt is not held for a statement"));
    }
    let Some(gehalten) =
        pg::feststellungen::gehaltene_rechnung(&state.pool, &state.tenant, id).await?
    else {
        return Err(ApiError::conflict("receipt is not held for a statement"));
    };
    let outcome = crate::handler::erneut_pruefen(&state, &gehalten)
        .await
        .map_err(|e| ApiError::upstream("invoicd", anyhow::anyhow!(e)))?;
    Ok(Json(serde_json::json!({ "receipt_id": id, "outcome": outcome })).into_response())
}

/// `POST /api/v1/leistungsnachweise`
///
/// Record that the MSB performed — or did not perform — an ordered Leistung
/// for an Artikel-ID over a period: Prüfschritt 310 for every position it
/// covers. Every invoice of that MSB held on that order walks again at once;
/// one recorded before its invoice arrives is read when it does.
#[utoipa::path(
    post,
    path = "/api/v1/leistungsnachweise",
    tag = "receipts",
    request_body(content = serde_json::Value, description = "`Leistungsnachweis`: Rechnungssteller, Bestellung, Artikel-ID, period, whether it was performed, and the Nachweis"),
    responses(
        (status = 201, description = "Recorded; every held invoice of that order walked again (`erneut_geprueft`)", body = serde_json::Value),
        (status = 403, description = "Cedar `record-feststellung` refused", body = ApiError),
        (status = 422, description = "No Nachweis, or `von` after `bis`", body = ApiError),
    )
)]
async fn post_leistungsnachweis(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(body): Json<pg::feststellungen::Leistungsnachweis>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "record-feststellung", &state.tenant)?;
    if body.nachweis.trim().is_empty() {
        return Err(ApiError::unprocessable("nachweis is required"));
    }
    if body.von > body.bis {
        return Err(ApiError::unprocessable("von is after bis"));
    }
    pg::feststellungen::record_leistungsnachweis(&state.pool, &state.tenant, &body, claims.sub())
        .await?;
    let gehalten = pg::feststellungen::gehalten_fuer_bestellung(
        &state.pool,
        &state.tenant,
        &body.rechnungssteller_mp_id,
        &body.bestellung_ref,
    )
    .await?;
    let mut erneut = Vec::with_capacity(gehalten.len());
    for g in &gehalten {
        let outcome = crate::handler::erneut_pruefen(&state, g).await;
        erneut.push(serde_json::json!({
            "process_id": g.process_id,
            "outcome": outcome.as_ref().ok(),
            "fehler": outcome.as_ref().err(),
        }));
    }
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "erneut_geprueft": erneut })),
    )
        .into_response())
}

/// `POST /api/v1/receipts/{id}/dispatch-answer`
///
/// Re-send the market answer for a receipt whose automatic dispatch failed.
///
/// The routing key, the command and the payload all come from the receipt:
///
/// - **The routing key** is the EDIFACT message reference, which is what
///   `makod` correlates the answer by.
/// - **The command** is the answering PID's, from [`crate::routing`].
/// - **The payload** is the one recorded at the check — Antwortcode, EBD,
///   Befunde and Ablehnungsgrund — so the REMADV states what the automatic
///   answer would have.
///
/// Getting either wrong fails silently — `makod` accepts the command and only
/// the correlation misses — so neither is defaulted.
#[utoipa::path(
    post,
    path = "/api/v1/receipts/{id}/dispatch-answer",
    tag = "receipts",
    params(
        ("id" = uuid::Uuid, Path, description = "The receipt"),
    ),
    responses(
        (status = 200, description = "The answer command was accepted by makod", body = serde_json::Value),
        (status = 403, description = "Cedar `write-receipt` refused", body = ApiError),
        (status = 404, description = "No such receipt in this tenant", body = ApiError),
        (status = 409, description = "The answer went out already", body = ApiError),
        (status = 422, description = "The receipt has no answer to send", body = ApiError),
        (status = 502, description = "makod did not accept the command", body = ApiError),
    )
)]
async fn dispatch_answer(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Response> {
    authorize(&cedar, &claims, "write-receipt", &state.tenant)?;
    let target = match pg::receipts::dispatch_target(&state.pool, id, &state.tenant).await {
        Ok(Some(t)) => t,
        Ok(None) => return Err(ApiError::NotFound),
        Err(e) => return Err(e.into()),
    };

    if target.already_dispatched {
        return Err(ApiError::conflict("receipt already dispatched"));
    }
    if target.outcome == "Offen" {
        return Err(ApiError::unprocessable(
            "the Entscheidungsbaum left Prüfschritte unanswered — this invoice has no answer \
             to dispatch until a person states the missing facts",
        ));
    }
    let Some(invoice_ref) = target.invoice_ref else {
        return Err(ApiError::unprocessable(
            "receipt has no INVOIC message reference — the answer cannot be routed",
        ));
    };
    let Some(route) = route_for(target.pid as u32) else {
        return Err(ApiError::unprocessable(format!(
            "PID {} is not answered by this service",
            target.pid
        )));
    };

    let Some(payload) = target.antwort else {
        return Err(ApiError::unprocessable(
            "receipt has no recorded answer — it was never checked to an answer",
        ));
    };
    let command = if target.outcome == "Dispute" {
        route.reject
    } else {
        route.accept
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
                &state.tenant,
                target.process_id,
                time::OffsetDateTime::now_utc(),
            )
            .await
            {
                tracing::warn!(%e, process_id = %target.process_id, "invoicd: manual dispatch not recorded");
            }
            Ok(Json(serde_json::json!({
                "dispatched":  true,
                "process_id":  target.process_id,
                "pid":         target.pid,
                "command":     command,
                "invoice_ref": invoice_ref,
            }))
            .into_response())
        }
        Err(e) => Err(ApiError::upstream("makod", e)),
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
/// handler state and MCP server, and spawn the background workers.
pub async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
    // The auth posture is refused in `Daemon::validate`, before `build` runs.

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
        auto_dispute_threshold_raw: cfg.check.auto_dispute_threshold_raw()?,
        pool: pool.clone(),
        tenant: tenant.clone(),
        marktrolle: cfg.identity.marktrolle,
    };

    // The ERP URL is the webhook endpoint `default`, signed with its secret;
    // integrators register further endpoints under /api/v1/webhooks/endpoints.
    let subs = subscriptions(
        pool.clone(),
        &tenant,
        cfg.subscriptions.clone(),
        cfg.erp.webhook_url.clone(),
        erp_hmac_secret,
    );

    let mcp_state = Arc::new(crate::mcp_server::InvoicdMcpState {
        pool: pool.clone(),
        tenant: tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            oidc.clone(),
            Some(cedar.clone()),
            &tenant,
        ),
        policy: ctx.einrichtung.policy.clone(),
        subs: subs.clone(),
    });

    if cfg.erp.webhook_url.is_none() {
        tracing::warn!(
            "invoicd: no [erp] webhook_url — de.invoic.receipt.* and de.invoic.payment.overdue \
             events reach only endpoints registered under /api/v1/webhooks/endpoints; without \
             one the ERP does not learn that an invoice was settled, disputed or has passed \
             its Zahlungsziel"
        );
    }
    subs.spawn(ctx.shutdown.clone());
    crate::payment_overdue::spawn(pool.clone(), tenant.clone(), ctx.shutdown.clone());

    let expected_tenant = mako_service::oidc::ExpectedTenant(state.tenant.clone());
    Ok(router(state, &subs)
        // Read-only: GET /admin/einrichtung (Cedar `read-config`) and
        // GET /api/v1/policy (Cedar `read-policy`), from the boot report.
        .merge(mako_service::einrichtung::router(ctx.einrichtung.clone()))
        .layer(Extension(cedar))
        .layer(Extension(expected_tenant))
        .layer(Extension(oidc))
        .merge(crate::mcp_server::router(mcp_state, ctx.shutdown.clone())))
}
