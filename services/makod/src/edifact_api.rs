//! REST API for submitting raw EDIFACT messages directly to makod.
//!
//! This provides an HTTP alternative to AS4 transport: operators can POST a
//! raw EDIFACT interchange (or single message) and receive a structured JSON
//! response describing how each message was parsed and routed.
//!
//! ## Endpoints
//!
//! ### `POST /edifact`
//!
//! Submit a raw EDIFACT interchange or single message.
//!
//! **Request**
//! - `Content-Type: text/plain; charset=utf-8` (or `application/octet-stream`)
//! - Body: raw EDIFACT bytes (UNB envelope optional; bare UNH…UNT also accepted)
//!
//! **Response `200 OK`** — at least one message parsed and routed successfully:
//! ```json
//! {
//!   "accepted": 1,
//!   "rejected": 0,
//!   "messages": [
//!     {
//!       "message_type": "UTILMD",
//!       "pid": 55001,
//!       "workflow": "GpkeSupplierChange",
//!       "status": "routed"
//!     }
//!   ]
//! }
//! ```
//!
//! **Response `422 Unprocessable Entity`** — body was received but no messages
//! could be parsed (syntax error in every message):
//! ```json
//! { "accepted": 0, "rejected": 1, "messages": [{ "status": "parse_error", "error": "…" }] }
//! ```
//!
//! **Response `400 Bad Request`** — empty body.
//!
//! ## Notes
//!
//! - When an [`EdifactIngestDispatcher`][crate::ingest_dispatcher::EdifactIngestDispatcher]
//!   is wired into `EdifactApiState::dispatcher`, workflow dispatch is executed
//!   immediately after routing for every `Routed` message.  Dispatch failures
//!   are non-fatal and logged at `warn` level — the HTTP response still returns
//!   `"status": "routed"` so the caller knows the message was accepted.
//! - A `pid` of `null` means the message was parsed successfully but carries no
//!   recognised Prüfidentifikator (e.g. CONTRL, APERAK without BGM).
//! - An unknown `pid` (not registered in the `PidRouter`) returns `status:
//!   "unknown_pid"` rather than `"routed"`. The message is still accepted.

use std::sync::Arc;

