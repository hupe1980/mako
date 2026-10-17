//! HTTP handlers of the `sperr` module.
//!
//! Every route is authenticated (`Claims`) **and** authorized (Cedar);
//! `tests/it/sperr/authorization_guard.rs` pins both. Authentication alone would let a
//! valid token from any tenant order a disconnection in this operator's name.

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mako_markt::makod_client::MakodClient;
use mako_service::Json;
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use mako_service::{ApiError, ApiResult};
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::sperr::{
    Tenant, events,
    model::OrderStatus,
    pg::{
        CreateOrderRequest, Outcome, Reported, cancel_order_pg, create_order_pg, fetch_order_pg,
        list_orders_pg, report_outcome, stats_pg,
    },
};

#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    /// `pending` | `executed` | `failed` | `cancelled`.
    pub status: Option<String>,
    pub malo_id: Option<String>,
    pub limit: Option<i64>,
    /// Only orders whose requested execution date has arrived.
    ///
    /// This is the field-dispatch list, and it measures the **Lieferant's** date
    /// (`DTM+203 Ausführungsdatum` or `DTM+469 frühestes Startdatum`): when the
    /// LF wanted the work done. The regulatory window — 6 WT after the
    /// frühestmöglicher Sperrtermin (GPKE Teil 2 § 3.5.1.2 Nr. 1,
    /// [`crate::sperr::pg::AUSFUEHRUNG_WERKTAGE`]) — is a different question and is
    /// reported separately as `frist_ueberschritten` on `/stats`.
    #[serde(default)]
    pub due: bool,
}

/// `POST /api/v1/sperr-orders`
pub async fn create_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Json(req): Json<CreateOrderRequest>,
) -> ApiResult<Response> {
    if let Err(e) = cedar.check(&claims.principal(), "create-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    req.validate().map_err(ApiError::unprocessable)?;
    let Some(id) = create_order_pg(&pool, &tenant, &req).await? else {
        // A duplicate is not a failure: the order already exists and the field
        // team already has it.
        return Err(ApiError::conflict(
            "an order for this process already exists",
        ));
    };
    events::auftrag_eingegangen(&pool, &tenant, id, &req).await;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response())
}

