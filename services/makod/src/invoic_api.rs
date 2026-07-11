//! REST API for INVOIC-related reads from `makod`.
//!
//! ## Routes
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | `GET`  | `/api/v1/invoic/{process_id}/rechnung` | Return the BO4E `Rechnung` for a WiM billing process |
//!
//! ## Design
//!
//! `wim-rechnung` stores the `Rechnung` BO4E object inside the
//! `WimRechnungInvoicReceived` event payload. This endpoint loads the event
//! stream for the given `process_id`, finds that event, and returns the
//! embedded `rechnung` field.
//!
//! This provides a resilient fallback for `invoicd` and enables operator
//! inspection without raw EDIFACT access.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use mako_engine::{
    event_store::EventStore as _,
    ids::{ProcessId, StreamId, TenantId},
    store_slatedb::SlateDbStore,
};
use uuid::Uuid;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct InvoicApiState {
    pub store: Arc<SlateDbStore>,
    pub tenant_id: TenantId,
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: Arc<InvoicApiState>) -> Router {
    Router::new()
        .route("/api/v1/invoic/{process_id}/rechnung", get(get_rechnung))
        .with_state(state)
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/invoic/{process_id}/rechnung`
///
/// Returns the BO4E `Rechnung` object embedded in the `WimRechnungInvoicReceived`
/// event for the given process.
///
/// - **200 OK** — JSON `Rechnung` (BO4E v202607 schema)
/// - **404 Not Found** — process does not exist or has no `InvoicReceived` event
/// - **422 Unprocessable Entity** — `process_id` is not a valid UUID
async fn get_rechnung(
    State(state): State<Arc<InvoicApiState>>,
    Path(process_id_str): Path<String>,
) -> impl IntoResponse {
    let process_uuid: Uuid = match process_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "INVALID_PROCESS_ID",
                    "message": "process_id must be a valid UUID v4"
                })),
            )
                .into_response();
        }
    };
    let process_id = ProcessId::from(process_uuid);
    let stream_id = StreamId::for_process(state.tenant_id, &process_id);

    let events = match state.store.load(&stream_id).await {
        Ok(evs) => evs,
        Err(e) => {
            tracing::warn!(
                process_id = %process_uuid,
                error = %e,
                "invoic_api: failed to load event stream"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "error": "STORE_ERROR",
                    "message": "failed to load event stream"
                })),
            )
                .into_response();
        }
    };

    if events.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "error": "PROCESS_NOT_FOUND",
                "message": format!("no events found for process {process_uuid}")
            })),
        )
            .into_response();
    }

    // Find the first WimRechnungInvoicReceived event and extract the rechnung.
    for env in &events {
        if env.event_type.as_ref() == "WimRechnungInvoicReceived" {
            if let Some(rechnung) = env.payload.get("data").and_then(|d| d.get("rechnung")) {
                return (StatusCode::OK, axum::Json(rechnung.clone())).into_response();
            }
            // rechnung field missing — event was written before this field existed
            return (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({
                    "error": "RECHNUNG_NOT_EMBEDDED",
                    "message": "InvoicReceived event predates rechnung embedding (process started before FV2025-10-01 cutover)"
                })),
            )
                .into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": "INVOIC_NOT_RECEIVED",
            "message": format!("no WimRechnungInvoicReceived event in stream for process {process_uuid}")
        })),
    )
        .into_response()
}
