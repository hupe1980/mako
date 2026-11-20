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
    model::{OrderStatus as Status, Responsible},
    pg::{
        Cancelled, CreateOrderRequest, EXECUTED_AFTER_CANCEL, ExternalRefTaken, Outcome, RefSet,
        Reported, SperrOrderView, cancel_order_pg, create_order_pg, fetch_order_pg, list_orders_pg,
        report_outcome, set_external_ref, stats_pg, validate_external_ref,
    },
};

/// Stable `code` of a report or a cancel that reached an order no longer
/// pending.
const ORDER_NOT_PENDING: &str = "order_not_pending";
/// Stable `code` of an `external_ref` another order already carries.
const EXTERNAL_REF_TAKEN: &str = "external_ref_taken";

fn rfc3339(at: Option<OffsetDateTime>) -> Option<String> {
    at.and_then(|t| {
        t.format(&time::format_description::well_known::Rfc3339)
            .ok()
    })
}

/// `409 order_not_pending`, with what the order became and when the act
/// happened — the answer to „which came first".
fn not_pending(status: Status, executed_at: Option<OffsetDateTime>) -> ApiError {
    ApiError::conflict_with(
        ORDER_NOT_PENDING,
        format!("the order is {} and no longer pending", status.as_str()),
        serde_json::json!({
            "status": status.as_str(),
            "executed_at": rfc3339(executed_at),
        }),
    )
}

fn external_ref_taken() -> ApiError {
    ApiError::conflict("another order already carries this external_ref")
        .with_code(EXTERNAL_REF_TAKEN)
}

#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    /// `pending` | `executed` | `failed` | `cancelled`.
    pub status: Option<String>,
    pub malo_id: Option<String>,
    /// The order that carries this `external_ref`.
    pub external_ref: Option<String>,
    pub limit: Option<i64>,
    /// Only orders whose day has come: the requested execution date has
    /// arrived, or the order names none (an Entsperrauftrag).
    ///
    /// This is the field-dispatch list, and it measures the **Lieferant's** date
    /// (`DTM+203 Ausführungsdatum` or `DTM+469 frühestes Startdatum`): when the
    /// LF wanted the work done. The regulatory window — 6 WT after the
    /// frühestmöglicher Sperrtermin (GPKE Teil 2 Kap. 3.5.1.2 Nr. 1,
    /// [`crate::sperr::pg::AUSFUEHRUNG_WERKTAGE`]) — is a different question and is
    /// reported separately as `frist_ueberschritten` on `/stats`.
    #[serde(default)]
    pub due: bool,
}

/// `POST /api/v1/sperr-orders`
#[utoipa::path(
    post,
    path = "/api/v1/sperr-orders",
    tag = "sperr",
    request_body = serde_json::Value,
    responses(
        (status = 201, description = "The order's id", body = serde_json::Value),
        (status = 403, description = "Cedar `create-sperr-order` refused", body = ApiError),
        (status = 409, description = "An order for this process already exists, or `external_ref_taken`", body = ApiError),
        (status = 422, description = "The order fails its validation", body = ApiError),
    ),
)]
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
    let created = match create_order_pg(&pool, &tenant, &req).await {
        Err(e) if e.is::<ExternalRefTaken>() => return Err(external_ref_taken()),
        other => other?,
    };
    let Some(id) = created else {
        // A duplicate is not a failure: the order already exists and the field
        // team already has it.
        return Err(ApiError::conflict(
            "an order for this process already exists",
        ));
    };
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response())
}

/// `GET /api/v1/sperr-orders`
#[utoipa::path(
    get,
    path = "/api/v1/sperr-orders",
    tag = "sperr",
    params(("status" = Option<String>, Query, description = "`pending`, `executed`, `failed` or `cancelled`"), ("malo_id" = Option<String>, Query, description = "One Marktlokation"), ("external_ref" = Option<String>, Query, description = "The order that carries this reference of the executing system"), ("limit" = Option<i64>, Query, description = "1–1000; 100 when absent"), ("due" = Option<bool>, Query, description = "Only orders whose day has come: the requested date has arrived, or the order names none")),
    responses(
        (status = 200, description = "The orders", body = serde_json::Value),
        (status = 400, description = "An unknown status", body = ApiError),
        (status = 403, description = "Cedar `read-sperr-order` refused", body = ApiError),
    ),
)]
pub async fn list_orders(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(lesart): Extension<crate::sperr::Lesart>,
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
        q.external_ref.as_deref(),
        q.due,
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await?;
    let views: Vec<SperrOrderView> = rows
        .into_iter()
        .map(|r| SperrOrderView::new(r, &lesart.0))
        .collect();
    Ok(Json(views).into_response())
}

