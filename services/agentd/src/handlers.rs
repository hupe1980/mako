//! HTTP handlers — CloudEvent ingest, manual runs, and inventory.
//!
//! The oversight surface (worklist, run views, case history, event delivery) is
//! **not** here: it is agentplane's own, mounted by [`plane::oversight`] under
//! `/api/v1/oversight`. Re-implementing it would put a second copy of an
//! authorization rule in this file, and the copy that drifts is the one people
//! read.
//!
//! [`plane::oversight`]: crate::plane::oversight

use crate::plane::{AgentDecision, Envelope, Plane};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_service::oidc::Claims;
use secrecy::ExposeSecret;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::config::{AgentdConfig, Secrets};

// ── Decision log ───────────────────────────────────────────────────────────

/// In-memory ring buffer of the last `capacity` [`AgentDecision`] results.
///
/// Best-effort and deliberately not the record: the record is the journal, and
/// `GET /api/v1/oversight/runs/{run_id}` is how an operator reads it. This is
/// the "what just happened" view a dashboard polls, and losing it on restart
/// costs nothing that matters.
///
/// Thread-safe via `std::sync::Mutex` — the lock is held only for the duration
/// of a `VecDeque` push or clone, making `parking_lot` unnecessary.
pub struct DecisionLog {
    inner: Mutex<VecDeque<AgentDecision>>,
    capacity: usize,
}

impl DecisionLog {
    #[must_use]
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
    #[must_use]
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
    pub cfg: Arc<AgentdConfig>,
    /// The same credentials with their `env:VAR` indirection resolved. The
    /// config keeps the placeholders; comparing an HMAC against one of those
    /// would reject every legitimate webhook.
    pub secrets: Secrets,
    /// The agentplane runtime and its routing table.
    ///
    /// Replaces the orchestrator, registry, session loop, MCP pool and
    /// dead-letter queue: routing is the only part agentplane does not do, and
    /// a failed run resumes from its journal rather than landing in a queue.
    pub plane: Plane,
    /// The last 100 agent decisions (best-effort; not persisted).
    pub decisions: DecisionLog,
    /// Bounds concurrent agent runs to `cfg.max_sessions`.
    pub session_sem: Arc<Semaphore>,
}

/// The three CloudEvents attributes this door cannot work without.
///
/// Refused rather than defaulted. `id` and `source` are the identity a
/// redelivery keeps and therefore the admission key: an unset attribute arrives
/// as `""`, which is a perfectly good key, so one message would claim it and
/// every later one — from any emitter — would be answered with that message's
/// run. `type` is what the router matches on, and a placeholder matches
/// nothing, which is answered `204` and reads exactly like "nobody subscribes".
fn identify(event: &Value) -> Result<(&str, &str, &str), &'static str> {
    let attribute = |name: &'static str| -> Result<&str, &'static str> {
        match event[name].as_str() {
            Some(v) if !v.trim().is_empty() => Ok(v),
            _ => Err(name),
        }
    };
    Ok((attribute("id")?, attribute("source")?, attribute("type")?))
}

/// A decision recording that dispatch exceeded its wall-clock ceiling.
///
/// The runs themselves are unaffected: their effects are journaled and they
/// resume from where they were. What timed out is our *wait* for them.
fn timed_out(event_id: &str, event_type: &str, secs: u64) -> AgentDecision {
    AgentDecision {
        agent_name: "timeout".to_owned(),
        run_id: String::new(),
        event_id: event_id.to_owned(),
        event_type: event_type.to_owned(),
        outcome: "timeout".to_owned(),
        summary: format!(
            "Waiting for this event's specialists exceeded {secs}s. Their runs are journaled \
             and continue; look them up under /api/v1/oversight/runs."
        ),
        admitted: None,
        waiting_for: None,
        tokens: 0,
    }
}

