//! AS4 inbound transport for BDEW MaKo market communication.
//!
//! This module wires the [`asx_rs`] AS4 receive pipeline to the same
//! EDIFACT dispatch layer used by the REST ingest API.  Every EDIFACT
//! UserMessage delivered over AS4 passes through:
//!
//! 1. **HTTP pre-validation** — performed by the `asx_rs` Axum router before
//!    our handler is invoked (method, `Content-Type: application/soap+xml`,
//!    body size / read timeout).
//! 2. **AS4 receive pipeline** — WS-Security signature verification, replay
//!    deduplication via [`SlateDbDedupBridge`], MIME multipart parsing, payload
//!    extraction (`receive_push_with_dedup_async`).
//! 3. **EDIFACT dispatch** — identical to the REST `POST /edifact` path:
//!    `Platform::parse_interchange` → `PidRouter` → structured log.
//! 4. **Synchronous receipt** — an `eb:Receipt` SignalMessage is returned on
//!    the same HTTP connection, satisfying the BDEW AS4 MEP requirement.
//!    When receipt-signing credentials are configured
//!    ([`BdewAs4IngestHandler::with_receipt_credentials`]), the receipt is
//!    **signed** and echoes the inbound message's `ds:Reference` digests as
//!    NonRepudiationInformation, satisfying Non-Repudiation of Receipt per
//!    BDEW AS4-Profil §2.2.4.  Without credentials the receipt is **unsigned**
//!    (dev/test only) and a warning is logged.
//!
//! # Security notes
//!
//! - Signing is **mandatory** for BDEW production deployments
//!   (`As4PushPolicy::regulated()`), including receipt signing — configure
//!   `with_receipt_credentials` with the operator's signing key pair.
//! - The dedup bridge is backed by SlateDB (single-node durable storage).
//!   Multi-replica deployments should replace it with a distributed backend.
//! - Receipt `message_id` values are UUID v4, scoped with a `makod@` prefix
//!   to distinguish them from sender-generated IDs.
//!
//! # Layering
//!
//! `mako-as4` remains a pure BDEW protocol/profile crate (no engine dep).
//! This module is the glue layer in `makod` that knows about both the engine
//! (`InboxStore`) and the transport (`asx_rs`).

use std::sync::Arc;

use asx_rs::as4::{
    As4ReceiptCredentials, As4ReceiveOutcome, As4ReceivePushRequest, generate_receipt_for_output,
    generate_signed_receipt_for_output, receive_push_with_dedup_async,
};
use asx_rs::core::{AsxError, ErrorCode, ErrorContext, SessionContext};
use asx_rs::crypto::wssec::WsSecOutboundKeyInfoProfile;
use asx_rs::observability::EventBus;
use asx_rs::storage::{BoxFuture, DedupStorage};
use asx_rs::transport::ingress::As4HttpIngress;
use asx_rs::transport::server::{As4AxumHandler, HandlerOutcome, as4_router};
use axum::Router;
use edi_energy::{AnyMessage, EdiEnergyMessage};
use mako_as4::server::RouterConfig;
use mako_engine::inbox::InboxStore;
use mako_engine::metrics::EngineMetrics;
use mako_engine::store_slatedb::SlateDbInboxStore;
use uuid::Uuid;

use crate::edifact_api::{EdifactApiState, MessageStatus};

// ── Dedup bridge ──────────────────────────────────────────────────────────────

/// Adapts [`SlateDbInboxStore`] to the async [`asx_rs::storage::DedupStorage`]
/// interface required by the AS4 receive pipeline.
///
/// In asx-rs 0.2, `DedupStorage::first_seen` returns a `BoxFuture`, so
/// `InboxStore::accept` can be called directly without `block_in_place` /
/// `block_on` boilerplate.
///
/// # Cluster safety
///
/// `cluster_safe()` returns `false` because SlateDB is a single-process store.
/// For multi-replica deployments, replace this with a distributed dedup backend
/// (Redis, PostgreSQL) that implements `DedupStorage` with `cluster_safe = true`.
pub struct SlateDbDedupBridge {
    store: Arc<SlateDbInboxStore>,
    /// `true` when the underlying SlateDB is backed by a persistent store
    /// (`--data-dir` or cloud object store). `false` when running in volatile
    /// (in-memory) mode — inbox dedup state is lost on restart and every AS4
    /// retry within the 72-hour window will re-process as a new message.
    ///
    /// Set from the `makod` configuration at startup so that the `asx-rs`
    /// receive pipeline can surface appropriate diagnostics via `is_durable()`.
    durable: bool,
}

