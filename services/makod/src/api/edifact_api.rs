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
    dead_letter::{AuditContext, DeadLetterReason, DeadLetterSink},
    ids::TenantId,
    marktrolle::Marktrolle,
    partner::{CommunicationChannel, PartnerRecord, PartnerStore as _},
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
    /// Our own MP-IDs → Sparte, for commodity-aware routing of Sparte-split
    /// shared PIDs (INSRPT 23001, and the WiM Gas device processes that reuse the
    /// Strom ORDERS/ORDRSP PIDs). The UNB DE0010 recipient MP-ID is the
    /// authoritative Sparte signal — identical to `ContrlAckService`.
    pub mp_id_registry: Arc<crate::core::party_registry::MpIdRegistry>,
    /// Cedar-based authorization engine for all protected endpoints.
    pub cedar: Arc<CedarAuthorizer>,
    /// Maximum allowed request body size in bytes.
    /// Applied to `POST /edifact` via [`DefaultBodyLimit`].
    pub max_body_bytes: usize,
    /// Partner store for automatic PARTIN upserts.
    ///
    /// When `Some`, every valid inbound PARTIN message (PIDs 37000–37014) is
    /// automatically extracted and upserted into the partner directory — no ERP
    /// integration or manual `PUT /admin/partners/{mp_id}` required.
    ///
    /// `None` disables auto-upsert (e.g. in unit tests or read-only contexts).
    pub partner_store: Option<Arc<SlateDbPartnerStore>>,
    /// Tenant identifier for partner store writes.
    pub tenant_id: TenantId,
    /// Dead-letter sink for § 147 AO / GoBD audit records.
    ///
    /// Every rejected, unroutable, or test-flagged message must produce a
    /// structured dead-letter record.  Use `LogDeadLetterSink` for production
    /// (logs structured `tracing::warn!`) or the SlateDB-backed sink for
    /// durable persistence.
    pub dl_sink: std::sync::Arc<dyn DeadLetterSink>,
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

impl EdifactApiState {
    /// Resolve the workflow name for `pid`, preferring the commodity-specific
    /// route when the interchange recipient's Sparte is known. See the free
    /// [`resolve_workflow`] function for the routing rules.
    #[must_use]
    pub fn resolve_workflow(&self, pid: u32, recipient_mp_id: &str) -> Option<&str> {
        resolve_workflow(&self.pid_router, &self.mp_id_registry, pid, recipient_mp_id)
    }
}

