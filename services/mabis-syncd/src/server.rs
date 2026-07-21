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
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use std::sync::Arc;
use time::{Date, OffsetDateTime};
use tracing::warn;
use uuid::Uuid;

use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;

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

/// Authorise `action` for the caller against this deployment's tenant.
///
/// Returns `Some(403)` on denial and `None` when permitted. A MaBiS submission
/// is a binding filing to the BIKO, so every route is authorised — including the
/// read routes, whose run history discloses which Bilanzierungsgebiete a tenant
/// settles.
fn deny(
    enforcer: &CedarEnforcer,
    claims: &Claims,
    action: &str,
    tenant: &str,
) -> Option<axum::response::Response> {
    match enforcer.check(&claims.principal(), action, tenant) {
        Ok(()) => None,
        Err(e) => Some(
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        ),
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/api/v1/sync", post(trigger_sync))
        .route("/api/v1/runs", get(list_runs))
        .route("/api/v1/runs/{id}", get(get_run))
        .route("/api/v1/runs/{id}/retry", put(retry_run))
        .route("/api/v1/datenstatus", post(post_datenstatus))
        .route("/api/v1/pruefmitteilung", post(post_pruefmitteilung))
        .route("/api/v1/korrekturbedarf", get(list_korrekturbedarf))
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(health_ready))
        .route("/metrics", get(|| async { "# mabis-syncd metrics\n" }))
        .with_state(state)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `POST /api/v1/sync` — trigger a manual aggregation run.
///
/// The version is assigned by the service, not the caller: BK6-24-174 Anlage 3
/// §3.8.2 requires it to ascend, and the settlement phase follows from where
/// the submission date falls in the Werktag calendar.
///
/// Request body:
/// ```json
/// {
///   "period_from": "2026-06-01",   // optional — default: previous calendar month
///   "period_to": "2026-06-30",     // optional
///   "corrects_run_id": "…"         // optional — the run this one corrects
/// }
/// ```
async fn trigger_sync(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(resp) = deny(
        &enforcer,
        &claims,
        "trigger-mabis-run",
        &state.cfg.identity.tenant,
    ) {
        return resp;
    }
    let corrects_run_id = body["corrects_run_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok());
    // Reconstruct the readings as they stood at this instant instead of taking
    // current values (§ 60 Abs. 6 MsbG). Used to rebuild what an earlier version
    // contained when preparing a correction.
    let as_of = body["as_of"].as_str().and_then(|s| {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
    });
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

    tokio::spawn(async move {
        match engine
            .run_aggregation(period_from, period_to, corrects_run_id, as_of)
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
            "period_from": period_from.to_string(),
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "note": "aggregation started asynchronously — check GET /api/v1/runs for status",
        })),
    )
        .into_response()
}

