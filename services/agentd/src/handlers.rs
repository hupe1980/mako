//! HTTP handlers — CloudEvent ingest, runs a person starts, and inventory.
//!
//! ## Two doors, both admit and return
//!
//! `POST /webhook` (an event) and `POST /api/v1/runs` (a person) **admit and
//! return**: the policy gate, the quota reservation, the case binding and the
//! claim on the admission key all commit inside the transaction that writes the
//! run's first record, and only then does the door answer `202` with the run
//! ids. So a `202` means *this will be acted on*; the run's state, steps and
//! result are read at `/api/v1/oversight/runs/{run}`.
//!
//! ## Human doors are authorized, not just authenticated
//!
//! `POST /api/v1/runs`, `POST /api/v1/erasure` and the two inventory reads ask the **same Cedar set** the
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

use crate::plane::{Accepted, Envelope, EraseError, Plane, Reception};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use mako_service::oidc::Claims;
use mako_service::{ApiError, ApiResult, Json};
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
/// `Option` rather than `Result`: "no refusal" reads more honestly than
/// "Ok(())" for a check whose only product is the refusal itself. The `403`
/// body names the verb, so an operator reading it can ask for the role rather
/// than guessing which door refused them. The roles come from the verified
/// token; nothing in the request body reaches this.
fn refusal(state: &AppState, claims: &Claims, action: &'static str) -> Option<ApiError> {
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
    Some(ApiError::forbidden(format!(
        "your roles do not admit `{action}` on this plane — authorization comes from the \
         Cedar set in agentd's policy, not from holding a token"
    )))
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

    admission_response(&event_id, &event_type, &accepted)
}

/// The answer to an admission — `202` with the run ids, or why not.
///
/// The status code is a retry instruction. mako's own emitter treats **429 and
/// 5xx as transient and every other 4xx as permanent** (`post_ce_with_retry`),
/// so the code chosen here decides whether a message is resent or
/// dead-lettered — and both mistakes cost. A permanent refusal answered `429`
/// burns the retry schedule and lands in the dead-letter list anyway, five
/// attempts later; a transient one answered `422` is dead-lettered at once, on
/// a market message. A person starting a run reads the same codes.
///
/// Partial success is success: a retry is answered with the runs already
/// holding those keys, so nothing is duplicated and nothing is lost.
fn admission_response(event_id: &str, event_type: &str, accepted: &[Accepted]) -> Response {
    match Plane::reception(accepted) {
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
            ApiError::too_many_requests(
                "no specialist could be admitted now; resend the event — the runs already \
                 admitted under its keys are answered again, not duplicated",
            )
            .with_fields(serde_json::json!({ "admissions": accepted }))
            .into_response()
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
            ApiError::unprocessable_with(
                "no specialist subscribing to this event can act on it, and resending cannot \
                 help",
                serde_json::json!({ "admissions": accepted }),
            )
            .into_response()
        }
    }
}

/// What a person asks a run for. Every field is named: an unknown one is a
/// `422`, because a misspelt `agent` read as absent would start every
/// subscribing specialist instead of the one named.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    /// The specialist to run; absent, every one subscribed to `event_type`.
    pub agent: Option<String>,
    /// The event type the run answers (`manual` when absent).
    pub event_type: Option<String>,
    /// The event's data, handed to the specialist as is.
    #[serde(default)]
    pub input: Value,
}

