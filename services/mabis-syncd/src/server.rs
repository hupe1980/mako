//! Axum router and HTTP server for `mabis-syncd`.
//!
//! ## Routes
//!
//! | Method | Path | Cedar action |
//! |--------|------|--------------|
//! | POST | `/api/v1/sync` | `trigger-mabis-run` |
//! | GET  | `/api/v1/runs` | `read-mabis-run` |
//! | GET  | `/api/v1/runs/{id}` | `read-mabis-run` |
//! | PUT  | `/api/v1/runs/{id}/retry` | `trigger-mabis-run` |
//! | POST | `/api/v1/datenstatus` | `record-biko-response` |
//! | POST | `/api/v1/pruefmitteilung` | `record-biko-response` |
//! | GET  | `/api/v1/korrekturbedarf` | `read-mabis-run` |
//!
//! `/health/*` and `/metrics` are the runner's and are not mounted here.

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
use crate::sync_engine::{SyncEngine, berlin_date, previous_month_period};

/// RFC 3339 for the wire — never `time`'s derived component array.
///
/// `OffsetDateTime`'s derived `Serialize` is `[y, ordinal, h, m, s, ns, ±h, ±m,
/// ±s]`, which is `time`'s internal layout and round-trips only through `time`
/// itself. It matters doubly for `version`: §3.8.2 identifies a Summenzeitreihe
/// by it, `POST /api/v1/pruefmitteilung` *parses* it as RFC 3339 — so a consumer
/// fed the array by a read endpoint cannot echo it back to the write endpoint.
pub(crate) fn rfc3339(t: time::OffsetDateTime) -> Option<String> {
    t.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

/// [`rfc3339`], lifted over the nullable columns.
pub(crate) fn rfc3339_opt(t: Option<time::OffsetDateTime>) -> Option<String> {
    t.and_then(rfc3339)
}

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
        .with_state(state)
}

/// Read an optional ISO-8601 period bound, defaulting only when it is absent.
///
/// A present-but-unparseable value is a 400, never the default: the caller named
/// a period, and filing a different one is a binding submission they did not ask
/// for.
fn parse_period(v: &serde_json::Value, default: Date) -> Result<Date, String> {
    let Some(s) = v.as_str() else {
        return Ok(default);
    };
    Date::parse(s, &time::format_description::well_known::Iso8601::DATE)
        .map_err(|e| format!("period must be an ISO-8601 date (YYYY-MM-DD): {s:?} — {e}"))
}

