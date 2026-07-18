//! HTTP handlers — CloudEvent webhook + manual run.

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

use crate::{
    agent::registry::glob_match,
    agent::{AgentDecision, AgentRegistry, OrchestratorAgent},
    config::AgentdConfig,
    dlq::{DlqEntry, DlqStore},
    mcp::McpPool,
    rag::RagEngine,
};

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
    pub orchestrator: OrchestratorAgent,
    pub registry: AgentRegistry,
    pub mcp: McpPool,
    pub rag: Option<RagEngine>,
    /// In-memory ring buffer of the last 100 agent decisions (best-effort; not persisted).
    pub sessions: SessionStore,
    /// OIDC verifier (None = dev mode, all requests accepted with warning).
    pub oidc: Option<Arc<OidcVerifier>>,
    /// Semaphore limiting concurrent agent sessions to `cfg.max_sessions`.
    pub session_sem: Arc<Semaphore>,
    /// Dead-letter queue for failed sessions.
    pub dlq: DlqStore,
}

pub async fn webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── Inbound HMAC verification ────────────────────────────────────────
    if let Some(ref secret) = state.cfg.inbound_hmac_secret {
        let expected = format!(
            "sha256={}",
            mako_service::webhook::hmac_hex(secret.expose_secret().as_bytes(), &body)
        );
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            tracing::warn!("agentd: inbound webhook HMAC mismatch — rejected");
            return StatusCode::FORBIDDEN.into_response();
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

    if !state
        .cfg
        .trigger_event_types
        .iter()
        .any(|t| glob_match(t, &event_type))
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
    let event_data_for_dlq = data.clone(); // capture for DLQ
    tokio::spawn(async move {
        let _permit = permit; // released when task completes
        let dispatch = state2.orchestrator.dispatch(
            event_id.clone(),
            event_type.clone(),
            data,
            &state2.registry,
            &state2.mcp,
            state2.rag.as_ref(),
            &state2.cfg.tenant,
        );
        let decision = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            dispatch,
        )
        .await
        {
            Ok(d) => d,
            Err(_) => {
                tracing::warn!(timeout_secs, "agentd: session timed out");
                AgentDecision {
                    agent_name: "timeout".into(),
                    session_id: uuid::Uuid::new_v4().to_string(),
                    event_id: event_id.clone(),
                    event_type: event_type.clone(),
                    outcome: "timeout".into(),
                    summary: format!("Session exceeded {timeout_secs}s wall-clock limit."),
                    tool_calls: 0,
                    turns: 0,
                    handoff_to: None,
                }
            }
        };

        // Push to DLQ if session failed so it can be retried
        if matches!(decision.outcome.as_str(), "error" | "timeout") {
            let entry = DlqEntry::new(
                &event_type,
                &event_id,
                event_data_for_dlq,
                &decision.summary,
                state2.cfg.dlq.max_retries,
                state2.cfg.dlq.base_backoff_secs,
            );
            state2.dlq.push(entry);
        }

        emit_audit(&state2, &decision).await;
    });
    StatusCode::ACCEPTED.into_response()
}