impl SlateDbDedupBridge {
    pub fn new(store: Arc<SlateDbInboxStore>, durable: bool) -> Self {
        Self { store, durable }
    }
}

impl DedupStorage for SlateDbDedupBridge {
    fn is_durable(&self) -> bool {
        // Report true only when the underlying SlateDB is backed by a
        // persistent store. In volatile (in-memory) mode, inbox dedup state is
        // lost on restart; `asx-rs` uses this flag to surface a diagnostic so
        // operators know that message idempotency is not guaranteed across
        // restarts. The `makod`-level VOLATILE MODE warning is still emitted at
        // startup regardless of this flag.
        self.durable
    }

    fn cluster_safe(&self) -> bool {
        false
    }

    fn first_seen<'a>(
        &'a self,
        idempotency_key: &'a str,
    ) -> BoxFuture<'a, asx_rs::core::Result<bool>> {
        let store = Arc::clone(&self.store);
        let key = idempotency_key.to_owned();
        Box::pin(async move {
            store.accept(&key).await.map_err(|e| {
                AsxError::new(
                    ErrorCode::StorageBackendFailure,
                    format!("inbox dedup store error: {e}"),
                    ErrorContext::new("as4_dedup"),
                )
            })
        })
    }
}

// ── AS4 receive handler ───────────────────────────────────────────────────────

/// AS4 inbound message handler for BDEW MaKo.
///
/// Shared across concurrent requests via `Arc<BdewAs4IngestHandler>`.
/// All fields are `Send + Sync`.
pub struct BdewAs4IngestHandler {
    /// Shared EDIFACT dispatch state (Platform + PidRouter).
    ingest: Arc<EdifactApiState>,
    /// Per-session WS-Security context (signing key + partner trust anchors).
    session: Arc<SessionContext>,
    /// Telemetry event bus.
    event_bus: Arc<EventBus>,
    /// Deduplication backend.
    dedup: Arc<dyn DedupStorage>,
    /// Operator's own AS4 inbound decryption private key (EC, BrainpoolP256r1).
    ///
    /// Per BDEW AS4-Profil v1.2 §2.2.6.2.2 inbound messages are encrypted with
    /// the operator's EC public key. asx-rs v0.7 decrypts them via ECDH-ES +
    /// ConcatKDF when an EC private key is supplied.
    decryption_key_pem: Option<std::sync::Arc<[u8]>>,
    /// Operator signing credentials for outbound `eb:Receipt` signals.
    ///
    /// Per BDEW AS4-Profil §2.2.4 receipts must be signed (Non-Repudiation of
    /// Receipt) and echo the inbound message's signed digests as
    /// NonRepudiationInformation.  When `None`, receipts are emitted
    /// **unsigned** — strict counterparties will reject them; dev/test only.
    receipt_credentials: Option<As4ReceiptCredentials>,
    /// CONTRL Empfangsbestätigung emitter for Gas interchanges.
    ///
    /// Per CONTRL AHB 1.0 §2.3.1 and APERAK AHB 1.0 §2.3 (Gas rules),
    /// a CONTRL must be sent for every inbound Gas interchange (except
    /// CONTRL-on-CONTRL) within 6 wall-clock hours. The AS4-level
    /// `eb:Receipt` is a separate protocol acknowledgement and does NOT
    /// satisfy this obligation. Set to `None` for Strom-only deployments.
    pub contrl_ack: Option<Arc<crate::contrl_ack::ContrlAckService>>,
}

impl BdewAs4IngestHandler {
    pub fn new(
        ingest: Arc<EdifactApiState>,
        session: Arc<SessionContext>,
        event_bus: Arc<EventBus>,
        dedup: Arc<dyn DedupStorage>,
    ) -> Self {
        Self {
            ingest,
            session,
            event_bus,
            dedup,
            decryption_key_pem: None,
            receipt_credentials: None,
            contrl_ack: None,
        }
    }

