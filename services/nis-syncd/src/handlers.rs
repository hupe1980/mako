//! HTTP handlers for `nis-syncd`.

use std::sync::Arc;

use axum::{Extension, Json, extract::Query, http::StatusCode, response::IntoResponse};
use mako_markt::marktd_client::MarktdClient;
use serde::Deserialize;

use crate::sync::{NisEntry, run_sync};

/// Extension alias for the NB MP-ID injected at startup.
pub type NbMpId = String;

/// Optional drift webhook URL — `None` when not configured.
pub type DriftWebhookUrl = Option<String>;

/// Query parameters for `POST /api/v1/grid/sync`.
#[derive(Debug, Deserialize, Default)]
pub struct SyncQuery {
    /// When `true`, compare without writing to `marktd`.
    #[serde(default)]
    pub dry_run: bool,
}

/// Request body for `POST /api/v1/grid/sync`.
#[derive(Debug, serde::Deserialize)]
pub struct SyncRequest {
    /// NIS export entries — one per MaLo.
    pub entries: Vec<NisEntry>,
}

/// `POST /api/v1/grid/sync`
///
/// Accepts a NIS/GIS export and pushes each `malo_grid` record to `marktd`.
///
/// Pass `?dry_run=true` to simulate without writing (returns `SyncReport` with
/// all records counted as `skipped` and `drift_detected` set if any differ).
///
/// When drift is detected and `drift_webhook_url` is configured,
/// a `de.markt.grid.drift.detected` CloudEvent 1.0 is emitted (fire-and-forget).
pub async fn sync_grid(
    Extension(client): Extension<Arc<MarktdClient>>,
    Extension(nb_mp_id): Extension<NbMpId>,
    Extension(drift_webhook_url): Extension<DriftWebhookUrl>,
    Query(q): Query<SyncQuery>,
    Json(req): Json<SyncRequest>,
) -> impl IntoResponse {
    if req.entries.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "entries must not be empty" })),
        )
            .into_response();
    }

    let report = run_sync(
        &client,
        &nb_mp_id,
        &req.entries,
        q.dry_run,
        drift_webhook_url.as_deref(),
    )
    .await;

    let status = if report.errors.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    };

    (status, Json(report)).into_response()
}
