//! BDEW AS4 outbound delivery for the Mako outbox.
//!
//! [`BdewAs4Sender`] is wired to the `OutboxWorker` in `main.rs` whenever
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
//!             1. resolve P-Mode from BdewAs4Profile (GLN + message type)
//!             2. extract endpoint URL from P-Mode
//!             3. serialize payload to wire bytes
//!             4. asx_rs::as4::send_async  →  As4SendOutput (signed SOAP)
//!             5. As4HttpTransport::send(endpoint_url, &output)
//!             6. OK  →  OutboxStore::acknowledge(msg.message_id)
//!             7. Err →  OutboxStore::reschedule(…, exponential backoff)
//! ```
//!
//! ## P-Mode registry
//!
//! Every trading partner's AS4 endpoint and protocol settings must be
//! registered in the [`BdewAs4Profile`] at startup via
//! `--as4-partner <GLN>=<HTTPS-URL>` (repeatable).  This registers one
//! P-Mode per standard BDEW EDIFACT message type for the partner.  Messages
//! destined for an unknown recipient GLN or with a missing endpoint URL are
//! dead-lettered immediately (permanent `PartnerUnknown` error).
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
//!
//! [`RenderError::InsufficientPayload`]: crate::edifact_renderer::RenderError::InsufficientPayload

use std::sync::Arc;

use asx_rs::as4::As4SendRequest;
use asx_rs::core::SessionContext;
use asx_rs::observability::EventBus;
use asx_rs::transport::{As4HttpTransport, TransportConfig};
use edi_energy::EdiEnergyMessage as _;
use mako_as4::bdew_action_from_str;
use mako_engine::builder::As4Sender;
use mako_engine::error::EngineError;
use mako_engine::metrics::EngineMetrics;
use mako_engine::outbox::OutboxMessage;

use mako_as4::profile::BdewAs4Profile;

use crate::edifact_renderer;
use crate::malo_ident_sender::MaloIdentSender;
use crate::party_registry::MpIdRegistry;

// ── WebhookEdifactSender ──────────────────────────────────────────────────────

/// An [`As4Sender`] that POSTs outbound EDIFACT messages to an HTTP webhook
/// instead of using the BDEW AS4 transport.
///
/// Each message is rendered to EDIFACT wire bytes and delivered as a
/// **[CloudEvents 1.0](https://cloudevents.io) structured-mode JSON** POST:
///
/// ```text
/// POST <webhook_url>
/// Content-Type: application/cloudevents+json
///
/// {
///   "specversion": "1.0",
///   "type": "de.mako.edifact.outbound",
///   "source": "urn:mako:tenant:<tenant_id>",
///   "id": "<message_id>",
///   "subject": "<process_id>",
///   "makomessagetype": "UTILMD",
///   "makorecipient": "<recipient_gln>",
///   "data": {
///     "message_type": "UTILMD",
///     "recipient": "<gln>",
///     "edifact": "UNB+…'UNZ+…'"
///   }
/// }
/// ```
///
/// `MaloIdentCallback` messages are still routed to the embedded
/// [`MaloIdentSender`] (unchanged from the AS4 path).
///
/// Intended for development / ERP integration without an AS4 infrastructure.
/// In production, use [`BdewAs4Sender`].
#[derive(Clone)]
pub struct WebhookEdifactSender {
    webhook_url: Arc<str>,
    mp_id_registry: Arc<MpIdRegistry>,
    http_client: reqwest::Client,
    malo_sender: MaloIdentSender,
}

impl WebhookEdifactSender {
    /// Create a new `WebhookEdifactSender`.
    #[must_use]
    pub fn new(
        webhook_url: impl Into<Arc<str>>,
        mp_id_registry: Arc<MpIdRegistry>,
        http_client: reqwest::Client,
        malo_sender: MaloIdentSender,
    ) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            mp_id_registry,
            http_client,
            malo_sender,
        }
    }
}