/// Constant-time comparison (timing-safe).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
                Json(serde_json::json!({"error": "max_sessions reached"}))
            ).into_response();
        }
    };

    let agent_name = req["agent"].as_str().map(|s| s.to_owned());
    let event_type = req["event_type"].as_str().unwrap_or("manual").to_owned();
    let event_id = uuid::Uuid::new_v4().to_string();
    // Accept both "context" (legacy) and "input" (A2A standard field)
    let data = req.get("input").or_else(|| req.get("context")).cloned().unwrap_or_default();
    tracing::info!(event_type, event_id, ?agent_name, "agentd: manual run");

    let dispatch = if let Some(ref name) = agent_name
        && let Some(specialist) = state.registry.get(name)
    {
        // Direct invocation of a named specialist (bypass orchestrator)
        use crate::agent::session::AgentSession;
        let peers: Vec<String> = state
            .registry
            .agent_names
            .iter()
            .filter(|n| *n != name)
            .cloned()
            .collect();
        let session = AgentSession::new(specialist, event_id.clone(), event_type.clone());
        session.run(data, &state.mcp, &peers, state.rag.as_ref()).await
    } else {
        state
            .orchestrator
            .dispatch(
                event_id,
                event_type,
                data,
                &state.registry,
                &state.mcp,
                state.rag.as_ref(),
                &state.cfg.tenant,
            )
            .await
    };

    let timeout_secs = state.cfg.session_timeout_secs;
    let decision = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        std::future::ready(dispatch),
    )
    .await
    {
        Ok(d) => d,
        Err(_) => AgentDecision {
            agent_name: "timeout".into(),
            session_id: uuid::Uuid::new_v4().to_string(),
            event_id: "timeout".into(),
            event_type: "timeout".into(),
            outcome: "timeout".into(),
            summary: format!("Session exceeded {timeout_secs}s wall-clock limit."),
            tool_calls: 0,
            turns: 0,
            handoff_to: None,
        },
    };
    emit_audit(&state, &decision).await;
    (StatusCode::OK, Json(decision)).into_response()
}

async fn emit_audit(state: &AppState, decision: &crate::agent::AgentDecision) {
    // Always push to the in-memory ring buffer (best-effort, never fails).
    state.sessions.push(decision.clone());

    let Some(ref url) = state.cfg.audit_webhook_url else {
        return;
    };
    let ce = decision.to_cloud_event(&state.cfg.tenant);
    let body = match serde_json::to_vec(&ce) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "audit webhook: failed to serialise CloudEvent");
            return;
        }
    };
    let mut req_builder = mako_service::http::default_client()
        .post(url)
        .header("Content-Type", "application/cloudevents+json")
        .body(body.clone());
    if let Some(ref secret) = state.cfg.audit_hmac_secret {
        // sha256= prefix per workspace standard (not bearer_auth)
        let sig = format!(
            "sha256={}",
            mako_service::webhook::hmac_hex(secret.expose_secret().as_bytes(), &body)
        );
        req_builder = req_builder.header("X-Mako-Signature", sig);
    }
    if let Err(e) = req_builder.send().await {
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

/// `POST /api/v1/rag/ingest`
///
/// Index a live text document into the LanceDB RAG store.
///
/// ## Intended callers
///
/// - `agentd` `msb-history-agent` after calling `edmd.get_device_history`
/// - ERP integrations that want to index operator runbooks or device notes
/// - Direct API clients for one-off document ingestion
///
/// ## M9 workflow
///
/// ```text or de.mako.process.initiated (WiM PID)
/// 2. msb-history-agent calls edmd MCP get_device_history { malo_id }
/// 3. msb-history-agent calls POST /api/v1/rag/ingest { source: "msb-{malo_id}", text: <document> }
/// 4. LanceDB stores chunks → available for natural-language queries
/// 5. Field operator: POST /api/v1/run { event_type: "msb.query", context: { query: "..." } }
/// ```
pub async fn rag_ingest(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(req): Json<RagIngestRequest>,
) -> impl axum::response::IntoResponse {
    use axum::http::StatusCode;

    let Some(ref rag) = state.rag else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "RAG is disabled — enable [rag] in agentd.toml",
        )
            .into_response();
    };

    if req.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "text must not be empty").into_response();
    }
    if req.source.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "source must not be empty").into_response();
    }

    let chunk_size = state.cfg.rag.chunk_size;
    let chunk_overlap = state.cfg.rag.chunk_overlap;

    match rag
        .index_live_text(&req.source, &req.text, chunk_size, chunk_overlap)
        .await
    {
        Ok(count) => {
            tracing::info!(source = %req.source, chunks = count, "RAG: live ingest complete");
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "source": req.source,
                    "chunks_indexed": count,
                    "note": "Document indexed into LanceDB RAG store. Available for natural-language queries immediately.",
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(source = %req.source, error = %e, "RAG: live ingest failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
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
    let agents = state.registry.list_agents();
    let builtin_count = agents.iter().filter(|a| a.is_builtin).count();
    let custom_count = agents.iter().filter(|a| !a.is_builtin).count();
    Json(serde_json::json!({
        "total": agents.len(),
        "builtin": builtin_count,
        "custom": custom_count,
        "agents": agents,
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
    let Some(agent) = state.registry.get(&name) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("agent '{name}' not found") })),
        )
            .into_response();
    };

    // Build A2A Agent Card
    let card = serde_json::json!({
        "name": agent.name,
        "description": agent.specialty,
        "version": env!("CARGO_PKG_VERSION"),
        "url": format!("/api/v1/agents/{}/run", agent.name),
        "provider": {
            "organization": "mako agentd",
            "url": "https://github.com/hupe1980/mako"
        },
        "capabilities": {
            "streaming": false,
            "push_notifications": false,
            "state_transition_history": true,
            "multi_turn": true
        },
        "skills": [{
            "id": agent.name.clone(),
            "name": agent.name.clone(),
            "description": agent.specialty,
            "inputModes": ["text", "application/json"],
            "outputModes": ["text"],
            "tags": agent.trigger_patterns.iter()
                .map(|p| p.replace("de.", "").replace(".*", ""))
                .collect::<Vec<_>>()
        }],
        "defaultInputModes": ["application/cloudevents+json"],
        "defaultOutputModes": ["text"],
        "authentication": {
            "schemes": ["Bearer"],
            "credentials": null
        },
        "meta": {
            "mcp_servers": agent.mcp_servers,
            "max_turns": agent.max_turns,
            "is_builtin": agent.is_builtin,
            "trigger_patterns": agent.trigger_patterns
        }
    });

    (StatusCode::OK, Json(card)).into_response()
}