/// `400 Bad Request` carrying `msg`.
fn bad_request(msg: impl Into<String>) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg.into() })),
    )
        .into_response()
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
    let today = berlin_date(OffsetDateTime::now_utc());
    let (default_from, default_to) = previous_month_period(today);

    // A malformed date must not fall back to the default: silently filing the
    // previous month under a caller-named period is a binding submission for a
    // period nobody asked for.
    let period_from = match parse_period(&body["period_from"], default_from) {
        Ok(d) => d,
        Err(msg) => return bad_request(msg),
    };
    let period_to = match parse_period(&body["period_to"], default_to) {
        Ok(d) => d,
        Err(msg) => return bad_request(msg),
    };
    if period_to < period_from {
        return bad_request("period_to precedes period_from");
    }

    // A Summenzeitreihe the BIKO has acked cannot be withdrawn, and a second
    // one for the same month is a *correction* under a higher version, not a
    // repeat — so two clicks, or a client retrying a request whose response was
    // lost, must not file the month twice. A correction is the one case meant
    // to file the period again, so `corrects_run_id` passes through.
    if corrects_run_id.is_none() {
        match pg::has_live_run_for_period(
            &state.pool,
            &state.cfg.identity.tenant,
            &state.cfg.identity.bilanzierungsgebiet_id,
            period_from,
            period_to,
        )
        .await
        {
            Ok(true) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!(
                            "a run for {period_from}-{period_to} already exists and has not \
                             failed. Filing the period again is a correction, not a retry: send \
                             `corrects_run_id` to answer a negative Pruefmitteilung under a \
                             higher version (BK6-24-174 Anlage 3 3.8.2, 9.8.1), or retry a \
                             failed run via PUT /api/v1/runs/{{id}}/retry."
                        ),
                        "period_from": period_from.to_string(),
                        "period_to": period_to.to_string(),
                    })),
                )
                    .into_response();
            }
            Ok(false) => {}
            Err(e) => {
                // Refuse rather than risk a duplicate binding filing.
                warn!(error = %e, "mabis-syncd: cannot check for an existing run");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": "cannot check whether this period has already been filed - \
                                  refusing rather than risking a duplicate submission",
                    })),
                )
                    .into_response();
            }
        }
    }

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
                        "version": rfc3339(r.version),
                        "status": r.status,
                        "malo_count": r.malo_count,
                        "total_kwh": r.total_kwh,
                        "has_substituted": r.has_substituted,
                        "triggered_at": rfc3339(r.triggered_at),
                        "submitted_at": rfc3339_opt(r.submitted_at),
                        "acked_at": rfc3339_opt(r.acked_at),
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
        Ok(Some(r)) => {
            // What actually went out, per Bilanzierungsgebiet. The run row's
            // single `message_ref` is the first territory's; a run spanning
            // several is only legible here.
            let series: Vec<serde_json::Value> = pg::list_series(&state.pool, r.id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        "bilanzierungsgebiet_id": s.bilanzierungsgebiet_id,
                        "mabis_zp_id":            s.mabis_zp_id,
                        "malo_count":             s.malo_count,
                        "interval_count":         s.interval_count,
                        "total_kwh":              s.total_kwh,
                        "status":                 s.status,
                        "message_ref":            s.message_ref,
                        "process_id":             s.process_id,
                        "error_msg":              s.error_msg,
                        "submitted_at":           rfc3339_opt(s.submitted_at),
                    })
                })
                .collect();
            Json(serde_json::json!({
                "id": r.id,
                "bilanzierungsgebiet_id": r.bilanzierungsgebiet_id,
                "period_from": r.period_from.to_string(),
                "period_to": r.period_to.to_string(),
                "version": rfc3339(r.version),
                "status": r.status,
                "malo_count": r.malo_count,
                "interval_count": r.interval_count,
                "total_kwh": r.total_kwh,
                "has_substituted": r.has_substituted,
                "triggered_at": rfc3339(r.triggered_at),
                "submitted_at": rfc3339_opt(r.submitted_at),
                "acked_at": rfc3339_opt(r.acked_at),
                "message_ref": r.message_ref,
                "process_id": r.process_id,
                "error_msg": r.error_msg,
                "attempt_count": r.attempt_count,
                "series": series,
            }))
            .into_response()
        }
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

    // Only a run that is known to have failed may be retried. 'pending' means
    // the original attempt may still be aggregating, and a retry then files a
    // second binding Summenzeitreihe for the same period.
    if run.status != "failed" {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!(
                    "run is in status {:?} — only a failed run can be retried",
                    run.status
                ),
            })),
        )
            .into_response();
    }

    let period_from = run.period_from;
    let period_to = run.period_to;
    // A retry of a correction is still a correction (§9.8.1): dropping
    // `corrects_run_id` leaves the negative Prüfmitteilung open on
    // /korrekturbedarf after the corrected BG-SZR has gone out.
    let corrects_run_id = run.corrects_run_id;
    let engine = state.engine.clone();

    tokio::spawn(async move {
        // A retry of a failed run is a fresh submission attempt, so it takes a
        // new version rather than reusing the one that failed. Territories the
        // BIKO already acked are not re-filed - `submit_all_to_makod` reads
        // them from `submission_series`.
        match engine
            .run_aggregation(period_from, period_to, corrects_run_id, None)
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
        "record-biko-response",
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
        "record-biko-response",
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

    // One transaction: the Prüfmitteilung record and — when it is negative —
    // the `de.mabis.korrekturbedarf.opened` outbox row commit together, so a
    // Korrekturbedarf cannot exist that nothing announced, and no announcement
    // can outlive a rolled-back record.
    let outcome = async {
        let mut tx = state.pool.begin().await?;
        let id = pg::record_pruefmitteilung(
            &mut tx,
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
        .await?;
        if !positiv {
            // The version rides the wire as RFC 3339, never as `time`'s derived
            // component array (`xtask check-wire-timestamps`).
            let version = target
                .version
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            let ce = mako_service::CloudEvent::new(
                mako_service::source("mabis-syncd", &state.cfg.identity.tenant),
                mako_events::mabis::KORREKTURBEDARF_OPENED,
                id.to_string(),
                serde_json::json!({
                    "pruefmitteilung_id": id.to_string(),
                    "bilanzierungsgebiet_id": target.bilanzierungsgebiet_id,
                    "period_from": target.period_from.to_string(),
                    "period_to": target.period_to.to_string(),
                    "version": version,
                    "sender_mp_id": body["sender_mp_id"].as_str().unwrap_or_default(),
                    "pid": body["pid"].as_i64().unwrap_or(0),
                    "begruendung": body["begruendung"].as_str(),
                }),
            );
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(id)
    }
    .await;

    match outcome {
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
                        "version": rfc3339(version),
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
