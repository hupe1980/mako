//! Axum router for `invoicd`.
//!
//! Routes:
//! - `POST /webhook`                                  — inbound MarktEvent CloudEvents from `marktd` (HMAC-auth)
//! - `GET  /api/v1/receipts`                          — query INVOIC receipts (OIDC+Cedar)
//! - `GET  /api/v1/receipts/:id`                      — get a single receipt (OIDC+Cedar)
//! - `POST /api/v1/receipts/:id/confirm-payment`      — ERP confirms payment received; sets `payment_confirmed_at` (§22 MessZV)
//! - `GET  /api/v1/disputes`                          — list open disputes (OIDC+Cedar)
//! - `GET  /api/v1/overdue-remadv`                    — receipts approaching `pay_by` without dispatch
//! - `GET  /api/v1/zahlungsstatus/{malo_id}`          — payment status per MaLo (pending / settled / overdue)
//! - `POST /api/v1/selbstausstellen/{malo_id}`        — trigger outbound selbstausgestellt INVOIC 31006 (M16)
//! - `GET  /metrics`                                  — Prometheus metrics (no auth, internal only)
//! - `GET  /health/live`                              — liveness probe (always 200)
//! - `GET  /health/ready`                             — readiness probe (200 OK)

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
        .route(
            "/api/v1/receipts/{id}/confirm-payment",
            post(confirm_payment),
        )
        .route(
            "/api/v1/receipts/{id}/dispatch-remadv",
            post(dispatch_remadv),
        )
        .route(
            "/api/v1/receipts/{id}/resolve-dispute",
            post(resolve_dispute_endpoint),
        )
        .route("/api/v1/receipts/{id}/rechnung", get(get_rechnung))
        .route("/api/v1/disputes", get(list_disputes))
        .route("/api/v1/overdue-remadv", get(list_overdue_remadv))
        .route("/api/v1/zahlungsstatus/{malo_id}", get(get_zahlungsstatus))
        .route(
            "/api/v1/selbstausstellen/{malo_id}",
            post(post_selbstausstellen),
        )
        .route("/metrics", get(metrics))
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(health_ready))
        .with_state(state)
}

async fn health_ready(State(_state): State<HandlerState>) -> impl IntoResponse {
    StatusCode::OK
}

/// `GET /metrics` — Prometheus-compatible operational metrics.
/// No authentication required; restrict network access at the ingress layer.
async fn metrics(State(state): State<HandlerState>) -> impl IntoResponse {
    let mut out = String::with_capacity(512);

    let (receipt_count, dispute_count) = if let Some(pool) = state.pool.as_ref() {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invoic_receipts")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        let disputes: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM invoic_receipts WHERE outcome = 'Dispute'")
                .fetch_one(pool)
                .await
                .unwrap_or(0);
        (total, disputes)
    } else {
        (0, 0)
    };

    let overdue: i64 = if let Some(pool) = state.pool.as_ref() {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM invoic_receipts \
             WHERE pay_by < now() + INTERVAL '3 days' AND dispatched_at IS NULL",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0)
    } else {
        0
    };

    out.push_str("# HELP invoicd_receipts_total Total INVOIC receipts persisted (§22 MessZV).\n");
    out.push_str("# TYPE invoicd_receipts_total gauge\n");
    out.push_str(&format!("invoicd_receipts_total {receipt_count}\n"));
    out.push_str("# HELP invoicd_disputes_total Receipts with Dispute outcome.\n");
    out.push_str("# TYPE invoicd_disputes_total gauge\n");
    out.push_str(&format!("invoicd_disputes_total {dispute_count}\n"));
    out.push_str("# HELP invoicd_overdue_remadv_total Receipts approaching pay_by without REMADV dispatch.\n");
    out.push_str("# TYPE invoicd_overdue_remadv_total gauge\n");
    out.push_str(&format!("invoicd_overdue_remadv_total {overdue}\n"));

    // Per-PID breakdowns — useful for detecting volume spikes on specific billing PIDs.
    if let Some(pool) = state.pool.as_ref() {
        let pid_rows: Vec<(i16, String, i64)> = sqlx::query_as(
            r"SELECT pid, outcome, COUNT(*) AS cnt
              FROM invoic_receipts
              WHERE tenant = $1
              GROUP BY pid, outcome",
        )
        .bind(&state.tenant)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        if !pid_rows.is_empty() {
            out.push_str(
                "# HELP invoicd_receipts_by_pid_outcome Receipts broken down by PID and outcome.\n",
            );
            out.push_str("# TYPE invoicd_receipts_by_pid_outcome gauge\n");
            for (pid, outcome, cnt) in &pid_rows {
                out.push_str(&format!(
                    "invoicd_receipts_by_pid_outcome{{pid=\"{pid}\",outcome=\"{outcome}\"}} {cnt}\n"
                ));
            }
        }
    }

    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
}

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

