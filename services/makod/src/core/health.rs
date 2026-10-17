//! Health probes for all `makod` HTTP servers.
//!
//! All exposed servers (EDIFACT REST, AS4 ingest, API-Webdienste) mount the
//! same three routes, so container orchestrators have a consistent probe target
//! on every port.
//!
//! ## Why three routes
//!
//! Liveness and readiness answer different questions, and a single endpoint
//! answering both makes one of them wrong. Kubernetes *restarts* a container
//! that fails liveness and merely *removes it from Service endpoints* when it
//! fails readiness. Reporting a stalled outbox worker or an unreachable object
//! store on the liveness probe turns a recoverable dependency outage into a
//! restart loop — and restarting mid-delivery costs an AS4 retry cycle without
//! fixing the object store.
//!
//! | Route | Answers | Fails when | Probe |
//! |---|---|---|---|
//! | `/health/live` | Is the process running? | never (the handler is reached) | `livenessProbe` |
//! | `/health/ready` | Can it serve traffic? | store unreachable, or a worker heartbeat is stale | `readinessProbe` |
//! | `/health` | alias of `/health/ready` | as above | compatibility |
//!
//! The readiness probe issues a single [`SlateDbStore::kv_get`] on a sentinel
//! key (`hc/ping`) — a point read that completes in microseconds when the store
//! is open and fails immediately when the handle is closed or the backend is
//! unreachable.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let health_state = health::HealthState::new(store.clone());
//! // Merge into every axum app before binding:
//! let app = my_router().merge(health::router(health_state));
//! ```

use std::sync::{Arc, RwLock};

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use mako_engine::store_slatedb::{KvNamespace, SlateDbStore};
use mako_service::Json;
use serde::Serialize;
use utoipa::ToSchema;

use crate::worker_health::WorkerWatch;

/// Namespace for the health-check sentinel key (`hc/ping`).
const HC: KvNamespace = KvNamespace::new("hc/");

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the health handlers.
#[derive(Clone)]
pub struct HealthState {
    store: SlateDbStore,
    instance_id: Arc<str>,
    worker_watches: Arc<RwLock<Vec<WorkerWatch>>>,
    gates: Arc<RwLock<Vec<ReadinessGate>>>,
}

/// A startup step `/health/ready` waits for.
///
/// Registered closed with [`HealthState::readiness_gate`]; readiness answers
/// `503 not_ready:<name>` until the step calls [`ReadinessGate::open`].
#[derive(Clone)]
pub struct ReadinessGate {
    name: &'static str,
    open: Arc<std::sync::atomic::AtomicBool>,
}

impl ReadinessGate {
    /// Mark the step done.
    pub fn open(&self) {
        self.open.store(true, std::sync::atomic::Ordering::Release);
    }

    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Acquire)
    }
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
            worker_watches: Arc::new(RwLock::new(Vec::new())),
            gates: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a closed [`ReadinessGate`] named `name`.
    #[must_use]
    pub fn readiness_gate(&self, name: &'static str) -> ReadinessGate {
        let gate = ReadinessGate {
            name,
            open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        self.gates
            .write()
            .expect("gates RwLock is never poisoned")
            .push(gate.clone());
        gate
    }

    /// Register a [`WorkerWatch`] for readiness monitoring.
    ///
    /// A stale watch flips `/health/ready` to 503. Call this after spawning each
    /// background worker.
    pub fn register_worker(&self, watch: WorkerWatch) {
        self.worker_watches
            .write()
            .expect("worker_watches RwLock is never poisoned")
            .push(watch);
    }
}

// ── Response ──────────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub(crate) struct HealthResponse {
    /// `"ok"` when the probe passes; `"degraded"` when it does not.
    #[schema(value_type = String, example = "ok")]
    status: &'static str,
    /// `$HOSTNAME-$PID` of the responding `makod` instance.
    #[schema(example = "mako-prod-01-12345")]
    instance_id: String,
    /// Daemon version (`CARGO_PKG_VERSION`).
    #[schema(example = "0.19.0")]
    version: &'static str,
    /// Present only when `status == "degraded"`. Stable category string — never
    /// contains internal paths or stack traces.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "store_unavailable")]
    reason: Option<String>,
}

