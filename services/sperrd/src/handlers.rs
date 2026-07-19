//! HTTP handlers for `sperrd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::makod_client::MakodClient;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    config::Tenant,
    pg::{
        CreateOrderRequest, cancel_order_pg, create_order_pg, execute_order_pg, fail_order_pg,
        fetch_order_pg, list_orders_pg, stats_pg,
    },
};

#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    pub status: Option<String>,
    pub malo_id: Option<String>,
    pub limit: Option<i64>,
    /// Filter to orders created more than `older_than_hours` hours ago.
    ///
    /// Used by the sperrd-agent daily compliance sweep:
    /// `?status=pending&older_than_hours=48` returns stuck orders past the
    /// 2-Werktage BK6-22-024 deadline.
    pub older_than_hours: Option<i64>,
}

/// `POST /api/v1/sperr-orders`
pub async fn create_order(
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Json(req): Json<CreateOrderRequest>,
) -> impl IntoResponse {
    match create_order_pg(&pool, &tenant, req).await {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/sperr-orders`
pub async fn list_orders(
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<OrdersQuery>,
) -> impl IntoResponse {
    match list_orders_pg(
        &pool,
        &tenant,
        q.status.as_deref(),
        q.malo_id.as_deref(),
        q.older_than_hours,
        q.limit.unwrap_or(100).min(1000),
    )
    .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/sperr-orders/{id}`
pub async fn get_order(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_order_pg(&pool, id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    /// Field-service confirmation note (optional).
    pub note: Option<String>,
    /// Actual execution timestamp (RFC 3339; defaults to now()).
    pub executed_at: Option<String>,
}

/// `PUT /api/v1/sperr-orders/{id}/execute`
///
/// Reports that the field team executed the disconnection/reconnection.
/// Auto-dispatches IFTSTA 21039 to `makod`.
pub async fn execute_order(
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Path(id): Path<Uuid>,
    Json(req): Json<ExecuteRequest>,
) -> impl IntoResponse {
    match execute_order_pg(
        &pool,
        &makod,
        id,
        req.note.as_deref(),
        req.executed_at.as_deref(),
    )
    .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct FailRequest {
    pub reason: String,
}

/// `PUT /api/v1/sperr-orders/{id}/fail`
///
/// Reports a field failure — escalates to operator review.
///
/// Auto-dispatches IFTSTA 21039 reporting non-execution, so the Lieferant learns
/// why the Sperrung did not happen instead of waiting out its 24-hour deadline
/// (GPKE BK6-22-024 §5).
pub async fn fail_order(
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Path(id): Path<Uuid>,
    Json(req): Json<FailRequest>,
) -> impl IntoResponse {
    match fail_order_pg(&pool, &makod, id, &req.reason).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/sperr-orders/{id}/cancel`
///
/// Operator-initiated cancellation of a pending order.
///
/// Only `pending` orders can be cancelled. Once `executed` or `failed`,
/// the order is terminal. No IFTSTA is dispatched — cancelled orders were
/// never executed.
pub async fn cancel_order(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match cancel_order_pg(&pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/sperr-orders/stats`
///
/// Aggregate statistics for Sperrung orders: counts by status, overdue pending
/// orders (planned_date < today), and executed orders missing IFTSTA dispatch.
///
/// Used by monitoring dashboards and the `sperrd-agent` compliance sweep.
pub async fn get_stats(
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
) -> impl IntoResponse {
    match stats_pg(&pool, &tenant).await {
        Ok(stats) => Json(stats).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
