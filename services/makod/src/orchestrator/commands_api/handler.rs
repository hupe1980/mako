//! HTTP surface: router, POST handler, and the registry-driven dispatch entry point.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the ERP commands router.
pub fn router(state: Arc<CommandsApiState>) -> Router {
    let max_body = state.max_body_bytes;
    Router::new()
        .route("/api/v1/commands", post(handle_command))
        .layer(DefaultBodyLimit::max(max_body))
        .with_state(state)
}

// ── Handler ───────────────────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/v1/commands",
    tag = "commands",
    request_body(content = ErpCommand, description = "ERP command envelope", content_type = "application/json"),
    responses(
        (status = 202, description = "Command accepted", body = CommandAccepted),
        (status = 400, description = "Malformed request body"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 422, description = "Unknown command or missing marktrolle"),
    ),
    security(
        (),
        ("bearer_token" = [])
    )
)]
pub(crate) async fn handle_command(
    State(state): State<Arc<CommandsApiState>>,
    headers: axum::http::HeaderMap,
    Json(envelope): Json<ErpCommand>,
) -> impl IntoResponse {
    // ── Authentication ────────────────────────────────────────────────────────
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Unauthorized",
                    "detail": "Authorization: Bearer <token> header required"
                })),
            )
                .into_response();
        }
    };

    // ── Resolve and validate Marktrolle ────────────────────────────────────────
    //
    // Single-role commands (e.g. `gpke.lieferbeginn.anmelden` → always `LF`):
    // the role is inferred; any asserted `marktrolle` in the request is ignored.
    //
    // Multi-role commands (e.g. `wim.geraetewechsel.beauftragen` → `NB` or `MSB`):
    // the ERP must supply `marktrolle` so the engine knows which EDIFACT
    // qualifier and workflow variant to use.
    let cmd_lower = envelope.command.to_lowercase();
    let asserted = envelope.marktrolle.as_deref().map(str::to_uppercase);

    let effective_marktrolle = match validate_command(
        &cmd_lower,
        asserted.as_deref(),
        &state.configured_marktrollen,
    ) {
        Ok(role) => role,
        Err(e) => {
            let (code, detail) = match e {
                CommandError::UnknownCommand => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "Unknown command {:?}. See module documentation for the command registry.",
                        envelope.command
                    ),
                ),
                CommandError::MarktrolleRequired => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "Command {:?} is permitted for multiple Marktrollen — \
                         supply \"marktrolle\" in the request body to disambiguate.",
                        envelope.command
                    ),
                ),
                CommandError::RoleNotPermitted => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "Marktrolle {:?} is not permitted to issue command {:?}.",
                        envelope.marktrolle, envelope.command
                    ),
                ),
                CommandError::RoleNotConfigured => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "This makod instance is not configured for the Marktrolle \
                         required by command {:?}. Set --marktrollen at startup.",
                        envelope.command
                    ),
                ),
            };
            return (
                code,
                Json(serde_json::json!({ "error": "command_rejected", "detail": detail })),
            )
                .into_response();
        }
    };

    // ── Cedar authorization ───────────────────────────────────────────────────
    if !state.cedar.authorize_command(
        &identity,
        &CommandResource {
            name: &cmd_lower,
            marktrolle: &effective_marktrolle,
            pid: command_primary_pid(&cmd_lower).map_or(0, Pruefidentifikator::as_u32),
            tenant: &state.tenant_id.to_string(),
        },
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error":  "Forbidden",
                "detail": "Cedar policy denied this command"
            })),
        )
            .into_response();
    }

    // ── Idempotency key ───────────────────────────────────────────────────────
    //
    // Required for all state-mutating commands. A missing or empty key means the
    // ERP has no stable retry identity and will generate a new "duplicate" event
    // on every HTTP retry before the engine's business-level DuplicateProcess guard
    // fires. Return 422 so the ERP retries are blocked until the key is supplied.
    let idempotency_key = match headers
        .get("idempotency-key")
        .or_else(|| headers.get("Idempotency-Key"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    {
        Some(key) if !key.is_empty() => key,
        _ => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error":  "missing_idempotency_key",
                    "detail": "The Idempotency-Key header is required for all commands. \
                               Use a stable UUID (e.g. your ERP order or correlation ID) \
                               to prevent duplicate process execution on retries.",
                })),
            )
                .into_response();
        }
    };

    info!(
        idempotency_key = %idempotency_key,
        command         = %cmd_lower,
        marktrolle      = %effective_marktrolle,
        tenant_id       = %state.tenant_id,
        "ERP command accepted",
    );

    // ── Dispatch to workflow ──────────────────────────────────────────────────
    let resolved_process_id = match dispatch_command(&state, &cmd_lower, &envelope.payload).await {
        Ok(DispatchOutcome::Spawned { process_id }) => {
            info!(
                idempotency_key = %idempotency_key,
                command         = %cmd_lower,
                process_id      = %process_id,
                "ERP command dispatched — process spawned",
            );
            // Track per-family process initiation for the Prometheus metrics endpoint.
            EngineMetrics::global().process_initiated("gpke");
            process_id
        }
        Ok(DispatchOutcome::Dispatched { process_id }) => {
            info!(
                idempotency_key = %idempotency_key,
                command         = %cmd_lower,
                process_id      = %process_id,
                "ERP command dispatched — existing process updated",
            );
            process_id
        }
        Err(DispatchError::MaloNotFound(malo_id)) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error":  "malo_not_found",
                    "detail": format!(
                        "MaLo {malo_id:?} is not in the cache. \
                         Seed it first via PUT /admin/malo/{malo_id}."
                    ),
                    "malo_id": malo_id,
                })),
            )
                .into_response();
        }
        Err(DispatchError::InvalidPayload(msg)) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error":  "invalid_payload",
                    "detail": msg,
                })),
            )
                .into_response();
        }
        Err(DispatchError::ProcessNotFound {
            business_key,
            workflow_name,
        }) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error":        "process_not_found",
                    "detail":       format!(
                        "No active {workflow_name} process found for business key {business_key:?}. \
                         Initiate the Anmeldung first via the corresponding anmelden command."
                    ),
                    "business_key":  business_key,
                    "workflow_name": workflow_name,
                })),
            )
                .into_response();
        }
        Err(DispatchError::AmbiguousProcess {
            business_key,
            count,
        }) => {
            tracing::error!(
                business_key = %business_key,
                count        = count,
                "ERP dispatch: multiple active processes for business key — data integrity issue",
            );
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error":        "ambiguous_process",
                    "detail":       format!(
                        "{count} active processes found for business key {business_key:?}; \
                         expected exactly one. This indicates a data integrity issue."
                    ),
                    "business_key": business_key,
                    "count":        count,
                })),
            )
                .into_response();
        }
        Err(DispatchError::DuplicateProcess {
            process_id,
            malo_id,
        }) => {
            tracing::warn!(
                malo_id    = %malo_id,
                process_id = %process_id,
                "ERP dispatch: duplicate anmelden rejected — active process exists",
            );
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error":      "duplicate_process",
                    "detail":     format!(
                        "An active process for MaLo {malo_id:?} already exists. \
                         Route follow-up commands (bestaetigen/ablehnen/aktivieren) \
                         to the existing process, or wait until it reaches a terminal \
                         state before re-initiating."
                    ),
                    "malo_id":    malo_id,
                    "process_id": process_id.to_string(),
                })),
            )
                .into_response();
        }
        Err(DispatchError::Engine(e)) => {
            // InvalidState is a business-logic conflict (e.g. bestaetigen on an
            // already-accepted process) — map to 409 so callers can distinguish
            // it from a true server error.
            if e.as_workflow_error().is_some_and(|w| w.is_invalid_state()) {
                tracing::warn!(
                    idempotency_key = %idempotency_key,
                    command         = %cmd_lower,
                    error           = %e,
                    "ERP dispatch: command rejected — invalid state transition",
                );
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error":  "invalid_state",
                        "detail": e.to_string(),
                    })),
                )
                    .into_response();
            }
            tracing::error!(
                idempotency_key = %idempotency_key,
                command         = %cmd_lower,
                error           = %e,
                "ERP dispatch: engine error",
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error":  "engine_error",
                    "detail": e.to_string(),
                })),
            )
                .into_response();
        }
        Err(DispatchError::NotImplemented(cmd)) => {
            tracing::warn!(
                command = %cmd,
                "ERP dispatch: command not yet implemented — returning 501",
            );
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({
                    "error":  "not_implemented",
                    "detail": format!("command '{cmd}' is not yet dispatchable — do not retry automatically"),
                    "hint":   "This command is registered but its workflow is not yet implemented \
                               in this release. Check the makod release notes for the planned \
                               delivery milestone, or open a support request."
                })),
            )
                .into_response();
        }
    };

    (
        StatusCode::ACCEPTED,
        Json(CommandAccepted {
            idempotency_key,
            command: envelope.command,
            marktrolle: effective_marktrolle,
            status: "accepted",
            process_id: resolved_process_id.to_string(),
        }),
    )
        .into_response()
}

/// Map an ERP command to a workflow command and dispatch it.
///
/// Looks up the command name in `COMMAND_REGISTRY` and calls the registered
/// `CommandDescriptor::dispatch` function.  If the command is unknown the
/// caller should have already rejected it in [`validate_command`]; this path
/// returns `NotImplemented` as a safety net.
pub async fn dispatch_command(
    state: &CommandsApiState,
    command: &str,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    match COMMAND_REGISTRY.iter().find(|d| d.name == command) {
        Some(desc) => (desc.dispatch)(state, payload).await,
        None => Err(DispatchError::NotImplemented(command.to_owned())),
    }
}
