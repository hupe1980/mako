//! `GET /metrics` — Prometheus text-format metrics endpoint.
//!
//! Exposes operational counters that help operators detect queue build-up
//! and regulatory deadline backlogs without requiring a full APM stack.
//!
//! ## Metrics exposed
//!
//! ### Gauges (polled from SlateDB on each scrape)
//!
//! | Metric | Type | Description |
//! |--------|------|-------------|
//! | `makod_outbox_pending_total` | gauge | Messages in outbox not yet delivered |
//! | `makod_deadline_pending_total` | gauge | Deadlines registered but not yet fired |
//! | `makod_overdue_deadlines_total` | gauge | Deadlines past `due_at` not yet dispatched (scheduler lag) |
//! | `makod_registry_total` | gauge | Process instances in the registry (live routing entries) |
//! | `makod_dead_letter_recent_total` | gauge | Dead-letter records in the durable DLQ (last 1000) |
//! | `makod_build_info` | gauge (1) | Build metadata (version) |
//!
//! ### Counters (from [`EngineMetrics::global()`], reset on restart)
//!
//! | Metric | Labels | Description |
//! |--------|--------|-------------|
//! | `makod_process_initiated_total` | `family` | Process instances initiated |
//! | `makod_process_completed_total` | `family`, `result` | Process instances completed |
//! | `makod_validation_failed_total` | `message_type`, `release` | AHB validation failures |
//! | `makod_outbox_delivery_attempts_total` | `result` | AS4 delivery attempts |
//! | `makod_deadline_fired_total` | `family` | Deadline scheduler firings |
//! | `makod_dead_letter_recorded_total` | `reason` | Dead-letter sink writes |
//!
//! ## Prometheus text format
//!
//! The response uses the standard Prometheus text exposition format (version
//! 0.0.4) so it integrates directly with `prometheus.io/scrape` pod
//! annotations in Kubernetes and with any Prometheus-compatible collector
//! (VictoriaMetrics, Grafana Alloy, OpenTelemetry Collector).
//!
//! ## Security
//!
//! The metrics endpoint is mounted on the same port as the operator API.
//! Cedar ABAC authorization is applied: scrape principals must hold the
//! `MaKo::Action::"AdminMaloStats"` (or equivalent metrics action) permission
//! under the active Cedar policy set.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let metrics_state = metrics_api::MetricsState::new(store.clone(), Arc::clone(&cedar));
//! let app = my_router().merge(metrics_api::router(Arc::new(metrics_state)));
//! ```

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use mako_engine::{
    deadline::DeadlineStore as _, metrics::EngineMetrics, outbox::OutboxStore as _,
    registry::ProcessRegistry as _, store_slatedb::SlateDbStore,
};

use crate::cedar_authz::{CedarAuthorizer, MetricsResource};

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the metrics handler.
#[derive(Clone)]
pub struct MetricsState {
    store: SlateDbStore,
    /// Cedar-based authorization engine.
    cedar: Arc<CedarAuthorizer>,
    /// Operator tenant identifier (GLN).
    tenant_id: String,
}

impl MetricsState {
    /// Create a new [`MetricsState`].
    pub fn new(
        store: SlateDbStore,
        cedar: Arc<CedarAuthorizer>,
        tenant_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            cedar,
            tenant_id: tenant_id.into(),
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the `/metrics` axum router.
pub fn router(state: Arc<MetricsState>) -> Router {
    Router::new()
        .route("/metrics", get(handler))
        .with_state(state)
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// Prometheus text-format metrics handler.
async fn handler(
    State(state): State<Arc<MetricsState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // ── Authentication ────────────────────────────────────────────────────────
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"))],
                "401 Unauthorized\n".to_owned(),
            );
        }
    };

    // ── Authorization: ReadMetrics ────────────────────────────────────────────
    if !state.cedar.authorize_metrics(
        &identity,
        &MetricsResource {
            tenant: &state.tenant_id,
        },
    ) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"))],
            "403 Forbidden\n".to_owned(),
        );
    }

    // Collect all metrics concurrently (independent reads).
    let deadline_store = state.store.as_deadline_store();
    let process_reg = state.store.as_process_registry();

    let (outbox_pending, deadline_pending, overdue_deadlines, registry_total, dead_letter_total) = tokio::join!(
        state.store.len(),
        deadline_store.len(),
        deadline_store.overdue_count(),
        process_reg.len(),
        state.store.list_dead_letters(1000),
    );

    let outbox_pending = outbox_pending.unwrap_or(usize::MAX);
    let deadline_pending = deadline_pending.unwrap_or(usize::MAX);
    let overdue_deadlines = overdue_deadlines.unwrap_or(usize::MAX);
    let registry_total = registry_total.unwrap_or(usize::MAX);
    let dead_letter_total = dead_letter_total.map(|v| v.len()).unwrap_or(usize::MAX);

    let version = env!("CARGO_PKG_VERSION");

    // Snapshot the per-family / per-outcome event counters from the global
    // EngineMetrics instance (AtomicU64 reads, no I/O).
    let event_counters = EngineMetrics::global().snapshot().render_prometheus();

    // Build Prometheus text exposition format (v0.0.4).
    let body = format!(
        "# HELP makod_outbox_pending_total Number of outbound messages waiting in the AS4 outbox.\n\
         # TYPE makod_outbox_pending_total gauge\n\
         makod_outbox_pending_total {outbox_pending}\n\
         # HELP makod_deadline_pending_total Number of process deadlines registered but not yet fired.\n\
         # TYPE makod_deadline_pending_total gauge\n\
         makod_deadline_pending_total {deadline_pending}\n\
         # HELP makod_overdue_deadlines_total Number of deadlines past their due_at that have not yet been dispatched (scheduler lag).\n\
         # TYPE makod_overdue_deadlines_total gauge\n\
         makod_overdue_deadlines_total {overdue_deadlines}\n\
         # HELP makod_registry_total Number of process instances in the registry (live routing entries).\n\
         # TYPE makod_registry_total gauge\n\
         makod_registry_total {registry_total}\n\
         # HELP makod_dead_letter_recent_total Number of dead-letter records in the durable DLQ (last 1000 scanned).\n\
         # TYPE makod_dead_letter_recent_total gauge\n\
         makod_dead_letter_recent_total {dead_letter_total}\n\
         # HELP makod_build_info A metric with a constant value 1 labelled with version information.\n\
         # TYPE makod_build_info gauge\n\
         makod_build_info{{version=\"{version}\"}} 1\n\
         {event_counters}",
    );

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        body,
    )
}