/// `GET /api/v1/sperr-orders`
pub async fn list_orders(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<OrdersQuery>,
) -> ApiResult<Response> {
    if let Err(e) = cedar.check(&claims.principal(), "read-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    // An unknown status is a client error, not a filter that quietly matches
    // nothing and looks like "no work outstanding".
    let status = q
        .status
        .as_deref()
        .map(str::parse::<OrderStatus>)
        .transpose()
        .map_err(ApiError::bad_request)?;
    let rows = list_orders_pg(
        &pool,
        &tenant,
        status,
        q.malo_id.as_deref(),
        q.due,
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await?;
    Ok(Json(rows).into_response())
}

/// `GET /api/v1/sperr-orders/{id}`
pub async fn get_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    if let Err(e) = cedar.check(&claims.principal(), "read-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    let row = fetch_order_pg(&pool, id, &tenant)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(row).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteRequest {
    /// `SG25 FTX+ACB` — the field reference the LF sees.
    pub note: Option<String>,
    /// `DTM+293 Fertigstellungsdatum` (RFC 3339). Defaults to now.
    pub executed_at: Option<String>,
    /// `SG15 STS DE9013` — the EBD Prüfschritt code for the "erfolgreich"
    /// cluster. The AHB makes it a **Muss**, so an IFTSTA built without one is
    /// invalid; it is optional here only because an operator-created order has
    /// no market correspondent to send an IFTSTA to.
    pub pruefschritt_code: Option<String>,
}

/// `PUT /api/v1/sperr-orders/{id}/execute`
///
/// The field team carried the order out. Records the outcome and dispatches
/// IFTSTA 21039 (`STS Z14 erfolgreich`).
pub async fn execute_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
    Json(req): Json<ExecuteRequest>,
) -> ApiResult<Response> {
    if let Err(e) = cedar.check(&claims.principal(), "execute-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    let executed_at = match req.executed_at.as_deref() {
        Some(s) => OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|e| ApiError::unprocessable(format!("executed_at is not RFC 3339: {e}")))?,
        None => OffsetDateTime::now_utc(),
    };
    // IFTSTA AHB 2.1, condition [495]: DTM+293 must be ≤ the document date. A
    // future Fertigstellungszeitpunkt produces an AHB-invalid message that the
    // Lieferant's validator rejects — and a field report from the future is
    // wrong regardless. Caught here rather than at the recipient.
    if executed_at > OffsetDateTime::now_utc() {
        return Err(ApiError::unprocessable(
            "executed_at lies in the future; DTM+293 Fertigstellungsdatum must be \
             at or before the IFTSTA document date (AHB 2.1 condition [495])",
        ));
    }

    let outcome = Outcome::Executed {
        at: executed_at,
        note: req.note.as_deref(),
        pruefschritt_code: req.pruefschritt_code.as_deref(),
    };
    finish(&pool, &makod, id, &tenant, &outcome).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailRequest {
    /// `SG25 FTX+ACB` — why it could not be carried out.
    pub reason: String,
    /// `SG15 STS DE9013` — the EBD Prüfschritt code from the "gescheitert"
    /// cluster (`A04`/`A05`/`A06` under EBD E_0472 for a Sperrung).
    pub pruefschritt_code: Option<String>,
    /// `true` when no further Sperrversuch will be made — a court injunction, a
    /// glaubhaft gemachter Verhinderungsgrund (§ 3.5.1.2 Nr. 5), or simply the
    /// field team calling it off.
    ///
    /// Defaults to `false`: GPKE Teil 2 § 3.5.1.2 Nr. 5 gives the NB **two**
    /// Sperrversuche within one Sperrauftrag, so an ordinary "nobody home"
    /// report records the attempt and leaves the order in the queue. The second
    /// failure closes it regardless of this flag.
    #[serde(default)]
    pub endgueltig: bool,
}

/// `PUT /api/v1/sperr-orders/{id}/fail`
///
/// A Sperrversuch that did not succeed.
///
/// The **first** one is recorded and the order stays in the queue: GPKE Teil 2
/// § 3.5.1.2 Nr. 5 gives the NB two Sperrversuche within one Sperrauftrag, and
/// answering `Z13 gescheitert` after one visit ends a process that still owes a
/// second. The second attempt — or `endgueltig: true` for a legal or factual
/// impossibility — closes the order and dispatches IFTSTA 21039 so the Lieferant
/// learns *why* instead of waiting out their window.
pub async fn fail_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
    Json(req): Json<FailRequest>,
) -> ApiResult<Response> {
    if let Err(e) = cedar.check(&claims.principal(), "execute-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    if req.reason.trim().is_empty() {
        return Err(ApiError::unprocessable(
            "reason must not be empty — it becomes the SG25 FTX+ACB free text the \
             Lieferant reads",
        ));
    }
    let outcome = Outcome::Failed {
        reason: req.reason.trim(),
        pruefschritt_code: req.pruefschritt_code.as_deref(),
        endgueltig: req.endgueltig,
    };
    finish(&pool, &makod, id, &tenant, &outcome).await
}

/// Record a terminal outcome and report what happened to the IFTSTA.
///
/// `202 Accepted` rather than `204` when the dispatch did not go through: the
/// order **is** recorded — the field team's report is not thrown away because a
/// downstream service was unreachable — but the Lieferant has not been told yet.
/// The response says which of the two it was, so the caller does not read a
/// queued IFTSTA as a delivered one.
async fn finish(
    pool: &PgPool,
    makod: &Arc<MakodClient>,
    id: Uuid,
    tenant: &str,
    outcome: &Outcome<'_>,
) -> ApiResult<Response> {
    match report_outcome(pool, makod, id, tenant, outcome).await? {
        Reported::NotFound => Err(ApiError::NotFound),
        // A Sperrversuch, not an outcome: the order is still owed a second
        // visit, so no IFTSTA goes out and the LF's process stays open.
        Reported::VersuchNotiert {
            sperrversuche,
            malo_id,
        } => {
            if let Outcome::Failed { reason, .. } = outcome {
                events::versuch_gescheitert(pool, tenant, id, &malo_id, reason, sperrversuche)
                    .await;
            }
            Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "recorded": true,
                    "status": "pending",
                    "sperrversuche": sperrversuche,
                    "iftsta_dispatched": false,
                    "note": "Sperrversuch recorded. GPKE Teil 2 § 3.5.1.2 Nr. 5 allows two \
                             attempts within one Sperrauftrag, so the order stays in the \
                             queue and no IFTSTA 21039 is sent yet. Use endgueltig=true to \
                             close it on a legal or factual impossibility.",
                })),
            )
                .into_response())
        }
        Reported::Recorded { iftsta_dispatched } => {
            events::outcome(pool, tenant, id, outcome, iftsta_dispatched).await;
            if iftsta_dispatched {
                Ok(StatusCode::NO_CONTENT.into_response())
            } else {
                Ok((
                    StatusCode::ACCEPTED,
                    Json(serde_json::json!({
                        "recorded": true,
                        "iftsta_dispatched": false,
                        "note": "outcome recorded; IFTSTA 21039 dispatch failed and is \
                                 queued for retry. The Lieferant has not been told yet.",
                    })),
                )
                    .into_response())
            }
        }
    }
}