    /// Set the operator's own AS4 inbound decryption private key.
    ///
    /// When set, `As4PushPolicy.inbound_decryption_key_pem` is populated so
    /// inbound encrypted AS4 messages can be decrypted. The key must be EC
    /// (BrainpoolP256r1) corresponding to the encryption certificate published
    /// to BDEW trading partners.
    #[must_use]
    pub fn with_decryption_key_pem(mut self, key_pem: Option<Vec<u8>>) -> Self {
        self.decryption_key_pem = key_pem.map(|k| std::sync::Arc::from(k.as_slice()));
        self
    }

    /// Set the operator signing credentials for outbound `eb:Receipt` signals.
    ///
    /// When set, synchronous receipts are signed (WS-Security XML Signature,
    /// `X509PKIPathv1` key-info per BDEW AS4-Profil §2.2.6.2.1) and carry the
    /// inbound message's `ds:Reference` digests as NonRepudiationInformation
    /// — satisfying Non-Repudiation of Receipt per BDEW AS4-Profil §2.2.4.
    ///
    /// **Required for BDEW production deployments.** Without it, receipts are
    /// emitted unsigned and strict counterparties will reject them.
    #[must_use]
    pub fn with_receipt_credentials(
        mut self,
        signing_key_pem: Vec<u8>,
        signing_cert_pem: Vec<u8>,
    ) -> Self {
        self.receipt_credentials = Some(As4ReceiptCredentials {
            signing_key_pem,
            signing_cert_pem,
            key_info_profile: WsSecOutboundKeyInfoProfile::X509PKIPathv1,
        });
        self
    }

    /// Wire a `ContrlAckService` to emit Gas CONTRL Empfangsbestätigungen
    /// on every inbound Gas interchange (CONTRL AHB 1.0 §2.3.1).
    #[must_use]
    pub fn with_contrl_ack(mut self, svc: Arc<crate::contrl_ack::ContrlAckService>) -> Self {
        self.contrl_ack = Some(svc);
        self
    }
}

