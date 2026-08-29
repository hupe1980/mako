//! Dead-letter queue admin endpoints for the durable fan-out (F-003).
//!
//! Dead-lettering is a **status column** on `event_delivery`: a per-subscriber
//! delivery with `dead_lettered_at IS NOT NULL` has exhausted all retry
//! attempts. Operators use these endpoints to inspect failures, requeue them
//! for the worker to redeliver, and discard entries after investigation.
//!
//! All endpoints require the `manage-fanout` Cedar action.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | `GET`    | `/admin/fanout/dlq` | List dead-lettered deliveries (newest first, paged) |
//! | `POST`   | `/admin/fanout/dlq/{event_id}/{subscriber_id}/retry` | Requeue for the worker to redeliver |
//! | `DELETE` | `/admin/fanout/dlq/{event_id}/{subscriber_id}` | Discard the delivery without retry |

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use std::sync::Arc;
use time::OffsetDateTime;

use super::{Claims, Tenant};

#[derive(Debug, Serialize)]
pub struct DlqEntry {
    pub event_id: String,
    pub subscriber_id: String,
    pub webhook_url: String,
    pub ce_type: String,
    pub event_body: serde_json::Value,
    pub attempts: i16,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub dead_lettered_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct DlqListQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_size")]
    pub size: u32,
}
fn default_page() -> u32 {
    0
}
fn default_size() -> u32 {
    50
}

fn map_row(r: &sqlx::postgres::PgRow) -> Result<DlqEntry, sqlx::Error> {
    Ok(DlqEntry {
        event_id: r.try_get("event_id")?,
        subscriber_id: r.try_get("subscriber_id")?,
        webhook_url: r.try_get("webhook_url")?,
        ce_type: r.try_get("ce_type")?,
        event_body: r.try_get("envelope")?,
        attempts: r.try_get("attempts")?,
        last_error: r.try_get("last_error")?,
        dead_lettered_at: r.try_get("dead_lettered_at")?,
    })
}

fn deny_forbidden() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"error": "Forbidden", "detail": "manage-fanout action required"})),
    )
        .into_response()
}

/// `GET /admin/fanout/dlq` — list dead-lettered deliveries.
pub async fn list_dlq(
    Extension(pool): Extension<PgPool>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Query(q): Query<DlqListQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "manage-fanout", &tenant)
        .is_err()
    {
        return deny_forbidden();
    }

    let offset = i64::from(q.page * q.size);
    let limit = i64::from(q.size);
    let sql = "SELECT d.event_id, d.subscriber_id, d.webhook_url, d.attempts, d.last_error, \
                      d.dead_lettered_at, l.ce_type, l.envelope \
               FROM event_delivery d \
               JOIN event_log l ON l.event_id = d.event_id \
               WHERE d.dead_lettered_at IS NOT NULL \
               ORDER BY d.dead_lettered_at DESC LIMIT $1 OFFSET $2";
    match sqlx::query(sql)
        .bind(limit)
        .bind(offset)
        .fetch_all(&pool)
        .await
    {
        Ok(rows) => Json(
            rows.iter()
                .filter_map(|r| map_row(r).ok())
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "dlq: list failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "db error"})),
            )
                .into_response()
        }
    }
}

/// `POST /admin/fanout/dlq/{event_id}/{subscriber_id}/retry` — requeue for the
/// worker to redeliver (clears the dead-letter mark and resets the attempt
/// counter; the fan-out worker picks it up on its next cycle).
pub async fn retry_dlq_entry(
    Extension(pool): Extension<PgPool>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path((event_id, subscriber_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "manage-fanout", &tenant)
        .is_err()
    {
        return deny_forbidden();
    }

    match sqlx::query(
        "UPDATE event_delivery
            SET dead_lettered_at = NULL, attempts = 0, next_attempt_at = now(), last_error = NULL
          WHERE event_id = $1 AND subscriber_id = $2 AND dead_lettered_at IS NOT NULL
          RETURNING event_id",
    )
    .bind(&event_id)
    .bind(&subscriber_id)
    .fetch_optional(&pool)
    .await
    {
        Ok(Some(_)) => {
            tracing::info!(%event_id, %subscriber_id, "dlq: entry requeued for redelivery");
            (StatusCode::OK, Json(serde_json::json!({"requeued": true}))).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found or not dead-lettered"})),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "dlq: requeue failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "db error"})),
            )
                .into_response()
        }
    }
}

/// `DELETE /admin/fanout/dlq/{event_id}/{subscriber_id}` — discard without retry.
pub async fn delete_dlq_entry(
    Extension(pool): Extension<PgPool>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path((event_id, subscriber_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "manage-fanout", &tenant)
        .is_err()
    {
        return deny_forbidden();
    }

    match sqlx::query(
        "DELETE FROM event_delivery
          WHERE event_id = $1 AND subscriber_id = $2 AND dead_lettered_at IS NOT NULL
          RETURNING event_id",
    )
    .bind(&event_id)
    .bind(&subscriber_id)
    .fetch_optional(&pool)
    .await
    {
        Ok(Some(_)) => {
            tracing::info!(%event_id, %subscriber_id, "dlq: entry discarded by operator");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found or not dead-lettered"})),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "dlq: delete failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "db error"})),
            )
                .into_response()
        }
    }
}
