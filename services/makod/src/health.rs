//! Shared health-check handler for all `makod` HTTP servers.
//!
//! All three exposed servers (EDIFACT REST, AS4 ingest, API-Webdienste) mount
//! `GET /health` from this module so container orchestrators have a consistent
//! liveness / readiness probe path on every port.
//!
//! ## Response contract
//!
//! | Store state   | HTTP  | `"status"` |
//! |---------------|-------|------------|
//! | Alive         | `200` | `"ok"`     |
//! | Unavailable   | `503` | `"degraded"` |
//!
//! The probe is intentionally lightweight: it issues a single
//! [`SlateDbStore::kv_get`] call on a sentinel key (`"hc/ping"`) — a point
//! read that completes in microseconds when the store is open, and fails
//! immediately when the database handle is closed or the backend is
//! unreachable.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let health_state = health::HealthState::new(store.clone());
//! // Merge into every axum app before binding:
//! let app = my_router().merge(health::router(health_state));
//! ```

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use mako_engine::store_slatedb::{KvNamespace, SlateDbStore};
use serde::Serialize;
use utoipa::ToSchema;

/// Namespace for the health-check sentinel key (`hc/ping`).
const HC: KvNamespace = KvNamespace::new("hc/");

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the health handler.
#[derive(Clone)]
pub struct HealthState {
    store: SlateDbStore,
    instance_id: Arc<str>,
}

impl HealthState {
    /// Create a new [`HealthState`].
    ///
    /// `instance_id` is derived from `$HOSTNAME` and the current process ID so
    /// that load-balancer logs can identify which replica responded.
    pub fn new(store: SlateDbStore) -> Self {
        let instance_id = format!(
            "{}-{}",
            std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned()),
            std::process::id(),
        );
        Self {
            store,
            instance_id: Arc::from(instance_id.as_str()),
        }
    }
}

// ── Handler ───────────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub(crate) struct HealthResponse {
    /// `"ok"` when the store is alive; `"degraded"` when the store is unavailable.
    #[schema(value_type = String, example = "ok")]
    status: &'static str,
    /// `$HOSTNAME-$PID` of the responding `makod` instance.
    #[schema(example = "mako-prod-01-12345")]
    instance_id: String,
    /// Present only when `status == "degraded"`. Stable category string — never
    /// contains internal paths or stack traces.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "store_unavailable")]
    reason: Option<String>,
}

/// Liveness + readiness probe handler.
///
/// Performs a lightweight SlateDB read and returns:
/// - `200 OK`  `{"status":"ok","instance_id":"..."}` — store is alive.
/// - `503 Service Unavailable`  `{"status":"degraded","instance_id":"...","reason":"..."}`
///   — store is closed or unreachable.
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Store is alive", body = HealthResponse),
        (status = 503, description = "Store is unavailable", body = HealthResponse),
    )
)]
pub(crate) async fn handler(
    State(state): State<HealthState>,
) -> (StatusCode, Json<HealthResponse>) {
    match state.store.kv_get(HC, "ping").await {
        Ok(_) => (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ok",
                instance_id: String::from(&*state.instance_id),
                reason: None,
            }),
        ),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "degraded",
                instance_id: String::from(&*state.instance_id),
                // Log full error internally; expose only a stable category to
                // external clients to avoid leaking filesystem paths or internal
                // SlateDB state-machine strings.
                reason: {
                    tracing::warn!(error = %e, "health check: store unavailable");
                    Some("store_unavailable".to_owned())
                },
            }),
        ),
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build a router containing only `GET /health`.
///
/// Merge this **before** any authentication middleware layers so that
/// load-balancer probes never need credentials:
///
/// ```rust,ignore
/// let app = protected_router(state)
///     .merge(health::router(health_state));
/// ```
pub fn router(state: HealthState) -> Router {
    Router::new()
        .route("/health", get(handler))
        .with_state(state)
}
