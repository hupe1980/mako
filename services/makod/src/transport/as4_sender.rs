//! BDEW AS4 outbound delivery for the Mako outbox.
//!
//! [`BdewAs4Sender`] is wired to the `OutboxWorker` whenever AS4 is
//! configured (every `[[party]]` carries `[party.as4]`). Without it, outbound
//! EDIFACT goes to the development webhook or, under `allow_no_signing`, is not
//! delivered.
//!
//! ## Design
//!
//! ```text
//! OutboxWorker::run()            stamps first_attempt_at durably, once
//!   → BdewAs4Sender::send(OutboxMessage)
//!       ├── MaloIdentCallback / NetzzugangAntrag → their own senders
//!       ├── recipient is an own MP-ID           → in-process loopback
//!       └── EDIFACT to a partner
//!             1. look up the [[as4.partner]] (endpoint + encryption cert)
//!             2. render the Übertragungsdatei, run the AHB gate
//!             3. pick the As4 client of the interchange's sender (UNB DE0004)
//!             4. Message::to(partner).service(MP)
//!                  .message_id("{outbox id}@{domain}")
//!                  .timestamp(first_attempt_at)  — identical on every retry
//!             5. As4::send → Verified<Receipt>   (sign, encrypt, POST, verify)
//!             6. keep receipt.evidence(), then acknowledge
//!             Err → retry inside the 72 h window, or dead-letter
//! ```
//!
//! ## Partners
//!
//! Every trading partner is a `[[as4.partner]]` with an endpoint and an
//! encryption certificate. A message for an unknown or receive-only recipient
//! is dead-lettered immediately (permanent `PartnerUnknown` error).
//!
//! ## Payload encoding
//!
//! `OutboxMessage.payload` is a domain-intent JSON value produced by workflow
//! `handle` implementations. This layer calls [`edifact_renderer::render_to_wire_bytes`]
//! to convert the JSON to BDEW-conformant EDIFACT wire bytes before handing them
//! to the AS4 transport.
//!
//! When no wire-format renderer exists for a message type the renderer returns
//! [`RenderError::InsufficientPayload`], and [`BdewAs4Sender`] **dead-letters
//! the entry permanently**. It does not fall back to putting the domain JSON on
//! the wire: a non-EDIFACT payload would breach BDEW MaKo interoperability, and
//! retrying could never succeed. [`WebhookEdifactSender`] — the development
//! transport, which delivers to an operator's own endpoint rather than to the
//! regulated network — does send the JSON, with a `warn!`.
//!
//! Every rendered message additionally passes a pre-send conformance gate: it is
//! parsed back and validated against its own release profile, and a failure is
//! permanent for the same reason.
//!
//! [`RenderError::InsufficientPayload`]: crate::edifact_renderer::RenderError::InsufficientPayload

use std::sync::Arc;

use edi_energy::EdiEnergyMessage as _;
use mako_as4::bdew_action_from_str;
use mako_engine::builder::As4Sender;
use mako_engine::error::EngineError;
use mako_engine::metrics::EngineMetrics;
use mako_engine::outbox::OutboxMessage;

use crate::as4_clients::As4Clients;
use crate::as4_evidence::As4Evidence;

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
///   "source": "urn:mako:makod:tenant:<tenant_id>",
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
/// `MaloIdentCallback` messages are routed to the embedded
/// [`MaloIdentSender`], as on the AS4 path.
///
/// Intended for development / ERP integration without an AS4 infrastructure.
/// In production, use [`BdewAs4Sender`].
#[derive(Clone)]
pub struct WebhookEdifactSender {
    webhook_url: Arc<str>,
    mp_id_registry: Arc<MpIdRegistry>,
    http_client: reqwest::Client,
    malo_sender: MaloIdentSender,
    netzzugang_sender: Option<Arc<crate::netzzugang::NetzzugangSender>>,
    /// Standard Webhooks signing key for the POSTed CloudEvent.
    ///
    /// The body carries the **rendered market message** — MaLo-IDs, the
    /// counterparty and the customer data the Vorgang is about — so the
    /// receiver has to be able to tell a mako delivery from anything else that
    /// can reach the URL. The same key the ERP adapter uses, because the
    /// receiver authenticates every mako CloudEvent the same way regardless of
    /// which emitter sent it.
    shared_secret: Option<secrecy::SecretString>,
}

