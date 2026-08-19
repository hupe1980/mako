//! GGV operators, § 41e Aggregatorverträge, and the outbound dead-letter queue.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_service::{ApiError, ApiResult, oidc::Claims};
use serde::Deserialize;
use time::Date;
use uuid::Uuid;

use super::{Ctx, is_exclusion_violation, ok};
use crate::{outbound, pg};

// ── GGV-Betreiber (§ 42b EnWG) ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetGgvBetreiberRequest {
    /// The Kunde operating the community — the BG-7 buyer of its bundled
    /// Sammelrechnung.
    pub kunden_id: Uuid,
}

/// `PUT /api/v1/ggv/{ggv_id}/betreiber` — record who operates a GGV.
///
/// The § 42b operator is a **Kunde**, not a Marktpartner: it has no MP-ID and
/// never appears in MaKo, but it is who the bundled GGV Sammelrechnung bills —
/// so the mapping from the operator-assigned `ggv_id` to a customer lives here,
/// beside every other buyer. Idempotent; a re-PUT moves the pointer.
pub async fn put_ggv_betreiber(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(ggv_id): Path<String>,
    Json(req): Json<SetGgvBetreiberRequest>,
) -> ApiResult<StatusCode> {
    if pg::upsert_ggv_betreiber(&ctx.pool, ctx.tenant(), &ggv_id, req.kunden_id)
        .await
        .map_err(ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// `GET /api/v1/ggv/{ggv_id}/betreiber` — the BG-7 buyer of the GGV bundle.
///
/// `404` until a Betreiber is recorded — billingd treats that as "no buyer
/// reachable" and its e-invoice findings say what is missing, exactly as an
/// unconfigured retail buyer does.
pub async fn get_ggv_betreiber(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(ggv_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let re = pg::fetch_rechnungsempfaenger_by_ggv(&ctx.pool, &ggv_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    ok(serde_json::json!({ "ggv_id": ggv_id, "rechnungsempfaenger": re }))
}

// ── Aggregatorverträge (§ 41e EnWG) ──────────────────────────────────────────

/// `PUT /api/v1/aggregatorvertraege/{sr_id}` — create or replace the § 41e
/// Aggregatorvertrag for a SteuerbareRessource.
///
/// Answers `409` when the validity window overlaps an existing contract for the
/// same resource: two simultaneously active Aggregatorverträge are not
/// representable, and the `agg_no_overlap` exclusion constraint is what says so.
pub async fn put_aggregatorvertrag(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(sr_id): Path<String>,
    Json(input): Json<pg::UpsertAggregatorvertragInput>,
) -> ApiResult<Json<serde_json::Value>> {
    match pg::upsert_aggregatorvertrag(&ctx.pool, ctx.tenant(), &sr_id, &input).await {
        Ok(id) => ok(serde_json::json!({ "id": id })),
        Err(e) if is_exclusion_violation(&e) => Err(ApiError::conflict(format!(
            "überlappender Aggregatorvertrag für SR {sr_id}"
        ))),
        Err(e) => Err(ApiError::Internal(e)),
    }
}

#[derive(Deserialize)]
pub struct AggregatorvertragQuery {
    pub sr_id: Option<String>,
    /// ISO 8601 date; defaults to today.
    pub on: Option<Date>,
}

/// `GET /api/v1/aggregatorvertraege`
///
/// With `?sr_id=…&on=YYYY-MM-DD`, returns the single contract in force for that
/// resource on that date (404 when none is) — the lookup `billingd` performs
/// when settling a dispatch.
pub async fn list_aggregatorvertraege(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<AggregatorvertragQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(sr_id) = q.sr_id {
        let on =
            q.on.unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
        let row = pg::find_active_aggregatorvertrag(&ctx.pool, ctx.tenant(), &sr_id, on)
            .await
            .map_err(ApiError::Internal)?
            .ok_or(ApiError::NotFound)?;
        return ok(row);
    }
    let rows = pg::list_aggregatorvertraege(&ctx.pool, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "count": rows.len(), "vertraege": rows }))
}

// ── Outbound dead-letter queue ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DeadQuery {
    pub limit: Option<i64>,
}

/// `GET /api/v1/outbound/dead` — obligations the retries could not discharge.
///
/// A registration that never reached the NB or a Schlussablesung that was never
/// ordered is the operator's problem the moment the queue gives up on it, so it
/// has somewhere to be seen instead of only a log line.
pub async fn list_dead_tasks(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<DeadQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = outbound::list_dead_lettered(
        &ctx.pool,
        ctx.tenant(),
        q.limit.unwrap_or(100).clamp(1, 500),
    )
    .await
    .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "count": rows.len(), "tasks": rows }))
}

/// `POST /api/v1/outbound/dead/{id}/retry` — queue a dead task again.
pub async fn retry_dead_task(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    if outbound::retry_dead_lettered(&ctx.pool, ctx.tenant(), id)
        .await
        .map_err(ApiError::Internal)?
    {
        ok(serde_json::json!({ "id": id, "requeued": true }))
    } else {
        Err(ApiError::NotFound)
    }
}