impl As4AxumHandler for BdewAs4IngestHandler {
    async fn handle(&self, ingress: As4HttpIngress) -> HandlerOutcome {
        let request = As4ReceivePushRequest {
            http_content_type: ingress.content_type.clone(),
            payload: Arc::clone(&ingress.body),
            receipt_payload: None,
            policy: mako_as4::bdew_push_policy(
                self.decryption_key_pem
                    .as_ref()
                    .map(|k| k.as_ref().to_vec()),
            ),
            authenticated_sender_scope: None,
        };

        match receive_push_with_dedup_async(
            &self.session,
            &self.event_bus,
            request,
            Arc::clone(&self.dedup),
        )
        .await
        {
            Err(e) => {
                tracing::warn!(
                    error      = %e,
                    error_code = ?e.code,
                    "AS4 inbound: receive pipeline failed",
                );
                HandlerOutcome::bad_request(format!("AS4 receive failed: {e}"))
            }

            Ok(As4ReceiveOutcome::Duplicate { message_id }) => {
                // Idempotent replay: the dedup store has already seen this
                // message_id.  Per BDEW AS4 §4.3 retransmissions must receive
                // a valid acknowledgement — return 200 without re-dispatching.
                tracing::debug!(
                    as4_message_id = %message_id,
                    "AS4 inbound: duplicate detected — returning idempotent 200",
                );
                HandlerOutcome::ok()
            }

            Ok(As4ReceiveOutcome::FirstSeen(output)) => {
                let msg_id = output.user_message.message_id.clone();
                let action = &output.user_message.action;
                let from = output
                    .user_message
                    .from_party_ids
                    .first()
                    .map(String::as_str)
                    .unwrap_or("<unknown>");
                let edifact = output.payload.clone().into_inner();

                tracing::info!(
                    as4_message_id = %msg_id,
                    action         = %action,
                    from_party     = %from,
                    payload_bytes  = edifact.len(),
                    "AS4 inbound: message received",
                );
                // ── Test-indicator guard (§AF §3 / Allgemeine Festlegungen V6.1d §3) ──
                // Reject before dispatching any messages.
                if let Ok(pi) = self.ingest.platform.parse_interchange_full(&edifact[..])
                    && pi.header.test_indicator
                {
                    use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                    let ctx = AuditContext::from_interchange(
                        &pi.header.sender_id,
                        &pi.header.receiver_id,
                        &pi.header.control_ref,
                    );
                    self.ingest
                        .dl_sink
                        .reject(&DeadLetterReason::TestMessage { context: ctx });
                    tracing::warn!(
                        as4_message_id = %msg_id,
                        sender = %pi.header.sender_id,
                        receiver = %pi.header.receiver_id,
                        control_ref = %pi.header.control_ref,
                        "AS4 ingest: test interchange (DE0035=1) rejected — \
                         must not process test messages on production endpoint (§AF §3)",
                    );
                    return HandlerOutcome::bad_request(
                        "test interchange rejected: DE0035=1 on production endpoint",
                    );
                }
                // ── EDIFACT dispatch ──────────────────────────────────────────
                // Collect parsed messages so they can be passed to ContrlAckService
                // after the dispatch loop (CONTRL AHB 1.0 §2.3.1 Gas obligation).
                let interchange_ref: String =
                    if let Ok(pi) = self.ingest.platform.parse_interchange_full(&edifact[..]) {
                        pi.header.control_ref.to_string()
                    } else {
                        msg_id.clone()
                    };
                let mut accepted = 0usize;
                let mut rejected = 0usize;
                let mut parsed_msgs: Vec<edi_energy::AnyMessage> = Vec::new();
                for result in self
                    .ingest
                    .platform
                    .parse_interchange(std::io::Cursor::new(&edifact[..]))
                {
                    match result {
                        Err(e) => {
                            rejected += 1;
                            // Count parse/validation failures for Prometheus alerting.
                            EngineMetrics::global().validation_failed("edifact", "parse_error");
                            // §22 MessZV: a message that fails to parse inside an
                            // otherwise-accepted interchange must leave a durable
                            // trace — the AS4 receipt confirms receipt of the whole
                            // interchange, so a metric + log alone would make the
                            // failed message vanish from the audit trail.
                            {
                                use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                                self.ingest
                                    .dl_sink
                                    .reject(&DeadLetterReason::ProcessingError {
                                        message: format!("EDIFACT parse error: {e}"),
                                        context: AuditContext::now()
                                            .with_message_type("UNPARSEABLE")
                                            .with_message_ref(msg_id.as_str()),
                                    });
                            }
                            tracing::warn!(
                                as4_message_id = %msg_id,
                                error          = %e,
                                "AS4 ingest: EDIFACT parse error — dead-lettered",
                            );
                        }
                        Ok(msg) => {
                            let message_type = msg.try_message_type().map(|t| t.to_string());
                            let pid = msg
                                .detect_pruefidentifikator()
                                .ok()
                                .and_then(|p| mako_engine::ids::Pid::from_u32(p.as_u32()));
                            let workflow = pid
                                .and_then(|p| self.ingest.pid_router.route(p.as_u32()))
                                .map(str::to_owned);

                            let status = match (pid, workflow.as_deref()) {
                                (None, _) => MessageStatus::NoPid,
                                (Some(_), Some(_)) => MessageStatus::Routed,
                                (Some(_), None) => MessageStatus::UnknownPid,
                            };

                            // Dead-letter unroutable messages (§22 MessZV).
                            if matches!(status, MessageStatus::UnknownPid) {
                                use mako_engine::dead_letter::{AuditContext, DeadLetterReason};
                                let ctx = AuditContext::now()
                                    .with_message_type(message_type.as_deref().unwrap_or(""));
                                let ctx = if let Some(p) = pid {
                                    ctx.with_pid(p)
                                } else {
                                    ctx
                                };
                                let dead_pid = pid.unwrap_or(mako_engine::ids::Pid::new(1));
                                self.ingest.dl_sink.reject(&DeadLetterReason::UnknownPid {
                                    pid: dead_pid,
                                    context: ctx,
                                });
                            }

                            tracing::info!(
                                as4_message_id = %msg_id,
                                message_type   = ?message_type,
                                pid            = pid.map(|p| p.as_u32()),
                                workflow       = ?workflow,
                                status         = ?status,
                                "AS4 ingest: EDIFACT message dispatched",
                            );

                            // Phase 2: execute workflow command if dispatcher is wired.
                            if let (Some(pid_val), Some(wf_name)) = (pid, workflow.as_deref())
                                && let Some(dispatcher) = self.ingest.dispatcher.as_deref()
                            {
                                match dispatcher.dispatch(&msg, wf_name, pid_val.as_u32()).await {
                                    Ok(outcome) => tracing::debug!(
                                        as4_message_id = %msg_id,
                                        workflow       = %wf_name,
                                        outcome        = ?outcome,
                                        "AS4 ingest: Phase 2 command dispatched",
                                    ),
                                    Err(e) => tracing::warn!(
                                        as4_message_id = %msg_id,
                                        workflow       = %wf_name,
                                        error          = %e,
                                        "AS4 ingest: Phase 2 command dispatch failed (non-fatal)",
                                    ),
                                }
                            }

                            accepted += 1;
                            // Collect for CONTRL Empfangsbestätigung (Gas interchanges).
                            parsed_msgs.push(msg);
                        }
                    }
                }

                if accepted == 0 && rejected > 0 {
                    return HandlerOutcome::bad_request(
                        "AS4 payload contained no valid EDIFACT messages",
                    );
                }

                // ── Gas CONTRL Empfangsbestätigung ────────────────────────────
                // CONTRL AHB 1.0 §2.3.1: for every inbound Gas interchange
                // (except CONTRL-on-CONTRL) the receiver must send a CONTRL
                // Empfangsbestätigung within 6 wall-clock hours.
                // The AS4 eb:Receipt above is a *protocol* acknowledgement and
                // does not satisfy this EDIFACT-level obligation.
                if let Some(contrl_svc) = self.contrl_ack.as_deref() {
                    let refs: Vec<&AnyMessage> = parsed_msgs.iter().collect();
                    contrl_svc
                        .emit_for_interchange(&refs, &interchange_ref)
                        .await;
                }

                // ── Synchronous receipt ───────────────────────────────────────
                // BDEW AS4-Profil §2.2.4: the receipt must be signed and echo
                // the inbound message's ds:Reference digests (NRR).  Unsigned
                // receipts are a dev/test fallback only — strict counterparties
                // reject them.
                let receipt_id = format!("makod@{}", Uuid::new_v4());
                let receipt = match &self.receipt_credentials {
                    Some(credentials) => generate_signed_receipt_for_output(
                        &self.session,
                        &receipt_id,
                        &output,
                        &ingress.body,
                        &ingress.content_type,
                        credentials,
                    ),
                    None => {
                        tracing::warn!(
                            as4_message_id = %msg_id,
                            "AS4 inbound: no receipt-signing credentials configured — \
                             emitting UNSIGNED receipt without NRI. This violates BDEW \
                             AS4-Profil §2.2.4; configure with_receipt_credentials for \
                             production.",
                        );
                        generate_receipt_for_output(&self.session, &receipt_id, &output)
                    }
                };
                match receipt {
                    Ok(receipt_xml) => {
                        tracing::debug!(
                            as4_message_id = %msg_id,
                            receipt_id     = %receipt_id,
                            signed         = self.receipt_credentials.is_some(),
                            "AS4 inbound: sending synchronous receipt",
                        );
                        HandlerOutcome::ok_with_body(receipt_xml, "application/soap+xml")
                    }
                    Err(e) => {
                        // Receipt generation failure is non-fatal for the
                        // business payload — message was already dispatched.
                        // Return 200 without a receipt body; the sender will
                        // retry and hit the dedup path.
                        tracing::error!(
                            as4_message_id = %msg_id,
                            error          = %e,
                            "AS4 inbound: receipt generation failed — returning 200 without body",
                        );
                        HandlerOutcome::ok()
                    }
                }
            }

            // `As4ReceiveOutcome` is `#[non_exhaustive]` — keep a catch-all for
            // future variants so new protocol outcomes don't silently fall through.
            Ok(_) => {
                tracing::warn!("AS4 inbound: unhandled receive outcome variant");
                HandlerOutcome::ok()
            }
        }
    }
}