pub async fn start_runs(
    State(state): State<Arc<AppState>>,
    claims: Claims,
    headers: HeaderMap,
    Json(req): Json<RunRequest>,
) -> impl IntoResponse {
    // Authenticated *and* authorized. Without the second half every principal
    // the realm issues a token to could start any specialist on any
    // Marktlokation and spend the tenant's model budget doing it.
    if let Some(refused) = refusal(&state, &claims, crate::plane::policy::action::RUN_START) {
        return refused.into_response();
    }

    // No concurrency cap here. It was an in-process semaphore, and the ceiling
    // that replaced it is `[quota] max_concurrent_runs` — reserved in the store
    // at admission, so it holds across instances and a refusal names the tenant
    // that reached it rather than the process that happened to receive the
    // request.
    let agent_name = req.agent;
    let event_type = req.event_type.unwrap_or_else(|| "manual".to_owned());
    let data = req.input;

    // A named agent that does not exist is a 404, not an empty list: `200 OK []`
    // is byte-identical to "the specialists ran and had nothing to say".
    if let Some(name) = agent_name.as_deref()
        && !state.plane.router().routes().iter().any(|r| r.name == name)
    {
        tracing::debug!(
            agent = name,
            "agentd: no specialist of that name is activated — GET /api/v1/agents lists the ones \
             that are"
        );
        return ApiError::NotFound.into_response();
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
    tracing::info!(
        event_type,
        event_id,
        ?agent_name,
        "agentd: run started by a person"
    );

    let envelope = Envelope {
        id: &event_id,
        source: &event_source,
        event_type: &event_type,
    };

    // Admitted, not awaited: the runs are journaled and leased like a
    // webhook's, and their state, steps and result are read where every run's
    // are — `/api/v1/oversight/runs/{run}`. A request held open for a model's
    // answer is a request a proxy, a browser or a person gives up on.
    let accepted = match agent_name.as_deref() {
        Some(name) => state
            .plane
            .accept_named(name, envelope, data)
            .await
            .into_iter()
            .collect(),
        None => state.plane.accept(envelope, data).await,
    };
    if accepted.is_empty() {
        return ApiError::unprocessable(format!(
            "no specialist subscribes to `{event_type}` — name one with `agent`"
        ))
        .into_response();
    }
    admission_response(&event_id, &event_type, &accepted)
}

/// `GET /api/v1/hinweise` — the open worklist rows the caller's roles may
/// answer, each filed under its Anker: the Desk's Hinweise. Read under the
/// verb the oversight worklist is.
pub async fn hinweise(
    State(state): State<Arc<AppState>>,
    claims: Claims,
) -> ApiResult<Json<Vec<crate::plane::Hinweis>>> {
    if let Some(refused) = refusal(&state, &claims, agentplane::api::action::TASK_LIST) {
        return Err(refused);
    }
    state
        .plane
        .hinweise(&claims.principal().roles)
        .await
        .map(Json)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("read the worklist: {e}")))
}

/// `GET /api/v1/runs/{run}/antwort` — a run's answer, sentence by sentence,
/// each with the call it cites and whether that call's output carries its
/// figures. Read under the verb the oversight run view is: whoever may read a
/// run may read its answer.
pub async fn answer(
    State(state): State<Arc<AppState>>,
    claims: Claims,
    axum::extract::Path(run): axum::extract::Path<String>,
) -> ApiResult<Json<crate::plane::Answer>> {
    if let Some(refused) = refusal(&state, &claims, agentplane::api::action::RUN_READ) {
        return Err(refused);
    }
    match state.plane.answer(&run, &claims.principal().sub).await {
        Ok(Some(answer)) => Ok(Json(answer)),
        Ok(None) => Err(ApiError::NotFound),
        Err(e) => Err(ApiError::internal(anyhow::anyhow!(
            "read the answer of run {run}: {e}"
        ))),
    }
}

/// A person's verdict on a run the Desk showed them: whether it helped, or
/// what became of its proposal — one of the two per request.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BewertungRequest {
    pub hilfreich: Option<bool>,
    /// Why, in their words — sealed with the run's data, and erased with it.
    pub begruendung: Option<String>,
    pub vorschlag: Option<crate::plane::VorschlagSchicksal>,
}

impl BewertungRequest {
    fn urteil(self) -> Result<crate::plane::Urteil, &'static str> {
        match (self.hilfreich, self.vorschlag) {
            (Some(hilfreich), None) => Ok(crate::plane::Urteil::Hilfreich {
                hilfreich,
                begruendung: self.begruendung,
            }),
            (None, Some(schicksal)) if self.begruendung.is_none() => {
                Ok(crate::plane::Urteil::Vorschlag(schicksal))
            }
            (None, Some(_)) => Err("a Begründung goes with `hilfreich`, not with `vorschlag`"),
            _ => Err("state exactly one of `hilfreich` or `vorschlag`"),
        }
    }
}

