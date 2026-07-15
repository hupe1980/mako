//! Axum router and HTTP server for `mabis-syncd`.
//!
//! ## Routes
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | POST | `/api/v1/sync` | Trigger aggregation run manually |
//! | GET  | `/api/v1/runs` | List recent submission runs |
//! | GET  | `/api/v1/runs/{id}` | Get single run details |
//! | PUT  | `/api/v1/runs/{id}/retry` | Retry a failed run |
//! | GET  | `/health/live` | Liveness probe |
//! | GET  | `/health/ready` | Readiness probe |
//! | GET  | `/metrics` | Prometheus metrics |

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

use crate::config::Config;
use crate::pg;
use crate::sync_engine::{SyncEngine, previous_month_period};

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ServerState {
    pub pool: sqlx::PgPool,
    pub engine: Arc<SyncEngine>,
    pub cfg: Arc<Config>,
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/api/v1/sync", post(trigger_sync))
        .route("/api/v1/runs", get(list_runs))
        .route("/api/v1/runs/{id}", get(get_run))
        .route("/api/v1/runs/{id}/retry", put(retry_run))
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(health_ready))
        .route("/metrics", get(|| async { "# mabis-syncd metrics\n" }))
        .with_state(state)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `POST /api/v1/sync` — trigger a manual aggregation run.
///
/// Request body:
/// ```json
/// {
///   "version": "vorlaeufig",       // optional — default: "vorlaeufig"
///   "period_from": "2026-06-01",   // optional — default: previous calendar month
///   "period_to": "2026-06-30"      // optional
/// }
/// ```
async fn trigger_sync(
    State(state): State<ServerState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let version = body["version"].as_str().unwrap_or("vorlaeufig");
    let today = OffsetDateTime::now_utc().date();
    let (default_from, default_to) = previous_month_period(today);

    let period_from = body["period_from"]
        .as_str()
        .and_then(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE).ok()
        })
        .unwrap_or(default_from);
    let period_to = body["period_to"]
        .as_str()
        .and_then(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE).ok()
        })
        .unwrap_or(default_to);

    let engine = state.engine.clone();
    let run_version = version.to_owned();
    tokio::spawn(async move {
        match engine
            .run_aggregation(period_from, period_to, &run_version)
            .await
        {
            Ok(run_id) => tracing::info!(run_id = %run_id, "mabis-syncd: async sync run completed"),
            Err(e) => warn!(error = %e, "mabis-syncd: async sync run failed"),
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "accepted",
            "version": version,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "note": "aggregation started asynchronously — check GET /api/v1/runs for status",
        })),
    )
}

/// `GET /api/v1/runs` — list recent submission runs.
async fn list_runs(State(state): State<ServerState>) -> impl IntoResponse {
    match pg::list_runs(&state.pool, &state.cfg.identity.tenant, 50).await {
        Ok(rows) => {
            let runs: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "bilanzierungsgebiet_id": r.bilanzierungsgebiet_id,
                        "period_from": r.period_from.to_string(),
                        "period_to": r.period_to.to_string(),
                        "version": r.version,
                        "status": r.status,
                        "malo_count": r.malo_count,
                        "total_kwh": r.total_kwh,
                        "has_substituted": r.has_substituted,
                        "triggered_at": r.triggered_at,
                        "submitted_at": r.submitted_at,
                        "acked_at": r.acked_at,
                        "message_ref": r.message_ref,
                        "error_msg": r.error_msg,
                    })
                })
                .collect();
            Json(serde_json::json!({ "runs": runs, "count": runs.len() })).into_response()
        }
        Err(e) => {
            warn!(error = %e, "mabis-syncd: list_runs query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/v1/runs/{id}` — get single run.
async fn get_run(State(state): State<ServerState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    match pg::get_run(&state.pool, id, &state.cfg.identity.tenant).await {
        Ok(Some(r)) => Json(serde_json::json!({
            "id": r.id,
            "bilanzierungsgebiet_id": r.bilanzierungsgebiet_id,
            "period_from": r.period_from.to_string(),
            "period_to": r.period_to.to_string(),
            "version": r.version,
            "status": r.status,
            "malo_count": r.malo_count,
            "interval_count": r.interval_count,
            "total_kwh": r.total_kwh,
            "has_substituted": r.has_substituted,
            "triggered_at": r.triggered_at,
            "submitted_at": r.submitted_at,
            "acked_at": r.acked_at,
            "message_ref": r.message_ref,
            "process_id": r.process_id,
            "error_msg": r.error_msg,
            "attempt_count": r.attempt_count,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found" })),
        )
            .into_response(),
        Err(e) => {
            warn!(error = %e, "mabis-syncd: get_run failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `PUT /api/v1/runs/{id}/retry` — retry a failed run.
async fn retry_run(State(state): State<ServerState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    let run = match pg::get_run(&state.pool, id, &state.cfg.identity.tenant).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "not found" })),
            )
                .into_response();
        }
        Err(e) => {
            warn!(error = %e, "mabis-syncd: retry get_run failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !matches!(run.status.as_str(), "failed" | "pending") {
        return (StatusCode::CONFLICT, Json(serde_json::json!({
            "error": format!("run is in status {:?} — only failed/pending runs can be retried", run.status)
        }))).into_response();
    }

    let period_from = run.period_from;
    let period_to = run.period_to;
    let version = run.version.clone();
    let engine = state.engine.clone();

    tokio::spawn(async move {
        match engine
            .run_aggregation(period_from, period_to, &version)
            .await
        {
            Ok(new_id) => {
                tracing::info!(original_id = %id, new_id = %new_id, "mabis-syncd: retry completed")
            }
            Err(e) => warn!(original_id = %id, error = %e, "mabis-syncd: retry failed"),
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "retry_accepted",
            "original_run_id": id,
        })),
    )
        .into_response()
}

async fn health_ready(State(state): State<ServerState>) -> impl IntoResponse {
    match sqlx::query("SELECT 1").fetch_one(&state.pool).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