impl As4Sender for WebhookEdifactSender {
    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send {
        let webhook_url = Arc::clone(&self.webhook_url);
        let mp_id_registry: Arc<MpIdRegistry> = Arc::clone(&self.mp_id_registry);
        let http_client = self.http_client.clone();
        let malo_sender = self.malo_sender.clone();
        let msg_owned = msg.clone();

        async move {
            // MaloIdentCallback: delegate to the MaLo-ID sender unchanged.
            if msg_owned.message_type.as_ref() == "MaloIdentCallback" {
                return malo_sender.send(&msg_owned).await;
            }

            // Render domain-intent JSON to EDIFACT wire bytes.
            let edifact_str =
                match edifact_renderer::render_to_wire_bytes(&msg_owned, &mp_id_registry) {
                    Ok(rendered) => String::from_utf8_lossy(&rendered.bytes).into_owned(),
                    Err(ref e) if edifact_renderer::is_suppressed(e) => {
                        // Gas positive APERAK: silence = acceptance per APERAK AHB 1.0 §2.3.
                        // No wire EDIFACT sent; deliver domain JSON to ERP webhook so the
                        // operator sees the positive outcome.
                        tracing::debug!(
                            message_id   = %msg_owned.message_id,
                            message_type = %msg_owned.message_type,
                            "WebhookEdifactSender: Gas positive APERAK suppressed — \
                             sending domain JSON (silence = acceptance, APERAK AHB 1.0 §2.3)",
                        );
                        msg_owned.payload.to_string()
                    }
                    Err(ref e) if edifact_renderer::is_insufficient_payload(e) => {
                        // No renderer registered for this message type — include
                        // the raw domain-intent JSON payload so the ERP still sees
                        // something useful, and log a warning.
                        tracing::warn!(
                            message_id   = %msg_owned.message_id,
                            message_type = %msg_owned.message_type,
                            "WebhookEdifactSender: no EDIFACT renderer — \
                             sending raw payload JSON",
                        );
                        msg_owned.payload.to_string()
                    }
                    Err(e) => {
                        return Err(EngineError::Serialization(format!(
                            "EDIFACT render failed for {}: {e}",
                            msg_owned.message_id
                        )));
                    }
                };

            let body = serde_json::json!({
                "specversion":     "1.0",
                "type":            "de.mako.edifact.outbound",
                "source":          format!("urn:mako:tenant:{}", mp_id_registry.primary_mp_id()),
                "id":              msg_owned.message_id.to_string(),
                "subject":         msg_owned.process_id.to_string(),
                "time":            time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                "datacontenttype": "application/json",
                "makoconvid":      msg_owned.conversation_id.to_string(),
                "makomessagetype": msg_owned.message_type.as_ref(),
                "makorecipient":   msg_owned.recipient.as_ref(),
                "data": {
                    "message_type": msg_owned.message_type.as_ref(),
                    "recipient":    msg_owned.recipient.as_ref(),
                    "edifact":      edifact_str,
                },
            });

            let resp = http_client
                .post(webhook_url.as_ref())
                .header("Content-Type", "application/cloudevents+json")
                .header("X-Idempotency-Key", msg_owned.message_id.to_string())
                .json(&body)
                .send()
                .await
                .map_err(|e| EngineError::Transport {
                    endpoint: webhook_url.as_ref().into(),
                    message: e.to_string(),
                })?;

            if resp.status().is_success() {
                tracing::info!(
                    message_id   = %msg_owned.message_id,
                    message_type = %msg_owned.message_type,
                    recipient    = %msg_owned.recipient,
                    url          = %webhook_url,
                    "WebhookEdifactSender: outbound EDIFACT delivered",
                );
                Ok(())
            } else {
                let status = resp.status().as_u16();
                let body_text = resp.text().await.unwrap_or_default();
                Err(EngineError::Transport {
                    endpoint: webhook_url.as_ref().into(),
                    message: format!("HTTP {status}: {body_text}"),
                })
            }
        }
    }
}

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
    profile: Arc<BdewAs4Profile>,
    malo_sender: MaloIdentSender,
    /// The operator's own GLN registry.
    ///
    /// Used by the EDIFACT renderer to resolve the correct sender GLN per role
    /// and by the loopback path to detect own-GLN recipients (combined-role
    /// deployments where NB and MSB, or GNB and gMSB, have different GLNs on
    /// the same instance).
    mp_id_registry: Arc<MpIdRegistry>,
    /// Optional in-process loopback handle for combined-role deployments.
    ///
    /// When `Some`, outbox messages addressed to `tenant_party_id` (own GLN)
    /// are delivered in-process via [`crate::edifact_api::EdifactApiState`]
    /// instead of being dead-lettered.  Required for NB+MSB, GNB+gMSB, and
    /// NB+LF deployments sharing a single GLN.
    loopback: Option<Arc<crate::edifact_api::EdifactApiState>>,
    /// Parse/validation platform for the pre-send AHB conformance gate.
    platform: Arc<edi_energy::Platform>,
    /// When `true`, a missing or mismatched synchronous `eb:Receipt` is only
    /// warned about instead of failing the delivery. BDEW MaKo AS4 requires
    /// the synchronous receipt, so the strict default treats its absence as a
    /// retryable delivery failure; this flag exists for interop debugging
    /// against non-conformant counterparties.
    lenient_receipts: bool,
}