impl WebhookEdifactSender {
    /// Create a new `WebhookEdifactSender`.
    ///
    /// `shared_secret` signs every POST with Standard Webhooks. `None` sends
    /// the market message unsigned, which the preflight refuses unless the
    /// operator asked for it by name.
    #[must_use]
    pub fn new(
        webhook_url: impl Into<Arc<str>>,
        mp_id_registry: Arc<MpIdRegistry>,
        http_client: reqwest::Client,
        malo_sender: MaloIdentSender,
        shared_secret: Option<secrecy::SecretString>,
    ) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            mp_id_registry,
            http_client,
            malo_sender,
            netzzugang_sender: None,
            shared_secret,
        }
    }

    /// Route `NetzzugangAntrag` outbox messages (§20b EnWG) to the given sender.
    #[must_use]
    pub fn with_netzzugang(mut self, sender: Arc<crate::netzzugang::NetzzugangSender>) -> Self {
        self.netzzugang_sender = Some(sender);
        self
    }
}

impl As4Sender for WebhookEdifactSender {
    /// Wire transport: everything except the ERP notifier's own messages.
    ///
    /// See [`crate::core::erp_adapter::is_erp_notification`] — both consumers of
    /// the shared outbox read the rule from there.
    fn handles(&self, msg: &OutboxMessage) -> bool {
        !crate::core::erp_adapter::is_erp_notification(&msg.message_type)
    }

    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send {
        let webhook_url = Arc::clone(&self.webhook_url);
        let mp_id_registry: Arc<MpIdRegistry> = Arc::clone(&self.mp_id_registry);
        let http_client = self.http_client.clone();
        let malo_sender = self.malo_sender.clone();
        let netzzugang_sender = self.netzzugang_sender.clone();
        let shared_secret = self.shared_secret.clone();
        let msg_owned = msg.clone();

        async move {
            // MaloIdentCallback: delegate to the MaLo-ID sender unchanged.
            if msg_owned.message_type.as_ref() == "MaloIdentCallback" {
                return malo_sender.send(&msg_owned).await;
            }
            // §20b Netzzugangsplattform requests: delegate to the netzzugang
            // sender when wired; else fall through to the raw-JSON webhook path.
            if msg_owned.message_type.as_ref() == crate::netzzugang::NETZZUGANG_MESSAGE_TYPE
                && let Some(nz) = &netzzugang_sender
            {
                return nz.send(&msg_owned).await;
            }

            // Render domain-intent JSON to EDIFACT wire bytes.
            let edifact_str =
                match edifact_renderer::render_to_wire_bytes(&msg_owned, &mp_id_registry) {
                    Ok(rendered) => String::from_utf8_lossy(&rendered.bytes).into_owned(),
                    Err(ref e) if edifact_renderer::is_suppressed(e) => {
                        // Gas positive APERAK: silence = acceptance per APERAK AHB 1.1 §2.3.
                        // No wire EDIFACT sent; deliver domain JSON to ERP webhook so the
                        // operator sees the positive outcome.
                        tracing::debug!(
                            message_id   = %msg_owned.message_id,
                            message_type = %msg_owned.message_type,
                            "WebhookEdifactSender: Gas positive APERAK suppressed — \
                             sending domain JSON (silence = acceptance, APERAK AHB 1.1 §2.3)",
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

            let ce = mako_service::CloudEvent::new(
                mako_service::source("makod", mp_id_registry.primary_mp_id()),
                mako_events::mako::EDIFACT_OUTBOUND,
                msg_owned.process_id.to_string(),
                serde_json::json!({
                    "message_type": msg_owned.message_type.as_ref(),
                    "recipient":    msg_owned.recipient.as_ref(),
                    "edifact":      edifact_str,
                }),
            )
            .with_id(msg_owned.message_id.to_string())
            .extension("makoconvid", msg_owned.conversation_id.to_string())
            .extension("makomessagetype", msg_owned.message_type.as_ref())
            .extension("makorecipient", msg_owned.recipient.as_ref());
            let body = ce.to_bytes().map_err(|e| {
                EngineError::Serialization(format!("EDIFACT_OUTBOUND CloudEvent: {e}"))
            })?;

            let mut builder = http_client
                .post(webhook_url.as_ref())
                .header("Content-Type", "application/cloudevents+json")
                .header("X-Idempotency-Key", msg_owned.message_id.to_string());

            // Standard Webhooks over the same body the receiver verifies, keyed
            // on the idempotency key it already deduplicates by — the one
            // canonical signer, shared with the ERP adapter.
            if let Some(secret) = &shared_secret {
                use secrecy::ExposeSecret as _;
                for (name, value) in mako_service::webhook::headers(
                    secret.expose_secret().as_bytes(),
                    &msg_owned.message_id.to_string(),
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    &body,
                ) {
                    builder = builder.header(name, value);
                }
            }

            let resp = builder
                .body(body)
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
    /// One `As4` client per local MP-ID and the partner directory.
    ///
    /// The client is chosen by the interchange's sender (UNB DE0004): the
    /// partner checks the signing certificate's OU against `eb:From`, so the
    /// MP-ID that wrote the interchange is the one that signs it.
    clients: As4Clients,
    /// Where each verified receipt's evidence is kept.
    evidence: As4Evidence,
    malo_sender: MaloIdentSender,
    /// The operator's own MP-ID registry.
    ///
    /// Used by the EDIFACT renderer to resolve the correct sender MP-ID per role
    /// and by the loopback path to detect own-MP-ID recipients (combined-role
    /// deployments where NB and MSB, or GNB and gMSB, have different MP-IDs on
    /// the same instance).
    mp_id_registry: Arc<MpIdRegistry>,
    /// Optional in-process loopback handle for combined-role deployments.
    ///
    /// When `Some`, outbox messages addressed to an own MP-ID are delivered
    /// in-process via [`crate::edifact_api::EdifactApiState`] instead of being
    /// dead-lettered.  Required for NB+MSB, GNB+gMSB, and NB+LF deployments.
    loopback: Option<Arc<crate::edifact_api::EdifactApiState>>,
    /// Parse/validation platform for the pre-send AHB conformance gate.
    platform: Arc<edi_energy::Platform>,
    /// §20b Netzzugangsplattform requests must never travel over AS4 — they
    /// route here when wired (see [`crate::netzzugang`]).
    netzzugang_sender: Option<Arc<crate::netzzugang::NetzzugangSender>>,
    /// Loopback hops seen per `conversation_id`, for the runaway guard.
    ///
    /// A combined-role deployment delivers self-addressed messages in-process:
    /// render → parse → dispatch → the workflow may emit another outbox message.
    /// If that message is self-addressed too, the cycle repeats — and nothing
    /// else bounds it. The outbox retry budget does not: a successful loopback
    /// *acknowledges*, and the message the workflow emits is a fresh entry with
    /// `attempt_count` 0. An unbounded cycle would spin the outbox worker and
    /// grow the event store without limit.
    ///
    /// A cycle stays inside one conversation, so counting hops per
    /// `conversation_id` bounds it. In-memory and per-process: a restart resets
    /// the counter, which is the right trade for a runaway guard — it must never
    /// be the reason a legitimate long conversation stalls.
    loopback_hops: Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>,
}

/// Count one loopback hop for `conversation_id`.
///
/// `Ok(n)` when the hop is within budget, `Err(n)` when it exceeds
/// [`MAX_LOOPBACK_HOPS`] and the delivery must be dead-lettered to break the
/// cycle. Split out of the send path so the budget is testable without an AS4
/// stack — the counting is the part that has to be right.
fn record_loopback_hop(
    hops: &std::sync::Mutex<std::collections::HashMap<String, u32>>,
    conversation_id: &str,
) -> Result<u32, u32> {
    let mut guard = hops
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Nothing signals that a conversation has ended, so the map only grows.
    // Drop every counter once it gets large: that is the same trade a restart
    // makes, and a real cycle re-trips the budget within MAX_LOOPBACK_HOPS hops.
    if guard.len() >= MAX_TRACKED_CONVERSATIONS && !guard.contains_key(conversation_id) {
        guard.clear();
    }
    let seen = guard.entry(conversation_id.to_owned()).or_insert(0);
    *seen += 1;
    if *seen > MAX_LOOPBACK_HOPS {
        Err(*seen)
    } else {
        Ok(*seen)
    }
}

/// Maximum in-process loopback deliveries per conversation.
///
/// Real combined-role exchanges are short: an NB→MSB Anfrage and its answer is
/// two hops, and the longest modelled chain (Geräteübernahme with a
/// Stornierung) stays in single digits. 32 leaves generous headroom while still
/// catching a cycle long before it costs anything.
const MAX_LOOPBACK_HOPS: u32 = 32;

/// Conversations the runaway guard tracks at once.
const MAX_TRACKED_CONVERSATIONS: usize = 10_000;

impl BdewAs4Sender {
    /// A sender over `clients`, keeping receipt evidence in `evidence`.
    #[must_use]
    pub fn new(
        clients: As4Clients,
        evidence: As4Evidence,
        malo_sender: MaloIdentSender,
        mp_id_registry: Arc<MpIdRegistry>,
        loopback: Option<Arc<crate::edifact_api::EdifactApiState>>,
        platform: Arc<edi_energy::Platform>,
    ) -> Self {
        Self {
            clients,
            evidence,
            malo_sender,
            mp_id_registry,
            loopback,
            platform,
            netzzugang_sender: None,
            loopback_hops: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Route `NetzzugangAntrag` outbox messages (§20b EnWG) to the given sender.
    #[must_use]
    pub fn with_netzzugang(mut self, sender: Arc<crate::netzzugang::NetzzugangSender>) -> Self {
        self.netzzugang_sender = Some(sender);
        self
    }
}

impl As4Sender for BdewAs4Sender {
    /// Wire transport: everything except the ERP notifier's own messages.
    ///
    /// See [`crate::core::erp_adapter::is_erp_notification`] — both consumers of
    /// the shared outbox read the rule from there.
    fn handles(&self, msg: &OutboxMessage) -> bool {
        !crate::core::erp_adapter::is_erp_notification(&msg.message_type)
    }

    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send {
        let clients = self.clients.clone();
        let evidence = self.evidence.clone();
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
        let netzzugang_sender = self.netzzugang_sender.clone();
        let loopback_hops = Arc::clone(&self.loopback_hops);
        async move {
            // Route MaloIdentCallback to the existing cache-lookup path.
            if message_type.as_ref() == "MaloIdentCallback" {
                return malo_sender.send(&msg_owned).await;
            }
            // §20b Netzzugangsplattform requests never travel over AS4.
            if message_type.as_ref() == crate::netzzugang::NETZZUGANG_MESSAGE_TYPE {
                if let Some(nz) = &netzzugang_sender {
                    return nz.send(&msg_owned).await;
                }
                return Err(EngineError::store(
                    "NetzzugangAntrag outbox message but no NetzzugangSender wired \
                     — configure the netzzugang adapter"
                        .to_owned(),
                ));
            }

            // Detect self-addressed messages (combined-role deployments).
            //
            // In integrated deployments (e.g. Stadtwerke operating as both NB and
            // MSB, or GNB and gMSB under the same MP-ID), the NB-side workflow emits
            // outbox messages addressed to the same MP-ID as the operator itself:
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
            // The registry is_own_mp_id check covers ALL own MP-IDs, so loopback works
            // even when NB and MSB have DIFFERENT MP-IDs on the same makod instance.
            if mp_id_registry.is_own_mp_id(recipient.as_ref()) {
                if let Some(ref loopback_state) = loopback {
                    // Runaway guard — see `loopback_hops`. Dead-letter rather
                    // than retry: a cycle is a workflow defect, and retrying it
                    // is what the guard exists to stop.
                    if let Err(seen) = record_loopback_hop(&loopback_hops, &conversation_id) {
                        {
                            tracing::error!(
                                message_id      = %message_id_str,
                                message_type    = %message_type,
                                conversation_id = %conversation_id,
                                hops            = seen,
                                "BdewAs4Sender loopback: conversation exceeded \
                                 MAX_LOOPBACK_HOPS — dead-lettering to break the cycle",
                            );
                            return Err(EngineError::Transport {
                                endpoint: "loopback".into(),
                                message: format!(
                                    "loopback cycle: conversation {conversation_id} exceeded \
                                     {MAX_LOOPBACK_HOPS} in-process hops — a workflow is \
                                     answering its own self-addressed message"
                                ),
                            });
                        }
                    }
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

                    // ── Pre-send AHB conformance gate ─────────────────────
                    //
                    // The same gate the network path runs, for the same reason.
                    // A self-addressed message is not a lesser message: in a
                    // combined-role VIU deployment it is a large share of the
                    // traffic, it produces the same § 147 AO records, and an
                    // invalid rendering that only ever loops back is a defect
                    // that would ship undetected until the day the counterparty
                    // moves to a separate MP-ID. Skipping the gate here made the
                    // loopback path the one place a non-conformant message could
                    // be accepted.
                    match loopback_state.platform.parse(&payload_bytes) {
                        Err(e) => {
                            return Err(EngineError::Serialization(format!(
                                "loopback gate: rendered EDIFACT does not parse for \
                                 {message_id_str} ({message_type}): {e}"
                            )));
                        }
                        Ok(parsed) => match parsed.validate_on_date(mako_fristen::heute()) {
                            Err(e) => {
                                return Err(EngineError::Serialization(format!(
                                    "loopback gate: AHB validation failed to run for \
                                     {message_id_str} ({message_type}): {e}"
                                )));
                            }
                            Ok(report) if !report.is_valid() => {
                                return Err(EngineError::Serialization(format!(
                                    "loopback gate: rendered EDIFACT violates its AHB \
                                     profile for {message_id_str} ({message_type}): {:?}",
                                    report.errors()
                                )));
                            }
                            Ok(_) => {}
                        },
                    }

                    // Recipient MP-ID (UNB DE0010) → commodity-aware routing of
                    // Sparte-split shared PIDs (INSRPT, WiM Gas device processes).
                    let loopback_recipient = loopback_state
                        .platform
                        .parse_interchange_full(&payload_bytes[..])
                        .map(|pi| pi.header.receiver_id.to_string())
                        .unwrap_or_default();

                    // A self-addressed DVGW interchange loops back through the
                    // DVGW parser, for the same reason the inbound paths sniff
                    // first: these bytes ride `ORDERS`/`ORDRSP`, so the BDEW
                    // parser would accept them and read a Prüfidentifikator from
                    // the wrong catalogue.
                    if let Some(report) =
                        crate::dvgw_ingest::try_ingest(loopback_state.as_ref(), &payload_bytes)
                            .await
                    {
                        // A refusal without its dead letter keeps the outbox
                        // entry, so the delivery is retried.
                        if let Some(e) = report.unrecorded {
                            return Err(EngineError::dead_letter(e));
                        }
                        tracing::info!(
                            message_id = %message_id_str,
                            accepted   = report.accepted(),
                            rejected   = report.rejected(),
                            "loopback: DVGW interchange dispatched in-process",
                        );
                        return Ok(());
                    }

                    // Every message in the interchange is visited. The previous
                    // version returned `Ok(())` at the *first* message whose PID
                    // had no workflow, which acknowledged the outbox entry while
                    // silently abandoning every message after it.
                    let mut dispatched = 0usize;
                    let mut unrouted_pids: Vec<u32> = Vec::new();
                    // A loopback delivery is received when it is delivered:
                    // the ÜZ of every message in it.
                    let received_at = time::OffsetDateTime::now_utc();
                    for parse_result in loopback_state
                        .platform
                        .parse_interchange(std::io::Cursor::new(&payload_bytes[..]))
                    {
                        // These are bytes *we* rendered and just validated, so a
                        // message that will not parse back is an internal defect,
                        // not counterparty input. Skipping it — as this loop used
                        // to — dropped a regulated message on the floor.
                        let parsed_msg = parse_result.map_err(|e| {
                            EngineError::Serialization(format!(
                                "loopback: self-rendered interchange contains an \
                                 unparseable message for {message_id_str} \
                                 ({message_type}): {e}"
                            ))
                        })?;

                        let pid_opt = parsed_msg
                            .detect_pruefidentifikator()
                            .ok()
                            .map(|p| p.as_u32());
                        let workflow_opt = pid_opt
                            .and_then(|p| loopback_state.resolve_workflow(p, &loopback_recipient));

                        match (pid_opt, workflow_opt, loopback_state.dispatcher.as_deref()) {
                            (Some(pid_val), Some(wf_name), Some(dispatcher)) => {
                                match dispatcher
                                    .dispatch(&parsed_msg, wf_name, pid_val, received_at)
                                    .await
                                {
                                    Ok(outcome) => {
                                        tracing::info!(
                                            message_id   = %message_id_str,
                                            message_type = %message_type,
                                            pid          = pid_val,
                                            workflow     = %wf_name,
                                            outcome      = ?outcome,
                                            "BdewAs4Sender loopback: in-process delivery succeeded",
                                        );
                                        dispatched += 1;
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
                            // PID carries no workflow on this side — e.g. ORDERS
                            // 17116 (Anfrage Sperrung, NB → MSB) in a build with
                            // no MSB-side workflow registered. Recorded and
                            // reported once after the loop rather than ending it.
                            (Some(pid_val), None, _) => unrouted_pids.push(pid_val),
                            (Some(_), Some(_), None) | (None, _, _) => {
                                return Err(EngineError::Serialization(format!(
                                    "loopback: no dispatcher wired, or no Prüfidentifikator \
                                     in a self-rendered message for {message_id_str} \
                                     ({message_type}) — this is a wiring defect, not \
                                     counterparty input"
                                )));
                            }
                        }
                    }

                    if dispatched == 0 {
                        if unrouted_pids.is_empty() {
                            // Nothing dispatched and nothing even identified: the
                            // interchange yielded no messages at all.
                            // Acknowledging here would retire an outbox entry
                            // that was never delivered anywhere.
                            return Err(EngineError::Serialization(format!(
                                "loopback: self-rendered interchange for {message_id_str} \
                                 ({message_type}) produced no messages — nothing was \
                                 delivered, so the outbox entry must not be acknowledged"
                            )));
                        }
                        // Every message was addressed to a role this build does
                        // not host. Acknowledge: retrying cannot change which
                        // workflows are compiled in, and the operator completes
                        // the exchange through the ERP command API.
                        tracing::warn!(
                            message_id   = %message_id_str,
                            message_type = %message_type,
                            pids         = ?unrouted_pids,
                            own_mp_ids   = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
                            "BdewAs4Sender loopback: no PID in this interchange has a \
                             workflow on this side; no counterpart role is registered. \
                             Use the ERP command API (e.g. gpke.sperrung.bestaetigen) \
                             to continue the exchange",
                        );
                    } else if !unrouted_pids.is_empty() {
                        tracing::warn!(
                            message_id   = %message_id_str,
                            message_type = %message_type,
                            dispatched,
                            unrouted     = ?unrouted_pids,
                            "BdewAs4Sender loopback: interchange partially dispatched — \
                             these PIDs have no workflow on this side",
                        );
                    }
                    return Ok(());
                }

                // ── No loopback configured — dead-letter ──────────────────────
                tracing::error!(
                    message_id   = %message_id_str,
                    message_type = %message_type,
                    own_mp_ids     = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
                    "BdewAs4Sender: outbox message addressed to own MP-ID \
                     (combined-role deployment — NB+MSB or GNB+gMSB sharing one MP-ID). \
                     No loopback handle configured. \
                     See site/content/docs/services/makod.md §Integrated operators for details.",
                );
                return Err(EngineError::PartnerUnknown { recipient });
            }

            // The recipient must be a configured partner with an inbox. Unknown
            // or receive-only partners are permanent (`PartnerUnknown`): the
            // outbox worker dead-letters immediately rather than retrying a
            // delivery no configuration change during the window can heal
            // without a restart.
            let Some(partner) = clients.partners().deliverable(&recipient).cloned() else {
                tracing::warn!(
                    message_id   = %message_id_str,
                    message_type = %message_type,
                    recipient    = %recipient,
                    "BdewAs4Sender: the recipient is not an [[as4.partner]] with an endpoint",
                );
                return Err(EngineError::PartnerUnknown { recipient });
            };
            let bdew_action = bdew_action_from_str(&message_type);

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
                    // Gas positive APERAK: silence = acceptance per APERAK AHB 1.1 §2.3.
                    // No AS4 message is sent; the outbox entry is acknowledged immediately.
                    tracing::debug!(
                        message_id   = %message_id_str,
                        message_type = %message_type,
                        recipient    = %recipient,
                        "BdewAs4Sender: Gas positive APERAK suppressed — \
                         no wire EDIFACT sent (silence = acceptance, APERAK AHB 1.1 §2.3)",
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
                Ok(parsed) => match parsed.validate_on_date(mako_fristen::heute()) {
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

            // The interchange's sender (UNB DE0004) selects the client: the
            // partner checks that the signing certificate's OU carries the
            // `eb:From` MP-ID, and it is this MP-ID's certificate that does.
            let Some(client) = clients.client(&rendered.sender_mp_id).cloned() else {
                return Err(EngineError::Serialization(format!(
                    "the interchange {message_id_str} is sent by {}, which has no AS4 identity \
                     ([party.as4]) in this deployment",
                    rendered.sender_mp_id
                )));
            };
            // The first attempt's instant, persisted by the outbox worker
            // before the first send. Every retry signs the same `eb:Timestamp`
            // under the same `eb:MessageId`, so the digests — and the partner's
            // NRR — are the first attempt's, and a receiver already holding the
            // message answers the retry as a duplicate.
            let Some(first_attempt_at) = msg_owned.first_attempt_at else {
                return Err(EngineError::Transport {
                    endpoint: recipient.to_string().into(),
                    message: format!(
                        "outbox entry {message_id_str} carries no first-attempt stamp; the \
                         outbox worker records it before the first send"
                    ),
                });
            };
            let as4_message_id = clients.message_id(&message_id_str);

            // Allgemeine Festlegungen V6.1d §2.12:
            // Nachrichtentyp_Anwendungsreferenz_von_an_yyyymmdd_DAR.txt, the
            // creation date in UTC, von/an from the interchange's UNB.
            let payload_filename = {
                let now = time::OffsetDateTime::now_utc();
                mako_as4::uebertragungsdatei_name(
                    &message_type,
                    anwendungsreferenz_for(&message_type),
                    &rendered.sender_mp_id,
                    &rendered.receiver_mp_id,
                    (now.year(), now.month() as u8, now.day()),
                    &rendered.dar,
                )
            };

            let message = asx::as4::Message::to(&partner)
                // BDEW AS4-Profil 1.2 §2.3.1.2.1: an EDIFACT Übertragungsdatei
                // of the Marktkommunikation is a Marktprozess.
                .service(bdew_action.service().uri())
                .message_id(as4_message_id.clone())
                .timestamp(std::time::SystemTime::from(first_attempt_at))
                .payload(
                    asx::Payload::new(rendered.bytes, mako_as4::constants::PAYLOAD_MIME_TYPE)
                        .named(payload_filename),
                );

            // Join the trace of the request that caused this message: the
            // `asx.send` span nests under this one.
            let span = tracing::info_span!(
                "makod.as4.send",
                outbox_id = %message_id_str,
                as4_message_id = %as4_message_id,
                recipient = %recipient,
                message_type = %message_type,
            );
            if let Some(tp) = msg_owned.trace_context.as_deref() {
                mako_service::telemetry::set_remote_parent(&span, tp);
            }
            let sent = tracing::Instrument::instrument(client.send(message), span).await;

            let receipt = match sent {
                Ok(receipt) => receipt,
                Err(e) => return Err(delivery_error(&e, &recipient, &message_id_str)),
            };

            // The verified receipt is the non-repudiation of receipt; it is
            // kept before the entry is acknowledged. A failed write keeps the
            // entry, and the resend is answered as a duplicate with a fresh
            // receipt.
            if let Err(e) = evidence
                .record_outbound(&message_id_str, receipt.evidence())
                .await
            {
                EngineMetrics::global().outbox_delivery_attempted("evidence_unrecorded");
                return Err(EngineError::Transport {
                    endpoint: recipient.to_string().into(),
                    message: format!(
                        "{message_id_str} was delivered, but its receipt evidence could not be \
                         kept: {e}"
                    ),
                });
            }

            tracing::info!(
                message_id     = %message_id_str,
                as4_message_id = %as4_message_id,
                message_type   = %message_type,
                recipient      = %recipient,
                "BdewAs4Sender: AS4 message delivered, receipt verified",
            );

            EngineMetrics::global().outbox_delivery_attempted("ok");
            Ok(())
        }
    }
}

/// The outbox error of a failed send.
///
/// Every failure is retryable inside the outbox window except a configuration
/// refusal (nothing was sent, and a resend sends nothing either): the message
/// id and timestamp are the first attempt's, so a resend is protocol-safe, and
/// a partner refusal (`eb:Error`) can heal when the operator fixes what the
/// partner refused. The metrics label separates the cases.
fn delivery_error(e: &asx::Error, recipient: &str, message_id: &str) -> EngineError {
    let label = match e.kind() {
        asx::ErrorKind::PartnerError => "counterparty_error",
        asx::ErrorKind::Indeterminate => "receipt_unverified",
        asx::ErrorKind::Config => "config_error",
        _ => "transport_error",
    };
    EngineMetrics::global().outbox_delivery_attempted(label);
    let detail = match e.ebms_code() {
        Some(code) => format!(
            "AS4 send of {message_id} failed ({code}, retryable: {}): {e}",
            e.is_retryable()
        ),
        None => format!(
            "AS4 send of {message_id} failed (retryable: {}): {e}",
            e.is_retryable()
        ),
    };
    if e.kind() == asx::ErrorKind::Config {
        return EngineError::Serialization(detail);
    }
    EngineError::Transport {
        endpoint: recipient.into(),
        message: detail,
    }
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

#[cfg(test)]
mod loopback_guard_tests {
    use super::{MAX_LOOPBACK_HOPS, MAX_TRACKED_CONVERSATIONS, record_loopback_hop};
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn hops() -> Mutex<HashMap<String, u32>> {
        Mutex::new(HashMap::new())
    }

    /// A real combined-role exchange is a handful of hops and must pass
    /// untouched — the guard must never be why a legitimate conversation stalls.
    #[test]
    fn an_ordinary_conversation_stays_within_budget() {
        let h = hops();
        for expected in 1..=8 {
            assert_eq!(record_loopback_hop(&h, "conv-1"), Ok(expected));
        }
    }

    /// The defect this guard exists to bound: a workflow answering its own
    /// self-addressed message. Nothing else stops it — a successful loopback
    /// *acknowledges*, so the message the workflow emits is a fresh outbox entry
    /// with `attempt_count` 0 and the retry budget never applies.
    #[test]
    fn a_cycle_is_broken_once_the_budget_is_exhausted() {
        let h = hops();
        for _ in 0..MAX_LOOPBACK_HOPS {
            assert!(record_loopback_hop(&h, "runaway").is_ok());
        }
        assert_eq!(
            record_loopback_hop(&h, "runaway"),
            Err(MAX_LOOPBACK_HOPS + 1),
            "the hop after the budget must be refused"
        );
        // And it stays refused — the cycle cannot resume by trying again.
        assert!(record_loopback_hop(&h, "runaway").is_err());
    }

    /// Conversations are counted independently: one runaway must not
    /// dead-letter unrelated traffic.
    #[test]
    fn conversations_do_not_share_a_budget() {
        let h = hops();
        for _ in 0..=MAX_LOOPBACK_HOPS {
            let _ = record_loopback_hop(&h, "runaway");
        }
        assert!(record_loopback_hop(&h, "runaway").is_err());
        assert_eq!(
            record_loopback_hop(&h, "healthy"),
            Ok(1),
            "an unrelated conversation must be unaffected"
        );
    }

    /// The counter map has no expiry, so it must not grow with every
    /// conversation the node ever sees.
    #[test]
    fn the_counter_map_stays_bounded() {
        let h = hops();
        for i in 0..=MAX_TRACKED_CONVERSATIONS {
            let _ = record_loopback_hop(&h, &format!("conv-{i}"));
        }
        assert!(
            h.lock().expect("not poisoned").len() <= MAX_TRACKED_CONVERSATIONS,
            "the loopback-hop map must be bounded"
        );
    }
}

#[cfg(test)]
mod bdew_envelope_tests {
    use mako_as4::constants;

    /// BDEW AS4-Profil §2.3.1.1 requires the `<eb:PartyId>/@type` attribute and
    /// derives it from the agency that issued the MP-ID. The AS4 party type and
    /// the EDIFACT NAD DE3055 agency describe the same fact, so they are derived
    /// from one source rather than configured twice.
    #[test]
    fn party_type_is_derived_from_the_registry_agency() {
        // The registry's own derivation: 99… → BDEW, 98… → DVGW, other 13 → GS1.
        let cases = [
            ("293", constants::PARTY_TYPE_BDEW),
            ("332", constants::PARTY_TYPE_DVGW),
            ("9", constants::PARTY_TYPE_GLN),
        ];
        for (agency, expected) in cases {
            assert_eq!(
                constants::party_id_type_for_agency(agency),
                expected,
                "agency {agency} must map to its ebCore party type"
            );
        }
    }
}
