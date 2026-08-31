//! BDEW AS4 outbound delivery for the Mako outbox.
//!
//! [`BdewAs4Sender`] is wired to the `OutboxWorker` in `main.rs` whenever
//! a signing key/cert pair is available (i.e. `--as4-signing-key-pem` and
//! `--as4-signing-cert-pem` are set).  Without one, only the
//! [`MaloIdentSender`] path runs and EDIFACT messages are not delivered.
//!
//! ## Design
//!
//! ```text
//! OutboxWorker::run()
//!   → BdewAs4Sender::send(OutboxMessage)
//!       ├── message_type == "MaloIdentCallback"
//!       │     → MaloIdentSender (existing cache-lookup path, unchanged)
//!       └── EDIFACT type (APERAK, CONTRL, UTILMD, …)
//!             1. resolve P-Mode from BdewAs4Profile (MP-ID + message type)
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
//! `--as4-partner <MP-ID>=<HTTPS-URL>` (repeatable).  This registers one
//! P-Mode per standard BDEW EDIFACT message type for the partner.  Messages
//! destined for an unknown recipient MP-ID or with a missing endpoint URL are
//! dead-lettered immediately (permanent `PartnerUnknown` error).
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
    netzzugang_sender: Option<Arc<crate::netzzugang::NetzzugangSender>>,
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
            netzzugang_sender: None,
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
    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send {
        let webhook_url = Arc::clone(&self.webhook_url);
        let mp_id_registry: Arc<MpIdRegistry> = Arc::clone(&self.mp_id_registry);
        let http_client = self.http_client.clone();
        let malo_sender = self.malo_sender.clone();
        let netzzugang_sender = self.netzzugang_sender.clone();
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

            let resp = http_client
                .post(webhook_url.as_ref())
                .header("Content-Type", "application/cloudevents+json")
                .header("X-Idempotency-Key", msg_owned.message_id.to_string())
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
    session: Arc<SessionContext>,
    event_bus: Arc<EventBus>,
    transport: Arc<As4HttpTransport>,
    profile: Arc<BdewAs4Profile>,
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
    /// When `Some`, outbox messages addressed to `tenant_party_id` (own MP-ID)
    /// are delivered in-process via [`crate::edifact_api::EdifactApiState`]
    /// instead of being dead-lettered.  Required for NB+MSB, GNB+gMSB, and
    /// NB+LF deployments sharing a single MP-ID.
    loopback: Option<Arc<crate::edifact_api::EdifactApiState>>,
    /// Parse/validation platform for the pre-send AHB conformance gate.
    platform: Arc<edi_energy::Platform>,
    /// When `true`, a missing or mismatched synchronous `eb:Receipt` is only
    /// warned about instead of failing the delivery. BDEW MaKo AS4 requires
    /// the synchronous receipt, so the strict default treats its absence as a
    /// retryable delivery failure; this flag exists for interop debugging
    /// against non-conformant counterparties.
    lenient_receipts: bool,
    /// §20b Netzzugangsplattform requests must never travel over AS4 — they
    /// route here when wired (see [`crate::netzzugang`]).
    netzzugang_sender: Option<Arc<crate::netzzugang::NetzzugangSender>>,
    /// The MP-ID emitted as `<eb:From>/<eb:PartyId>` on every outbound message.
    ///
    /// BDEW AS4-Profil §2.3.2 lets the receiver accept any value here **only**
    /// if it matches the subject of the presented certificate. This daemon
    /// signs everything with one key, so the AS4 party is the signing identity
    /// — not the interchange's `NAD+MS` sender, which in a combined Strom+Gas
    /// deployment can be a different own MP-ID.
    as4_party_id: Arc<str>,
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
        let mp_id_registry_party = mp_id_registry.primary_mp_id().to_owned();
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
            netzzugang_sender: None,
            as4_party_id: Arc::from(mp_id_registry_party),
            loopback_hops: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        })
    }

    /// Set the `<eb:From>/<eb:PartyId>` emitted on outbound messages.
    ///
    /// Defaults to the registry's primary MP-ID; override it when the AS4
    /// signing certificate is issued to a different identity (`--as4-party-id`).
    #[must_use]
    pub fn with_party_id(mut self, party_id: impl Into<Arc<str>>) -> Self {
        self.as4_party_id = party_id.into();
        self
    }

    /// Route `NetzzugangAntrag` outbox messages (§20b EnWG) to the given sender.
    #[must_use]
    pub fn with_netzzugang(mut self, sender: Arc<crate::netzzugang::NetzzugangSender>) -> Self {
        self.netzzugang_sender = Some(sender);
        self
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
        let as4_party_id: Arc<str> = Arc::clone(&self.as4_party_id);
        let platform = Arc::clone(&self.platform);
        let lenient_receipts = self.lenient_receipts;
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
                                match dispatcher.dispatch(&parsed_msg, wf_name, pid_val).await {
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

            // Resolve the P-Mode for the recipient MP-ID + message type.
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
                    "BdewAs4Sender: no P-Mode registered for this recipient MP-ID. \
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

            let mut policy = pm.to_send_policy().map_err(|e| EngineError::Transport {
                endpoint: endpoint.clone().into(),
                message: format!("P-Mode policy materialisation failed for {message_id_str}: {e}"),
            })?;
            policy.conversation_id = Some(conversation_id);

            // The ebMS3 party/role/agreement fields carry fixed BDEW values that
            // `to_send_policy` cannot know — it sees only the P-Mode. The receiving
            // MSH resolves its P-Mode from them, and §2.3.1.1 requires the PartyId
            // `type` attribute to name the agency that issued each MP-ID, so the
            // ebCore URI is derived from the same agency code the EDIFACT layer
            // puts in NAD DE3055 rather than assumed.
            policy.from_party_id = Some(as4_party_id.to_string());
            policy.to_party_id = Some(rendered.receiver_mp_id.to_string());
            policy.from_party_id_type = Some(
                mako_as4::constants::party_id_type_for_agency(
                    mp_id_registry.agency_for_mp_id(&as4_party_id),
                )
                .to_owned(),
            );
            policy.to_party_id_type = Some(
                mako_as4::constants::party_id_type_for_agency(
                    mp_id_registry.agency_for_mp_id(&rendered.receiver_mp_id),
                )
                .to_owned(),
            );
            policy.from_role = mako_as4::constants::ROLE_INITIATOR.to_owned();
            policy.to_role = mako_as4::constants::ROLE_RESPONDER.to_owned();
            // §2.3.2: fixed agreement value, and its `pmode`/`type` attributes
            // must not be emitted.
            policy.agreement_ref = Some(mako_as4::constants::AGREEMENT_REF.to_owned());
            policy.agreement_ref_type = None;

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
                // §2.2.3.2: the payload is an EDIFACT interchange carried as binary
                // in its own MIME part. Stated explicitly because asx-rs would
                // otherwise label it `application/xml` — its default whenever
                // encryption is on, which for this profile is always.
                payload_mime_type: Some(mako_as4::constants::PAYLOAD_MIME_TYPE.to_owned()),
                // Derived from the content digest; BDEW pins no Content-ID.
                payload_content_id: None,
                // One interchange per AS4 message — the profile defines no
                // multi-payload UserMessage.
                additional_payloads: Vec::new(),
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

            // Verify the counterparty's synchronous AS4 signal (BDEW MaKo AS4-Profil
            // 2.0 §4.6.3). asx-rs parses it namespace-correctly, verifies the receipt
            // signature *and that it covers the acted-on eb:SignalMessage*, checks
            // `RefToMessageId`, verifies every NonRepudiationInformation digest against
            // what the sent message was signed over, and enforces a replay window — the
            // Non-Repudiation-of-Receipt guarantee, proven end to end rather than assumed.
            let content_type = outcome
                .header("Content-Type")
                .unwrap_or("application/soap+xml")
                .to_string();
            let receipt_policy = if lenient_receipts {
                // Interop bring-up: accept unsigned / non-NRR receipts (still verifies
                // any signature/digest that IS present). Never a production posture.
                asx_rs::as4::As4ReceiptPolicy::relaxed()
            } else {
                asx_rs::as4::As4ReceiptPolicy::regulated()
            };
            match asx_rs::as4::verify_sync_response(
                &session,
                &event_bus,
                &output,
                &outcome.body,
                &content_type,
                &receipt_policy,
            ) {
                Ok(asx_rs::as4::As4SyncSignal::Receipt(_)) => {
                    // Delivery confirmed (NRR proven under the regulated policy).
                }
                Ok(asx_rs::as4::As4SyncSignal::Error(err)) => {
                    // The counterparty rejected the message with an eb:Error — a
                    // confirmed rejection, not a lost receipt. Deliberately still
                    // *retryable* (bounded by the outbox retry window): re-sending
                    // is protocol-safe — the ebMS MessageId is the stable outbox id
                    // and the AS4-Profil mandates receiver duplicate elimination
                    // (`ReceptionAwareness.DuplicateDetection` MUSS true) — and a
                    // retry can genuinely succeed, because the P-Mode, partner
                    // certificate and rendering are resolved per attempt, so an
                    // operator fixing the configuration the counterparty rejected
                    // heals the delivery without re-enqueueing. The metrics label
                    // separates this from a lost receipt for observability.
                    EngineMetrics::global().outbox_delivery_attempted("counterparty_error");
                    return Err(EngineError::Transport {
                        endpoint: endpoint.as_str().into(),
                        message: format!(
                            "counterparty rejected {message_id_str} with an AS4 eb:Error: {}",
                            err.summary()
                        ),
                    });
                }
                Err(reason) if lenient_receipts => {
                    tracing::warn!(
                        message_id   = %message_id_str,
                        recipient    = %recipient,
                        message_type = %message_type,
                        reason       = %reason,
                        "BdewAs4Sender: --as4-lenient-receipts — receipt verification \
                         failed; delivery acknowledged anyway",
                    );
                }
                Err(reason) => {
                    // Strict default: a delivery without a verifiable synchronous receipt
                    // is not a confirmed delivery. Retryable — the outbox worker backs
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
                Ok(other) if !lenient_receipts => {
                    // `As4SyncSignal` is #[non_exhaustive]: a future signal variant is
                    // not a confirmed receipt, so fail closed under the regulated policy.
                    EngineMetrics::global().outbox_delivery_attempted("receipt_unverified");
                    return Err(EngineError::Transport {
                        endpoint: endpoint.as_str().into(),
                        message: format!(
                            "unrecognized synchronous AS4 signal for {message_id_str}: {other:?}"
                        ),
                    });
                }
                Ok(_) => {}
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

    /// §2.3.2 lets a receiver accept any `<eb:From>/<eb:PartyId>` **only** when
    /// it matches the subject of the presented certificate. This daemon signs
    /// every outbound message with one key, so the AS4 party is the signing
    /// identity — never the interchange's `NAD+MS` sender, which in a combined
    /// Strom+Gas deployment is a different own MP-ID and would present a
    /// certificate that does not match.
    #[test]
    fn the_agreement_ref_carries_no_type_attribute() {
        // The fixed agreement value selects the profile's dynamic party model;
        // §2.3.2 forbids the accompanying `pmode` and `type` attributes, which
        // the sender expresses by passing `None` for the type.
        assert_eq!(
            constants::AGREEMENT_REF,
            "https://www.bdew.de/as4/communication/agreement"
        );
    }
}