impl HealthResponse {
    fn ok(state: &HealthState) -> Self {
        Self {
            status: "ok",
            instance_id: String::from(&*state.instance_id),
            version: env!("CARGO_PKG_VERSION"),
            reason: None,
        }
    }

    fn degraded(state: &HealthState, reason: String) -> Self {
        Self {
            status: "degraded",
            instance_id: String::from(&*state.instance_id),
            version: env!("CARGO_PKG_VERSION"),
            reason: Some(reason),
        }
    }
}

/// A probe report answers with the status its own verdict decides — `200` for
/// `ok`, `503` for `degraded` — so a probe that reads only the code and a
/// person reading the body see the same answer.
impl IntoResponse for HealthResponse {
    fn into_response(self) -> Response {
        let status = if self.reason.is_none() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        (status, Json(self)).into_response()
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// Liveness probe — reports that the process is running and its HTTP stack
/// responds.
///
/// Deliberately checks nothing else. A dependency this handler could fail on is
/// a dependency a restart would not fix, and Kubernetes answers a liveness
/// failure with a restart.
#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    responses((status = 200, description = "Process is alive", body = HealthResponse))
)]
pub(crate) async fn live(State(state): State<HealthState>) -> HealthResponse {
    HealthResponse::ok(&state)
}

/// Readiness probe — reports whether this instance can serve traffic.
///
/// Fails while a startup step registered as a [`ReadinessGate`] has not
/// finished, when the event store is unreachable, or when a background worker
/// has stopped heartbeating, because in each case an accepted message would not
/// be processed to the point of durability or answered from current data.
#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    responses(
        (status = 200, description = "Store is alive and all workers are healthy", body = HealthResponse),
        (status = 503, description = "Store is unavailable or a worker has stalled", body = HealthResponse),
    )
)]
pub(crate) async fn ready(State(state): State<HealthState>) -> HealthResponse {
    // 1. Startup steps and worker heartbeats first (cheap atomic reads).
    {
        let gates = state.gates.read().expect("gates RwLock is never poisoned");
        if let Some(gate) = gates.iter().find(|g| !g.is_open()) {
            return HealthResponse::degraded(&state, format!("not_ready:{}", gate.name));
        }
    }
    {
        let watches = state
            .worker_watches
            .read()
            .expect("worker_watches RwLock is never poisoned");
        for watch in watches.iter() {
            if watch.is_stale() {
                tracing::warn!(worker = watch.name, "readiness: worker heartbeat stale");
                return HealthResponse::degraded(&state, format!("worker_stale:{}", watch.name));
            }
        }
    }

    // 2. Then verify the store is alive with a sentinel key-value read.
    match state.store.kv_get(HC, "ping").await {
        Ok(_) => HealthResponse::ok(&state),
        Err(e) => {
            // Log the full error internally; expose only a stable category
            // externally, so the response never leaks filesystem paths or
            // internal SlateDB state-machine strings.
            tracing::warn!(error = %e, "readiness: store unavailable");
            HealthResponse::degraded(&state, "store_unavailable".to_owned())
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build a router containing the health routes.
///
/// Merge this **before** any authentication middleware layer so that
/// load-balancer probes never need credentials:
///
/// ```rust,ignore
/// let app = protected_router(state)
///     .merge(health::router(health_state));
/// ```
///
/// Note that the caller must also keep these routes out of the per-peer rate
/// limiter — see `is_health_path`.
pub fn router(state: HealthState) -> Router {
    Router::new()
        .route("/health", get(ready))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(state)
}

/// `true` when `path` is one of the health routes.
///
/// The rate limiters key on the peer address. On the AS4 port that peer is a
/// trading partner, but on any port behind a proxy or a shared NAT it can be the
/// same address the orchestrator probes from — and a throttled probe reads as a
/// dead container. Health checks are cheap and unauthenticated by design, so
/// they are exempt.
#[must_use]
pub fn is_health_path(path: &str) -> bool {
    matches!(path, "/health" | "/health/live" | "/health/ready")
}