/// Resolve the workflow name for `pid`, preferring the commodity-specific route
/// when the interchange recipient's Sparte is known.
///
/// The recipient MP-ID (UNB DE0010) is one of our own parties, and every
/// `[[party]]` covers exactly one Sparte (BDEW §2.13) — the authoritative Sparte
/// signal (identical to [`ContrlAckService`](crate::contrl_ack::ContrlAckService)).
/// PIDs split by commodity via [`PidRouter::register_with_sparte`] (INSRPT 23001,
/// and the WiM Gas Gerätewechsel/Geräteübernahme processes that reuse the Strom
/// ORDERS/ORDRSP PIDs) then resolve to the correct per-Sparte workflow. For a
/// `Both` (Sparte-neutral own party) or unknown recipient, the unambiguous
/// [`PidRouter::route`] table is used. `route_with_sparte` itself falls back to
/// that same table when a PID has no commodity-specific entry, so this is safe
/// for the overwhelming majority of PIDs that are not Sparte-split.
#[must_use]
pub fn resolve_workflow<'a>(
    router: &'a PidRouter,
    registry: &crate::core::party_registry::MpIdRegistry,
    pid: u32,
    recipient_mp_id: &str,
) -> Option<&'a str> {
    use crate::core::party_registry::RoleSparte;
    use mako_engine::types::Sparte;
    match registry.sparte_of(recipient_mp_id) {
        Some(RoleSparte::Strom) => router.route_with_sparte(pid, Sparte::Strom),
        Some(RoleSparte::Gas) => router.route_with_sparte(pid, Sparte::Gas),
        Some(RoleSparte::Both) | None => router.route(pid),
    }
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
    #[schema(example = "51238696012")]
    pub malo_id: Option<String>,

    /// Human-readable parse error, present only when `status == "parse_error"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    /// Parsed and matched to a registered workflow.
    Routed,
    /// Parsed successfully; PID present but not registered in PidRouter.
    UnknownPid,
    /// Parsed successfully; this message type carries no Prüfidentifikator by
    /// design. CONTRL is the only one — it is the EDIFACT syntax
    /// acknowledgement, and its AHB publishes no PID at all.
    NoPid,
    /// Parsed successfully, but `NAD+MS` or `NAD+MR` is absent.
    ///
    /// Allgemeine Festlegungen V6.1d §2.13 identifies the *fachliche* sender
    /// and receiver on message level in those two segments and applies that
    /// "für alle EDI@Energy EDIFACT Nachrichten und -dateien einheitlich".
    /// Both are load-bearing: the sender is the address of the answer and the
    /// APERAK, the receiver decides which of the operator's own MP-IDs — hence
    /// which Sparte and which Marktrolle — was addressed. A message missing
    /// either is refused rather than processed against substituted blanks.
    MissingParty,
    /// Parsed successfully, but the Prüfidentifikator is **missing** from a
    /// message type that must carry one.
    ///
    /// A defect, not a routing gap: without a PID nothing can be routed, no
    /// APERAK can name a process, and the message would otherwise be discarded
    /// behind a `200 OK`. Counted as rejected and dead-lettered, so the loss is
    /// visible rather than silent.
    MissingPid,
    /// Could not be parsed at all.
    ParseError,
}

impl MessageStatus {
    /// Classify one parsed message from its Prüfidentifikator and the workflow
    /// the router resolved for it.
    ///
    /// Shared by both ingest doors. They used to classify independently and had
    /// already drifted: the REST path distinguished [`MessageStatus::NoPid`]
    /// from [`MessageStatus::MissingPid`] and dead-lettered the second, while
    /// the AS4 path — the production door — called every PID-less message
    /// `NoPid` and counted it as accepted, so a UTILMD whose `RFF+Z13` never
    /// parsed was acknowledged and dropped with no § 147 AO record.
    #[must_use]
    pub(crate) fn classify(
        msg: &AnyMessage,
        pid: Option<mako_engine::ids::Pid>,
        workflow: Option<&str>,
    ) -> Self {
        // CONTRL is the EDIFACT syntax acknowledgement, and the one message type
        // that carries neither a Prüfidentifikator nor a `NAD` — it is a
        // UN/EDIFACT service message rather than an EDI@Energy business one.
        // For every other type, a missing PID or a missing party is a defect
        // rather than a shape.
        let is_contrl = matches!(
            msg.try_message_type(),
            Some(edi_energy::MessageType::Contrl)
        );
        if !is_contrl && Self::missing_party(msg).is_some() {
            return Self::MissingParty;
        }
        match (pid, workflow) {
            (None, _) if is_contrl => Self::NoPid,
            (None, _) => Self::MissingPid,
            (Some(_), Some(_)) => Self::Routed,
            (Some(_), None) => Self::UnknownPid,
        }
    }

    /// Which `NAD` qualifier is absent or blank, if either is.
    ///
    /// `"MS"` is reported before `"MR"` when both are missing: the sender is
    /// the one an operator has to chase, because without it there is nobody to
    /// answer.
    #[must_use]
    pub(crate) fn missing_party(msg: &AnyMessage) -> Option<&'static str> {
        if msg.nad_sender().is_none_or(str::is_empty) {
            return Some("MS");
        }
        if msg.nad_receiver().is_none_or(str::is_empty) {
            return Some("MR");
        }
        None
    }

    /// `true` when this status means the message was accepted at the transport
    /// and then lost — the set the ingest doors dead-letter and do not
    /// dispatch.
    #[must_use]
    pub(crate) fn is_unroutable(self) -> bool {
        matches!(
            self,
            Self::UnknownPid | Self::MissingPid | Self::MissingParty
        )
    }
}

