//! HTTP handlers — CloudEvent ingest, manual runs, and inventory.
//!
//! ## Two doors, and the difference is durability
//!
//! `POST /webhook` **admits and returns**: the policy gate, the quota
//! reservation, the case binding and the claim on the admission key all commit
//! inside the transaction that writes the run's first record, and only then does
//! this answer `202`. So a `202` means *this message will be acted on*.
//!
//! `POST /api/v1/run` **waits**: an operator asked for an answer, so the request
//! is held until every run concludes or suspends, under a wall-clock ceiling.
//!
//! ## Human doors are authorized, not just authenticated
//!
//! `POST /api/v1/run`, `POST /api/v1/erasure` and the two inventory reads ask the **same Cedar set** the
//! runtime checks every effect against, under agentd's own `api:` verbs. A
//! `Claims` extractor on its own proves the realm knows the caller and says
//! nothing about whether they may spend a run on a Marktlokation, so each of
//! those handlers begins with a `refusal` check and a `403` that names the
//! verb. `POST /webhook` is the machine door and is authenticated by the
//! Standard Webhooks signature instead.
//!
//! Nothing is here that agentplane's operator surface already answers. The
//! worklist, run views, case history and event delivery are its own, mounted by
//! [`plane::oversight`] under `/api/v1/oversight`: re-implementing them would put
//! a second copy of an authorization rule in this file, and the copy that drifts
//! is the one people read.
//!
//! The same argument is why there is no in-memory decision log here. It would be
//! a second copy of a fact the journal owns, and the weaker one: lost on a
//! restart, and holding only what *this instance* handled on a service whose
//! Postgres backend exists for several sharing one store.
//!
//! [`plane::oversight`]: crate::plane::oversight

use crate::plane::{AgentDecision, Envelope, Plane, Reception};
use std::sync::Arc;

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

use crate::config::{AgentdConfig, Secrets};

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
    ///
    /// Back-pressure lives with it rather than here: `[quota]
    /// max_concurrent_runs` is reserved in the store at admission, so it holds
    /// across instances and counts *runs* rather than one permit for a fan-out
    /// that starts six of them.
    pub plane: Plane,
    /// The same compiled Cedar set the runtime checks every effect against.
    ///
    /// Held here so agentd's **own** doors are authorized by the policy the
    /// deployment reviewed, rather than by the fact that a caller had a token.
    /// One engine, not two: a second policy source for the REST surface is a
    /// second place for an audience to drift from the one the worklist uses.
    pub policy: Arc<dyn agentplane::core::PolicyEngine>,
}

/// The refusal a caller the policy set does not admit gets, or `None` when they
/// are admitted.
///
/// `Option` rather than `Result`: an `axum::Response` is a large `Err` variant,
/// and "no refusal" reads more honestly than "Ok(())" for a check whose only
/// product is the refusal itself. The `403` body names the verb, so an operator
/// reading it can ask for the role rather than guessing which door refused them.
/// The roles come from the verified token; nothing in the request body reaches
/// this.
fn refusal(
    state: &AppState,
    claims: &Claims,
    action: &'static str,
) -> Option<axum::response::Response> {
    if crate::plane::policy::caller_may(
        state.policy.as_ref(),
        claims.sub(),
        &claims.0.mako_roles,
        claims.tenant(),
        action,
    ) {
        return None;
    }
    tracing::warn!(
        sub = claims.sub(),
        action,
        "agentd: caller is authenticated but not authorized for this action"
    );
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!(
                    "your roles do not admit `{action}` on this plane — authorization comes \
                     from the Cedar set in agentd's policy, not from holding a token"
                )
            })),
        )
            .into_response(),
    )
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

    tracing::info!(event_type, event_id, "agentd: trigger received");

    // ── Admit, durably, before anything is acknowledged ──────────────────
    //
    // The emitter advances its outbox cursor on the 2xx, so an acknowledgement
    // that returns before anything durable is written is a promise about nothing:
    // a process that stops before the admission key is claimed — a deploy, a
    // SIGTERM, a crash — loses the event with no record that it arrived.
    //
    // `Plane::accept` returns once admission has committed. The *work* continues
    // in the background and is durable by its own mechanism: the run holds a
    // lease, and a lease that lapses without release is taken over and resumed by
    // the sweeper's recovery pass.
    let envelope = Envelope {
        id: &event_id,
        source: &event_source,
        event_type: &event_type,
    };
    let accepted = state.plane.accept(envelope, data).await;

    // ── The status code is a retry instruction ───────────────────────────
    //
    // mako's own emitter treats **429 and 5xx as transient and every other 4xx
    // as permanent** (`post_ce_with_retry`), so the code chosen here decides
    // whether a message is resent or dead-lettered — and both mistakes cost.
    // A permanent refusal answered `429` burns the retry schedule and lands in
    // the dead-letter list anyway, five attempts later; a transient one
    // answered `422` is dead-lettered at once, on a market message.
    //
    // Partial success is success: a retry is answered with the runs already
    // holding those keys, so nothing is duplicated and nothing is lost.
    match Plane::reception(&accepted) {
        Reception::Admitted => {
            // The run ids, in the body. A bare `202` tells a caller the message
            // was taken and gives them nothing to follow; these are the journal
            // keys `/api/v1/oversight/runs/{id}` takes.
            (StatusCode::ACCEPTED, Json(accepted)).into_response()
        }
        Reception::Retry => {
            tracing::warn!(
                event_id,
                event_type,
                specialists = accepted.len(),
                "agentd: no specialist could be admitted — asking the emitter to resend"
            );
            (StatusCode::TOO_MANY_REQUESTS, Json(accepted)).into_response()
        }
        Reception::Unprocessable => {
            // Loud, because this one does not come back. The message is going
            // to the emitter's dead-letter list now, which is where an operator
            // will see it — and that is the point: a payload no subscribing
            // specialist can act on should surface today, not after a retry
            // schedule that was never going to change the answer.
            tracing::error!(
                event_id,
                event_type,
                specialists = accepted.len(),
                "agentd: no specialist can act on this event and resending cannot help — \
                 it will be dead-lettered by the emitter"
            );
            (StatusCode::UNPROCESSABLE_ENTITY, Json(accepted)).into_response()
        }
    }
}