// ── Router builder ────────────────────────────────────────────────────────────

/// Build the Axum sub-router that mounts the BDEW AS4 inbound endpoint.
///
/// The router accepts `POST /as4/inbox` with `Content-Type: application/soap+xml`.
/// Merge it into the top-level application router:
///
/// ```ignore
/// app = app.merge(as4_ingest::router(handler, config));
/// ```
pub fn router(handler: Arc<BdewAs4IngestHandler>, config: RouterConfig) -> Router {
    as4_router(handler, "/as4/inbox", config)
}

// ── AS4 inbound rate-limit middleware ─────────────────────────────────────────

/// Per-peer-IP GCRA rate limiter for the AS4 inbound endpoint.
///
/// **Keyed by client IP**: each peer gets its own token bucket (sustained
/// **100 requests/second**, burst **50**), so one noisy or malicious
/// counterparty cannot exhaust the budget for everyone else — the failure
/// mode of a single global bucket. Returns `HTTP 429 Too Many Requests` when
/// a peer's bucket is empty, protecting the event store from capacity
/// exhaustion (OWASP A05).
///
/// The key is the socket peer address (`ConnectInfo`); deployments behind a
/// load balancer terminate client connections there, so the LB must enforce
/// its own per-client limits (`X-Forwarded-For` is spoofable and deliberately
/// not trusted here).
static AS4_RATE_LIMITER: std::sync::LazyLock<
    governor::RateLimiter<
        std::net::IpAddr,
        governor::state::keyed::DefaultKeyedStateStore<std::net::IpAddr>,
        governor::clock::DefaultClock,
    >,