pub async fn webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── Inbound Standard Webhooks verification ───────────────────────────
    //
    // The shared verifier, not a hand-rolled header read: it also refuses a
    // stale `webhook-timestamp`, which is the half of the check a local copy
    // omits.
    //
    // The returned `webhook-id` is deliberately dropped: it *is* the CloudEvent
    // id, and this plane admits on the CloudEvents `(source, id)` pair, so
    // deduplicating on the transport header too would be the same check under a
    // narrower identity.
    //
    // The refusal is matched rather than mapped straight to a status, because
    // "the signature did not match" and "this arrived forty minutes ago" are
    // different operational problems behind the same 401. Nothing warns about a
    // missing secret here: the daemon says that once at startup, and a line per
    // request trains operators to skip it.
    let secret = state
        .secrets
        .inbound_hmac_secret
        .as_ref()
        .map(|s| s.expose_secret().as_bytes().to_vec());
    match mako_service::webhook::verify_request(secret.as_deref(), &headers, &body) {
        Ok(_deduplicated_on_the_cloudevent_pair_instead) => {}
        Err(err) => {
            tracing::warn!(%err, "agentd: inbound webhook refused");
            return StatusCode::from(err).into_response();
        }
    }

    // ── Parse CloudEvent ─────────────────────────────────────────────────
    let event: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "agentd: malformed CloudEvent body");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // Identity first, and refused rather than defaulted — see `identify`.
    let (event_id, event_source, event_type) = match identify(&event) {
        Ok(triple) => triple,
        Err(missing) => {
            tracing::warn!(
                attribute = missing,
                "agentd: CloudEvent is missing a required attribute — refused"
            );
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let (event_id, event_source, event_type) = (
        event_id.to_owned(),
        event_source.to_owned(),
        event_type.to_owned(),
    );
    let data = event["data"].clone();

    // Tenant binding: a CloudEvent carrying a `tenantid` extension for a
    // different operator must not start a run under our tenant.
    if let Some(ev_tenant) = event["tenantid"].as_str()
        && ev_tenant != state.cfg.tenant
    {
        tracing::warn!(event_id, ev_tenant, "agentd: tenant mismatch — rejected");
        return StatusCode::FORBIDDEN.into_response();
    }

    // There is deliberately no duplicate check here: deduplication happens where
    // the run is admitted, so it holds across instances and across a restart
    // rather than in this process's memory. See `Envelope::admission_key`.
    //
    // The routing table is therefore the only admission filter. A second
    // event-type list in config that nothing reconciles with the manifests mutes
    // a specialist behind a 204 that reads exactly like "nobody subscribes".
    if !state.plane.router().accepts(&event_type) {
        tracing::debug!(event_type, "agentd: no specialist subscribes to this event");
        return StatusCode::NO_CONTENT.into_response();
    }

    // ── Concurrency cap ──────────────────────────────────────────────────
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
        let envelope = Envelope {
            id: &event_id,
            source: &event_source,
            event_type: &event_type,
        };
        let decisions = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            state2.plane.dispatch(envelope, data),
        )
        .await
        {
            Ok(d) => d,
            Err(_) => {
                tracing::warn!(timeout_secs, "agentd: dispatch timed out");
                vec![timed_out(&event_id, &event_type, timeout_secs)]
            }
        };

        for decision in &decisions {
            record_decision(&state2, decision);
        }
    });
    StatusCode::ACCEPTED.into_response()
}

pub async fn manual_run(
    State(state): State<Arc<AppState>>,
    _claims: Claims,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    // ── Concurrency cap ──────────────────────────────────────────────────
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

    let agent_name = req["agent"].as_str().map(str::to_owned);
    let event_type = req["event_type"].as_str().unwrap_or("manual").to_owned();
    let data = req.get("input").cloned().unwrap_or_default();

    // A named agent that does not exist is a 404, not an empty list: `200 OK []`
    // is byte-identical to "the specialists ran and had nothing to say".
    if let Some(name) = agent_name.as_deref()
        && !state.plane.router().routes().iter().any(|r| r.name == name)
    {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "no specialist named '{name}' is activated in this deployment — \
                     GET /api/v1/agents lists the ones that are"
                )
            })),
        )
            .into_response();
    }

    // The webhook's at-most-once admission, offered rather than imposed: with an
    // `Idempotency-Key` a retried POST is answered with the run it already
    // started; without one, every request is its own event.
    let event_id = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    // The plane itself is the producer here, which keeps a manual run's key out
    // of every bus emitter's namespace.
    let event_source = mako_service::source("agentd", &state.cfg.tenant);
    tracing::info!(event_type, event_id, ?agent_name, "agentd: manual run");

    let envelope = Envelope {
        id: &event_id,
        source: &event_source,
        event_type: &event_type,
    };

    // The wall-clock timeout must wrap the dispatch FUTURE — wrapping the
    // already-awaited value would make the timeout a no-op.
    let timeout_secs = state.cfg.session_timeout_secs;
    // One run per subscribing specialist, each on its own journal. A named
    // agent bypasses routing and addresses that specialist directly.
    let dispatch = async {
        match agent_name.as_deref() {
            Some(name) => state
                .plane
                .dispatch_one(name, envelope, data)
                .await
                .map(|d| vec![d])
                .unwrap_or_default(),
            None => state.plane.dispatch(envelope, data).await,
        }
    };

    let decisions =
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), dispatch).await {
            Ok(d) => d,
            Err(_) => vec![timed_out(&event_id, &event_type, timeout_secs)],
        };

    for decision in &decisions {
        record_decision(&state, decision);
    }
    (StatusCode::OK, Json(decisions)).into_response()
}

/// Record what just happened in the in-memory view.
///
/// **This delivers nothing.** The decision reaches the ERP through the
/// journal-backed outbox — a destination registered at admission, with a cursor
/// that advances only on HTTP 2xx — so a receiver that is down for a deploy is
/// caught up afterwards instead of having missed everything. Posting from here
/// at request time would make `de.agent.decision.made` the one mako event that
/// can be silently lost.
fn record_decision(state: &AppState, decision: &AgentDecision) {
    state.decisions.push(decision.clone());
}

// ── GET /api/v1/decisions ─────────────────────────────────────────────────────