/// `POST /api/v1/runs/{run}/bewertung` — record whether an answer or a
/// Hinweis helped, or what became of a proposal, under the caller's own name. A later verdict by the same
/// person replaces theirs; the judged run is never changed.
pub async fn bewerten(
    State(state): State<Arc<AppState>>,
    claims: Claims,
    axum::extract::Path(run): axum::extract::Path<String>,
    Json(req): Json<BewertungRequest>,
) -> ApiResult<StatusCode> {
    if let Some(refused) = refusal(&state, &claims, agentplane::api::action::RUN_ASSESS) {
        return Err(refused);
    }
    let urteil = req.urteil().map_err(ApiError::unprocessable)?;
    match state
        .plane
        .bewerten(&run, &claims.principal().sub, urteil)
        .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(crate::plane::BewertungError::NichtBewertbar(why)) => Err(ApiError::conflict(why)),
        Err(crate::plane::BewertungError::Ungueltig(why)) => Err(ApiError::unprocessable(why)),
        Err(crate::plane::BewertungError::Journal(e)) => Err(ApiError::internal(anyhow::anyhow!(
            "record the verdict on run {run}: {e}"
        ))),
    }
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
) -> ApiResult<Json<Value>> {
    if let Some(refused) = refusal(
        &state,
        &claims,
        crate::plane::policy::action::ERASURE_EXECUTE,
    ) {
        return Err(refused);
    }
    let reason = request.reason.trim();
    let subject = request
        .memory_subject
        .as_deref()
        .map(str::trim)
        .filter(|subject| !subject.is_empty());
    if reason.is_empty() || (request.case_id.is_none() && subject.is_none()) {
        return Err(ApiError::unprocessable(
            "a non-empty reason and at least one of case_id or memory_subject are required",
        ));
    }
    let case = request
        .case_id
        .as_deref()
        .map(agentplane::core::CaseId::parse)
        .transpose()
        .map_err(|_| ApiError::unprocessable("case_id is invalid"))?;
    let forgotten_memories = state
        .plane
        .erase(case, subject, reason)
        .await
        .map_err(erase_failure)?;
    Ok(Json(serde_json::json!({
        "case_erased": request.case_id.is_some(),
        "memory_subject": subject,
        "forgotten_memories": forgotten_memories,
    })))
}

/// The answer an [`EraseError`] gets: a deployment without a key ring is a
/// `409` naming that, and a store failure a `500` whose text stays in the log.
fn erase_failure(error: EraseError) -> ApiError {
    match error {
        EraseError::NoKeyRing => ApiError::conflict(EraseError::NoKeyRing.to_string()),
        EraseError::Store(cause) => ApiError::internal(cause),
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
        return refused.into_response();
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
) -> ApiResult<Json<agentplane::peers::AgentCard>> {
    // Only an activated specialist gets a card. Advertising one this deployment
    // does not run would publish a capability no request could reach.
    if !state.plane.router().routes().iter().any(|r| r.name == name) {
        return Err(ApiError::NotFound);
    }
    let Some(embedded) = crate::plane::find_manifest(&name) else {
        return Err(ApiError::NotFound);
    };
    let manifest = embedded;

    let url = format!(
        "{}/api/v1/runs",
        state.cfg.public_base_url.trim_end_matches('/')
    );
    agentplane::peers::AgentCard::derive(manifest, url)
        .map(Json)
        .map_err(|e| ApiError::internal(anyhow::anyhow!("derive the agent card of {name}: {e}")))
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
        return refused.into_response();
    }
    let active: Vec<String> = state
        .plane
        .router()
        .routes()
        .iter()
        .map(|r| r.name.to_owned())
        .collect();
    let faehigkeiten: Vec<Faehigkeit> = crate::builtin::all()
        .map(|def| faehigkeit(def, active.iter().any(|n| n == def.name)))
        .collect();
    Json(Katalog { faehigkeiten }).into_response()
}

/// Every specialist compiled into this binary, as the Desk's Beipackzettel
/// shows it.
#[derive(Debug, serde::Serialize)]
pub struct Katalog {
    pub faehigkeiten: Vec<Faehigkeit>,
}

/// One specialist, read off its manifest — never a second copy beside it.
#[derive(Debug, serde::Serialize)]
pub struct Faehigkeit {
    pub name: String,
    /// `mako.de/titel`: what an operator calls it.
    pub titel: Option<String>,
    /// `mako.de/zweck`: what it is for, in one sentence.
    pub zweck: Option<String>,
    pub version: Option<String>,
    /// The digest over the canonical manifest the journal records on every run.
    pub digest: Option<String>,
    /// Activated in this deployment.
    pub aktiv: bool,
    /// The event types that start it; empty when only a person or a schedule does.
    pub ausloeser: Vec<String>,
    /// `provider/model`, or `None` for a skill that runs no model at all.
    pub modell: Option<String>,
    /// Each tool it may call, `server/tool`, with the manifest's reason.
    pub liest: Vec<Lesezugriff>,
    /// No grant can change anything: every one is declared non-mutating.
    pub nur_lesend: bool,
    /// Grants that stop for a person's approval.
    pub mit_freigabe: usize,
}