/// `PUT /api/v1/sperr-orders/{id}/cancel`
///
/// Withdraw a pending order. No IFTSTA is dispatched — nothing happened.
pub async fn cancel_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    if cedar
        .check(&claims.principal(), "cancel-sperr-order", &tenant)
        .is_err()
    {
        return Err(ApiError::Forbidden);
    }
    let done = async {
        let mut tx = pool.begin().await?;
        let cancelled = cancel_order_pg(&mut *tx, id, &tenant).await?;
        if let Some((malo_id, lf_mp_id)) = cancelled.as_ref() {
            events::storniert(&mut tx, &tenant, id, malo_id, lf_mp_id).await?;
        }
        tx.commit().await?;
        anyhow::Ok(cancelled)
    }
    .await?;

    if done.is_some() {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// `GET /api/v1/sperr-orders/stats`
pub async fn get_stats(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
) -> ApiResult<Response> {
    if let Err(e) = cedar.check(&claims.principal(), "read-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    Ok(Json(stats_pg(&pool, &tenant).await?).into_response())
}

// ── Market ingest ─────────────────────────────────────────────────────────────

/// Turn an inbound **ORDERS 17115 Sperrauftrag** or **17117 Entsperrauftrag**
/// into a work order.
///
/// Called by processd's one `POST /webhook` for every verified
/// `de.mako.process.initiated`, after the read model has projected it. Returns
/// `Ok(None)` when the event is not an executable Sperr-/Entsperrauftrag, so
/// the webhook hands it on to the decision modules.
///
/// Without this hop the only way an order entered the queue would be an
/// operator POSTing one by hand, so a Sperrauftrag arriving over AS4 from a
/// third-party Lieferant would spawn a `makod` process, register a
/// correlation, and then reach nobody: no field dispatch, no execution, and no
/// IFTSTA 21039 — while the LF's own process waits for one.
///
/// # Errors
///
/// `422` when the ORDERS fails AHB validation; a store failure as `5xx`, so the
/// fan-out redelivers.
pub async fn ingest(
    pool: &PgPool,
    tenant: &str,
    event: &serde_json::Value,
) -> ApiResult<Option<Response>> {
    let Some(req) = crate::sperr::ingest::order_from_process_initiated(event) else {
        // Not ours. Every other process kind lands here too, so this is the
        // normal case, not a failure.
        return Ok(None);
    };
    if let Err(e) = req.validate() {
        tracing::warn!(error = %e, "processd sperr: inbound ORDERS failed AHB validation");
        return Err(ApiError::unprocessable(e));
    }

    match create_order_pg(pool, tenant, &req).await? {
        Some(id) => {
            events::auftrag_eingegangen(pool, tenant, id, &req).await;
            tracing::info!(
                order_id = %id, malo_id = %req.malo_id, pid = req.order_type.pid(),
                "processd sperr: ORDERS queued for field execution"
            );
            Ok(Some(
                (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
            ))
        }
        // Redelivery. AS4 ReceptionAwareness re-sends, so this is expected.
        None => Ok(Some(StatusCode::NO_CONTENT.into_response())),
    }
}