/// `GET /api/v1/decisions` — the last 100 agent decisions, oldest first.
///
/// A convenience view over what just happened. The record is the journal:
/// `GET /api/v1/oversight/runs/{run_id}` answers *why* a run ended that way,
/// and `GET /api/v1/oversight/tasks` answers what is waiting for a human.
///
/// **Authenticated**, and it was not. A decision's `summary` is the specialist's
/// answer about a real Marktlokation — MaLo-IDs, counterparty MP-IDs, the
/// reasoning behind a Sperrung recommendation — and this route served the last
/// hundred of them to anyone who could reach the port. Every other route that
/// returns business data on this plane takes a `Claims`; a handler that does not
/// is not a laxer policy, it is no policy, and nothing in the type system says
/// so. (This is `marktd`'s pinned failure class: *a handler without `Claims` is
/// unauthenticated.*)
pub async fn get_decisions(
    _claims: Claims,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    Json(state.decisions.list()).into_response()
}

// ── GET /api/v1/agents ────────────────────────────────────────────────────────

/// `GET /api/v1/agents` — the specialists this deployment activated.
///
/// What is *running here*, as opposed to `/api/v1/agents/catalog`, which lists
/// everything compiled in whether activated or not.
///
/// Authenticated: in a combined-role deployment the activated set names which
/// arm's specialists this process runs, which is § 9 EnWG-relevant deployment
/// detail rather than public capability advertising. The Agent Cards under
/// `/.well-known/agents/{name}` stay open — a card is what an agent *is*, and
/// carries no endpoint credential.
pub async fn list_agents(_claims: Claims, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let routes = state.plane.router().routes();
    Json(serde_json::json!({
        "total": routes.len(),
        "agents": routes.iter().map(|r| serde_json::json!({
            "name": r.name,
            "capability": r.capability,
            "triggers": r.triggers,
            // `planned` agents compile a plan from trusted input before reading
            // anything a counterparty wrote; tool-calling ones react turn by
            // turn. It decides what the agent is admitted with, so it is worth
            // showing an operator.
            "execution": if r.plans { "planned" } else { "tool-calling" },
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

// ── GET /.well-known/agents/{name} ────────────────────────────────────────────

/// `GET /.well-known/agents/{name}` — the A2A Agent Card for a specialist.
///
/// Derived from the manifest by agentplane rather than assembled here, so the
/// card advertises exactly what the declaration says — its capabilities are the
/// ones the plane would actually dispatch, and its version is the manifest's.
/// A hand-written card is a second statement of the same facts, and the two
/// disagree the first time somebody edits one.
///
/// Unauthenticated: an agent's capabilities are public, not secret. The card
/// carries no endpoint credential, and every operation it points at is
/// authenticated.
pub async fn agent_card(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("agent '{name}' not found") })),
        )
    };

    // Only an activated specialist gets a card. Advertising one this deployment
    // does not run would publish a capability no request could reach.
    if !state.plane.router().routes().iter().any(|r| r.name == name) {
        return not_found().into_response();
    }
    let Some(embedded) = crate::plane::find_manifest(&name) else {
        return not_found().into_response();
    };
    let manifest = embedded;

    let url = format!(
        "{}/api/v1/run",
        state.cfg.public_base_url.trim_end_matches('/')
    );
    match agentplane::peers::AgentCard::derive(manifest, url) {
        Ok(card) => Json(card).into_response(),
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, "agent card could not be derived");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "the agent card could not be derived" })),
            )
                .into_response()
        }
    }
}

// ── GET /api/v1/agents/catalog ────────────────────────────────────────────────

/// `GET /api/v1/agents/catalog` — every specialist compiled into this binary.
///
/// Lists what is *available*, whether or not it is activated, so an operator can
/// see what `[bundled_agents] enable` accepts. The declared ceilings come from
/// each specialist's manifest, which is the authority on them.
///
/// Authenticated for the same reason as `/api/v1/agents`: the catalogue is the
/// compiled set, which in a role-scoped build is the deployment's Marktrolle.
pub async fn agents_catalog(_claims: Claims) -> impl IntoResponse {
    let catalog: Vec<serde_json::Value> = crate::builtin::all()
        .map(|def| {
            let manifest = crate::plane::find_manifest(def.name);

            let (model, max_turns, tools, approvals) =
                manifest.as_ref().map_or((None, None, 0, 0), |m| {
                    (
                        m.spec
                            .models
                            .as_ref()
                            .and_then(|x| x.privileged.as_ref())
                            .map(|p| format!("{}/{}", p.provider, p.model)),
                        m.spec.execution.as_ref().map(|e| e.max_turns),
                        m.spec.tools.len(),
                        m.spec.tools.iter().filter(|t| t.requires_approval).count(),
                    )
                });

            serde_json::json!({
                "name": def.name,
                "specialty": def.specialty,
                "trigger_patterns": def.trigger_patterns,
                "model": model,
                "max_turns": max_turns,
                "tool_grants": tools,
                // How many of those grants stop for a human. A specialist with
                // none acts unattended; one where every grant needs approval is
                // usually a manifest that mislabelled its reads.
                "grants_needing_approval": approvals,
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
