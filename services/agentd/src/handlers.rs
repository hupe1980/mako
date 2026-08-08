//! HTTP handlers — CloudEvent webhook + manual run.

use crate::plane::{AgentDecision, Plane};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_service::oidc::{Claims, OidcVerifier};
use secrecy::ExposeSecret;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::config::AgentdConfig;

// ── Session ring buffer ────────────────────────────────────────────────────

/// In-memory ring buffer of the last `capacity` `AgentDecision` results.
///
/// Thread-safe via `std::sync::Mutex` — the lock is held only for the
/// duration of a `VecDeque` push or clone, making `parking_lot` unnecessary.
pub struct SessionStore {
    inner: Mutex<VecDeque<AgentDecision>>,
    capacity: usize,
}

impl SessionStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Append a decision; silently evicts the oldest entry when at capacity.
    pub fn push(&self, decision: AgentDecision) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= self.capacity {
            guard.pop_front();
        }
        guard.push_back(decision);
    }

    /// Snapshot of all stored decisions, oldest first.
    pub fn list(&self) -> Vec<AgentDecision> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

// ── AppState ──────────────────────────────────────────────────────────────────

/// Shared application state injected into all handlers via `axum::extract::State`.
pub struct AppState {
    pub cfg: AgentdConfig,
    /// The agentplane runtime and its routing table.
    ///
    /// Replaces the orchestrator, registry, session loop, MCP pool and
    /// dead-letter queue: routing is the only part agentplane does not do, and
    /// a failed run resumes from its journal rather than landing in a queue.
    pub plane: Plane,
    /// In-memory ring buffer of the last 100 agent decisions (best-effort; not persisted).
    pub sessions: SessionStore,
    /// OIDC verifier (None = dev mode, all requests accepted with warning).
    pub oidc: Option<Arc<OidcVerifier>>,
    /// Semaphore limiting concurrent agent sessions to `cfg.max_sessions`.
    pub session_sem: Arc<Semaphore>,
    /// CloudEvent-id dedup window (at-least-once delivery must not double-spawn).
    pub seen_events: SeenEvents,
}

/// Bounded, TTL-windowed CloudEvent-id dedup set.
///
/// Inbound webhooks are at-least-once: the emitter retries until it sees a
/// 2xx, so the same `ce_id` can arrive more than once. One agent session per
/// event id within the window; entries expire after `ttl` and the set is
/// capped so a flood of unique ids cannot grow memory unboundedly.
#[derive(Clone)]
pub struct SeenEvents {
    inner: Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    ttl: std::time::Duration,
    capacity: usize,
}

impl SeenEvents {
    #[must_use]
    pub fn new(ttl: std::time::Duration, capacity: usize) -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            ttl,
            capacity,
        }
    }

    /// `true` when this id has not been seen within the TTL window (and
    /// records it); `false` for a duplicate.
    pub fn first_seen(&self, id: &str) -> bool {
        let now = std::time::Instant::now();
        let mut map = self.inner.lock().expect("seen_events mutex");
        map.retain(|_, t| now.duration_since(*t) < self.ttl);
        if map.contains_key(id) {
            return false;
        }
        if map.len() >= self.capacity {
            // Evict the oldest entry — losing dedup for the oldest id beats
            // unbounded growth under an id flood.
            if let Some(oldest) = map.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone()) {
                map.remove(&oldest);
            }
        }
        map.insert(id.to_owned(), now);
        true
    }
}

