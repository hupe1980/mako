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

use super::{CedarEnforcer, Ctx, authorize, is_exclusion_violation, ok};
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
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(ggv_id): Path<String>,
    Json(req): Json<SetGgvBetreiberRequest>,
) -> ApiResult<StatusCode> {
    authorize(&enforcer, &claims, "write-stammdaten", ctx.tenant())?;
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
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(ggv_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-stammdaten", ctx.tenant())?;
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
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(sr_id): Path<String>,
    Json(input): Json<pg::UpsertAggregatorvertragInput>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "write-stammdaten", ctx.tenant())?;
    match pg::upsert_aggregatorvertrag(&ctx.pool, ctx.tenant(), &sr_id, &input).await {
        Ok(id) => ok(serde_json::json!({ "id": id })),
        Err(e) if is_exclusion_violation(&e) => Err(ApiError::conflict(format!(
            "überlappender Aggregatorvertrag für SR {sr_id}"
        ))),
        Err(e) => Err(ApiError::Internal(e)),
    }
}

/// `GET /api/v1/messstellenvertraege/{melo_id}/{msb_mp_id}`
///
/// The Messstellenbetriebsvertrag the MSB holds at a Messlokation, plus the
/// date a Kündigung received on `?on=` (default today) could take effect.
///
/// `processd` reads this to answer a WiM Kündigung MSB out of `E_0200`. A `404`
/// is **no contract** — the `ZC9` case; a `5xx` is a lookup that could not be
/// performed and must never be read as absence.
///
/// `?haushaltskunde=true` applies the § 309 Nr. 9 lit. c BGB one-month cap to
/// the contractual notice period.
pub async fn get_messstellenvertrag(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((melo_id, msb_mp_id)): Path<(String, String)>,
    Query(q): Query<MessstellenvertragQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-stammdaten", ctx.tenant())?;
    let row = pg::find_messstellenvertrag(&ctx.pool, ctx.tenant(), &melo_id, &msb_mp_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    let on = q.on.unwrap_or_else(mako_fristen::heute);
    ok(row.view(on, q.haushaltskunde.unwrap_or(true)))
}

/// `PUT /api/v1/messstellenvertraege/{melo_id}/{msb_mp_id}`
///
/// Answers `409` when the term overlaps an existing contract for the same MSB
/// and Messlokation — two simultaneously active ones would let the Kündigung
/// answer depend on row order.
pub async fn put_messstellenvertrag(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((melo_id, msb_mp_id)): Path<(String, String)>,
    Json(input): Json<pg::UpsertMessstellenvertragInput>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "write-stammdaten", ctx.tenant())?;
    match pg::upsert_messstellenvertrag(&ctx.pool, ctx.tenant(), &melo_id, &msb_mp_id, &input).await
    {
        Ok(id) => ok(serde_json::json!({ "id": id })),
        Err(e) if is_exclusion_violation(&e) => Err(ApiError::conflict(format!(
            "überlappender Messstellenvertrag für MeLo {melo_id}"
        ))),
        Err(e) => Err(ApiError::Internal(e)),
    }
}

#[derive(Deserialize)]
pub struct MessstellenvertragQuery {
    /// ISO 8601 date the next admissible Kündigungstermin is computed against;
    /// defaults to today.
    pub on: Option<Date>,
    /// Whether the Anschlussnutzer is a Haushaltskunde — decides the
    /// § 309 Nr. 9 lit. c BGB cap. Defaults to `true`, the protective reading.
    pub haushaltskunde: Option<bool>,
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
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<AggregatorvertragQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-stammdaten", ctx.tenant())?;
    if let Some(sr_id) = q.sr_id {
        let on = q.on.unwrap_or_else(mako_fristen::heute);
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
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<DeadQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-outbound-tasks", ctx.tenant())?;
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
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "retry-outbound-task", ctx.tenant())?;
    if outbound::retry_dead_lettered(&ctx.pool, ctx.tenant(), id)
        .await
        .map_err(ApiError::Internal)?
    {
        ok(serde_json::json!({ "id": id, "requeued": true }))
    } else {
        Err(ApiError::NotFound)
    }
}
