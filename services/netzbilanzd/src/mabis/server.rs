//! The MaBiS REST surface, nested under `/api/v1/mabis`.
//!
//! ## Routes
//!
//! | Method | Path | Cedar action |
//! |--------|------|--------------|
//! | POST | `/api/v1/mabis/sync` | `trigger-mabis-run` |
//! | GET  | `/api/v1/mabis/runs` | `read-mabis-run` |
//! | GET  | `/api/v1/mabis/runs/{id}` | `read-mabis-run` |
//! | PUT  | `/api/v1/mabis/runs/{id}/retry` | `trigger-mabis-run` |
//! | POST | `/api/v1/mabis/datenstatus` | `record-biko-response` |
//! | POST | `/api/v1/mabis/pruefmitteilung` | `record-biko-response` |
//! | GET  | `/api/v1/mabis/korrekturbedarf` | `read-mabis-run` |
//!
//! A MaBiS submission is a binding filing to the BIKO, so every route is
//! authorised — including the read routes, whose run history discloses which
//! Bilanzierungsgebiete a tenant settles.

use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
};
use mako_service::{ApiError, ApiResult, Json};
use std::sync::Arc;
use time::{Date, OffsetDateTime};
use tracing::warn;
use uuid::Uuid;

use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;

use super::config::Settings;
use super::pg;
use super::sync_engine::{SyncEngine, previous_month_period};
use crate::handlers::authorize;

/// RFC 3339 for the wire — never `time`'s derived component array.
///
/// `OffsetDateTime`'s derived `Serialize` is `[y, ordinal, h, m, s, ns, ±h, ±m,
/// ±s]`, which is `time`'s internal layout and round-trips only through `time`
/// itself. It matters doubly for `version`: §3.8.2 identifies a Summenzeitreihe
/// by it, `POST /api/v1/mabis/pruefmitteilung` *parses* it as RFC 3339 — so a
/// consumer fed the array by a read endpoint cannot echo it back to the write
/// endpoint.
pub(crate) fn rfc3339(t: time::OffsetDateTime) -> Option<String> {
    t.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

/// [`rfc3339`], lifted over the nullable columns.
pub(crate) fn rfc3339_opt(t: Option<time::OffsetDateTime>) -> Option<String> {
    t.and_then(rfc3339)
}

// ── State ─────────────────────────────────────────────────────────────────────

/// What the MaBiS handlers share.
#[derive(Clone)]
pub struct ServerState {
    pub pool: sqlx::PgPool,
    pub engine: Arc<SyncEngine>,
    pub cfg: Arc<Settings>,
}

/// A query failure, logged with what failed and answered as a 500.
fn internal(what: &'static str, e: impl std::fmt::Display) -> ApiError {
    warn!(error = %e, "netzbilanzd mabis: {what} failed");
    ApiError::Internal(anyhow::anyhow!("{what} failed: {e}"))
}

/// `POST /api/v1/mabis/sync` body.
///
/// Every field is typed, so a value that does not parse is a 422 naming the
/// field rather than an absent one: a malformed `period_from` that fell back
/// to the previous month would file a period nobody asked for, and a malformed
/// `corrects_run_id` that read as absent would turn a correction into a
/// refused first filing — or, on an empty period, into a second binding
/// filing that corrects nothing.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncRequest {
    /// First day of the period (ISO 8601) — default: the previous calendar month.
    #[serde(default)]
    pub period_from: Option<Date>,
    /// Last day of the period (ISO 8601) — default: the previous calendar month.
    #[serde(default)]
    pub period_to: Option<Date>,
    /// The run this one corrects.
    #[serde(default)]
    pub corrects_run_id: Option<Uuid>,
    /// Rebuild the readings as they stood at this instant (RFC 3339) instead of
    /// taking current values (§ 60 Abs. 6 MsbG).
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub as_of: Option<OffsetDateTime>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `POST /api/v1/mabis/sync` — trigger a manual aggregation run.
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
///   "corrects_run_id": "…",        // optional — the run this one corrects
///   "as_of": "2026-07-15T08:00:00Z" // optional — readings as they stood then
/// }
/// ```
///
/// An unknown key, or a value that does not parse, is a 422 — see
/// [`SyncRequest`].
pub async fn trigger_sync(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Json(body): Json<SyncRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&cedar, &claims, "trigger-mabis-run", &state.cfg.tenant)?;
    let SyncRequest {
        period_from,
        period_to,
        corrects_run_id,
        as_of,
    } = body;
    let (default_from, default_to) = previous_month_period(mako_fristen::heute());
    let period_from = period_from.unwrap_or(default_from);
    let period_to = period_to.unwrap_or(default_to);
    if period_to < period_from {
        return Err(ApiError::bad_request("period_to precedes period_from"));
    }

    // A Summenzeitreihe the BIKO has acked cannot be withdrawn, and a second
    // one for the same month is a *correction* under a higher version, not a
    // repeat — so two clicks, or a client retrying a request whose response was
    // lost, must not file the month twice. A correction is the one case meant
    // to file the period again, so `corrects_run_id` passes through.
    if corrects_run_id.is_none() {
        match pg::has_live_run_for_period(
            &state.pool,
            &state.cfg.tenant,
            &state.cfg.mabis.bilanzierungsgebiet_id,
            period_from,
            period_to,
        )
        .await
        {
            Ok(true) => {
                return Err(ApiError::conflict(format!(
                    "a run for {period_from}-{period_to} already exists and has not \
                     failed. Filing the period again is a correction, not a retry: send \
                     `corrects_run_id` to answer a negative Pruefmitteilung under a \
                     higher version (BK6-24-174 Anlage 3 3.8.2, 9.8.1), or retry a \
                     failed run via PUT /api/v1/mabis/runs/{{id}}/retry."
                )));
            }
            Ok(false) => {}
            // Refuse rather than risk a duplicate binding filing.
            Err(e) => return Err(internal("the check for an existing run", e)),
        }
    }

    let engine = state.engine.clone();

    tokio::spawn(async move {
        match engine
            .run_aggregation(period_from, period_to, corrects_run_id, as_of)
            .await
        {
            Ok(run_id) => {
                tracing::info!(run_id = %run_id, "netzbilanzd mabis: async sync run completed")
            }
            Err(e) => warn!(error = %e, "netzbilanzd mabis: async sync run failed"),
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "accepted",
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "note": "aggregation started asynchronously — check GET /api/v1/mabis/runs for status",
        })),
    ))
}