/// `GET /api/v1/sperr-orders/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/sperr-orders/{id}",
    tag = "sperr",
    params(("id" = uuid::Uuid, Path, description = "The order id")),
    responses(
        (status = 200, description = "The order", body = serde_json::Value),
        (status = 403, description = "Cedar `read-sperr-order` refused", body = ApiError),
        (status = 404, description = "No such order", body = ApiError),
    ),
)]
pub async fn get_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(lesart): Extension<crate::sperr::Lesart>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    if let Err(e) = cedar.check(&claims.principal(), "read-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    let row = fetch_order_pg(&pool, id, &tenant)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(SperrOrderView::new(row, &lesart.0)).into_response())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
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
#[utoipa::path(
    put,
    path = "/api/v1/sperr-orders/{id}/execute",
    tag = "sperr",
    params(("id" = uuid::Uuid, Path, description = "The order id")),
    request_body = ExecuteRequest,
    responses(
        (status = 202, description = "Recorded; its IFTSTA 21039 is queued. With `conflict: executed_after_cancel` the order was withdrawn before the report arrived: the report is kept and announced, and no IFTSTA is queued", body = serde_json::Value),
        (status = 204, description = "Recorded; an operator-created order owes no IFTSTA"),
        (status = 403, description = "Cedar `execute-sperr-order` refused", body = ApiError),
        (status = 404, description = "No such order", body = ApiError),
        (status = 409, description = "`order_not_pending`: already executed or failed; carries `status` and `executed_at`", body = ApiError),
        (status = 422, description = "`executed_at` is not RFC 3339 or lies in the future", body = ApiError),
    ),
)]
pub async fn execute_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
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
    finish(&pool, id, &tenant, &outcome).await
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FailRequest {
    /// `SG25 FTX+ACB` — why it could not be carried out.
    pub reason: String,
    /// `SG15 STS DE9013` — the EBD Prüfschritt code from the "gescheitert"
    /// cluster (`A04`/`A05`/`A06` under EBD E_0472 for a Sperrung).
    pub pruefschritt_code: Option<String>,
    /// `true` when no further Sperrversuch will be made — a court injunction, a
    /// glaubhaft gemachter Verhinderungsgrund (Kap. 3.5.1.2 Nr. 5), or simply the
    /// field team calling it off.
    ///
    /// Defaults to `false`: GPKE Teil 2 Kap. 3.5.1.2 Nr. 5 gives the NB **two**
    /// Sperrversuche within one Sperrauftrag, so an ordinary "nobody home"
    /// report records the attempt and leaves the order in the queue. The second
    /// failure closes it regardless of this flag.
    #[serde(default)]
    pub endgueltig: bool,
    /// Whom the visit is put down to: `customer`, `contractor`, `operator` or
    /// `nobody`. Kept on the order and in its events; the Lieferant asks why.
    pub responsible: Option<Responsible>,
}

/// `PUT /api/v1/sperr-orders/{id}/fail`
///
/// A Sperrversuch that did not succeed.
///
/// The **first** one is recorded and the order stays in the queue: GPKE Teil 2
/// Kap. 3.5.1.2 Nr. 5 gives the NB two Sperrversuche within one Sperrauftrag, and
/// answering `Z13 gescheitert` after one visit ends a process that still owes a
/// second. The second attempt — or `endgueltig: true` for a legal or factual
/// impossibility — closes the order and dispatches IFTSTA 21039 so the Lieferant
/// learns *why* instead of waiting out their window.
#[utoipa::path(
    put,
    path = "/api/v1/sperr-orders/{id}/fail",
    tag = "sperr",
    params(("id" = uuid::Uuid, Path, description = "The order id")),
    request_body = FailRequest,
    responses(
        (status = 202, description = "Recorded; its IFTSTA 21039 is queued, or the order stays open for a second Sperrversuch", body = serde_json::Value),
        (status = 204, description = "Recorded; an operator-created order owes no IFTSTA"),
        (status = 403, description = "Cedar `execute-sperr-order` refused", body = ApiError),
        (status = 404, description = "No such order", body = ApiError),
        (status = 409, description = "`order_not_pending`: executed, failed or withdrawn; carries `status` and `executed_at`", body = ApiError),
        (status = 422, description = "An empty `reason`", body = ApiError),
    ),
)]
pub async fn fail_order(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
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
        responsible: req.responsible,
    };
    finish(&pool, id, &tenant, &outcome).await
}