use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::post,
};
use edi_energy::{AnyMessage, EdiEnergyMessage as _, Platform};
use mako_engine::{
    ids::TenantId,
    partner::{CommunicationChannel, MarketRole, PartnerRecord, PartnerStore as _},
    pid_router::PidRouter,
    store_slatedb::SlateDbPartnerStore,
    types::MarktpartnerCode,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::cedar_authz::{CedarAuthorizer, IngestResource};

// ── Shared state ─────────────────────────────────────────────────────────────

/// Shared state for the EDIFACT REST API.
pub struct EdifactApiState {
    pub platform: Arc<Platform>,
    pub pid_router: PidRouter,
    /// Cedar-based authorization engine for all protected endpoints.
    pub cedar: Arc<CedarAuthorizer>,
    /// Maximum allowed request body size in bytes.
    /// Applied to `POST /edifact` via [`DefaultBodyLimit`].
    pub max_body_bytes: usize,
    /// Partner store for automatic PARTIN upserts.
    ///
    /// When `Some`, every valid inbound PARTIN message (PIDs 37000–37014) is
    /// automatically extracted and upserted into the partner directory — no ERP
    /// integration or manual `PUT /admin/partners/{gln}` required.
    ///
    /// `None` disables auto-upsert (e.g. in unit tests or read-only contexts).
    pub partner_store: Option<Arc<SlateDbPartnerStore>>,
    /// Tenant identifier for partner store writes.
    pub tenant_id: TenantId,
    /// Phase 2 ingest dispatcher.
    ///
    /// When `Some`, every routed message is forwarded to the domain workflow
    /// process after classification.  When `None`, ingest stops at classification
    /// (Phase 1 only — useful in read-only / test contexts).
    pub dispatcher: Option<Arc<crate::ingest_dispatcher::EdifactIngestDispatcher>>,
    /// Gas CONTRL Empfangsbestätigung emitter (CONTRL AHB 1.0 §1.2).
    ///
    /// When `Some`, a CONTRL (UCI=7) is enqueued for every inbound Gas interchange
    /// that contains at least one non-CONTRL, non-APERAK message. Required for
    /// regulatory compliance with the mandatory 6-hour CONTRL obligation.
    ///
    /// `None` disables CONTRL emission (e.g. in read-only / test contexts without
    /// an outbox store).
    pub contrl_ack: Option<Arc<crate::contrl_ack::ContrlAckService>>,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub struct IngestResponse {
    /// Number of messages that were parsed and routed (or had a known PID).
    #[schema(example = 2)]
    pub accepted: usize,
    /// Number of messages that could not be parsed at all.
    #[schema(example = 0)]
    pub rejected: usize,
    pub messages: Vec<MessageResult>,
}

#[derive(Serialize, ToSchema)]
pub struct MessageResult {
    /// EDIFACT message type, e.g. `"UTILMD"`, `"MSCONS"`, `"APERAK"`.
    /// `null` when the message type could not be determined (parse error).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "UTILMD")]
    pub message_type: Option<String>,

    /// The Prüfidentifikator extracted from the BGM segment, if present.
    /// `null` for message types that carry no PID (e.g. CONTRL).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 55001)]
    pub pid: Option<u32>,

    /// Workflow name from the `PidRouter`, if the PID is registered.
    /// `null` when `pid` is `null` or when the PID is not registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "gpke-lieferbeginn")]
    pub workflow: Option<String>,

    /// Routing outcome for this message.
    pub status: MessageStatus,

    /// UUID of the process that was spawned or resumed.
    ///
    /// Matches the `subject` of the `de.mako.process.initiated` CloudEvent
    /// sent to the ERP webhook. Present only when `status == "routed"` and
    /// Phase 2 dispatch succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "3181967a-02d1-4d0e-9105-0cc46f3b25c9")]
    pub process_id: Option<String>,

    /// Marktlokations-ID extracted from the message (LOC+Z16), if present.
    ///
    /// Use this to correlate the ingest response with the corresponding
    /// command API call (`gpke.lieferbeginn.bestaetigen` etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "51238696781")]
    pub malo_id: Option<String>,

    /// Human-readable parse error, present only when `status == "parse_error"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Debug, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    /// Parsed and matched to a registered workflow.
    Routed,
    /// Parsed successfully; PID present but not registered in PidRouter.
    UnknownPid,
    /// Parsed successfully; no PID in this message type (e.g. CONTRL acknowledgement).
    NoPid,
    /// Could not be parsed at all.
    ParseError,
}

// ── Auth middleware ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ApiError {
    error: &'static str,
}

/// Bearer-token authentication middleware.
///
/// All routes require `Authorization: Bearer <token>`. The token is verified
/// via the Cedar authorizer's constant-time key comparison.
async fn require_bearer_auth(
    State(state): State<Arc<EdifactApiState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let identity = match state.cedar.authenticate(request.headers()) {
        Some(id) => id,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiError {
                    error: "invalid or missing bearer token",
                }),
            )
                .into_response();
        }
    };

    // Authorization for EDIFACT ingest uses the instance tenant.
    if !state.cedar.authorize_ingest(
        &identity,
        &IngestResource {
            tenant: &state.tenant_id.to_string(),
        },
    ) {
        return (StatusCode::FORBIDDEN, Json(ApiError { error: "forbidden" })).into_response();
    }

    next.run(request).await
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the axum sub-router for EDIFACT REST ingress.
///
/// Mount at the application root or under a path prefix:
/// ```rust,ignore
/// app.merge(edifact_api::router(state));
/// // or
/// app.nest("/api/v1", edifact_api::router(state));
/// ```
pub fn router(state: Arc<EdifactApiState>) -> Router {
    Router::new()
        .route("/edifact", post(ingest_edifact))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_bearer_auth,
        ))
        .layer(DefaultBodyLimit::max(state.max_body_bytes))
        .with_state(state)
}

// ── PARTIN partner extraction ─────────────────────────────────────────────────

