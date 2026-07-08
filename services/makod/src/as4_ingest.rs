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
//! 4. **Synchronous receipt** — a signed `eb:Receipt` SignalMessage is returned
//!    on the same HTTP connection, satisfying the BDEW AS4 MEP requirement.
//!
//! # Security notes
//!
//! - Signing is **mandatory** for BDEW production deployments
//!   (`As4PushPolicy::regulated()`).
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
    As4PushPolicy, As4ReceiveOutcome, As4ReceivePushRequest, generate_receipt_for_output,
    receive_push_with_dedup_async,
};
use asx_rs::core::{AsxError, ErrorCode, ErrorContext, SessionContext};
use asx_rs::observability::EventBus;
use asx_rs::storage::{BoxFuture, DedupStorage};
use asx_rs::transport::ingress::As4HttpIngress;
use asx_rs::transport::server::{As4AxumHandler, HandlerOutcome, as4_router};
use axum::Router;
use edi_energy::EdiEnergyMessage;
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
        }
    }
}

impl As4AxumHandler for BdewAs4IngestHandler {
    async fn handle(&self, ingress: As4HttpIngress) -> HandlerOutcome {
        let request = As4ReceivePushRequest {
            http_content_type: ingress.content_type.clone(),
            payload: Arc::clone(&ingress.body),
            receipt_payload: None,
            policy: As4PushPolicy::regulated(),
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
                let mut accepted = 0usize;
                let mut rejected = 0usize;
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
                            tracing::warn!(
                                as4_message_id = %msg_id,
                                error          = %e,
                                "AS4 ingest: EDIFACT parse error",
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
                        }
                    }
                }

                if accepted == 0 && rejected > 0 {
                    return HandlerOutcome::bad_request(
                        "AS4 payload contained no valid EDIFACT messages",
                    );
                }

                // ── Synchronous receipt ───────────────────────────────────────
                let receipt_id = format!("makod@{}", Uuid::new_v4());
                match generate_receipt_for_output(&self.session, &receipt_id, &output) {
                    Ok(receipt_xml) => {
                        tracing::debug!(
                            as4_message_id = %msg_id,
                            receipt_id     = %receipt_id,
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