/// `GET /api/v1/mabis/runs` — list recent submission runs.
pub async fn list_runs(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&cedar, &claims, "read-mabis-run", &state.cfg.tenant)?;
    let rows = pg::list_runs(&state.pool, &state.cfg.tenant, 50)
        .await
        .map_err(|e| internal("list_runs", e))?;
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
    Ok(Json(
        serde_json::json!({ "runs": runs, "count": runs.len() }),
    ))
}

/// `GET /api/v1/mabis/runs/{id}` — get single run.
pub async fn get_run(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&cedar, &claims, "read-mabis-run", &state.cfg.tenant)?;
    let r = pg::get_run(&state.pool, id, &state.cfg.tenant)
        .await
        .map_err(|e| internal("get_run", e))?
        .ok_or(ApiError::NotFound)?;
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
    Ok(Json(serde_json::json!({
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
    })))
}

/// `PUT /api/v1/mabis/runs/{id}/retry` — retry a failed run.
pub async fn retry_run(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Path(id): Path<Uuid>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&cedar, &claims, "trigger-mabis-run", &state.cfg.tenant)?;
    let run = pg::get_run(&state.pool, id, &state.cfg.tenant)
        .await
        .map_err(|e| internal("retry get_run", e))?
        .ok_or(ApiError::NotFound)?;

    // Only a run that is known to have failed may be retried. 'pending' means
    // the original attempt may still be aggregating, and a retry then files a
    // second binding Summenzeitreihe for the same period.
    if run.status != "failed" {
        return Err(ApiError::conflict(format!(
            "run is in status {:?} — only a failed run can be retried",
            run.status
        )));
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
        // BIKO already acked are not re-filed: `run_aggregation` reads them
        // from `submission_series` for the whole **Bilanzierungsmonat** — not
        // for the new run, which has no rows of its own yet — and hands them to
        // `submit_all_to_makod` as the skip list. A retry that finds every
        // territory already acked files nothing and succeeds.
        match engine
            .run_aggregation(period_from, period_to, corrects_run_id, None)
            .await
        {
            Ok(new_id) => {
                tracing::info!(original_id = %id, new_id = %new_id, "netzbilanzd mabis: retry completed")
            }
            Err(e) => warn!(original_id = %id, error = %e, "netzbilanzd mabis: retry failed"),
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "retry_accepted",
            "original_run_id": id,
        })),
    ))
}