/// Build a [`PartnerRecord`] from a parsed `PartinMessage`.
///
/// Extracts the sender's GLN from `NAD+MS` and maps all `COM` segments to
/// [`CommunicationChannel`] entries.  The market role is derived from the
/// BDEW Prüfidentifikator using [`MarketRole::from_pid`].
///
/// Returns `None` when the message has no sender GLN (malformed PARTIN).
pub(crate) fn partin_to_partner_record(
    msg: &edi_energy::messages::partin::PartinMessage,
    pid: Option<u32>,
) -> Option<PartnerRecord> {
    let sender = msg.sender()?;
    let gln_str = sender.party_id.as_deref().filter(|s| !s.is_empty())?;

    let channels: Vec<CommunicationChannel> = msg
        .com_segments()
        .iter()
        .filter_map(|c| {
            let number = c.number.as_deref()?.to_owned();
            let qualifier = c.channel.as_deref()?.to_owned();
            Some(CommunicationChannel::new(qualifier, number))
        })
        .collect();

    let roles = pid
        .and_then(MarketRole::from_pid)
        .map(|r| vec![r])
        .unwrap_or_default();

    Some(PartnerRecord {
        gln: MarktpartnerCode::from(gln_str),
        display_name: sender.party_name.as_deref().map(Into::into),
        channels,
        roles,
        valid_from: None,
        contacts: vec![],
        country_code: None,
        updated_at: time::OffsetDateTime::now_utc(),
    })
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// Accepted EDIFACT `Content-Type` values.
///
/// Per RFC 2838, `application/edifact` is the registered media type for
/// EDIFACT interchanges. `text/plain` is widely used in practice; both are
/// accepted. Any other content-type is rejected with `415 Unsupported Media
/// Type` before the body is read — this limits CPU/memory waste from
/// malformed or hostile requests.
fn is_edifact_content_type(ct: &str) -> bool {
    let base = ct.split(';').next().unwrap_or("").trim();
    matches!(
        base,
        "application/edifact"
            | "text/plain"
            | "application/octet-stream"
            | "text/plain; charset=utf-8"
            | "text/plain; charset=us-ascii"
    )
}

#[utoipa::path(
    post,
    path = "/edifact",
    tag = "edifact",
    request_body(content = String, description = "Raw EDIFACT interchange (UNA+UNB…UNZ)", content_type = "application/edifact"),
    responses(
        (status = 200, description = "Ingest report", body = IngestResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 415, description = "Unsupported content type"),
    ),
    security(
        (),
        ("bearer_token" = [])
    )
)]
pub(crate) async fn ingest_edifact(
    State(state): State<Arc<EdifactApiState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<IngestResponse>) {
    // Reject non-EDIFACT content types before reading the body.
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.is_empty() && !is_edifact_content_type(content_type) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(IngestResponse {
                accepted: 0,
                rejected: 0,
                messages: vec![MessageResult {
                    message_type: None,
                    pid: None,
                    workflow: None,
                    status: MessageStatus::ParseError,
                    process_id: None,
                    malo_id: None,
                    error: Some(format!(
                        "unsupported Content-Type '{content_type}'; \
                         expected 'application/edifact' or 'text/plain'"
                    )),
                }],
            }),
        );
    }

    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(IngestResponse {
                accepted: 0,
                rejected: 0,
                messages: vec![],
            }),
        );
    }

    let mut messages = Vec::new();
    // Collect successfully-parsed messages for CONTRL Empfangsbestätigung emission
    // after the loop (CONTRL AHB 1.0 §1.2: one CONTRL per interchange, not per message).
    let mut parsed_msgs: Vec<AnyMessage> = Vec::new();

    for result in state
        .platform
        .parse_interchange(std::io::Cursor::new(&body[..]))
    {
        match result {
            Err(e) => {
                tracing::warn!(error = %e, "EDIFACT REST ingest: parse error");
                messages.push(MessageResult {
                    message_type: None,
                    pid: None,
                    workflow: None,
                    status: MessageStatus::ParseError,
                    process_id: None,
                    malo_id: None,
                    error: Some(e.to_string()),
                });
            }
            Ok(msg) => {
                let message_type = msg.try_message_type().map(|t| t.to_string());
                let pid = msg.detect_pruefidentifikator().ok().map(|p| p.as_u32());
                let workflow = pid
                    .and_then(|p| state.pid_router.route(p))
                    .map(str::to_owned);

                let status = match (pid, workflow.as_deref()) {
                    (None, _) => MessageStatus::NoPid,
                    (Some(_), Some(_)) => MessageStatus::Routed,
                    (Some(_), None) => MessageStatus::UnknownPid,
                };

                // Auto-upsert PARTIN: when we receive a PARTIN message and a
                // PartnerStore is wired, extract the sender's communication data
                // and store it immediately — no ERP integration needed.
                if let (AnyMessage::Partin(partin), Some(ps)) =
                    (&msg, state.partner_store.as_deref())
                {
                    match partin_to_partner_record(partin, pid) {
                        Some(record) => {
                            if let Err(e) = ps.upsert(state.tenant_id, &record).await {
                                tracing::warn!(
                                    gln = %record.gln,
                                    error = %e,
                                    "PARTIN auto-upsert failed — partner data not stored",
                                );
                            } else {
                                tracing::info!(
                                    gln = %record.gln,
                                    pid,
                                    "PARTIN auto-upsert: partner record stored",
                                );
                            }
                        }
                        None => {
                            tracing::warn!(
                                pid,
                                "PARTIN received but sender GLN missing — skipping auto-upsert",
                            );
                        }
                    }
                }

                tracing::info!(
                    message_type = ?message_type,
                    pid,
                    workflow = ?workflow,
                    status   = ?status,
                    "EDIFACT message received via REST",
                );

                // Phase 2: execute workflow command if dispatcher is wired.
                let mut dispatch_process_id: Option<String> = None;
                let mut dispatch_malo_id: Option<String> = None;
                if let (Some(pid_val), Some(wf_name), Some(dispatcher)) =
                    (pid, workflow.as_deref(), state.dispatcher.as_deref())
                    && matches!(status, MessageStatus::Routed)
                {
                    match dispatcher.dispatch(&msg, wf_name, pid_val).await {
                        Ok(outcome) => {
                            use crate::ingest_dispatcher::IngestOutcome;
                            match &outcome {
                                IngestOutcome::Spawned { process_id, .. }
                                | IngestOutcome::Dispatched { process_id, .. } => {
                                    dispatch_process_id = Some(process_id.to_string());
                                }
                                IngestOutcome::Skipped { .. } => {}
                            }
                            // Extract MaLo from the raw message for the response.
                            dispatch_malo_id =
                                Some(crate::ingest_dispatcher::extract_malo_from_msg(&msg))
                                    .filter(|s| !s.is_empty());
                            tracing::debug!(
                                workflow    = %wf_name,
                                pid         = pid_val,
                                outcome     = ?outcome,
                                process_id  = ?dispatch_process_id,
                                malo_id     = ?dispatch_malo_id,
                                "EDIFACT REST ingest: Phase 2 command dispatched",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                workflow = %wf_name,
                                pid      = pid_val,
                                error    = %e,
                                "EDIFACT REST ingest: Phase 2 command dispatch failed \
                                 (non-fatal — message was routed)",
                            );
                        }
                    }
                }

                messages.push(MessageResult {
                    message_type,
                    pid,
                    workflow,
                    status,
                    process_id: dispatch_process_id,
                    malo_id: dispatch_malo_id,
                    error: None,
                });
                parsed_msgs.push(msg);
            }
        }
    }

    // Emit CONTRL Empfangsbestätigung for Gas interchanges (CONTRL AHB 1.0 §1.2).
    // One CONTRL per interchange (not per message) — emitted once after all messages
    // from the UNB…UNZ have been collected.
    if let Some(contrl_svc) = state.contrl_ack.as_deref() {
        let refs: Vec<&AnyMessage> = parsed_msgs.iter().collect();
        contrl_svc.emit_for_interchange(&refs).await;
    }

    let accepted = messages
        .iter()
        .filter(|m| !matches!(m.status, MessageStatus::ParseError))
        .count();
    let rejected = messages.len() - accepted;

    let http_status = if messages.is_empty() || accepted == 0 {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };

    (
        http_status,
        Json(IngestResponse {
            accepted,
            rejected,
            messages,
        }),
    )
}