pub async fn webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── Inbound HMAC verification ────────────────────────────────────────
    if let Some(ref secret) = state.cfg.inbound_hmac_secret {
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !mako_service::webhook::verify_hmac(secret.expose_secret().as_bytes(), &body, provided) {
            tracing::warn!("agentd: inbound webhook HMAC mismatch — rejected");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    } else {
        tracing::warn!("agentd: inbound_hmac_secret not set — accepting all webhooks (dev mode)");
    }

    // ── Parse CloudEvent ─────────────────────────────────────────────────
    let event: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "agentd: malformed CloudEvent body");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let event_type = event["type"].as_str().unwrap_or("unknown").to_owned();
    let event_id = event["id"].as_str().unwrap_or("unknown").to_owned();
    let data = event["data"].clone();

    // Tenant binding: a CloudEvent carrying a `tenantid` extension for a
    // different operator must not spawn a session under our tenant.
    if let Some(ev_tenant) = event["tenantid"].as_str()
        && ev_tenant != state.cfg.tenant
    {
        tracing::warn!(event_id, ev_tenant, "agentd: tenant mismatch — rejected");
        return StatusCode::FORBIDDEN.into_response();
    }

    // Duplicate suppression: at-least-once event delivery must not spawn a
    // second session for the same CloudEvent id within the dedup window.
    if !state.seen_events.first_seen(&event_id) {
        tracing::info!(event_id, "agentd: duplicate CloudEvent suppressed");
        return StatusCode::ACCEPTED.into_response();
    }

    if !state
        .cfg
        .trigger_event_types
        .iter()
        .any(|t| mako_events::matches(t, &event_type))
    {
        tracing::debug!(event_type, "agentd: ignoring non-trigger event");
        return StatusCode::NO_CONTENT.into_response();
    }

    // ── Session concurrency cap ──────────────────────────────────────────
    let permit = match Arc::clone(&state.session_sem).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!(
                max_sessions = state.cfg.max_sessions,
                "agentd: max_sessions reached — dropping webhook event"
            );
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    };

    tracing::info!(event_type, event_id, "agentd: trigger received");
    let state2 = Arc::clone(&state);
    let timeout_secs = state.cfg.session_timeout_secs;
    tokio::spawn(async move {
        let _permit = permit; // released when the task completes

        // One run per subscribing specialist. There is no dead-letter queue:
        // a run that fails is durable, and resumes from its last completed
        // effect rather than being replayed from the top by a retry loop.
        let decisions = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            state2.plane.dispatch(&event_id, &event_type, data),
        )
        .await
        {
            Ok(d) => d,
            Err(_) => {
                tracing::warn!(timeout_secs, "agentd: dispatch timed out");
                vec![AgentDecision {
                    agent_name: "timeout".into(),
                    session_id: String::new(),
                    event_id: event_id.clone(),
                    event_type: event_type.clone(),
                    outcome: "timeout".into(),
                    summary: format!("Dispatch exceeded {timeout_secs}s wall-clock limit."),
                    tool_calls: 0,
                    turns: 0,
                    handoff_to: None,
                }]
            }
        };

        for decision in &decisions {
            emit_audit(&state2, decision).await;
        }
    });
    StatusCode::ACCEPTED.into_response()
}

pub async fn manual_run(
    State(state): State<Arc<AppState>>,
    _claims: Claims,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    // ── Session concurrency cap ──────────────────────────────────────────
    let _permit = match Arc::clone(&state.session_sem).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": "max_sessions reached"})),
            )
                .into_response();
        }
    };

    let agent_name = req["agent"].as_str().map(|s| s.to_owned());
    let event_type = req["event_type"].as_str().unwrap_or("manual").to_owned();
    let event_id = uuid::Uuid::new_v4().to_string();
    // Accept both "context" (legacy) and "input" (A2A standard field)
    let data = req
        .get("input")
        .or_else(|| req.get("context"))
        .cloned()
        .unwrap_or_default();
    tracing::info!(event_type, event_id, ?agent_name, "agentd: manual run");

    // The wall-clock timeout must wrap the dispatch FUTURE — wrapping the
    // already-awaited value would make the timeout a no-op.
    let timeout_secs = state.cfg.session_timeout_secs;
    // One run per subscribing specialist, each on its own journal. A named
    // agent bypasses routing and addresses that specialist directly.
    let dispatch = async {
        match agent_name.as_deref() {
            Some(name) => match state
                .plane
                .router()
                .routes()
                .iter()
                .find(|r| r.name == name)
            {
                Some(route) => state
                    .plane
                    .dispatch_one(route.name, &event_id, &event_type, data)
                    .await
                    .map(|d| vec![d])
                    .unwrap_or_default(),
                None => Vec::new(),
            },
            None => state.plane.dispatch(&event_id, &event_type, data).await,
        }
    };

    let decisions =
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), dispatch).await {
            Ok(d) => d,
            Err(_) => vec![AgentDecision {
                agent_name: "timeout".into(),
                session_id: String::new(),
                event_id: event_id.clone(),
                event_type: event_type.clone(),
                outcome: "timeout".into(),
                summary: format!("Dispatch exceeded {timeout_secs}s wall-clock limit."),
                tool_calls: 0,
                turns: 0,
                handoff_to: None,
            }],
        };

    for decision in &decisions {
        emit_audit(&state, decision).await;
    }
    (StatusCode::OK, Json(decisions)).into_response()
}

async fn emit_audit(state: &AppState, decision: &AgentDecision) {
    // Always push to the in-memory ring buffer (best-effort, never fails).
    state.sessions.push(decision.clone());

    let Some(ref url) = state.cfg.audit_webhook_url else {
        return;
    };
    let ce = decision.to_cloud_event(&state.cfg.tenant);
    let secret = state
        .cfg
        .audit_hmac_secret
        .as_ref()
        .map(|s| s.expose_secret().as_bytes());
    let client = mako_service::http::default_client();
    if let Err(e) = mako_service::post_ce_with_retry(&client, url, &ce, secret).await {
        tracing::warn!(error = %e, "audit webhook failed");
    }
}

// ── M9: RAG ingest endpoint ────────────────────────────────────────────────

