//! Handler for the CloudEvent replay log (B11).
//!
//! Route: `GET /admin/events`
//!
//! Returns a time-windowed slice of the `event_log` table.  Useful for
//! replaying events to newly onboarded subscribers and for post-incident
//! forensics without requiring a separate event broker.
//!
//! Query parameters (all optional):
//! - `from`: RFC 3339 lower bound (inclusive), defaults to 24 h ago.
//! - `to`:   RFC 3339 upper bound (inclusive), defaults to now.
//! - `type`: CloudEvents `type` exact-match filter.
//! - `limit`: max rows (default 500, max 5000).

use axum::{Extension, Json, extract::Query, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use sqlx::PgPool;
use tracing::warn;

/// Query parameters for `GET /admin/events`.
#[derive(Debug, Deserialize)]
pub struct EventLogQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(rename = "type")]
    pub ce_type: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /admin/events`
///
/// Returns up to `limit` rows from `event_log` in the given time window,
/// ordered by `received_at ASC` (oldest first for deterministic replay).
pub async fn list_event_log(
    Extension(pool): Extension<PgPool>,
    Query(q): Query<EventLogQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);

    // Parse optional RFC 3339 timestamps.
    let from: Option<time::OffsetDateTime> = q.from.as_deref().and_then(|s| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
    });
    let to: Option<time::OffsetDateTime> = q.to.as_deref().and_then(|s| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
    });

    let result = sqlx::query_as::<_, EventLogRow>(
        r"SELECT id::TEXT, event_id, ce_type, ce_source, subject, data, received_at
          FROM event_log
          WHERE ($1::TIMESTAMPTZ IS NULL OR received_at >= $1)
            AND ($2::TIMESTAMPTZ IS NULL OR received_at <= $2)
            AND ($3::TEXT IS NULL OR ce_type = $3)
          ORDER BY received_at ASC
          LIMIT $4",
    )
    .bind(from)
    .bind(to)
    .bind(&q.ce_type)
    .bind(limit)
    .fetch_all(&pool)
    .await;

    match result {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            warn!(error = %e, "event_log: query failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct EventLogRow {
    pub id: String,
    pub event_id: String,
    pub ce_type: String,
    pub ce_source: Option<String>,
    pub subject: Option<String>,
    pub data: Option<serde_json::Value>,
    pub received_at: time::OffsetDateTime,
}
