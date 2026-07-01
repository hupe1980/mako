//! BDEW AS4 outbound delivery for the Mako outbox.
//!
//! [`BdewAs4Sender`] is wired to the [`OutboxWorker`] in `main.rs` whenever
//! a signing key/cert pair is available (i.e. `--as4-signing-key-pem` and
//! `--as4-signing-cert-pem` are set).  It replaces the stub
//! [`MaloIdentSender`]-only path that previously discarded all EDIFACT
//! messages.
//!
//! ## Design
//!
//! ```text
//! OutboxWorker::run()
//!   → BdewAs4Sender::send(OutboxMessage)
//!       ├── message_type == "MaloIdentCallback"
//!       │     → MaloIdentSender (existing cache-lookup path, unchanged)
//!       └── EDIFACT type (APERAK, CONTRL, UTILMD, …)
//!             1. look up recipient GLN in PartnerDirectory
//!             2. map message_type → BdewAction URI
//!             3. serialize payload to wire bytes
//!             4. asx_rs::as4::send_async  →  As4SendOutput (signed SOAP)
//!             5. As4HttpTransport::send(endpoint_url, &output)
//!             6. OK  →  OutboxStore::acknowledge(msg.message_id)
//!             7. Err →  OutboxStore::reschedule(…, exponential backoff)
//! ```
//!
//! ## Partner directory
//!
//! Every trading partner's AS4 endpoint must be registered at startup via
//! `--as4-partner <GLN>=<HTTPS-URL>` (repeatable).  Messages destined for
//! an unknown recipient GLN are rescheduled after a structured `WARN` log.
//!
//! ## Payload encoding
//!
//! `OutboxMessage.payload` is a domain-intent JSON value produced by workflow
//! `handle` implementations. This layer calls [`edifact_renderer::render_to_wire_bytes`]
//! to convert the JSON to BDEW-conformant EDIFACT wire bytes before handing them
//! to the AS4 transport.
//!
//! For message types whose payload carries only domain intent without the actual
//! business data required for a conformant wire message (e.g. MSCONS without
//! meter readings), the renderer returns [`RenderError::InsufficientPayload`] and
//! the sender falls back to transmitting the JSON blob. A structured `warn!` is
//! emitted for every such fallback so the operator knows about non-conformant
//! transmissions.

use std::sync::Arc;

use asx_rs::as4::{As4SendPolicy, As4SendRequest};
use asx_rs::core::SessionContext;
use asx_rs::observability::EventBus;
use asx_rs::transport::{As4HttpTransport, TransportConfig};
use edi_energy::EdiEnergyMessage as _;
use mako_as4::{BdewAction, constants};
use mako_engine::builder::As4Sender;
use mako_engine::error::EngineError;
use mako_engine::metrics::EngineMetrics;
use mako_engine::outbox::OutboxMessage;

use mako_as4::PartnerDirectory;

use crate::edifact_renderer;
use crate::malo_ident_sender::MaloIdentSender;

// ── BdewAs4Sender ─────────────────────────────────────────────────────────────

/// AS4 outbound sender that delivers EDIFACT messages via the BDEW MaKo
/// transport profile.
///
/// Implements [`As4Sender`] — pass to
/// [`EngineContext::run_outbox_worker`](mako_engine::builder::EngineContext::run_outbox_worker)
/// at startup.
///
/// `MaloIdentCallback` messages are routed to the embedded
/// [`MaloIdentSender`]; all other message types are rendered to EDIFACT wire
/// bytes via [`edifact_renderer::render_to_wire_bytes`] before AS4 dispatch.
/// Message types with no implemented renderer return
/// [`EngineError::RendererNotImplemented`] (a permanent error) so the outbox
/// worker dead-letters them immediately rather than transmitting non-EDIFACT
/// blobs over AS4.
#[derive(Clone)]
pub struct BdewAs4Sender {
    session: Arc<SessionContext>,
    event_bus: Arc<EventBus>,
    transport: Arc<As4HttpTransport>,
    partners: Arc<PartnerDirectory>,
    malo_sender: MaloIdentSender,
    /// The operator's own GLN (from `--tenant-id`).
    ///
    /// Used by the EDIFACT renderer as the sender for ORDERS and similar
    /// messages where the payload does not carry an explicit sender party ID.
    tenant_party_id: Box<str>,
    /// Optional in-process loopback handle for combined-role deployments.
    ///
    /// When `Some`, outbox messages addressed to `tenant_party_id` (own GLN)
    /// are delivered in-process via [`crate::edifact_api::EdifactApiState`]
    /// instead of being dead-lettered.  Required for NB+MSB, GNB+gMSB, and
    /// NB+LF deployments sharing a single GLN.
    loopback: Option<Arc<crate::edifact_api::EdifactApiState>>,
}