// ── Inbound BIKO responses ────────────────────────────────────────────────────

/// `POST /api/v1/mabis/datenstatus` body: the Summenzeitreihe version the
/// BIKO names, and the Datenstatus it assigned.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatenstatusRequest {
    pub bilanzierungsgebiet_id: String,
    pub period_from: Date,
    pub period_to: Date,
    /// The version's Erstellungszeitpunkt (RFC 3339), as filed.
    #[serde(with = "time::serde::rfc3339")]
    pub version: OffsetDateTime,
    /// The wire name, e.g. `"Abrechnungsdaten"`.
    pub datenstatus: String,
}

/// The Prüfidentifikatoren on which a Prüfmitteilung reaches the
/// Netzbetreiber: 21000 LF → NB, 21001 benachbarter NB → verantwortlicher NB
/// (PID-Übersicht 4.0; `mako_mabis::bilanzkreisabrechnung::IFTSTA_PRUEFMITTEILUNG_PIDS`
/// minus 21005, which the NB sends to the BIKO).
pub const INBOUND_PRUEFMITTEILUNG_PIDS: [i32; 2] = [21_000, 21_001];

/// `POST /api/v1/mabis/pruefmitteilung` body.
///
/// `sender_mp_id` and `pid` are required: the record names who objected and on
/// which Prüfidentifikator, and a defaulted empty sender or PID `0` would be a
/// record that answers neither.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PruefmitteilungRequest {
    pub bilanzierungsgebiet_id: String,
    pub period_from: Date,
    pub period_to: Date,
    /// The version's Erstellungszeitpunkt (RFC 3339), as filed.
    #[serde(with = "time::serde::rfc3339")]
    pub version: OffsetDateTime,
    /// `false` signals Korrekturbedarf (§9.8.1).
    pub positiv: bool,
    /// The MP-ID of the party that sent the Prüfmitteilung, as received.
    pub sender_mp_id: String,
    /// One of [`INBOUND_PRUEFMITTEILUNG_PIDS`].
    pub pid: i32,
    #[serde(default)]
    pub begruendung: Option<String>,
}

/// `POST /api/v1/mabis/datenstatus`
///
/// Record the Datenstatus the BIKO assigned to a submitted version, as received
/// via IFTSTA (SG7 STS+Z04, PID 21003 to NB/ÜNB or 21004 to BKV/NB).
///
/// The Datenstatus is assigned exclusively by the BIKO (BK6-24-174 Anlage 3
/// §3.8.3), so this route only records what arrived — it never derives one.
pub async fn post_datenstatus(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Json(target): Json<DatenstatusRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&cedar, &claims, "record-biko-response", &state.cfg.tenant)?;

    let Some(status) = pg::Datenstatus::from_wire(&target.datenstatus) else {
        return Err(ApiError::unprocessable_with(
            "unknown datenstatus",
            serde_json::json!({
                "expected": [
                    "Prüfdaten", "Abrechnungsdaten", "Abrechnungsdaten KBKA",
                    "abgerechnete Daten", "abgerechnete Daten KBKA",
                ],
            }),
        ));
    };

    let updated = pg::record_datenstatus(
        &state.pool,
        &state.cfg.tenant,
        &target.bilanzierungsgebiet_id,
        target.period_from,
        target.period_to,
        target.version,
        status,
    )
    .await
    .map_err(|e| internal("record_datenstatus", e))?;
    // No matching row: the BIKO named a version this instance never sent.
    // Reported rather than accepted, since silently succeeding would hide a
    // disagreement about what was filed.
    if updated == 0 {
        warn!(
            bilanzierungsgebiet_id = %target.bilanzierungsgebiet_id,
            period_from = %target.period_from,
            version = ?rfc3339(target.version),
            "netzbilanzd mabis: Datenstatus names no submission this instance filed"
        );
        return Err(ApiError::NotFound);
    }
    Ok(Json(serde_json::json!({
        "datenstatus": status.as_str(),
        "settles": status.settles(),
    })))
}