/// Record a terminal outcome with the IFTSTA 21039 it owes.
///
/// `202 Accepted` when the IFTSTA is queued: the order **is** recorded and the
/// Auftragsstatus committed with it, but the outbox worker has yet to hand it
/// to makod, so the Lieferant has not been told yet. `204` for an
/// operator-created order, which owes no IFTSTA.
async fn finish(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    outcome: &Outcome<'_>,
) -> ApiResult<Response> {
    match report_outcome(pool, id, tenant, outcome).await? {
        Reported::NotFound => Err(ApiError::NotFound),
        Reported::NichtOffen {
            status,
            executed_at,
        } => Err(not_pending(status, executed_at)),
        // Accepted, not refused: the supply was interrupted whatever the queue
        // says. The report is on the order and announced for a person.
        Reported::NachStorno { cancelled_at } => Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "recorded": true,
                "status": "cancelled",
                "conflict": EXECUTED_AFTER_CANCEL,
                "cancelled_at": rfc3339(cancelled_at),
                "iftsta_queued": false,
            })),
        )
            .into_response()),
        // A Sperrversuch, not an outcome: the order is still owed a second
        // visit, so no IFTSTA goes out and the LF's process stays open.
        Reported::VersuchNotiert { sperrversuche } => Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "recorded": true,
                "status": "pending",
                "sperrversuche": sperrversuche,
                "iftsta_queued": false,
                "note": "Sperrversuch recorded. GPKE Teil 2 Kap. 3.5.1.2 Nr. 5 allows two \
                         attempts within one Sperrauftrag, so the order stays in the \
                         queue and no IFTSTA 21039 is sent yet. Use endgueltig=true to \
                         close it on a legal or factual impossibility.",
            })),
        )
            .into_response()),
        Reported::Recorded { iftsta_queued } => {
            if iftsta_queued {
                Ok((
                    StatusCode::ACCEPTED,
                    Json(serde_json::json!({
                        "recorded": true,
                        "iftsta_queued": true,
                        "note": "outcome recorded with its IFTSTA 21039, which the outbox \
                                 delivers to makod; `iftsta_dispatched_at` on the order says \
                                 when it went.",
                    })),
                )
                    .into_response())
            } else {
                Ok(StatusCode::NO_CONTENT.into_response())
            }
        }
    }
}

/// `PUT /api/v1/sperr-orders/{id}/cancel`
///
/// Withdraw a pending order. No IFTSTA is dispatched — nothing happened.
///
/// A cancel that reaches an order already carried out answers `409` with the
/// instant of the act, so the caller knows which came first.
#[utoipa::path(
    put,
    path = "/api/v1/sperr-orders/{id}/cancel",
    tag = "sperr",
    params(("id" = uuid::Uuid, Path, description = "The order id")),
    responses(
        (status = 204, description = "Withdrawn"),
        (status = 403, description = "Cedar `cancel-sperr-order` refused", body = ApiError),
        (status = 404, description = "No such order", body = ApiError),
        (status = 409, description = "`order_not_pending`: the act or an earlier withdrawal came first; carries `status` and `executed_at`", body = ApiError),
    ),
)]
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
        let cancelled = cancel_order_pg(&mut tx, id, &tenant).await?;
        if let Cancelled::Done(order) = &cancelled {
            events::storniert(&mut tx, &tenant, id, order).await?;
        }
        tx.commit().await?;
        anyhow::Ok(cancelled)
    }
    .await?;

    match done {
        Cancelled::Done(_) => Ok(StatusCode::NO_CONTENT),
        Cancelled::NichtOffen {
            status,
            executed_at,
        } => Err(not_pending(status, executed_at)),
        Cancelled::NotFound => Err(ApiError::NotFound),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalRefRequest {
    /// The executing system's own reference: 1–200 characters, opaque, unique
    /// per tenant.
    pub external_ref: String,
}

/// `PUT /api/v1/sperr-orders/{id}/external-ref`
///
/// Attach the executing system's reference to an order — the way an order
/// that arrived from the market gets one. Every later `de.mako.sperr.*` event of
/// the order echoes it, and `GET /api/v1/sperr-orders?external_ref=` finds it.
#[utoipa::path(
    put,
    path = "/api/v1/sperr-orders/{id}/external-ref",
    tag = "sperr",
    params(("id" = uuid::Uuid, Path, description = "The order id")),
    request_body = ExternalRefRequest,
    responses(
        (status = 204, description = "Attached"),
        (status = 403, description = "Cedar `execute-sperr-order` refused", body = ApiError),
        (status = 404, description = "No such order", body = ApiError),
        (status = 409, description = "`external_ref_taken`: another order carries it", body = ApiError),
        (status = 422, description = "Blank, longer than 200 characters, or with a control character", body = ApiError),
    ),
)]
pub async fn put_external_ref(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
    Json(req): Json<ExternalRefRequest>,
) -> ApiResult<StatusCode> {
    if let Err(e) = cedar.check(&claims.principal(), "execute-sperr-order", &tenant) {
        return Err(ApiError::forbidden(e.to_string()));
    }
    validate_external_ref(&req.external_ref).map_err(ApiError::unprocessable)?;
    match set_external_ref(&pool, id, &tenant, &req.external_ref).await? {
        RefSet::Done => Ok(StatusCode::NO_CONTENT),
        RefSet::Taken => Err(external_ref_taken()),
        RefSet::NotFound => Err(ApiError::NotFound),
    }
}

/// `GET /api/v1/sperr-orders/stats`
#[utoipa::path(
    get,
    path = "/api/v1/sperr-orders/stats",
    tag = "sperr",
    responses(
        (status = 200, description = "Counts per status and the orders past their Frist", body = serde_json::Value),
        (status = 403, description = "Cedar `read-sperr-order` refused", body = ApiError),
    ),
)]
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
/// `de.mako.engine.process.initiated`, after the read model has projected it. Returns
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