/// One tool a specialist may call.
#[derive(Debug, serde::Serialize)]
pub struct Lesezugriff {
    /// `server/tool`.
    pub werkzeug: String,
    pub wozu: Option<String>,
}

fn faehigkeit(def: &crate::builtin::Specialist, aktiv: bool) -> Faehigkeit {
    let manifest = crate::plane::find_manifest(def.name);
    let declaration = crate::plane::declaration(def.name);
    let annotation = |key: &str| {
        manifest
            .as_ref()
            .and_then(|m| m.metadata.annotations.get(key).cloned())
    };
    let tools = manifest
        .as_ref()
        .map(|m| m.spec.tools.as_slice())
        .unwrap_or_default();
    Faehigkeit {
        name: def.name.to_owned(),
        titel: annotation("mako.de/titel"),
        zweck: annotation("mako.de/zweck"),
        version: declaration.as_ref().map(|d| d.version.clone()),
        digest: declaration.as_ref().and_then(|d| d.digest.clone()),
        aktiv,
        ausloeser: def
            .trigger_patterns
            .iter()
            .map(|t| (*t).to_owned())
            .collect(),
        modell: manifest
            .as_ref()
            .and_then(|m| m.spec.models.as_ref())
            .and_then(|x| x.privileged.as_ref())
            .map(|p| format!("{}/{}", p.provider, p.model)),
        liest: tools
            .iter()
            .map(|t| Lesezugriff {
                werkzeug: t.reference.trim_start_matches("tool://").to_owned(),
                wozu: t.description.clone(),
            })
            .collect(),
        nur_lesend: tools.iter().all(|t| !t.mutates),
        mit_freigabe: tools.iter().filter(|t| t.requires_approval).count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body(error: ApiError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).expect("json"))
    }

    /// A key-service failure is this service's fault: a `500` with an
    /// `error_id`, and the store's own text — which names its internals — only
    /// in the log.
    #[tokio::test]
    async fn a_store_failure_on_erasure_is_an_internal_error_that_echoes_nothing() {
        let (status, body) = body(erase_failure(EraseError::Store(anyhow::anyhow!(
            "destroy case wrapping key: vault token s.SECRET rejected"
        ))))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body["error_id"].is_string(), "{body}");
        assert!(!body.to_string().contains("SECRET"), "{body}");
    }

    /// A deployment without a key ring cannot erase a case however often it is
    /// asked, and the caller is told why.
    #[tokio::test]
    async fn erasure_without_a_key_ring_is_a_conflict_that_names_it() {
        let (status, body) = body(erase_failure(EraseError::NoKeyRing)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["detail"],
            "case erasure requires a configured key ring"
        );
    }

    /// A misspelt field is refused: read as absent, `agnet` would leave the
    /// run unaddressed and start every subscribing specialist.
    #[test]
    fn a_misspelt_run_field_is_refused_not_ignored() {
        let typo = serde_json::json!({ "agnet": "billing-anomaly-agent", "input": {} });
        assert!(serde_json::from_value::<super::RunRequest>(typo).is_err());
        let named: super::RunRequest =
            serde_json::from_value(serde_json::json!({ "agent": "billing-anomaly-agent" }))
                .expect("a named agent");
        assert_eq!(named.agent.as_deref(), Some("billing-anomaly-agent"));
        assert_eq!(named.input, serde_json::Value::Null);
    }

    /// Every specialist names itself for an operator — a Beipackzettel entry
    /// without a title or a purpose would show a crate-style name instead.
    #[test]
    fn every_specialist_carries_a_title_and_a_purpose() {
        let missing: Vec<&str> = crate::builtin::all()
            .map(|d| (d.name, super::faehigkeit(d, true)))
            .filter(|(_, f)| {
                f.titel.as_deref().is_none_or(str::is_empty)
                    || f.zweck.as_deref().is_none_or(str::is_empty)
            })
            .map(|(n, _)| n)
            .collect();
        assert!(
            missing.is_empty(),
            "add mako.de/titel and mako.de/zweck: {missing:?}"
        );

        let desk = crate::builtin::all()
            .find(|d| d.name == "desk-assistant")
            .map(|d| super::faehigkeit(d, true))
            .expect("the desk assistant is compiled in");
        assert!(
            desk.ausloeser.is_empty(),
            "started by a question, never an event"
        );
        assert!(desk.nur_lesend);
        assert!(desk.modell.is_some());
        assert_eq!(desk.liest.len(), 5);
        assert!(
            desk.liest
                .iter()
                .any(|l| l.werkzeug == "makod/get_process_history")
        );
    }
}