/// `POST /api/v1/receipts/{id}/confirm-payment`
///
/// Called by the ERP when it confirms that payment for an invoice has been
/// received (bank transfer confirmed).  Sets `payment_confirmed_at = now()`.
///
/// This closes the §22 MessZV payment audit trail: every `invoic_receipt`
/// record transitions from `dispatched` → `payment_confirmed` state once
/// the ERP sends this callback.
///
/// Request body: optional `{ "reference": "bank-transfer-id" }` (ignored).
///
/// Response: `204 No Content` on success; `404` if receipt not found.
async fn confirm_payment(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-receipt", &state.tenant) {
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

    let result = sqlx::query(
        r"UPDATE invoic_receipts
          SET payment_confirmed_at = now()
          WHERE id = $1 AND tenant = $2 AND payment_confirmed_at IS NULL",
    )
    .bind(id)
    .bind(&state.tenant)
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "receipt not found or already confirmed" })),
        )
            .into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/receipts/{id}/dispatch-remadv`
///
/// Manually trigger a REMADV dispatch for a receipt whose auto-dispatch failed
/// or was never attempted.  Useful when `dispatched_at IS NULL` and the
/// Zahlungsziel is approaching.
///
/// The command dispatched depends on the current `outcome`:
/// - `Ok` / `Warn` / `AcceptedPartial` → REMADV 33001 (Zahlungsavis / acceptance)
/// - `Dispute` → REMADV 33002 (dispute re-dispatch)
///
/// Returns `409 Conflict` when the receipt was already dispatched.
async fn dispatch_remadv(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-receipt", &state.tenant) {
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

    // Fetch the receipt.
    let row = sqlx::query(
        r"SELECT process_id, pid, outcome, dispatched_at
          FROM invoic_receipts
          WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(pool)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "receipt not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    use sqlx::Row;
    let already_dispatched = row
        .try_get::<Option<time::OffsetDateTime>, _>("dispatched_at")
        .ok()
        .flatten()
        .is_some();
    if already_dispatched {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "receipt already dispatched" })),
        )
            .into_response();
    }

    let process_id: uuid::Uuid = match row.try_get("process_id") {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let pid: i16 = row.try_get("pid").unwrap_or(0);
    let outcome: String = row.try_get("outcome").unwrap_or_default();

    let (cmd_name, is_dispute) = match outcome.as_str() {
        "Dispute" => ("gpke.abrechnung.ablehnen", true),
        _ => ("gpke.abrechnung.annehmen", false),
    };

    let idem = uuid::Uuid::new_v5(&process_id, b"manual-dispatch").to_string();
    let payload = if is_dispute {
        serde_json::json!({ "invoice_ref": process_id.to_string(), "ablehnungsgrund": "Manuell ausgelöst (Operator)" })
    } else {
        serde_json::json!({ "invoice_ref": process_id.to_string() })
    };

    let cmd = mako_markt::makod_client::ForwardCommand {
        marktrolle: None,
        command: cmd_name.to_owned(),
        malo_id: None,
        melo_id: None,
        payload,
    };

    match state.makod.post_command(&idem, &cmd).await {
        Ok(_) => {
            let _ =
                pg::receipts::mark_dispatched(pool, process_id, time::OffsetDateTime::now_utc())
                    .await;
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "dispatched": true,
                    "process_id": process_id,
                    "pid": pid,
                    "command": cmd_name,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("makod dispatch failed: {e}") })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/receipts/{id}/resolve-dispute`
