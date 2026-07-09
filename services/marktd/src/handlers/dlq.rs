//! Dead-letter queue admin endpoints for the fanout worker (F-003).
//!
//! The `fanout_dlq` table stores events that exhausted all delivery retry
//! attempts. Operators use these endpoints to inspect failures, trigger
//! manual retries, and discard entries after investigation.
//!
//! All endpoints require the `manage-fanout` Cedar action.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | `GET` | `/admin/fanout/dlq` | List unresolved DLQ entries (newest first, paged) |
//! | `POST` | `/admin/fanout/dlq/{id}/retry` | Re-deliver and mark resolved on success |
//! | `DELETE` | `/admin/fanout/dlq/{id}` | Discard entry without retry |

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::cloudevents::compute_signature;
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Claims, TenantGln};

#[derive(Debug, Serialize)]
pub struct DlqEntry {
    pub id: Uuid,
    pub subscriber_id: String,
    pub webhook_url: String,
    pub event_type: String,
    pub event_body: serde_json::Value,
    pub attempts: i32,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub failed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct DlqListQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_size")]
    pub size: u32,
    #[serde(default)]
    pub include_resolved: bool,
}
fn default_page() -> u32 {
    0
}
fn default_size() -> u32 {
    50
}

fn map_row(r: &sqlx::postgres::PgRow) -> Result<DlqEntry, sqlx::Error> {
    Ok(DlqEntry {
        id: r.try_get("id")?,
        subscriber_id: r.try_get("subscriber_id")?,
        webhook_url: r.try_get("webhook_url")?,
        event_type: r.try_get("event_type")?,
        event_body: r.try_get("event_body")?,
        attempts: r.try_get("attempts")?,
        last_error: r.try_get("last_error")?,
        failed_at: r.try_get("failed_at")?,
        resolved_at: r.try_get("resolved_at")?,
    })
}

fn deny_forbidden() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"error": "Forbidden", "detail": "manage-fanout action required"})),
    )
        .into_response()
}

/// `GET /admin/fanout/dlq`
pub async fn list_dlq(
    Extension(pool): Extension<PgPool>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
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
    let sql = if q.include_resolved {
        "SELECT id,subscriber_id,webhook_url,event_type,event_body,attempts,last_error,failed_at,resolved_at \
         FROM fanout_dlq ORDER BY failed_at DESC LIMIT $1 OFFSET $2"
    } else {
        "SELECT id,subscriber_id,webhook_url,event_type,event_body,attempts,last_error,failed_at,resolved_at \
         FROM fanout_dlq WHERE resolved_at IS NULL ORDER BY failed_at DESC LIMIT $1 OFFSET $2"
    };
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

/// `POST /admin/fanout/dlq/{id}/retry` — re-deliver and mark resolved on success.
pub async fn retry_dlq_entry(
    Extension(pool): Extension<PgPool>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Extension(http): Extension<reqwest::Client>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "manage-fanout", &tenant)
        .is_err()
    {
        return deny_forbidden();
    }

    let row = match sqlx::query(
        "SELECT subscriber_id,webhook_url,event_body FROM fanout_dlq WHERE id=$1 AND resolved_at IS NULL"
    ).bind(id).fetch_optional(&pool).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found or already resolved"}))).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "dlq: fetch for retry failed");
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "db error"}))).into_response();
        }
    };

    let subscriber_id: String = row.try_get("subscriber_id").unwrap_or_default();
    let webhook_url: String = row.try_get("webhook_url").unwrap_or_default();
    let event_body: serde_json::Value =
        row.try_get("event_body").unwrap_or(serde_json::Value::Null);

    let body = match serde_json::to_vec(&event_body) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("serialize: {e}")})),
            )
                .into_response();
        }
    };

    let secret: Option<String> =
        sqlx::query_scalar("SELECT webhook_secret FROM subscriptions WHERE subscriber_id=$1")
            .bind(&subscriber_id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .flatten();

    let sig = secret
        .as_deref()
        .map(|s| compute_signature(s.as_bytes(), &body));
    let mut req = http
        .post(&webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .body(body);
    if let Some(sig) = &sig {
        req = req.header("X-Mako-Signature", sig);
    }

    match req.send().await {
        Ok(r) if r.status().is_success() => {
            let _ = sqlx::query("UPDATE fanout_dlq SET resolved_at=now() WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await;
            tracing::info!(dlq_id = %id, subscriber_id = %subscriber_id, "dlq: retry succeeded");
            (
                StatusCode::OK,
                Json(serde_json::json!({"retried": true, "resolved": true})),
            )
                .into_response()
        }
        Ok(r) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": format!("HTTP {}", r.status().as_u16())})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `DELETE /admin/fanout/dlq/{id}` — discard without retry.
pub async fn delete_dlq_entry(
    Extension(pool): Extension<PgPool>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "manage-fanout", &tenant)
        .is_err()
    {
        return deny_forbidden();
    }

    match sqlx::query(
        "UPDATE fanout_dlq SET resolved_at=now() WHERE id=$1 AND resolved_at IS NULL RETURNING id",
    )
    .bind(id)
    .fetch_optional(&pool)
    .await
    {
        Ok(Some(_)) => {
            tracing::info!(dlq_id = %id, "dlq: entry discarded by operator");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found or already resolved"})),
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