impl BdewAs4Sender {
    /// Construct from the shared AS4 session context, event bus, P-Mode
    /// profile, and MaLo cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `reqwest::Client` cannot be built
    /// (e.g. missing TLS stack).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: Arc<SessionContext>,
        event_bus: Arc<EventBus>,
        profile: Arc<BdewAs4Profile>,
        malo_sender: MaloIdentSender,
        mp_id_registry: Arc<MpIdRegistry>,
        loopback: Option<Arc<crate::edifact_api::EdifactApiState>>,
        platform: Arc<edi_energy::Platform>,
        lenient_receipts: bool,
    ) -> anyhow::Result<Self> {
        let transport = As4HttpTransport::new(TransportConfig::default())
            .map_err(|e| anyhow::anyhow!("AS4 HTTP transport init failed: {e}"))?;
        Ok(Self {
            session,
            event_bus,
            transport: Arc::new(transport),
            profile,
            malo_sender,
            mp_id_registry,
            loopback,
            platform,
            lenient_receipts,
        })
    }
}

// action_uri_for is now bdew_action_from_str() in mako-as4.

impl As4Sender for BdewAs4Sender {
    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send {
        let session = Arc::clone(&self.session);
        let event_bus = Arc::clone(&self.event_bus);
        let transport = Arc::clone(&self.transport);
        let profile = Arc::clone(&self.profile);
        let malo_sender = self.malo_sender.clone();
        let loopback = self.loopback.clone();

        // Clone all needed fields out of the reference so the returned future
        // is `'static` (required by the `As4Sender` bound).
        let msg_owned = msg.clone();
        let message_type = msg.message_type.clone();
        let recipient = msg.recipient.clone();
        let message_id_str = msg.message_id.to_string();
        let conversation_id = msg.conversation_id.to_string();
        let mp_id_registry: Arc<MpIdRegistry> = Arc::clone(&self.mp_id_registry);
        let platform = Arc::clone(&self.platform);
        let lenient_receipts = self.lenient_receipts;
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
            // The registry is_own_mp_id check covers ALL own GLNs, so loopback works
            // even when NB and MSB have DIFFERENT GLNs on the same makod instance.
            if mp_id_registry.is_own_mp_id(recipient.as_ref()) {
                if let Some(ref loopback_state) = loopback {
                    // ── In-process loopback delivery ──────────────────────────
                    let payload_bytes = match edifact_renderer::render_to_wire_bytes(
                        &msg_owned,
                        &mp_id_registry,
                    ) {
                        Ok(rendered) => rendered.bytes,
                        Err(ref e) if edifact_renderer::is_insufficient_payload(e) => {
                            tracing::error!(
                                message_id   = %message_id_str,
                                message_type = %message_type,
                                own_mp_ids     = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
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
                                            own_mp_ids     = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
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
                                    own_mp_ids     = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
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
                            own_mp_ids     = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
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
                    own_mp_ids     = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
                    "BdewAs4Sender: outbox message addressed to own GLN \
                     (combined-role deployment — NB+MSB or GNB+gMSB sharing one GLN). \
                     No loopback handle configured. \
                     See docs/makod.md §Integrated operators for details.",
                );
                return Err(EngineError::PartnerUnknown { recipient });
            }

            // Resolve the P-Mode for the recipient GLN + message type.
            // P-Modes are registered at startup via
            // BdewAs4Profile::register_partner_all_actions.
            // Use PartnerUnknown (permanent error) so the outbox worker
            // dead-letters immediately rather than retrying indefinitely.
            let bdew_action = bdew_action_from_str(&message_type);
            let Some(pm) = profile.resolve_pmode_by_action(&recipient, &bdew_action) else {
                tracing::warn!(
                    message_id   = %message_id_str,
                    message_type = %message_type,
                    recipient    = %recipient,
                    "BdewAs4Sender: no P-Mode registered for this recipient GLN. \
                     Add --as4-partner {}=<URL> to register it.",
                    recipient,
                );
                return Err(EngineError::PartnerUnknown { recipient });
            };
            let Some(endpoint_ref) = pm.endpoint_url.as_deref() else {
                tracing::warn!(
                    message_id   = %message_id_str,
                    message_type = %message_type,
                    recipient    = %recipient,
                    "BdewAs4Sender: P-Mode has no endpoint_url. \
                     Re-register with --as4-partner {}=<URL>.",
                    recipient,
                );
                return Err(EngineError::PartnerUnknown { recipient });
            };
            let endpoint = endpoint_ref.to_owned();

            // Record the delivery attempt before rendering/sending.

            // Render domain-intent JSON to BDEW-conformant EDIFACT wire bytes.
            //
            // `InsufficientPayload` means no wire-format renderer exists for this
            // message type. Return a permanent `RendererNotImplemented` error so
            // the outbox worker dead-letters the entry immediately instead of
            // retrying (retries would never succeed) or transmitting a non-EDIFACT
            // JSON blob over AS4 (which violates BDEW MaKo interoperability).
            let rendered = match edifact_renderer::render_to_wire_bytes(&msg_owned, &mp_id_registry)
            {
                Ok(rendered) => {
                    tracing::debug!(
                        message_id   = %message_id_str,
                        message_type = %message_type,
                        bytes        = rendered.bytes.len(),
                        "BdewAs4Sender: rendered EDIFACT Übertragungsdatei",
                    );
                    rendered
                }
                Err(ref e) if edifact_renderer::is_suppressed(e) => {
                    // Gas positive APERAK: silence = acceptance per APERAK AHB 1.0 §2.3.
                    // No AS4 message is sent; the outbox entry is acknowledged immediately.
                    tracing::debug!(
                        message_id   = %message_id_str,
                        message_type = %message_type,
                        recipient    = %recipient,
                        "BdewAs4Sender: Gas positive APERAK suppressed — \
                         no wire EDIFACT sent (silence = acceptance, APERAK AHB 1.0 §2.3)",
                    );
                    return Ok(());
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

            if rendered.bytes.is_empty() {
                return Err(EngineError::Serialization(format!(
                    "rendered payload is empty for message_id={message_id_str}"
                )));
            }

            // ── Pre-send AHB conformance gate ─────────────────────────────
            // A message that fails to parse or validate against its own
            // release profile must not reach the regulated network: parse the
            // rendered Übertragungsdatei back and run the edi-energy profile
            // validation. Failures are permanent (Serialization → dead-letter)
            // — retrying an invalid rendering can never succeed.
            match platform.parse(&rendered.bytes) {
                Err(e) => {
                    return Err(EngineError::Serialization(format!(
                        "pre-send gate: rendered EDIFACT does not parse for \
                         {message_id_str} ({message_type}): {e}"
                    )));
                }
                Ok(parsed) => match parsed.validate() {
                    Err(e) => {
                        return Err(EngineError::Serialization(format!(
                            "pre-send gate: AHB validation failed to run for \
                             {message_id_str} ({message_type}): {e}"
                        )));
                    }
                    Ok(report) if !report.is_valid() => {
                        return Err(EngineError::Serialization(format!(
                            "pre-send gate: rendered EDIFACT violates its AHB \
                             profile for {message_id_str} ({message_type}): {:?}",
                            report.errors()
                        )));
                    }
                    Ok(_) => {}
                },
            }

            let mut policy = pm.to_send_policy().map_err(|e| EngineError::Transport {
                endpoint: endpoint.clone().into(),
                message: format!("P-Mode policy materialisation failed for {message_id_str}: {e}"),
            })?;
            policy.conversation_id = Some(conversation_id);
            let action = bdew_action.as_uri();

            // Build the §AF §2.12 Content-Disposition filename:
            // Nachrichtentyp_Anwendungsreferenz_von_an_yyyymmdd_DAR.txt
            // (Allgemeine Festlegungen V6.1d §2.12)
            //
            // - Nachrichtentyp: EDIFACT message type from UNH DE0065
            // - Anwendungsreferenz: VL/TL/EM from UNB DE0026; empty for most types
            //   (UTILMD, APERAK, CONTRL, etc.) — empty → double underscore
            // - von/an: the interchange's UNB DE0004/DE0010 MP-IDs — the same
            //   values the renderer put in the envelope and in NAD+MS/NAD+MR
            // - yyyymmdd: creation date **in UTC** (not German local time)
            // - DAR: the interchange's UNB DE0020 Datenaustauschreferenz
            let payload_filename = {
                let now = time::OffsetDateTime::now_utc();
                let yyyymmdd =
                    format!("{:04}{:02}{:02}", now.year(), now.month() as u8, now.day(),);
                // Anwendungsreferenz is empty for all types except MSCONS
                // (TL = Lastgang, VL = Zählerstand/Energiemenge).
                // Per §2.12 example: empty → rendered as double underscore.
                let anwendungsref = anwendungsreferenz_for(&message_type);
                let name = format!(
                    "{}_{}_{}_{}_{}_{}.txt",
                    message_type,
                    anwendungsref,
                    rendered.sender_mp_id,
                    rendered.receiver_mp_id,
                    yyyymmdd,
                    rendered.dar,
                );
                // PayloadFilename validates printable-ASCII + length invariants.
                // All components are ASCII digits/uppercase letters/underscores.
                asx_rs::as4::PayloadFilename::new(&name).ok()
            };

            let request = As4SendRequest {
                message_id: message_id_str.clone(),
                payload: rendered.bytes,
                policy,
                // Populate the recipient's encryption certificate.
                // BDEW AS4-Profil v1.2 §2.2.6.2.2: every outbound message must be encrypted
                // with the recipient's EC (BrainpoolP256r1) certificate via ECDH-ES.
                // asx-rs v0.7 implements ECDH-ES + ConcatKDF + AES-128-KW when an EC cert
                // is supplied — no RSA fallback path on the BDEW profile.
                credentials: profile.get_partner_encryption_cert(recipient.as_ref()).map(
                    |cert_pem| asx_rs::as4::As4SendCredentials {
                        recipient_cert_pem: Some(std::sync::Arc::from(cert_pem)),
                        signing_cert_pem: None,
                        signing_key_pem: None,
                    },
                ),
                payload_filename,
            };

            // Build the signed SOAP envelope (CPU-bound; runs on Tokio blocking pool).
            let mut output = asx_rs::as4::send_async(&session, &event_bus, request)
                .await
                .map_err(|e| {
                    EngineMetrics::global().outbox_delivery_attempted("transport_error");
                    EngineError::Transport {
                        endpoint: endpoint.as_str().into(),
                        message: format!("AS4 envelope build failed for {message_id_str}: {e}"),
                    }
                })?;

            // W3C trace continuity to the counterparty: when the outbox
            // message carries the trace context of the request that caused
            // it, forward THAT `traceparent` on the AS4 HTTP egress instead
            // of the fresh one asx-rs generates — the receiving MSH then
            // joins the same distributed trace as the original caller.
            if let Some(tp) = msg_owned
                .trace_context
                .as_deref()
                .and_then(asx_rs::transport::trace_context::normalize_traceparent)
            {
                output.traceparent = Some(tp);
            }

            // HTTP POST to the recipient's AS4 endpoint.
            let outcome = transport.send(&endpoint, &output).await.map_err(|e| {
                EngineMetrics::global().outbox_delivery_attempted("transport_error");
                EngineError::Transport {
                    endpoint: endpoint.as_str().into(),
                    message: format!("HTTP POST failed for {message_id_str}: {e}"),
                }
            })?;

            // Inspect the synchronous AS4 receipt in the response body.
            // Per BDEW MaKo AS4-Profil 2.0 §4.6.3, the receiver must return a
            // synchronous `eb:Receipt` SignalMessage.  Missing or mismatched
            // receipts are non-fatal (we already got HTTP 200) but warrant a
            // structured warning so operators can diagnose counterparty conformance
            // issues without losing the delivery confirmation.
            if let Err(reason) =
                verify_sync_receipt(&outcome.body, &message_id_str, &recipient, &message_type)
            {
                if lenient_receipts {
                    tracing::warn!(
                        message_id   = %message_id_str,
                        recipient    = %recipient,
                        message_type = %message_type,
                        reason       = %reason,
                        "BdewAs4Sender: --as4-lenient-receipts — synchronous \
                         eb:Receipt verification failed; delivery acknowledged anyway",
                    );
                } else {
                    // Strict default: a delivery without a verifiable synchronous
                    // receipt is not a confirmed delivery (BDEW MaKo AS4 MEP).
                    // Transport errors are retryable — the outbox worker backs
                    // off and dead-letters after the retry budget.
                    EngineMetrics::global().outbox_delivery_attempted("receipt_unverified");
                    return Err(EngineError::Transport {
                        endpoint: endpoint.as_str().into(),
                        message: format!(
                            "synchronous eb:Receipt verification failed for \
                             {message_id_str}: {reason}"
                        ),
                    });
                }
            }

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

// ── Synchronous receipt inspection ───────────────────────────────────────────

/// Verify the synchronous `eb:Receipt` in the counterparty's response body.
///
/// The BDEW MaKo AS4 MEP requires a synchronous `eb:Receipt` SignalMessage on
/// the same HTTP connection, referencing the sent `message_id`. This returns
/// `Err(reason)` when:
///
/// - the response body is empty or not UTF-8,
/// - no `eb:Receipt` element is present,
/// - `eb:RefToMessageId` is absent or references a different message.
///
/// A missing `eb:NonRepudiationInformation` is warned about but does not fail
/// verification — the receipt still confirms receipt of the correct message.
///
/// The caller decides whether a failure is fatal (strict default: retryable
/// delivery failure) or advisory (`--as4-lenient-receipts`).
fn verify_sync_receipt(
    body: &[u8],
    message_id: &str,
    recipient: &str,
    message_type: &str,
) -> Result<(), String> {
    if body.is_empty() {
        return Err(
            "counterparty returned an empty response body — no synchronous \
             eb:Receipt received (BDEW MaKo AS4-Profil 2.0 §4.6.3)"
                .to_owned(),
        );
    }

    let body_str = std::str::from_utf8(body)
        .map_err(|_| "response body is not valid UTF-8 — cannot inspect eb:Receipt".to_owned())?;

    if !body_str.contains("<eb:Receipt") && !body_str.contains("<eb3:Receipt") {
        return Err(
            "counterparty response does not contain eb:Receipt — synchronous \
             receipt is absent (BDEW MaKo AS4-Profil 2.0 §4.6.3)"
                .to_owned(),
        );
    }

    let ref_id_opt = extract_element_text(body_str, "eb:RefToMessageId")
        .or_else(|| extract_element_text(body_str, "eb3:RefToMessageId"));
    match ref_id_opt {
        Some(ref_id) if ref_id != message_id => {
            return Err(format!(
                "eb:Receipt.RefToMessageId {ref_id:?} does not reference the \
                 sent message_id {message_id:?}"
            ));
        }
        None => {
            return Err(
                "eb:Receipt present but eb:RefToMessageId is absent — cannot \
                 verify the receipt references the correct message"
                    .to_owned(),
            );
        }
        Some(_) => {}
    }

    // BDEW MaKo AS4-Profil 2.0 §4.6.3 requires NonRepudiationInformation for
    // signed messages. Advisory: the receipt already references the correct
    // message, so absence is a counterparty conformance warning, not a
    // delivery failure.
    if !body_str.contains("<eb:NonRepudiationInformation")
        && !body_str.contains("<eb3:NonRepudiationInformation")
    {
        tracing::warn!(
            message_id   = %message_id,
            recipient    = %recipient,
            message_type = %message_type,
            "BdewAs4Sender: eb:Receipt is present but eb:NonRepudiationInformation \
             is absent (BDEW MaKo AS4-Profil 2.0 §4.6.3) — counterparty NRI gap",
        );
    }
    Ok(())
}

/// Extract the text content of the first occurrence of `<tag>…</tag>` in `xml`.
///
/// Returns `None` when the opening tag is absent.  Does not handle CDATA
/// sections, namespaced tags with different prefixes, or nested identical tags.
/// Sufficient for the BDEW AS4 receipt `eb:RefToMessageId` check which always
/// appears exactly once in a well-formed receipt.
fn extract_element_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(open.as_str())? + open.len();
    let end = xml[start..].find(close.as_str()).map(|i| start + i)?;
    Some(xml[start..end].trim())
}

// ── §AF §2.12 helpers ─────────────────────────────────────────────────────────

/// Returns the `Anwendungsreferenz` (UNB DE0026) token for a given EDIFACT
/// message type, as defined by the BDEW Nachrichtenbeschreibungen.
///
/// For most types (UTILMD, APERAK, CONTRL, etc.) the field is empty — the
/// §AF §2.12 filename then renders the second position as an empty string,
/// producing a double underscore (e.g. `UTILMD__9900…`).
///
/// For MSCONS the value depends on the message content and is not knowable
/// from the type alone, so we leave it empty here too.  The AS4 metadata
/// (`BDEWApplicationReference` PartProperty) carries the same information and
/// is also absent because our builders produce bare UNH…UNT without UNB.
fn anwendungsreferenz_for(_message_type: &str) -> &'static str {
    // Per AF §2.12 and BDEW Nachrichtenbeschreibungen:
    // VL / TL / EM are only used for MSCONS, and only when the sender
    // explicitly sets UNB DE0026 — which our bare-UNH builders never do.
    ""
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_element_text ───────────────────────────────────────────────────

    #[test]
    fn extract_element_text_returns_content() {
        let xml = "<root><eb:RefToMessageId>msg-123</eb:RefToMessageId></root>";
        assert_eq!(
            extract_element_text(xml, "eb:RefToMessageId"),
            Some("msg-123")
        );
    }

    #[test]
    fn extract_element_text_trims_whitespace() {
        let xml = "<eb:RefToMessageId>  msg-456  </eb:RefToMessageId>";
        assert_eq!(
            extract_element_text(xml, "eb:RefToMessageId"),
            Some("msg-456")
        );
    }

    #[test]
    fn extract_element_text_absent_returns_none() {
        let xml = "<root><other>value</other></root>";
        assert_eq!(extract_element_text(xml, "eb:RefToMessageId"), None);
    }

    #[test]
    fn extract_element_text_empty_xml_returns_none() {
        assert_eq!(extract_element_text("", "eb:RefToMessageId"), None);
    }

    // ── verify_sync_receipt ────────────────────────────────────────────────────
    //
    // verify_sync_receipt returns Err for missing/mismatched receipts and
    // must never panic for edge-case inputs.

    #[test]
    fn verify_sync_receipt_rejects_empty_body() {
        let err = verify_sync_receipt(b"", "msg-001", "9903462000005", "APERAK").unwrap_err();
        assert!(err.contains("empty response body"), "{err}");
    }

    #[test]
    fn verify_sync_receipt_rejects_non_utf8() {
        let err =
            verify_sync_receipt(&[0xFF, 0xFE], "msg-001", "9903462000005", "APERAK").unwrap_err();
        assert!(err.contains("not valid UTF-8"), "{err}");
    }

    #[test]
    fn verify_sync_receipt_accepts_matching_receipt_and_rejects_mismatch() {
        let ok = b"<eb:Receipt><eb:RefToMessageId>msg-001</eb:RefToMessageId>\
                   <eb:NonRepudiationInformation/></eb:Receipt>";
        assert!(verify_sync_receipt(ok, "msg-001", "99", "APERAK").is_ok());
        let err = verify_sync_receipt(ok, "msg-OTHER", "99", "APERAK").unwrap_err();
        assert!(err.contains("does not reference"), "{err}");
    }

    #[test]
    fn verify_sync_receipt_rejects_missing_receipt_element() {
        let body = b"<soap:Envelope><soap:Body></soap:Body></soap:Envelope>";
        let err = verify_sync_receipt(body, "msg-001", "9903462000005", "APERAK").unwrap_err();
        assert!(err.contains("does not contain eb:Receipt"), "{err}");
    }

    #[test]
    fn verify_sync_receipt_accepts_matching_receipt_in_envelope() {
        let body = r#"<soap:Envelope>
  <soap:Body>
    <eb:Receipt>
      <eb:RefToMessageId>msg-001</eb:RefToMessageId>
      <eb:NonRepudiationInformation/>
    </eb:Receipt>
  </soap:Body>
</soap:Envelope>"#;
        assert!(verify_sync_receipt(body.as_bytes(), "msg-001", "9903462000005", "APERAK").is_ok());
    }

    #[test]
    fn verify_sync_receipt_rejects_mismatched_ref_id() {
        let body = r#"<eb:Receipt>
  <eb:RefToMessageId>different-msg</eb:RefToMessageId>
  <eb:NonRepudiationInformation/>
</eb:Receipt>"#;
        let err =
            verify_sync_receipt(body.as_bytes(), "msg-001", "9903462000005", "APERAK").unwrap_err();
        assert!(err.contains("does not reference"), "{err}");
    }

    #[test]
    fn verify_sync_receipt_missing_nri_is_advisory_only() {
        let body = r#"<eb:Receipt>
  <eb:RefToMessageId>msg-001</eb:RefToMessageId>
</eb:Receipt>"#;
        assert!(verify_sync_receipt(body.as_bytes(), "msg-001", "9903462000005", "APERAK").is_ok());
    }
}