///
/// Record the resolution of a disputed receipt after operator negotiation (e.g.
/// via phone, COMDIS, or corrected re-invoice).  Transitions `outcome` from
/// `'Dispute'` to `'Resolved'` and stores an optional operator note.
///
/// Request body (JSON):
/// ```json
/// { "note": "NB confirmed pricing error; corrected invoice received PID 31001 on 2026-08-01" }
/// ```
///
/// Returns `404` if not found or not currently in `'Dispute'` state.
#[derive(serde::Deserialize)]
struct ResolveDisputeBody {
    note: Option<String>,
}

async fn resolve_dispute_endpoint(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    body: Option<Json<ResolveDisputeBody>>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-receipt", &state.tenant) {
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

    let note = body.as_ref().and_then(|b| b.note.as_deref());
    match pg::receipts::resolve_dispute(pool, id, &state.tenant, note).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "receipt not found or not in Dispute state" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/receipts/{id}/rechnung`
///
/// Retrieve the full BO4E `Rechnung` JSON stored for a receipt.  Useful for
/// debugging disputes: the full invoice as received is returned without
/// modification, alongside the `bo4e_version` it was stored under.
///
/// Restricted to callers with the `read-receipt` Cedar action.
async fn get_rechnung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
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

    let row = sqlx::query(
        r"SELECT rechnung, bo4e_version, pid FROM invoic_receipts WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(pool)
    .await;

    match row {
        Ok(Some(r)) => {
            use sqlx::Row;
            let rechnung: serde_json::Value = r.try_get("rechnung").unwrap_or_default();
            let bo4e_version: String = r.try_get("bo4e_version").unwrap_or_default();
            let pid: i16 = r.try_get("pid").unwrap_or(0);
            Json(serde_json::json!({ "rechnung": rechnung, "bo4e_version": bo4e_version, "pid": pid })).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/zahlungsstatus/{malo_id}`
///
/// Returns the payment status for all INVOIC receipts linked to a MaLo,
/// grouped by status:
///
/// - `pending`  — REMADV dispatched but `payment_confirmed_at IS NULL` and
///   `pay_by` is still in the future.
/// - `overdue`  — REMADV dispatched, `pay_by` has passed, `payment_confirmed_at IS NULL`.
/// - `settled`  — `payment_confirmed_at IS NOT NULL`.
///
/// Uses the indexed `malo_id` column (migration 0002) — no JSONB scan.
async fn get_zahlungsstatus(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
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

    // Use the indexed `malo_id` column (migration 0002) — avoids full JSONB scan.
    // Includes all outcome states so disputes and resolved receipts are visible.
    let rows = sqlx::query(
        r"SELECT id, process_id, pid, sender_mp_id, outcome, pay_by,
                 dispatched_at, payment_confirmed_at, received_at
          FROM invoic_receipts
          WHERE tenant = $1 AND malo_id = $2
          ORDER BY received_at DESC
          LIMIT 100",
    )
    .bind(&state.tenant)
    .bind(&malo_id)
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    use sqlx::Row;
                    let pay_by = r
                        .try_get::<time::OffsetDateTime, _>("pay_by")
                        .ok();
                    let dispatched = r
                        .try_get::<Option<time::OffsetDateTime>, _>("dispatched_at")
                        .ok()
                        .flatten();
                    let confirmed = r
                        .try_get::<Option<time::OffsetDateTime>, _>("payment_confirmed_at")
                        .ok()
                        .flatten();

                    let zahlungsstatus = if confirmed.is_some() {
                        "settled"
                    } else if dispatched.is_some()
                        && pay_by.is_some_and(|d| d < time::OffsetDateTime::now_utc())
                    {
                        "overdue"
                    } else if dispatched.is_some() {
                        "pending"
                    } else {
                        "undispatched"
                    };

                    let fmt = |t: time::OffsetDateTime| {
                        use time::format_description::well_known::Rfc3339;
                        t.format(&Rfc3339).ok()
                    };

                    serde_json::json!({
                        "id":                     r.try_get::<uuid::Uuid, _>("id").ok(),
                        "process_id":             r.try_get::<uuid::Uuid, _>("process_id").ok(),
                        "pid":                    r.try_get::<i16, _>("pid").ok(),
                        "sender_mp_id":           r.try_get::<String, _>("sender_mp_id").ok(),
                        "zahlungsstatus":         zahlungsstatus,
                        "pay_by":                 pay_by.and_then(fmt),
                        "dispatched_at":          dispatched.and_then(fmt),
                        "payment_confirmed_at":   confirmed.and_then(fmt),
                        "received_at":            r.try_get::<time::OffsetDateTime, _>("received_at").ok().and_then(fmt),
                    })
                })
                .collect();

            let overdue_count = items
                .iter()
                .filter(|i| i.get("zahlungsstatus").and_then(|v| v.as_str()) == Some("overdue"))
                .count();
            let pending_count = items
                .iter()
                .filter(|i| i.get("zahlungsstatus").and_then(|v| v.as_str()) == Some("pending"))
                .count();
            let settled_count = items
                .iter()
                .filter(|i| i.get("zahlungsstatus").and_then(|v| v.as_str()) == Some("settled"))
                .count();

            Json(serde_json::json!({
                "malo_id":        malo_id,
                "overdue_count":  overdue_count,
                "pending_count":  pending_count,
                "settled_count":  settled_count,
                "items":          items,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── BO4E conversion (service-layer concern) ───────────────────────────────────

/// Convert a `GridInvoice` domain result into a BO4E `Rechnung`.
///
/// `grid-billing` has no rubo4e dependency; this function owns the mapping.
fn grid_billing_into_rechnung(invoice: &grid_billing::GridInvoice) -> rubo4e::current::Rechnung {
    use rubo4e::current::{Betrag, Menge, Mengeneinheit, Preis, Rechnungsposition, Zeitraum};

    let lz = Zeitraum {
        startdatum: Some(invoice.period_from),
        enddatum: Some(invoice.period_to),
        ..Default::default()
    };

    let positions: Vec<Rechnungsposition> = invoice
        .positions
        .iter()
        .map(|p| {
            let einheit = match p.unit {
                grid_billing::QuantityUnit::Kwh => Some(Mengeneinheit::Kwh),
                grid_billing::QuantityUnit::Kw => Some(Mengeneinheit::Kw),
                grid_billing::QuantityUnit::Monat => Some(Mengeneinheit::Monat),
            };
            Rechnungsposition {
                positionsnummer: Some(p.number as i64),
                positionstext: Some(p.text.clone()),
                lieferungszeitraum: Some(lz.clone()),
                positions_menge: Some(Menge {
                    wert: Some(p.quantity),
                    einheit,
                    ..Default::default()
                }),
                einzelpreis: Some(Preis {
                    wert: Some(p.unit_price_eur.round_dp(6)),
                    ..Default::default()
                }),
                gesamtpreis: Some(Betrag {
                    wert: Some(p.net_eur.round_dp(5)),
                    ..Default::default()
                }),
                ..Default::default()
            }
        })
        .collect();

    rubo4e::current::Rechnung {
        rechnungsnummer: Some(invoice.rechnungsnummer.clone()),
        rechnungsdatum: Some(invoice.invoice_date),
        faelligkeitsdatum: Some(invoice.due_date),
        rechnungsperiode: Some(lz),
        gesamtnetto: Some(Betrag {
            wert: Some(invoice.total_eur),
            ..Default::default()
        }),
        rechnungspositionen: Some(positions),
        ..Default::default()
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

    // ── Step 1: Fetch MeterBillingPeriod from edmd ───────────────────────────
    // Required for RLM (Leistungspreis) and Gas (Brennwert/Zustandszahl).
    let Some(ref edmd_url) = state.edmd_url else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "edmd not configured — add [edmd] url to invoicd.toml for PID 31006"
            })),
        )
            .into_response();
    };

    let billing_period_url = format!(
        "{edmd_url}/api/v1/billing-period/{malo_id}?from={}&to={}",
        body.period_from, body.period_to
    );
    let mut req = state.http_client.get(&billing_period_url);
    if let Some(ref api_key) = state.edmd_api_key {
        use secrecy::ExposeSecret as _;
        req = req.bearer_auth(api_key.expose_secret());
    }
    let billing_period: mako_edm::domain::MeterBillingPeriod = match req.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json().await {
            Ok(bp) => bp,
            Err(e) => {
                tracing::warn!(%e, malo_id, "invoicd: selbstausstellen: failed to parse MeterBillingPeriod from edmd");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": format!("edmd parse error: {e}") })),
                )
                    .into_response();
            }
        },
        Ok(resp) => {
            let status = resp.status();
            tracing::warn!(%status, malo_id, "invoicd: selbstausstellen: edmd returned non-2xx");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("edmd returned {status}") })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::warn!(%e, malo_id, "invoicd: selbstausstellen: edmd unreachable");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("edmd unreachable: {e}") })),
            )
                .into_response();
        }
    };

    // ── Step 2: Fetch PreisblattNetznutzung from marktd ───────────────────────
    let sheet = state
        .preisblatt_client
        .get_preisblatt(&body.nb_mp_id, period_from)
        .await
        .ok()
        .flatten();
    let Some(sheet) = sheet else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("no PreisblattNetznutzung for NB {} on {period_from}", body.nb_mp_id)
            })),
        )
            .into_response();
    };

    // ── Step 3: Extract tariff params from PreisblattNetznutzung ─────────────
    // Find Arbeitspreis (Leistungstyp::ArbeitspreisWirkarbeit) from Preisblatt.
    use rubo4e::current::Leistungstyp;

    let arbeitspreis_ct = sheet
        .preispositionen
        .iter()
        .flatten()
        .find(|pos| {
            pos.leistungstyp
                .as_ref()
                .is_some_and(|lt| *lt == Leistungstyp::ArbeitspreisWirkarbeit)
        })
        .and_then(|pos| pos.preisstaffeln.as_ref())
        .and_then(|staffeln| staffeln.first())
        .and_then(|s| s.preis);

    let Some(arbeitspreis_ct) = arbeitspreis_ct else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "PreisblattNetznutzung has no ArbeitspreisWirkarbeit position — cannot generate Rechnung"
            })),
        )
            .into_response();
    };

    // Find Leistungspreis if present (RLM only).
    let leistungspreis_eur = sheet
        .preispositionen
        .iter()
        .flatten()
        .find(|pos| {
            pos.leistungstyp
                .as_ref()
                .is_some_and(|lt| *lt == Leistungstyp::LeistungspreisWirkleistung)
        })
        .and_then(|pos| pos.preisstaffeln.as_ref())
        .and_then(|staffeln| staffeln.first())
        .and_then(|s| s.preis);

    // ── Step 4: Build NneInput and generate Rechnung via grid-billing ─────
    use time::OffsetDateTime;

    let invoice_date = OffsetDateTime::now_utc().date();
    // Standard Zahlungsziel: 30 days from invoice date.
    let due_date = invoice_date + time::Duration::days(30);
    let rechnungsnummer = format!(
        "SELBST-{}-{}-{}",
        state.tenant,
        malo_id,
        invoice_date.to_string().replace('-', "")
    );

    let input = grid_billing::NneInput {
        malo_id: malo_id.clone(),
        nb_mp_id: body.nb_mp_id.clone(),
        lf_mp_id: state.tenant.clone(), // LF is selbstaussteller (= our own tenant)
        rechnungsnummer,
        period_from,
        period_to,
        invoice_date,
        due_date,
        arbeitsmenge_kwh: billing_period.arbeitsmenge_kwh,
        arbeitspreis_ct_per_kwh: arbeitspreis_ct,
        // §14a Modul 2 ToU: populate from MeterBillingPeriod when available.
        // invoicd selbstausstellen uses flat rate; ToU billing is done by netzbilanzd.
        arbeitsmenge_ht_kwh: billing_period.arbeitsmenge_ht_kwh,
        arbeitspreis_ht_ct_per_kwh: None, // ToU prices come from PreisblattNetznutzung; not looked up here
        arbeitsmenge_nt_kwh: billing_period.arbeitsmenge_nt_kwh,
        arbeitspreis_nt_ct_per_kwh: None,
        spitzenleistung_kw: billing_period.spitzenleistung_kw,
        leistungspreis_eur_per_kw: if billing_period.spitzenleistung_kw.is_some() {
            leistungspreis_eur
        } else {
            None
        },
        ka_satz_ct_per_kwh: None, // KA lookup not implemented; ERP can add via COMDIS
    };

    let billing_result = match grid_billing::calculate_nne_invoice(&input) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, malo_id, "invoicd: selbstausstellen: invoice generation failed");
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invoice generation error: {e}") })),
            )
                .into_response();
        }
    };

    // Convert domain invoice to BO4E Rechnung (service-layer concern; grid-billing is BO4E-free)
    let rechnung = grid_billing_into_rechnung(&billing_result);
    let rechnung_json = serde_json::to_value(&rechnung).unwrap_or_default();

    tracing::info!(
        malo_id = %malo_id,
        nb_mp_id = %body.nb_mp_id,
        period_from = %period_from,
        period_to = %period_to,
        total_eur = %billing_result.total_eur,
        "invoicd: selbstausstellen 31006 — full Rechnung generated"
    );

    // ── Step 5: Persist as Dispatched (§22 MessZV) ───────────────────────────
    let process_id = uuid::Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();

    let row = pg::ReceiptRow {
        process_id,
        pid: 31006,
        direction: "Outbound".to_owned(),
        sender_mp_id: state.tenant.clone(),
        receiver_gln: body.nb_mp_id.clone(),
        malo_id: Some(malo_id.clone()),
        rechnung: rechnung_json, // real BO4E Rechnung (not a placeholder)
        bo4e_version: "v202607.0.0".to_owned(),
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

    // ── Step 6: Dispatch to makod ─────────────────────────────────────────────
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
            "total_eur": billing_result.total_eur.to_string(),
            "rechnung": rechnung,
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
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "process_id": accepted.process_id,
                    "malo_id": malo_id,
                    "nb_mp_id": body.nb_mp_id,
                    "period_from": body.period_from,
                    "period_to": body.period_to,
                    "total_eur": billing_result.total_eur.to_string(),
                    "outcome": "Dispatched",
                })),
            )
                .into_response()
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
    /// Optional ERP webhook URL for `de.invoic.receipt.*` CloudEvents.
    pub erp_webhook_url: Option<String>,
    /// Optional HMAC-SHA256 secret for signing outbound ERP webhook requests.
    pub erp_hmac_secret: Option<SecretString>,
    /// `edmd` base URL for `MeterBillingPeriod` lookup in selbstausstellen.
    /// When `None`, `POST /api/v1/selbstausstellen` returns 503.
    pub edmd_url: Option<String>,
    /// `edmd` Bearer token.
    pub edmd_api_key: Option<SecretString>,
    /// OIDC verifier.  Use [`OidcVerifier::disabled`] in dev/test.
    pub oidc: OidcVerifier,
    /// Cedar ABAC enforcer loaded from `policies/invoicd.cedar`.
    pub cedar: Arc<CedarEnforcer>,
    /// MCP server auth config (API-key fallback + optional per-named-key identity).
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
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
        // Schema must be applied manually — see migrations/0001_initial.sql for DDL.
        tracing::info!("invoicd: database connected");
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
        erp_webhook_url: cfg.erp_webhook_url.clone(),
        erp_hmac_secret: cfg.erp_hmac_secret.clone(),
        http_client: mako_service::http::default_client(),
        edmd_url: cfg.edmd_url.clone(),
        edmd_api_key: cfg.edmd_api_key.clone(),
    };

    // ── MCP state ─────────────────────────────────────────────────────────────
    let mcp_state = pool.as_ref().map(|p| {
        Arc::new(crate::mcp_server::InvoicdMcpState {
            pool: p.clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                &cfg.mcp,
                cfg.oidc.clone(),
                Some(cfg.cedar.clone()),
                &cfg.tenant,
            ),
        })
    });

    // Spawn the ERP outbox worker when both pool and erp_webhook_url are configured.
    // The worker retries failed ERP notifications with exponential backoff.
    if let (Some(db_pool), Some(erp_url)) = (&pool, &cfg.erp_webhook_url) {
        crate::erp_outbox::spawn(
            db_pool.clone(),
            cfg.tenant.clone(),
            erp_url.clone(),
            cfg.erp_hmac_secret.clone(),
            cfg.shutdown.clone(),
        );

        // Spawn the payment-overdue worker (polls every 6 h).
        // Emits `de.invoic.payment.overdue` when `pay_by` has passed
        // without `payment_confirmed_at` being set — closes §22 MessZV dunning gap.
        crate::payment_overdue::spawn(
            db_pool.clone(),
            cfg.tenant.clone(),
            erp_url.clone(),
            cfg.erp_hmac_secret.clone(),
            cfg.shutdown.clone(),
        );
    }

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