/// Validate one routed message against its own release profile and record the
/// outcome; returns `true` when it conforms.
///
/// # Why this is at the boundary
///
/// 36 of the 66 adapter registries ask `adapters::ahb_verdict` and put the
/// answer on their command; the other 30 — mostly the answer-PID adapters,
/// whose messages publish no AHB Anwendungsfall — never validate at all.
/// Emitting `makod_validation_failed_total` from inside the adapters would
/// therefore count adapter invocations rather than messages, and would report
/// nothing for whole families. Here it is exactly once per inbound message,
/// with the message's real type and release as labels.
///
/// A message whose release has no registered profile is **not** counted: that
/// is "there was no rule to break", not "the message broke a rule", and it is
/// the normal shape for an answer PID. `ahb_verdict` reports that case as not
/// passed with an empty error list, which is the discriminator used below.
///
/// The verdict is recorded, not enforced: a non-conforming message still
/// reaches its workflow, because whether to answer it with an Ablehnung, a
/// negative APERAK, or nothing at all is a per-process decision the families
/// that model `validation_passed` already make.
pub(crate) fn record_ahb_conformance(msg: &AnyMessage) -> bool {
    let (passed, errors) = crate::adapters::ahb_verdict(msg);
    if passed || errors.is_empty() {
        return passed;
    }
    let message_type = msg
        .try_message_type()
        .map_or("unknown", edi_energy::MessageType::as_str);
    let release = msg
        .detect_release()
        .map_or("unknown", edi_energy::Release::as_str);
    mako_engine::metrics::EngineMetrics::global().validation_failed(message_type, release);
    tracing::warn!(
        message_type,
        release,
        message_ref = %msg.message_ref(),
        errors = %errors.join("; "),
        "inbound message does not conform to its AHB profile",
    );
    false
}

