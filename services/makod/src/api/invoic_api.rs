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
//! `wim-invoic` stores the `Rechnung` BO4E object inside the
//! `WimInvoicReceived` event payload. This endpoint loads the event
//! stream for the given `process_id`, finds that event, and returns the
//! embedded `rechnung` field.
//!
//! This provides a resilient fallback for `invoicd` and enables operator
//! inspection without raw EDIFACT access.

use std::sync::Arc;

use axum::extract::{Path, State};
use mako_engine::{
    event_store::EventStore as _,
    ids::{ProcessId, StreamId, TenantId},
    store_slatedb::SlateDbStore,
};
use mako_service::{ApiError, ApiResult};
use uuid::Uuid;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct InvoicApiState {
    pub store: Arc<SlateDbStore>,
    pub tenant_id: TenantId,
    /// Cedar-based authorization engine — the stored BO4E `Rechnung` carries
    /// customer billing data and must never be an unauthenticated read.
    pub cedar: Arc<crate::cedar_authz::CedarAuthorizer>,
    /// Operator tenant (MP-ID) — the Cedar resource scope.
    pub tenant: String,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// The route with its document entry.
pub fn routes<S: Clone + Send + Sync + 'static>(
    state: Arc<InvoicApiState>,
) -> utoipa_axum::router::OpenApiRouter<S> {
    utoipa_axum::router::OpenApiRouter::new()
        .routes(utoipa_axum::routes!(get_rechnung))
        .with_state(state)
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/invoic/{process_id}/rechnung`
///
/// Returns the BO4E `Rechnung` object embedded in the `WimInvoicReceived`
/// event for the given process.
///
/// - **200 OK** — JSON `Rechnung` (BO4E v202607 schema)
/// - **404 Not Found** — process does not exist or has no `InvoicReceived` event
/// - **422 Unprocessable Entity** — `process_id` is not a valid UUID
/// - **500** — the stream could not be read, or its `WimInvoicReceived` event
///   carries no `rechnung` (the adapter embeds one on every such event, so its
///   absence is a malformed stream, not an invoice that has yet to arrive)
#[utoipa::path(
    get,
    path = "/api/v1/invoic/{process_id}/rechnung",
    tag = "invoic",
    params(("process_id" = String, Path, description = "The WiM billing process (UUID)")),
    responses(
        (status = 200, description = "The BO4E `Rechnung` the INVOIC carried", body = serde_json::Value),
        (status = 401, description = "No valid bearer token", body = mako_service::ApiError),
        (status = 403, description = "Cedar `ReadRechnung` denied", body = mako_service::ApiError),
        (status = 404, description = "No such process, or no INVOIC received on it", body = mako_service::ApiError),
        (status = 422, description = "`process_id` is not a UUID", body = mako_service::ApiError),
        (status = 500, description = "The stream could not be read or is malformed", body = mako_service::ApiError),
    ),
    security(("bearer_token" = []))
)]
async fn get_rechnung(
    State(state): State<Arc<InvoicApiState>>,
    headers: axum::http::HeaderMap,
    Path(process_id_str): Path<String>,
) -> ApiResult<axum::Json<serde_json::Value>> {
    let identity = state
        .cedar
        .authenticate(&headers)
        .ok_or(ApiError::Unauthorized)?;
    if !state.cedar.authorize_rechnung(
        &identity,
        &crate::cedar_authz::RechnungResource {
            tenant: &state.tenant,
        },
    ) {
        return Err(ApiError::forbidden("ReadRechnung permission denied"));
    }
    let process_uuid: Uuid = process_id_str
        .parse()
        .map_err(|_| ApiError::unprocessable("process_id must be a valid UUID v4"))?;
    let process_id = ProcessId::from(process_uuid);
    let stream_id = StreamId::for_process(state.tenant_id, &process_id);

    let events = state.store.load(&stream_id).await.map_err(|e| {
        ApiError::internal(
            anyhow::Error::new(e).context(format!("invoic_api: load the stream of {process_uuid}")),
        )
    })?;

    // Find the first WimInvoicReceived event and extract the rechnung. No
    // events at all, or none of that type: the invoice has not arrived.
    let Some(env) = events
        .iter()
        .find(|env| env.event_type.as_ref() == "WimInvoicReceived")
    else {
        return Err(ApiError::NotFound);
    };
    env.payload
        .get("data")
        .and_then(|d| d.get("rechnung"))
        .cloned()
        .map(axum::Json)
        .ok_or_else(|| {
            ApiError::internal(anyhow::anyhow!(
                "invoic_api: the WimInvoicReceived event of {process_uuid} carries no rechnung"
            ))
        })
}