/// `GET /api/v1/runs` — list recent submission runs.
async fn list_runs(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
) -> impl IntoResponse {
    if let Some(resp) = deny(
        &enforcer,
        &claims,
        "read-mabis-run",
        &state.cfg.identity.tenant,
    ) {
        return resp;
    }
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
async fn get_run(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Some(resp) = deny(
        &enforcer,
        &claims,
        "read-mabis-run",
        &state.cfg.identity.tenant,
    ) {
        return resp;
    }
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
async fn retry_run(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Some(resp) = deny(
        &enforcer,
        &claims,
        "trigger-mabis-run",
        &state.cfg.identity.tenant,
    ) {
        return resp;
    }
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
    let engine = state.engine.clone();

    tokio::spawn(async move {
        // A retry of a failed run is a fresh submission attempt, so it takes a
        // new version rather than reusing the one that failed.
        match engine
            .run_aggregation(period_from, period_to, None, None)
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

// ── Inbound BIKO responses ────────────────────────────────────────────────────

/// Body identifying one Summenzeitreihe version.
#[derive(serde::Deserialize)]
struct VersionRef {
    bilanzierungsgebiet_id: String,
    period_from: Date,
    period_to: Date,
    #[serde(with = "time::serde::rfc3339")]
    version: OffsetDateTime,
}

/// `POST /api/v1/datenstatus`
///
/// Record the Datenstatus the BIKO assigned to a submitted version, as received
/// via IFTSTA (SG7 STS+Z04, PID 21003 to NB/ÜNB or 21004 to BKV/NB).
///
/// The Datenstatus is assigned exclusively by the BIKO (BK6-24-174 Anlage 3
/// §3.8.3), so this route only records what arrived — it never derives one.
async fn post_datenstatus(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(resp) = deny(
        &enforcer,
        &claims,
        "trigger-mabis-run",
        &state.cfg.identity.tenant,
    ) {
        return resp;
    }

    let Ok(target) = serde_json::from_value::<VersionRef>(body.clone()) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "expected bilanzierungsgebiet_id, period_from, period_to, version",
            })),
        )
            .into_response();
    };

    let Some(status) = body["datenstatus"]
        .as_str()
        .and_then(pg::Datenstatus::from_wire)
    else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "unknown datenstatus",
                "expected": [
                    "Prüfdaten", "Abrechnungsdaten", "Abrechnungsdaten KBKA",
                    "abgerechnete Daten", "abgerechnete Daten KBKA",
                ],
            })),
        )
            .into_response();
    };

    match pg::record_datenstatus(
        &state.pool,
        &state.cfg.identity.tenant,
        &target.bilanzierungsgebiet_id,
        target.period_from,
        target.period_to,
        target.version,
        status,
    )
    .await
    {
        // No matching row: the BIKO named a version this instance never sent.
        // Reported rather than accepted, since silently succeeding would hide a
        // disagreement about what was filed.
        Ok(0) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no submission matches that Bilanzierungsgebiet, period and version",
            })),
        )
            .into_response(),
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "datenstatus": status.as_str(),
                "settles": status.settles(),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/pruefmitteilung`
///
/// Record an inbound Prüfmitteilung (IFTSTA PID 21000/21001).
///
/// A negative one signals Korrekturbedarf (§9.8.1) and is answered by a
/// corrected Summenzeitreihe under a higher version, which the operator
/// triggers via `POST /api/v1/sync` with `corrects_run_id`.
async fn post_pruefmitteilung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Some(resp) = deny(
        &enforcer,
        &claims,
        "trigger-mabis-run",
        &state.cfg.identity.tenant,
    ) {
        return resp;
    }

    let Ok(target) = serde_json::from_value::<VersionRef>(body.clone()) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "expected bilanzierungsgebiet_id, period_from, period_to, version",
            })),
        )
            .into_response();
    };
    let Some(positiv) = body["positiv"].as_bool() else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "`positiv` is required" })),
        )
            .into_response();
    };

    match pg::record_pruefmitteilung(
        &state.pool,
        &state.cfg.identity.tenant,
        &target.bilanzierungsgebiet_id,
        target.period_from,
        target.period_to,
        target.version,
        positiv,
        body["sender_mp_id"].as_str().unwrap_or_default(),
        body["pid"].as_i64().unwrap_or(0) as i32,
        body["begruendung"].as_str(),
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id.to_string(),
                "korrektur_erforderlich": !positiv,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/korrekturbedarf`
///
/// Negative Prüfmitteilungen with no correcting submission yet — open
/// obligations under §9.8.1, not history.
async fn list_korrekturbedarf(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
) -> impl IntoResponse {
    if let Some(resp) = deny(
        &enforcer,
        &claims,
        "read-mabis-run",
        &state.cfg.identity.tenant,
    ) {
        return resp;
    }

    match pg::open_korrekturbedarf(&state.pool, &state.cfg.identity.tenant).await {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|(id, gebiet, from, to, version)| {
                    serde_json::json!({
                        "pruefmitteilung_id": id.to_string(),
                        "bilanzierungsgebiet_id": gebiet,
                        "period_from": from.to_string(),
                        "period_to": to.to_string(),
                        "version": version,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "count": items.len(),
                    "korrekturbedarf": items,
                    "regulatory_note":
                        "BK6-24-174 Anlage 3 §9.8.1: a negative Prüfmitteilung is answered \
                         with a corrected Summenzeitreihe under a higher version, via \
                         POST /api/v1/sync with corrects_run_id.",
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