/// `POST /api/v1/mabis/pruefmitteilung`
///
/// Record an inbound Prüfmitteilung (IFTSTA PID 21000/21001).
///
/// A negative one signals Korrekturbedarf (§9.8.1) and is answered by a
/// corrected Summenzeitreihe under a higher version, which the operator
/// triggers via `POST /api/v1/mabis/sync` with `corrects_run_id`.
pub async fn post_pruefmitteilung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
    Json(target): Json<PruefmitteilungRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&cedar, &claims, "record-biko-response", &state.cfg.tenant)?;

    if !INBOUND_PRUEFMITTEILUNG_PIDS.contains(&target.pid) {
        return Err(ApiError::unprocessable_with(
            "pid is not a Prüfidentifikator on which a Prüfmitteilung reaches the Netzbetreiber",
            serde_json::json!({ "expected": INBOUND_PRUEFMITTEILUNG_PIDS }),
        ));
    }
    if target.sender_mp_id.trim().is_empty() {
        return Err(ApiError::unprocessable("sender_mp_id must not be empty"));
    }
    let positiv = target.positiv;

    // One transaction: the Prüfmitteilung record and — when it is negative —
    // the `de.mabis.korrekturbedarf.opened` outbox row commit together, so a
    // Korrekturbedarf cannot exist that nothing announced, and no announcement
    // can outlive a rolled-back record.
    let outcome = async {
        let mut tx = state.pool.begin().await?;
        let id = pg::record_pruefmitteilung(
            &mut tx,
            &state.cfg.tenant,
            &target.bilanzierungsgebiet_id,
            target.period_from,
            target.period_to,
            target.version,
            positiv,
            &target.sender_mp_id,
            target.pid,
            target.begruendung.as_deref(),
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
                mako_service::source("netzbilanzd", &state.cfg.tenant),
                mako_events::mabis::KORREKTURBEDARF_OPENED,
                id.to_string(),
                serde_json::json!({
                    "pruefmitteilung_id": id.to_string(),
                    "bilanzierungsgebiet_id": target.bilanzierungsgebiet_id,
                    "period_from": target.period_from.to_string(),
                    "period_to": target.period_to.to_string(),
                    "version": version,
                    "sender_mp_id": target.sender_mp_id,
                    "pid": target.pid,
                    "begruendung": target.begruendung,
                }),
            );
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(id)
    }
    .await;

    let id = outcome.map_err(|e| internal("record_pruefmitteilung", e))?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id.to_string(),
            "korrektur_erforderlich": !positiv,
        })),
    ))
}

/// `GET /api/v1/mabis/korrekturbedarf`
///
/// Negative Prüfmitteilungen with no correcting submission yet — open
/// obligations under §9.8.1, not history.
pub async fn list_korrekturbedarf(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    State(state): State<ServerState>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&cedar, &claims, "read-mabis-run", &state.cfg.tenant)?;

    let rows = pg::open_korrekturbedarf(&state.pool, &state.cfg.tenant)
        .await
        .map_err(|e| internal("open_korrekturbedarf", e))?;
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
    Ok(Json(serde_json::json!({
        "count": items.len(),
        "korrekturbedarf": items,
        "regulatory_note":
            "BK6-24-174 Anlage 3 §9.8.1: a negative Prüfmitteilung is answered \
             with a corrected Summenzeitreihe under a higher version, via \
             POST /api/v1/mabis/sync with corrects_run_id.",
    })))
}