impl BdewAs4Sender {
    /// Construct from the shared AS4 session context, event bus, partner
    /// directory, and MaLo cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `reqwest::Client` cannot be built
    /// (e.g. missing TLS stack).
    pub fn new(
        session: Arc<SessionContext>,
        event_bus: Arc<EventBus>,
        partners: Arc<PartnerDirectory>,
        malo_sender: MaloIdentSender,
        tenant_party_id: impl Into<Box<str>>,
        loopback: Option<Arc<crate::edifact_api::EdifactApiState>>,
    ) -> anyhow::Result<Self> {
        let transport = As4HttpTransport::new(TransportConfig::default())
            .map_err(|e| anyhow::anyhow!("AS4 HTTP transport init failed: {e}"))?;
        Ok(Self {
            session,
            event_bus,
            transport: Arc::new(transport),
            partners,
            malo_sender,
            tenant_party_id: tenant_party_id.into(),
            loopback,
        })
    }
}

// action_uri_for is now BdewAction::from_message_type_str in mako-as4.

impl As4Sender for BdewAs4Sender {
    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send {
        let session = Arc::clone(&self.session);
        let event_bus = Arc::clone(&self.event_bus);
        let transport = Arc::clone(&self.transport);
        let partners = Arc::clone(&self.partners);
        let malo_sender = self.malo_sender.clone();
        let loopback = self.loopback.clone();

        // Clone all needed fields out of the reference so the returned future
        // is `'static` (required by the `As4Sender` bound).
        let msg_owned = msg.clone();
        let message_type = msg.message_type.clone();
        let recipient = msg.recipient.clone();
        let message_id_str = msg.message_id.to_string();
        let conversation_id = msg.conversation_id.to_string();
        let tenant_party_id = self.tenant_party_id.clone();
        async move {
            // Route MaloIdentCallback to the existing cache-lookup path.
            if message_type.as_ref() == "MaloIdentCallback" {
                return malo_sender.send(&msg_owned).await;
            }

            // Detect self-addressed messages (combined-role deployments).
            //
            // In integrated deployments (e.g. Stadtwerke operating as both NB and
            // MSB, or GNB and gMSB under the same GLN), the NB-side workflow emits
            // outbox messages addressed to the same GLN as the operator itself:
            //
            //   • ORDERS 17116 (Anfrage Sperrung, NB → MSB / GNB → gMSB)
            //   • ORDERS 17134/17135 (Konfiguration, NB → MSB)
            //   • ORDERS 17001/17009 (Geräteübernahme, MSBN → MSBA)
            //
            // When a loopback handle is wired (combined-role deployment with
            // EdifactApiState carrying a dispatcher), the message is rendered to
            // EDIFACT wire bytes, re-parsed, and dispatched in-process — zero AS4
            // round-trip, zero network latency.
            //
            // When no loopback handle is wired (single-role deployment), the
            // message is dead-lettered with an actionable error log.
            if recipient.as_ref() == tenant_party_id.as_ref() {
                if let Some(ref loopback_state) = loopback {
                    // ── In-process loopback delivery ──────────────────────────
                    let payload_bytes = match edifact_renderer::render_to_wire_bytes(
                        &msg_owned,
                        &tenant_party_id,
                    ) {
                        Ok(bytes) => bytes,
                        Err(ref e) if edifact_renderer::is_insufficient_payload(e) => {
                            tracing::error!(
                                message_id   = %message_id_str,
                                message_type = %message_type,
                                own_gln      = %tenant_party_id,
                                "BdewAs4Sender loopback: no renderer registered — dead-lettering",
                            );
                            return Err(EngineError::RendererNotImplemented {
                                message_type: message_type.as_ref().into(),
                                message_id: message_id_str.as_str().into(),
                            });
                        }
                        Err(e) => {
                            return Err(EngineError::Serialization(format!(
                                "loopback render failed for {message_id_str}: {e}"
                            )));
                        }
                    };

                    let mut any_dispatched = false;
                    for parse_result in loopback_state
                        .platform
                        .parse_interchange(std::io::Cursor::new(&payload_bytes[..]))
                    {
                        let Ok(parsed_msg) = parse_result else {
                            continue;
                        };
                        let pid_opt = parsed_msg
                            .detect_pruefidentifikator()
                            .ok()
                            .map(|p| p.as_u32());
                        let workflow_opt = pid_opt.and_then(|p| loopback_state.pid_router.route(p));

                        match (pid_opt, workflow_opt, loopback_state.dispatcher.as_deref()) {
                            (Some(pid_val), Some(wf_name), Some(dispatcher)) => {
                                match dispatcher.dispatch(&parsed_msg, wf_name, pid_val).await {
                                    Ok(outcome) => {
                                        tracing::info!(
                                            message_id   = %message_id_str,
                                            message_type = %message_type,
                                            workflow     = %wf_name,
                                            outcome      = ?outcome,
                                            own_gln      = %tenant_party_id,
                                            "BdewAs4Sender loopback: in-process delivery succeeded",
                                        );
                                        any_dispatched = true;
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            message_id = %message_id_str,
                                            workflow   = %wf_name,
                                            error      = %e,
                                            "BdewAs4Sender loopback: dispatch failed",
                                        );
                                        return Err(e);
                                    }
                                }
                            }
                            (Some(_), None, _) => {
                                // PID not in dispatch table — e.g. ORDERS 17116 (Anfrage Sperrung,
                                // NB → MSB) when no MSB-side workflow is registered.
                                // Acknowledge the outbox entry (return Ok) so the outbox worker
                                // does not retry indefinitely.  The MSB confirmation must be
                                // provided via the ERP command API.
                                tracing::warn!(
                                    message_id   = %message_id_str,
                                    message_type = %message_type,
                                    pid          = ?pid_opt,
                                    own_gln      = %tenant_party_id,
                                    "BdewAs4Sender loopback: PID not in dispatch table — \
                                     no MSB-side workflow registered; use ERP command API \
                                     (e.g. gpke.sperrung.bestaetigen) to confirm",
                                );
                                return Ok(());
                            }
                            _ => {}
                        }
                    }

                    if !any_dispatched {
                        tracing::warn!(
                            message_id   = %message_id_str,
                            message_type = %message_type,
                            own_gln      = %tenant_party_id,
                            "BdewAs4Sender loopback: rendered message produced no \
                             dispatchable messages (no dispatcher wired or parse yielded nothing)",
                        );
                    }
                    return Ok(());
                }

                // ── No loopback configured — dead-letter ──────────────────────
                tracing::error!(
                    message_id   = %message_id_str,
                    message_type = %message_type,
                    own_gln      = %tenant_party_id,
                    "BdewAs4Sender: outbox message addressed to own GLN \
                     (combined-role deployment — NB+MSB or GNB+gMSB sharing one GLN). \
                     No loopback handle configured. \
                     See docs/makod.md §Integrated operators for details.",
                );
                return Err(EngineError::PartnerUnknown { recipient });
            }

            // Look up the recipient's AS4 endpoint URL.
            // Use PartnerUnknown (permanent error) so the outbox worker
            // dead-letters immediately rather than retrying indefinitely.
            let endpoint = match partners.endpoint(&recipient) {
                Some(url) => url.to_owned(),
                None => {
                    tracing::warn!(
                        message_id   = %message_id_str,
                        message_type = %message_type,
                        recipient    = %recipient,
                        "BdewAs4Sender: no AS4 endpoint configured for this recipient GLN. \
                         Add --as4-partner {}=<URL> to register it.",
                        recipient,
                    );
                    return Err(EngineError::PartnerUnknown { recipient });
                }
            };

            // Record the delivery attempt before rendering/sending.

            // Render domain-intent JSON to BDEW-conformant EDIFACT wire bytes.
            //
            // `InsufficientPayload` means no wire-format renderer exists for this
            // message type. Return a permanent `RendererNotImplemented` error so
            // the outbox worker dead-letters the entry immediately instead of
            // retrying (retries would never succeed) or transmitting a non-EDIFACT
            // JSON blob over AS4 (which violates BDEW MaKo interoperability).
            let payload_bytes =
                match edifact_renderer::render_to_wire_bytes(&msg_owned, &tenant_party_id) {
                    Ok(bytes) => {
                        tracing::debug!(
                            message_id   = %message_id_str,
                            message_type = %message_type,
                            bytes        = bytes.len(),
                            "BdewAs4Sender: rendered EDIFACT wire bytes",
                        );
                        bytes
                    }
                    Err(ref e) if edifact_renderer::is_insufficient_payload(e) => {
                        tracing::error!(
                            message_id   = %message_id_str,
                            message_type = %message_type,
                            recipient    = %recipient,
                            detail       = %e,
                            "BdewAs4Sender: no EDIFACT renderer for this message type — \
                             dead-lettering outbox entry. Implement a wire-format renderer \
                             for '{}' before enabling this workflow path in production.",
                            message_type,
                        );
                        return Err(EngineError::RendererNotImplemented {
                            message_type: message_type.as_ref().into(),
                            message_id: message_id_str.as_str().into(),
                        });
                    }
                    Err(e) => {
                        return Err(EngineError::Serialization(format!(
                            "EDIFACT render failed for {message_id_str} ({message_type}): {e}"
                        )));
                    }
                };

            if payload_bytes.is_empty() {
                return Err(EngineError::Serialization(format!(
                    "rendered payload is empty for message_id={message_id_str}"
                )));
            }

            let action = BdewAction::from_message_type_str(&message_type).as_uri();

            let policy = As4SendPolicy {
                action: action.clone(),
                service: constants::SERVICE.to_owned(),
                service_type: constants::SERVICE_TYPE.to_owned(),
                conversation_id: Some(conversation_id),
                ..As4SendPolicy::regulated()
            };

            let request = As4SendRequest {
                message_id: message_id_str.clone(),
                payload: payload_bytes,
                policy,
                credentials: None,
            };

            // Build the signed SOAP envelope (CPU-bound; runs on Tokio blocking pool).
            let output = asx_rs::as4::send_async(&session, &event_bus, request)
                .await
                .map_err(|e| {
                    EngineMetrics::global().outbox_delivery_attempted("transport_error");
                    EngineError::Transport {
                        endpoint: endpoint.as_str().into(),
                        message: format!("AS4 envelope build failed for {message_id_str}: {e}"),
                    }
                })?;

            // HTTP POST to the recipient's AS4 endpoint.
            transport.send(&endpoint, &output).await.map_err(|e| {
                EngineMetrics::global().outbox_delivery_attempted("transport_error");
                EngineError::Transport {
                    endpoint: endpoint.as_str().into(),
                    message: format!("HTTP POST failed for {message_id_str}: {e}"),
                }
            })?;

            tracing::info!(
                message_id   = %message_id_str,
                message_type = %message_type,
                recipient    = %recipient,
                action       = %action,
                endpoint     = %endpoint,
                "BdewAs4Sender: AS4 message delivered",
            );

            EngineMetrics::global().outbox_delivery_attempted("ok");
            Ok(())
        }
    }
}