// ── GET /api/v1/agents/catalog ────────────────────────────────────────────────

/// `GET /api/v1/agents/catalog` — list all 26 built-in agent definitions.
///
/// Returns the full catalog of built-in agents regardless of whether they are
/// currently enabled. Useful for operators exploring available specialists before
/// adding them to `[bundled_agents]`.
pub async fn agents_catalog() -> impl IntoResponse {
    let catalog: Vec<serde_json::Value> = crate::builtin::all()
        .map(|def| {
            serde_json::json!({
                "name": def.name,
                "specialty": def.specialty,
                "default_trigger_patterns": def.default_trigger_patterns,
                "default_mcp_servers": def.default_mcp_servers,
                "default_max_turns": def.default_max_turns,
                "default_use_rag": def.default_use_rag,
            })
        })
        .collect();
    Json(serde_json::json!({
        "total": catalog.len(),
        "note": "Enable agents via [bundled_agents] enable = [\"name\"] in agentd.toml",
        "agents": catalog
    }))
    .into_response()
}

// ── POST /api/v1/rag/search ───────────────────────────────────────────────────

/// Request body for `POST /api/v1/rag/search`.
#[derive(Debug, serde::Deserialize)]
pub struct RagSearchRequest {
    /// Natural-language query.
    pub query: String,
}

/// `POST /api/v1/rag/search`
///
/// Query the LanceDB RAG knowledge base directly and return the raw retrieved context
/// string (the same text that gets injected into agent system prompts).
///
/// Useful for operators who want to verify what background knowledge an agent has
/// access to for a given topic, or for debugging RAG quality.
pub async fn rag_search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RagSearchRequest>,
) -> impl IntoResponse {
    let Some(ref rag) = state.rag else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "RAG is disabled — enable [rag] in agentd.toml",
        )
            .into_response();
    };
    if req.query.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "query must not be empty").into_response();
    }
    let context = rag.search(&req.query).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "query": req.query,
            "context": context,
            "note": "This is the exact context injected into agent system prompts for this query.",
        })),
    )
        .into_response()
}

// ── GET /api/v1/dlq ───────────────────────────────────────────────────────────

/// `GET /api/v1/dlq` — dead-letter queue status.
///
/// Returns the current DLQ depth (pending and exhausted) plus up to 20 recent
/// exhausted entries.  Use this endpoint to detect persistent session failures
/// that require operator intervention.
pub async fn get_dlq(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.dlq.snapshot()).into_response()
}
