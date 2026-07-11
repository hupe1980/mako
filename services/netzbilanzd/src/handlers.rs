//! HTTP handlers for `netzbilanzd`.

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

use crate::billing::{BillingRunRequest, run_billing_internal};
use crate::pg::{approve_and_dispatch, fetch_draft, list_drafts_pg, reject_draft_pg};

// ── POST /api/v1/billing/run ──────────────────────────────────────────────────

/// `POST /api/v1/billing/run`
///
/// Generates invoice drafts for the given MaLos in the specified billing period.
/// Each draft is stored with `status = 'draft'` and validated against
/// `invoic-checker` checks 1–3 before storage.
pub async fn run_billing(
    Extension(pool): Extension<PgPool>,
    Json(req): Json<BillingRunRequest>,
) -> impl IntoResponse {
    match run_billing_internal(&pool, req).await {
        Ok(ids) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "draft_ids": ids })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── GET /api/v1/billing/drafts ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DraftsQuery {
    pub status: Option<String>,
    pub malo_id: Option<String>,
    pub nb_mp_id: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/billing/drafts`
pub async fn list_drafts(
    Extension(pool): Extension<PgPool>,
    Query(q): Query<DraftsQuery>,
) -> impl IntoResponse {
    match list_drafts_pg(
        &pool,
        q.status.as_deref(),
        q.malo_id.as_deref(),
        q.nb_mp_id.as_deref(),
        q.limit.unwrap_or(100).min(1000),
    )
    .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── GET /api/v1/billing/drafts/{id} ──────────────────────────────────────────

/// `GET /api/v1/billing/drafts/{id}`
pub async fn get_draft(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_draft(&pool, id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── PUT /api/v1/billing/drafts/{id}/dispatch ─────────────────────────────────

/// `PUT /api/v1/billing/drafts/{id}/dispatch`
///
/// Validates the draft via `invoic-checker`, then dispatches it via `makod`
/// if the check outcome is not `Dispute`.  Updates status to `dispatched`.
pub async fn dispatch_draft(
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match approve_and_dispatch(&pool, &makod, id).await {
        Ok(ref_id) => (
            StatusCode::OK,
            Json(serde_json::json!({ "dispatch_ref": ref_id })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── PUT /api/v1/billing/drafts/{id}/reject ───────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RejectRequest {
    pub reason: String,
}

/// `PUT /api/v1/billing/drafts/{id}/reject`
pub async fn reject_draft(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
    Json(req): Json<RejectRequest>,
) -> impl IntoResponse {
    match reject_draft_pg(&pool, id, &req.reason).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