pub async fn manual_run(
    State(state): State<Arc<AppState>>,
    claims: Claims,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    // Authenticated *and* authorized. Without the second half every principal
    // the realm issues a token to could start any specialist on any
    // Marktlokation and spend the tenant's model budget doing it.
    if let Some(refused) = refusal(&state, &claims, crate::plane::policy::action::RUN_START) {
        return refused;
    }

    // No concurrency cap here. It was an in-process semaphore, and the ceiling
    // that replaced it is `[quota] max_concurrent_runs` — reserved in the store
    // at admission, so it holds across instances and a refusal names the tenant
    // that reached it rather than the process that happened to receive the
    // request.
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

    (StatusCode::OK, Json(decisions)).into_response()
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureRequest {
    pub case_id: Option<String>,
    pub memory_subject: Option<String>,
    pub reason: String,
}

/// Destroy a case wrapping key and/or forget a memory subject.
pub async fn erase(
    State(state): State<Arc<AppState>>,
    claims: Claims,
    Json(request): Json<ErasureRequest>,
) -> impl IntoResponse {
    if let Some(refused) = refusal(
        &state,
        &claims,
        crate::plane::policy::action::ERASURE_EXECUTE,
    ) {
        return refused;
    }
    let reason = request.reason.trim();
    let subject = request
        .memory_subject
        .as_deref()
        .map(str::trim)
        .filter(|subject| !subject.is_empty());
    if reason.is_empty() || (request.case_id.is_none() && subject.is_none()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "a non-empty reason and at least one of case_id or memory_subject are required"
            })),
        )
            .into_response();
    }
    let case = match request.case_id.as_deref() {
        Some(raw) => match agentplane::core::CaseId::parse(raw) {
            Ok(case) => Some(case),
            Err(_) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({ "error": "case_id is invalid" })),
                )
                    .into_response();
            }
        },
        None => None,
    };
    match state.plane.erase(case, subject, reason).await {
        Ok(forgotten_memories) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "case_erased": request.case_id.is_some(),
                "memory_subject": subject,
                "forgotten_memories": forgotten_memories,
            })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
    }
}

// ── GET /api/v1/agents ────────────────────────────────────────────────────────

/// `GET /api/v1/agents` — the specialists this deployment activated.
///
/// What is *running here*, as opposed to `/api/v1/agents/catalog`, which lists
/// everything compiled in whether activated or not.
///
/// Authorized, not merely authenticated: in a combined-role deployment the
/// activated set names which arm's specialists this process runs, which is
/// §§ 6a and 7a EnWG-relevant deployment detail rather than public capability
/// advertising, and a token is not a reason to be shown it. The Agent Cards
/// under `/.well-known/agents/{name}` stay open — a card is what an agent *is*,
/// and carries no endpoint credential.
pub async fn list_agents(claims: Claims, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if let Some(refused) = refusal(&state, &claims, crate::plane::policy::action::AGENT_LIST) {
        return refused;
    }
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
            "execution": r.execution.as_str(),
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
/// Gated by the same verb as `/api/v1/agents`: the catalogue is the compiled
/// set, which in a role-scoped build is the deployment's Marktrolle.
pub async fn agents_catalog(
    claims: Claims,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Some(refused) = refusal(&state, &claims, crate::plane::policy::action::AGENT_LIST) {
        return refused;
    }
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

            let declaration = crate::plane::declaration(def.name);

            serde_json::json!({
                "name": def.name,
                // Which revision this binary embeds. The journal records the
                // same identity on every run, so a decision can be traced to a
                // declaration; this is the only way to ask the *running* plane.
                "version": declaration.as_ref().map(|d| d.version.clone()),
                "digest": declaration.as_ref().and_then(|d| d.digest.clone()),
                // The manifest's own `identity.role` — the sentence the model
                // is given. A second copy in Rust drifted from it, so the
                // catalogue reads the one the agent actually runs on.
                "specialty": manifest
                    .as_ref()
                    .and_then(|m| m.spec.identity.as_ref())
                    .map(|i| i.role.as_str()),
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