> = std::sync::LazyLock::new(|| {
    use std::num::NonZeroU32;
    let quota = governor::Quota::per_second(NonZeroU32::new(100).unwrap())
        .allow_burst(NonZeroU32::new(50).unwrap());
    governor::RateLimiter::keyed(quota)
});

/// Per-sender-MP-ID GCRA rate limiter for the AS4 inbound endpoint.
///
/// **Keyed by the `eb:PartyId` inside `eb:From`**, extracted from the SOAP
/// header *before* the expensive receive pipeline (signature verification,
/// decryption) runs — that cost is exactly what a flood tries to trigger.
///
/// The value is **unverified** at this point and therefore spoofable; that is
/// acceptable for a rate limiter because both limits always apply: a spoofing
/// attacker still burns their own per-IP budget, and a spoofed partner can at
/// worst see extra `429`s (never extra capacity). The verified identity is
/// established later by WS-Security. Per-partner quota: 50 req/s sustained,
/// burst 25 — half the per-IP quota, still far above any real MSH's peak.
static AS4_SENDER_RATE_LIMITER: std::sync::LazyLock<
    governor::RateLimiter<
        String,
        governor::state::keyed::DefaultKeyedStateStore<String>,
        governor::clock::DefaultClock,
    >,
> = std::sync::LazyLock::new(|| {
    use std::num::NonZeroU32;
    let quota = governor::Quota::per_second(NonZeroU32::new(50).unwrap())
        .allow_burst(NonZeroU32::new(25).unwrap());
    governor::RateLimiter::keyed(quota)
});

/// Extract the first `eb:From/eb:PartyId` text from a SOAP prefix.
///
/// A deliberately cheap scan over the first bytes of the (possibly MIME-
/// wrapped) request body — no XML parse, no allocation beyond the result.
/// Returns `None` when the structure is absent; the caller then applies only
/// the per-IP limit.
fn extract_sender_mp_id(body: &[u8]) -> Option<String> {
    // The ebMS header sits early in the envelope; 16 KiB is generous.
    let window = &body[..body.len().min(16 * 1024)];
    let text = std::str::from_utf8(window).ok()?;
    let from_idx = text.find(":From>").or_else(|| text.find(":From "))?;
    let after_from = &text[from_idx..];
    let pid_open = after_from.find(":PartyId")?;
    let after_open = &after_from[pid_open..];
    let gt = after_open.find('>')?;
    let rest = &after_open[gt + 1..];
    let lt = rest.find('<')?;
    let value = rest[..lt].trim();
    // MP-IDs are 13-digit codes or 16-char EIC — reject anything else so a
    // crafted header cannot grow the keyed state with garbage keys.
    let valid = (value.len() == 13 && value.bytes().all(|b| b.is_ascii_digit()))
        || (value.len() == 16
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    valid.then(|| value.to_owned())
}

/// Axum middleware combining the per-peer and per-sender AS4 rate limits.
///
/// Order: per-IP first (no body read), then per-MP-ID on the buffered body.
/// The body is buffered once here and handed onward — the AS4 router buffers
/// it anyway, so this adds no extra copy of the payload bytes.
pub async fn as4_rate_limit_middleware(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if AS4_RATE_LIMITER.check_key(&peer.ip()).is_err() {
        tracing::warn!(
            peer = %peer.ip(),
            "AS4 inbound rate limit exceeded (100 req/s per peer) — returning 429",
        );
        return too_many_requests();
    }

    // Buffer the body to inspect the ebMS sender. 64 MiB cap matches the
    // AS4 router's own body limit.
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return too_many_requests_status(axum::http::StatusCode::PAYLOAD_TOO_LARGE),
    };
    if let Some(sender) = extract_sender_mp_id(&bytes)
        && AS4_SENDER_RATE_LIMITER.check_key(&sender).is_err()
    {
        tracing::warn!(
                peer      = %peer.ip(),
                sender_mp_id = %sender,
                "AS4 inbound per-sender rate limit exceeded (50 req/s) — returning 429. \
                 Sender identity is pre-verification (spoofable); the per-IP limit \
                 has already been applied.",
        );
        return too_many_requests();
    }
    let req = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    next.run(req).await
}

