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
//! - The endpoint parses and routes but does **not** yet execute the workflow.
//!   Workflow dispatch requires the full `EngineContext` which is wired in a
//!   later phase (see `ERP.md` §13 Phase 2).
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
use edi_energy::{EdiEnergyMessage, Platform};
use mako_engine::pid_router::PidRouter;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Serialize;
use subtle::ConstantTimeEq;

// ── Shared state ─────────────────────────────────────────────────────────────

/// Shared state for the EDIFACT REST API.
pub struct EdifactApiState {
    pub platform: Arc<Platform>,
    pub pid_router: PidRouter,
    /// When `Some`, every request to protected endpoints must supply
    /// `Authorization: Bearer <token>`. When `None`, the API is unauthenticated
    /// (a startup warning is logged by `main`).
    pub optional_token: Option<SecretString>,
    /// Maximum allowed request body size in bytes.
    /// Applied to `POST /edifact` via [`DefaultBodyLimit`].
    pub max_body_bytes: usize,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct IngestResponse {
    /// Number of messages that were parsed and routed (or had a known PID).
    pub accepted: usize,
    /// Number of messages that could not be parsed at all.
    pub rejected: usize,
    pub messages: Vec<MessageResult>,
}

#[derive(Serialize)]
pub struct MessageResult {
    /// EDIFACT message type, e.g. `"UTILMD"`, `"MSCONS"`, `"APERAK"`.
    /// `null` when the message type could not be determined (parse error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_type: Option<String>,

    /// The Prüfidentifikator extracted from the BGM segment, if present.
    /// `null` for message types that carry no PID (e.g. CONTRL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,

    /// Workflow name from the `PidRouter`, if the PID is registered.
    /// `null` when `pid` is `null` or when the PID is not registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,

    /// Routing outcome for this message.
    pub status: MessageStatus,

    /// Human-readable parse error, present only when `status == "parse_error"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Debug)]
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
/// All routes require `Authorization: Bearer <token>` when
/// [`EdifactApiState::optional_token`] is `Some`.
async fn require_bearer_auth(
    State(state): State<Arc<EdifactApiState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if let Some(expected) = &state.optional_token {
        let provided = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        // Constant-time comparison prevents timing side-channel attacks
        // where an attacker could recover the token byte-by-byte by measuring
        // response latency.
        let authenticated = provided.is_some_and(|p| {
            p.as_bytes()
                .ct_eq(expected.expose_secret().as_bytes())
                .into()
        });

        if !authenticated {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ApiError {
                    error: "invalid or missing bearer token",
                }),
            )
                .into_response();
        }
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

async fn ingest_edifact(
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

                tracing::info!(
                    message_type = ?message_type,
                    pid,
                    workflow = ?workflow,
                    status   = ?status,
                    "EDIFACT message received via REST",
                );

                messages.push(MessageResult {
                    message_type,
                    pid,
                    workflow,
                    status,
                    error: None,
                });
            }
        }
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
