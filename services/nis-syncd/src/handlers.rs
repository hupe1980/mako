//! HTTP handlers for `nis-syncd`.

use std::sync::Arc;

use axum::{Extension, Json, extract::Query, http::StatusCode, response::IntoResponse};
use mako_markt::marktd_client::MarktdClient;
use serde::Deserialize;

use crate::sync::{LastSyncReport, NisEntry, run_sync};

/// Extension alias for the NB MP-ID injected at startup.
pub type NbMpId = String;

/// Optional drift webhook URL — `None` when not configured.
pub type DriftWebhookUrl = Option<String>;

/// Handler-level configuration injected as an Axum `Extension`.
///
/// Fields are read from `nis-syncd.toml` and shared across all requests.
#[derive(Clone)]
pub struct HandlerConfig {
    /// Max concurrent `marktd` PUT calls per sync pass (default: 20).
    pub sync_concurrency: usize,
    /// Maximum entries per single sync request body (default: 50 000).
    ///
    /// Protects against accidental overload of `marktd` and memory exhaustion
    /// from unbounded JSON payloads.
    pub max_batch_size: usize,
}

impl Default for HandlerConfig {
    fn default() -> Self {
        Self {
            sync_concurrency: 20,
            max_batch_size: 50_000,
        }
    }
}

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
    Extension(hcfg): Extension<HandlerConfig>,
    Extension(last_report): Extension<LastSyncReport>,
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

    if req.entries.len() > hcfg.max_batch_size {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "error": format!(
                    "batch too large: {} entries exceeds max {}",
                    req.entries.len(),
                    hcfg.max_batch_size
                )
            })),
        )
            .into_response();
    }

    // Validate each entry before any I/O
    for entry in &req.entries {
        if let Err(e) = entry.validate() {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid entry: {e}") })),
            )
                .into_response();
        }
    }

    let report = run_sync(
        client,
        &nb_mp_id,
        &req.entries,
        q.dry_run,
        drift_webhook_url.as_deref(),
        hcfg.sync_concurrency,
    )
    .await;

    // Cache latest report for MCP introspection (best-effort; non-fatal if lock poisoned).
    *last_report.write().await = Some(report.clone());

    let status = if report.errors.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    };

    (status, Json(report)).into_response()
}