/// The dead-letter record for a message [`MessageStatus::is_unroutable`]
/// refused, and the `result` label its metric carries.
///
/// Shared by both ingest doors so a new unroutable status cannot be recorded
/// under one reason on `POST /edifact` and another over AS4 — the drift that
/// made the AS4 door accept PID-less messages the REST door dead-lettered.
pub(crate) fn unroutable_rejection(
    status: MessageStatus,
    msg: &AnyMessage,
    pid: mako_engine::ids::Pid,
    context: AuditContext,
) -> (DeadLetterReason, &'static str) {
    match status {
        MessageStatus::MissingParty => (
            DeadLetterReason::MissingInterchangeParty {
                // `classify` only returns this status when one is missing.
                qualifier: MessageStatus::missing_party(msg).unwrap_or("MS"),
                context,
            },
            "missing_interchange_party",
        ),
        _ => (DeadLetterReason::UnknownPid { pid, context }, "unknown_pid"),
    }
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
/// Extracts the sender's MP-ID from `NAD+MS` and maps all `COM` segments to
/// [`CommunicationChannel`] entries.  The market role is derived from the
/// BDEW Prüfidentifikator using [`Marktrolle::from_partin_pid`].
///
/// Returns `None` when the message has no sender MP-ID (malformed PARTIN).
pub(crate) fn partin_to_partner_record(
    msg: &edi_energy::messages::partin::PartinMessage,
    pid: Option<u32>,
) -> Option<PartnerRecord> {
    let sender = msg.sender()?;
    let mp_id_str = sender.party_id.as_deref().filter(|s| !s.is_empty())?;

    // A `COM` segment on an inbound PARTIN is the counterparty telling us where
    // to send their messages — which is what PARTIN is for, and also the most
    // exposed of the three paths that can set a delivery endpoint. Contact data
    // is stored as given; a delivery channel that is not on a secure transport
    // is dropped, because accepting it would let a counterparty's own message
    // direct their regulated traffic, and a MaLo-ID callback, over plaintext.
    // The rest of the record still lands: refusing it wholesale would discard
    // legitimate contact updates over one bad channel.
    let channels: Vec<CommunicationChannel> = msg
        .com_segments()
        .iter()
        .filter_map(|c| {
            let number = c.number.as_deref()?.to_owned();
            let qualifier = c.channel.as_deref()?.to_owned();
            if crate::preflight::is_insecure_delivery_channel(&qualifier, &number) {
                tracing::warn!(
                    mp_id = mp_id_str,
                    qualifier,
                    address = number,
                    "PARTIN import: dropping a delivery channel that is not an \
                     https:// URL",
                );
                return None;
            }
            Some(CommunicationChannel::new(qualifier, number))
        })
        .collect();

    let roles = pid
        .and_then(Marktrolle::from_partin_pid)
        .map(|r| vec![r])
        .unwrap_or_default();

    Some(PartnerRecord {
        mp_id: MarktpartnerCode::from(mp_id_str),
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

    // ── DVGW gas transport ──────────────────────────────────────────────────
    //
    // Tried before the BDEW parse, and nothing above this line may parse the
    // body — a DVGW message rides `ORDERS` or `ORDRSP`, so the BDEW parser
    // accepts an ALOCAT as a well-formed `ORDRSP` *and* reads its
    // Prüfidentifikator correctly out of `RFF+Z13`, which is exactly where it
    // looks. The message therefore routes to the right workflow and arrives as
    // the wrong type: no document code, no gas day, no positions. Neither `UNH`
    // nor the Prüfidentifikator separates the families; only `BGM` DE 1001 does,
    // which is what `try_ingest` sniffs before it commits to the bytes.
    //
    // It returns `None` for a BDEW interchange, so that path pays one head-only
    // scan and nothing else.
    if let Some(report) = crate::dvgw_ingest::try_ingest(state.as_ref(), &body).await {
        let messages: Vec<MessageResult> = report
            .messages
            .iter()
            .map(|m| MessageResult {
                message_type: m.document.map(|d| d.message_type().to_string()),
                pid: m.pruefidentifikator,
                workflow: m.workflow.clone(),
                status: if m.error.is_some() {
                    MessageStatus::ParseError
                } else if m.pruefidentifikator.is_none() {
                    MessageStatus::MissingPid
                } else if m.workflow.is_none() {
                    MessageStatus::UnknownPid
                } else {
                    MessageStatus::Routed
                },
                process_id: m.process_id.clone(),
                // A DVGW message has no MaLo. Its correlation key is the
                // published Zuordnungstupel, which is not a MaLo-shaped value.
                malo_id: None,
                error: m
                    .error
                    .clone()
                    .or_else(|| m.skipped.map(|r| format!("skipped: {r}"))),
            })
            .collect();
        // The CONTRL Empfangsbestätigung is owed here too. CONTRL AHB 1.0 §2.3.1
        // keys the obligation on Sparte, and the DVGW formats *are* the gas
        // transport layer, so it applies unconditionally — and this handler
        // returns before the BDEW path's own emission block below.
        if let Some(contrl_svc) = state.contrl_ack.as_deref() {
            let sender = report.sender_mp_id.clone().unwrap_or_default();
            if let Err(e) = contrl_svc
                .emit_for_dvgw_interchange(
                    &sender,
                    &report.interchange_ref,
                    &report.recipient_mp_id,
                )
                .await
            {
                state.dl_sink.reject(&DeadLetterReason::ProcessingError {
                    message: format!("contrl_ack_failed: {e}"),
                    context: AuditContext::now()
                        .with_message_type("CONTRL")
                        .with_receiver_eic(report.recipient_mp_id.as_str())
                        .with_message_ref(report.interchange_ref.as_str())
                        .with_tenant_id(state.tenant_id.to_string()),
                });
            }
        }

        return (
            StatusCode::OK,
            Json(IngestResponse {
                accepted: report.accepted(),
                rejected: report.rejected(),
                messages,
            }),
        );
    }

    let mut messages = Vec::new();
    // Collect successfully-parsed messages for CONTRL Empfangsbestätigung emission
    // after the loop (CONTRL AHB 1.0 §1.2: one CONTRL per interchange, not per message).
    let mut parsed_msgs: Vec<AnyMessage> = Vec::new();

    // ── Parse interchange (single pass) ──────────────────────────────────────
    //
    // `parse_interchange_full` parses the entire UNB…UNZ envelope in one shot
    // and returns a `ParsedInterchange` with the interchange header and all
    // contained messages.  A single parse is used for three reasons:
    //
    // 1. **Correctness**: `InterchangeHeader::test_indicator` must be checked
    //    *before* dispatching any messages (§AF §3).  Doing this in a separate
    //    pre-scan parse would double-charge CPU for every production request.
    //
    // 2. **Richer context**: each `MessageEnvelope` carries the interchange
    //    header alongside the message, so the § 147 AO / GoBD `AuditContext` for
    //    `UnknownPid` rejections can include the interchange sender/receiver/ref
    //    instead of synthesising a timestamp-only context.
    //
    // 3. **Structural validation**: `ParsedInterchange::is_structurally_valid()`
    //    checks UNZ message-count and control-ref integrity in one expression.
    let pi = match state.platform.parse_interchange_full(&body[..]) {
        Ok(pi) => pi,
        Err(e) => {
            tracing::warn!(error = %e, "EDIFACT REST ingest: interchange parse error");
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(IngestResponse {
                    accepted: 0,
                    rejected: 1,
                    messages: vec![MessageResult {
                        message_type: None,
                        pid: None,
                        workflow: None,
                        status: MessageStatus::ParseError,
                        process_id: None,
                        malo_id: None,
                        error: Some(e.to_string()),
                    }],
                }),
            );
        }
    };

    // ── Test-indicator guard (§AF §3 / Allgemeine Festlegungen V6.1d §3) ──────
    // DE0035 = "1" means test interchange — must never reach production workflows.
    if pi.header.test_indicator {
        let ctx = AuditContext::from_interchange(
            &pi.header.sender_id,
            &pi.header.receiver_id,
            &pi.header.control_ref,
        )
        .with_tenant_id(state.tenant_id.to_string());
        state
            .dl_sink
            .reject(&DeadLetterReason::TestMessage { context: ctx });
        tracing::warn!(
            sender = %pi.header.sender_id,
            receiver = %pi.header.receiver_id,
            control_ref = %pi.header.control_ref,
            "EDIFACT REST ingest: test interchange (DE0035=1) rejected — \
             must not process test messages on production endpoint (§AF §3)",
        );
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(IngestResponse {
                accepted: 0,
                rejected: 1,
                messages: vec![MessageResult {
                    message_type: None,
                    pid: None,
                    workflow: None,
                    status: MessageStatus::ParseError,
                    process_id: None,
                    malo_id: None,
                    error: Some(
                        "test interchange rejected (UNB DE0035=1): \
                         test messages must not reach a production endpoint"
                            .to_owned(),
                    ),
                }],
            }),
        );
    }

    // The UNB DE0010 recipient MP-ID drives commodity-aware routing of
    // Sparte-split shared PIDs (INSRPT, WiM Gas device processes).
    let recipient_mp_id = pi.header.receiver_id.to_string();
    for env in pi.messages {
        // Partial move: `env.header` remains accessible for AuditContext
        // after `env.message` is moved into `msg`.
        let msg = env.message;

        let message_type = msg.try_message_type().map(|t| t.to_string());
        let pid = msg
            .detect_pruefidentifikator()
            .ok()
            .and_then(|p| mako_engine::ids::Pid::from_u32(p.as_u32()));
        let workflow = pid
            .and_then(|p| state.resolve_workflow(p.as_u32(), &recipient_mp_id))
            .map(str::to_owned);

        let status = MessageStatus::classify(&msg, pid, workflow.as_deref());

        // Conformance is recorded for every routed message, not only the ones
        // whose adapter happens to ask — see `record_ahb_conformance`.
        if status == MessageStatus::Routed {
            record_ahb_conformance(&msg);
        }

        // Dead-letter unroutable messages (§ 147 AO / GoBD). A missing PID is
        // dead-lettered on the same path: the message is just as lost, and a
        // silent `accepted: 1` is worse than a rejection because nothing
        // signals it.
        if status.is_unroutable() {
            let ctx = AuditContext::from_interchange(
                &env.header.sender_id,
                &env.header.receiver_id,
                &env.header.control_ref,
            )
            .with_message_type(message_type.as_deref().unwrap_or(""));
            let ctx = if let Some(p) = pid {
                ctx.with_pid(p)
            } else {
                ctx
            };
            let ctx = ctx.with_tenant_id(state.tenant_id.to_string());
            let dead_pid = pid.unwrap_or(mako_engine::ids::Pid::new(1));
            let (reason, result) = unroutable_rejection(status, &msg, dead_pid, ctx);
            // Track the per-PID refusal in the inbound_received metric so
            // Alertmanager can alert on `makod_inbound_messages_total{result=…}`.
            mako_engine::metrics::EngineMetrics::global()
                .inbound_received(dead_pid.as_u32(), result);
            state.dl_sink.reject(&reason);
        }

        // Auto-upsert PARTIN: when we receive a PARTIN message and a
        // PartnerStore is wired, extract the sender's communication data
        // and store it immediately — no ERP integration needed.
        if let (AnyMessage::Partin(partin), Some(ps)) = (&msg, state.partner_store.as_deref()) {
            match partin_to_partner_record(partin, pid.map(|p| p.as_u32())) {
                Some(record) => {
                    if let Err(e) = ps.upsert(state.tenant_id, &record).await {
                        tracing::warn!(
                            mp_id = %record.mp_id,
                            error = %e,
                            "PARTIN auto-upsert failed — partner data not stored",
                        );
                    } else {
                        tracing::info!(
                            mp_id = %record.mp_id,
                            pid = pid.map(|p| p.as_u32()),
                            "PARTIN auto-upsert: partner record stored",
                        );
                    }
                }
                None => {
                    tracing::warn!(
                        pid = pid.map(|p| p.as_u32()),
                        "PARTIN received but sender MP-ID missing — skipping auto-upsert",
                    );
                }
            }
        }

        tracing::info!(
            message_type = ?message_type,
            pid          = pid.map(|p| p.as_u32()),
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
            match dispatcher.dispatch(&msg, wf_name, pid_val.as_u32()).await {
                Ok(outcome) => {
                    use crate::ingest_dispatcher::IngestOutcome;
                    match &outcome {
                        IngestOutcome::Spawned { process_id, .. }
                        | IngestOutcome::Dispatched { process_id, .. } => {
                            dispatch_process_id = Some(process_id.to_string());
                        }
                        IngestOutcome::Skipped { .. } => {}
                    }
                    // The router resolved this PID, so the interchange was
                    // accepted — but no arm consumed it. An acknowledged
                    // inbound message with no process behind it is exactly the
                    // trace § 147 AO / GoBD require, so it is recorded rather
                    // than only logged.
                    if let Some((wf, reason)) = outcome.coverage_gap() {
                        state.dl_sink.reject(&DeadLetterReason::NotDispatchable {
                            workflow_name: wf.to_owned(),
                            pid: pid_val,
                            reason: reason.to_owned(),
                            context: AuditContext::from_interchange(
                                &env.header.sender_id,
                                &env.header.receiver_id,
                                &env.header.control_ref,
                            )
                            .with_message_type(message_type.as_deref().unwrap_or(""))
                            .with_pid(pid_val)
                            .with_tenant_id(state.tenant_id.to_string()),
                        });
                    }
                    // Extract MaLo from the raw message for the response.
                    dispatch_malo_id = Some(String::from(
                        crate::ingest_dispatcher::extract_malo_from_msg(&msg),
                    ))
                    .filter(|s| !s.is_empty());
                    tracing::debug!(
                        workflow    = %wf_name,
                        pid         = pid_val.as_u32(),
                        outcome     = ?outcome,
                        process_id  = ?dispatch_process_id,
                        malo_id     = ?dispatch_malo_id,
                        "EDIFACT REST ingest: Phase 2 command dispatched",
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        workflow = %wf_name,
                        pid      = pid_val.as_u32(),
                        error    = %e,
                        "EDIFACT REST ingest: Phase 2 command dispatch failed \
                         (non-fatal — message was routed)",
                    );
                }
            }
        }

        messages.push(MessageResult {
            message_type,
            pid: pid.map(|p| p.as_u32()),
            workflow,
            status,
            process_id: dispatch_process_id,
            malo_id: dispatch_malo_id,
            error: None,
        });
        parsed_msgs.push(msg);
    }

    // Emit CONTRL Empfangsbestätigung for Gas interchanges (CONTRL AHB 1.0 §1.2).
    // One CONTRL per interchange (not per message) — emitted once after all messages
    // from the UNB…UNZ have been collected.
    if let Some(contrl_svc) = state.contrl_ack.as_deref() {
        let refs: Vec<&AnyMessage> = parsed_msgs.iter().collect();
        if let Err(e) = contrl_svc
            .emit_for_interchange(&refs, &pi.header.control_ref, &pi.header.receiver_id)
            .await
        {
            state.dl_sink.reject(&DeadLetterReason::ProcessingError {
                message: format!("contrl_ack_failed: {e}"),
                context: AuditContext::from_interchange(
                    &pi.header.sender_id,
                    &pi.header.receiver_id,
                    &pi.header.control_ref,
                )
                .with_message_type("CONTRL"),
            });
        }
    }

    let accepted = messages
        .iter()
        .filter(|m| {
            !matches!(
                m.status,
                MessageStatus::ParseError | MessageStatus::MissingPid
            )
        })
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod classify_tests {
    use super::MessageStatus;
    use edi_energy::AnyMessage;

    const LF: &str = "9900204000002";
    const NB: &str = "9900357000004";

    /// A GPKE Anmeldung, with `NAD+MS` / `NAD+MR` under the caller's control.
    fn utilmd(nad: &str) -> AnyMessage {
        let wire = format!(
            "UNB+UNOC:3+{LF}:500+{NB}:500+260804:1045+R1'\
UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E01+55001+9'DTM+137:20260804:102'\
{nad}UNT+6+1'UNZ+1+R1'"
        );
        edi_energy::parse(wire.as_bytes()).expect("the fixture parses")
    }

    fn pid() -> Option<mako_engine::ids::Pid> {
        mako_engine::ids::Pid::from_u32(55001)
    }

    /// The control: with both parties present the message routes.
    #[test]
    fn a_message_carrying_both_parties_is_routed() {
        let msg = utilmd(&format!("NAD+MS+{LF}::293'NAD+MR+{NB}::293'"));
        assert_eq!(
            MessageStatus::classify(&msg, pid(), Some("gpke-lf-anmeldung")),
            MessageStatus::Routed
        );
        assert_eq!(MessageStatus::missing_party(&msg), None);
    }

    /// # Why this is a test
    ///
    /// Every adapter reads the sender as
    /// `sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")` — 47 sites.
    /// That empty string becomes the workflow's counterparty, and the
    /// counterparty is what the answer's outbox entry is addressed to. A
    /// message with no `NAD+MS` therefore used to spawn a real process, run a
    /// real Frist, and produce an answer addressed to nobody — surfacing much
    /// later as an `OutboxExhausted` dead letter for partner `""` rather than
    /// as the defective message it was.
    #[test]
    fn a_message_without_a_sender_party_is_refused() {
        let msg = utilmd(&format!("NAD+MR+{NB}::293'"));
        assert_eq!(MessageStatus::missing_party(&msg), Some("MS"));
        assert_eq!(
            MessageStatus::classify(&msg, pid(), Some("gpke-lf-anmeldung")),
            MessageStatus::MissingParty
        );
        assert!(MessageStatus::MissingParty.is_unroutable());
    }

    /// `NAD+MR` decides which of the operator's own MP-IDs was addressed, and
    /// therefore the Sparte and the Marktrolle. Without it every Gas message
    /// silently reads as Strom and is answered out of the wrong
    /// Entscheidungsbaum.
    #[test]
    fn a_message_without_a_receiver_party_is_refused() {
        let msg = utilmd(&format!("NAD+MS+{LF}::293'"));
        assert_eq!(MessageStatus::missing_party(&msg), Some("MR"));
        assert_eq!(
            MessageStatus::classify(&msg, pid(), Some("gpke-lf-anmeldung")),
            MessageStatus::MissingParty
        );
    }

    /// `makod_validation_failed_total` has exactly one emitter, and it is the
    /// boundary recorder.
    ///
    /// # Why this is a test
    ///
    /// The metric is documented as carrying `message_type` and `release`. Its
    /// only emitter used to be the AS4 parse-error branch, with both labels
    /// hard-coded to `("edifact", "parse_error")` — so it reported a message
    /// type that does not exist, never reported a release, and never counted a
    /// single AHB validation failure. Moving it into the adapters would have
    /// counted adapter invocations instead of messages, and reported nothing
    /// for the 30 registries that never validate. One emitter, at the
    /// boundary, is the only shape that counts each inbound message once.
    #[test]
    fn the_conformance_counter_has_one_emitter() {
        const SOURCES: &[(&str, &str)] = &[
            ("edifact_api.rs", include_str!("edifact_api.rs")),
            ("as4_ingest.rs", include_str!("../transport/as4_ingest.rs")),
            (
                "adapters/mod.rs",
                include_str!("../orchestrator/adapters/mod.rs"),
            ),
            (
                "adapters/gpke.rs",
                include_str!("../orchestrator/adapters/gpke.rs"),
            ),
            (
                "adapters/wim.rs",
                include_str!("../orchestrator/adapters/wim.rs"),
            ),
            (
                "adapters/geli_gas.rs",
                include_str!("../orchestrator/adapters/geli_gas.rs"),
            ),
            (
                "adapters/gabi_gas.rs",
                include_str!("../orchestrator/adapters/gabi_gas.rs"),
            ),
            (
                "adapters/mabis.rs",
                include_str!("../orchestrator/adapters/mabis.rs"),
            ),
        ];
        // Split so the needle does not appear verbatim in this file, which is
        // one of the sources being scanned.
        let needle = concat!(".validation", "_failed(");
        for (file, src) in SOURCES {
            let calls = src.matches(needle).count();
            let expected = usize::from(*file == "edifact_api.rs");
            assert_eq!(
                calls, expected,
                "{file} emits makod_validation_failed_total {calls} time(s), expected \
                 {expected}. The single emitter is record_ahb_conformance."
            );
        }
    }

    /// CONTRL is a UN/EDIFACT service message, not an EDI@Energy business one:
    /// it carries no `NAD` and no Prüfidentifikator by construction, so the
    /// party rule must not turn every acknowledgement into a dead letter.
    #[test]
    fn a_contrl_is_exempt_from_both_rules() {
        let wire = format!(
            "UNB+UNOC:3+{NB}:500+{LF}:500+260804:1045+R2'\
UNH+1+CONTRL:D:3:UN:2.0b'UCI+R1+{NB}:500+{LF}:500+7'UNT+3+1'UNZ+1+R2'"
        );
        let msg = edi_energy::parse(wire.as_bytes()).expect("the CONTRL fixture parses");
        assert_eq!(
            MessageStatus::classify(&msg, None, None),
            MessageStatus::NoPid
        );
        assert!(!MessageStatus::NoPid.is_unroutable());
    }
}