fn too_many_requests() -> axum::response::Response {
    too_many_requests_status(axum::http::StatusCode::TOO_MANY_REQUESTS)
}

fn too_many_requests_status(status: axum::http::StatusCode) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .header("Retry-After", "1")
        .header("Content-Type", "text/plain")
        .body(axum::body::Body::from(
            "AS4 inbound rate limit exceeded. Retry after 1 second.",
        ))
        .unwrap_or_else(|_| {
            axum::response::Response::builder()
                .status(status)
                .body(axum::body::Body::empty())
                .unwrap()
        })
}

/// Axum middleware that enforces the per-peer AS4 inbound rate limit.
///
/// Returns `429 Too Many Requests` with a `Retry-After: 1` header when the
/// peer's GCRA token bucket is exhausted. A `tracing::warn!` is emitted on
/// each rejection so operators can detect unusual traffic patterns.
pub async fn rate_limit_middleware(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    match AS4_RATE_LIMITER.check_key(&peer.ip()) {
        Ok(()) => next.run(req).await,
        Err(_) => {
            tracing::warn!(
                method = %req.method(),
                uri    = %req.uri(),
                peer   = %peer.ip(),
                "AS4 inbound rate limit exceeded (100 req/s per peer) — returning 429. \
                 Possible misconfigured or malicious counterparty."
            );
            axum::response::Response::builder()
                .status(axum::http::StatusCode::TOO_MANY_REQUESTS)
                .header("Retry-After", "1")
                .header("Content-Type", "text/plain")
                .body(axum::body::Body::from(
                    "AS4 inbound rate limit exceeded. Retry after 1 second.",
                ))
                .unwrap_or_else(|_| {
                    axum::response::Response::builder()
                        .status(axum::http::StatusCode::TOO_MANY_REQUESTS)
                        .body(axum::body::Body::empty())
                        .unwrap()
                })
        }
    }
}

#[cfg(test)]
mod sender_extract_tests {
    use super::extract_sender_mp_id;

    /// The happy path: a BDEW ebMS header yields the 13-digit sender MP-ID.
    #[test]
    fn extracts_thirteen_digit_sender() {
        let soap = br#"<S12:Envelope><S12:Header><eb:Messaging>
            <eb:UserMessage><eb:PartyInfo>
            <eb:From><eb:PartyId type="urn:oasis:names:tc:ebcore:partyid-type:unregistered:BDEW">9900001000001</eb:PartyId>
            <eb:Role>ZSH</eb:Role></eb:From>
            <eb:To><eb:PartyId>9900001000002</eb:PartyId></eb:To>
            </eb:PartyInfo></eb:UserMessage></eb:Messaging></S12:Header></S12:Envelope>"#;
        assert_eq!(extract_sender_mp_id(soap).as_deref(), Some("9900001000001"));
    }

    /// Garbage values must not become limiter keys — an attacker could
    /// otherwise grow the keyed state unboundedly.
    #[test]
    fn rejects_invalid_shapes() {
        let bad = br#"<eb:From><eb:PartyId>DROP TABLE x; --</eb:PartyId></eb:From>"#;
        assert_eq!(extract_sender_mp_id(bad), None);
        assert_eq!(extract_sender_mp_id(b"no xml at all"), None);
        assert_eq!(extract_sender_mp_id(&[0xFF, 0xFE]), None);
    }

    /// A 16-char EIC is accepted.
    #[test]
    fn accepts_eic() {
        let soap = br#"<eb:From><eb:PartyId>10XDE-EON-NETZ-I</eb:PartyId></eb:From>"#;
        assert_eq!(
            extract_sender_mp_id(soap).as_deref(),
            Some("10XDE-EON-NETZ-I")
        );
    }
}