/// Request body for `POST /api/v1/rag/ingest`.
///
/// Accepts pre-formatted text (e.g. from `edmd.get_device_history`) for live
/// LanceDB RAG indexing.  This is the write-through path for M9 MSB device
/// history RAG.
#[derive(Debug, serde::Deserialize)]
pub struct RagIngestRequest {
    /// Source identifier for this document in search results.
    /// Convention for MSB history: `"msb-{malo_id}"`.
    pub source: String,
    /// The document text to chunk and index.
    pub text: String,
    /// Optional metadata (stored alongside the chunk; not searched).
    #[allow(dead_code)]
    pub metadata: Option<serde_json::Value>,
}

// ── GET /api/v1/sessions ──────────────────────────────────────────────────────

/// `GET /api/v1/sessions` — list the last 100 agent decisions (in-memory ring buffer).
///
/// Returns decisions oldest-first. Useful for inspecting recent automated actions
/// and debugging agent routing.
pub async fn get_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.sessions.list()).into_response()
}

// ── GET /api/v1/agents ────────────────────────────────────────────────────────

/// `GET /api/v1/agents` — list all registered specialists with their capabilities.
///
/// Returns all agents active in this agentd instance (both built-in and custom),
/// including their specialty descriptions, trigger patterns, MCP servers, and
/// whether they are compiled-in (`is_builtin: true`) or operator-defined.
///
/// ## Use cases
///
/// - Operators inspecting which built-in specialists are active
/// - Orchestrator LLM context for routing decisions
/// - A2A agent discovery
pub async fn list_agents(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let routes = state.plane.router().routes();
    Json(serde_json::json!({
        "total": routes.len(),
        "agents": routes.iter().map(|r| serde_json::json!({
            "name": r.name,
            "capability": r.capability,
            "triggers": r.triggers,
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

// ── GET /.well-known/agents/{name} ────────────────────────────────────────────

/// `GET /.well-known/agents/{name}` — A2A Agent Card for a named specialist.
///
/// Returns an [Agent-to-Agent (A2A) protocol](https://a2a-protocol.org/) Agent Card
/// describing a specialist's capabilities, supported skills, and input/output schemas.
///
/// Agent Cards enable external systems and other agents to discover and interact with
/// agentd specialists in a standards-based way without prior configuration.
///
/// ## A2A Protocol reference
///
/// The response follows the A2A Agent Card format:
/// `{ name, description, version, url, capabilities, skills }`
///
/// Unauthenticated endpoint — agents are public capabilities, not secrets.
pub async fn agent_card(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(route) = state
        .plane
        .router()
        .routes()
        .iter()
        .find(|r| r.name == name)
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("agent '{name}' not found") })),
        )
            .into_response();
    };
    Json(serde_json::json!({
        "protocolVersion": "0.3.0",
        "name": route.name,
        "description": route.capability,
        "skills": [{
            "id": route.capability,
            "name": route.name,
            "tags": route.triggers,
        }],
        "defaultInputModes": ["application/cloudevents+json"],
        "defaultOutputModes": ["application/json"],
    }))
    .into_response()
}

// ── GET /api/v1/agents/catalog ────────────────────────────────────────────────

/// `GET /api/v1/agents/catalog` — every specialist compiled into this binary.
///
/// Lists what is *available*, whether or not it is activated, so an operator can
/// see what `[bundled_agents] enable` accepts. The declared ceilings come from
/// each specialist's manifest, which is the authority on them.
pub async fn agents_catalog() -> impl IntoResponse {
    let catalog: Vec<serde_json::Value> = crate::builtin::all()
        .map(|def| {
            let manifest = crate::plane::MANIFESTS
                .iter()
                .find(|(n, _)| *n == def.name)
                .and_then(|(_, src)| crate::plane::parse_manifest(src).ok());

            let (model, max_turns, tools) = manifest.as_ref().map_or((None, None, 0), |m| {
                (
                    m.spec
                        .models
                        .as_ref()
                        .and_then(|x| x.privileged.as_ref())
                        .map(|p| format!("{}/{}", p.provider, p.model)),
                    m.spec.execution.as_ref().map(|e| e.max_turns),
                    m.spec.tools.len(),
                )
            });

            serde_json::json!({
                "name": def.name,
                "specialty": def.specialty,
                "trigger_patterns": def.trigger_patterns,
                "model": model,
                "max_turns": max_turns,
                "tool_grants": tools,
            })
        })
        .collect();
    Json(serde_json::json!({
        "total": catalog.len(),
        "note": "Activate via [bundled_agents] enable = [\"name\"] (or enable_all) in agentd.toml. \
                 Prompts, models, tool grants and ceilings are declared in each agent's manifest.",
        "agents": catalog
    }))
    .into_response()
}
