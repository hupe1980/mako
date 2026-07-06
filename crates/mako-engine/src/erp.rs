//! ERP integration traits and reference implementations.
//!
//! ## Role
//!
//! `mako-engine` is a protocol processor — it handles EDIFACT parsing, BDEW
//! process rules, AS4 delivery, and regulatory deadlines. All contract data,
//! billing logic, and master data live in the operator's ERP.
//!
//! This module defines the **stable integration contract** between `mako-engine`
//! and external ERP or backend systems.  The payload contract is **BO4E**, not
//! raw EDIFACT.  ERP adapters never see EDIFACT segment codes or format-version
//! identifiers — those are absorbed inside `mako-engine`.
//!
//! ## Outbound: mako → ERP
//!
//! Implement [`ErpAdapter`] and register it at startup.  Every domain event
//! that requires ERP action is delivered as an [`ErpEvent`].  The production
//! `WebhookErpAdapter` (in `makod`) serialises events as
//! **[CloudEvents 1.0](https://cloudevents.io) structured-mode JSON** and POSTs
//! them to the configured ERP endpoint.
//!
//! ```text
//! POST <erp_webhook_url>
//! Content-Type: application/cloudevents+json
//! X-Idempotency-Key: <event.idempotency_key>
//! X-Mako-Signature: <hmac-sha256-hex>   ← only when secret is configured
//!
//! {
//!   "specversion": "1.0",
//!   "id": "<idempotency_key>",
//!   "source": "urn:mako:tenant:<tenant_id>",
//!   "type": "de.mako.aperak.accepted",
//!   "time": "2026-10-01T10:15:00+02:00",
//!   "subject": "<process_id>",
//!   "dataschema": "https://.../Marktlokation.json",
//!   "datacontenttype": "application/json",
//!   "makoconvid": "<conversation_id>",
//!   "makocausationid": "<causation_id>",
//!   "makopid": 55001,
//!   "data": { "_typ": "MARKTLOKATION", ... }
//! }
//! ```
//!
//! See [`ErpEventType::cloud_event_type`] for the full type → CE type mapping.
//! The BO4E payload is always in the `data` field; the `payload_schema` URL
//! maps to the CloudEvents `dataschema` attribute.
//!
//! ## Inbound: ERP → mako (event-driven)
//!
//! For ERP systems with a message bus, implement [`ErpCommandSource`] to feed
//! BO4E business objects into the engine without a synchronous REST round-trip.
//!
//! ```rust,ignore
//! struct MyKafkaSource { consumer: KafkaConsumer }
//!
//! impl ErpCommandSource for MyKafkaSource {
//!     async fn next(&self) -> Result<Option<InboundErpCommand>, ErpAdapterError> {
//!         let msg = self.consumer.poll(Duration::from_millis(100)).await;
//!         Ok(msg.map(|m| InboundErpCommand {
//!             idempotency_key: m.offset().to_string(),
//!             tenant_id: TenantId::new(),
//!             payload_schema: "…/Marktlokation.json".into(),
//!             payload: serde_json::from_slice(m.payload()).unwrap(),
//!         }))
//!     }
//!
//!     async fn ack(&self, id: &str) -> Result<(), ErpAdapterError> {
//!         self.consumer.commit_offset(id.parse().unwrap()).await
//!             .map_err(ErpAdapterError::transport)
//!     }
//!
//!     async fn nack(&self, _id: &str, _reason: &str) -> Result<(), ErpAdapterError> {
//!         Ok(()) // Kafka auto-redelivers on next poll
//!     }
//! }
//! ```
//!
//! ## Reference implementations
//!
//! | Type | Feature | Use case |
//! |------|---------|---------|
//! | `NoopErpAdapter` | `testing` | Unit tests, CI |
//! | [`LogErpAdapter`] | — | Structured log output; starting point for new integrations |
//! | `NoopErpCommandSource` | `testing` | No-op inbound source for tests |
//!
//! For the production `WebhookErpAdapter` and `POST /api/v1/commands` endpoint,
//! see `makod/src/erp_adapter.rs`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{ConversationId, EventId, ProcessId, TenantId};

// ── ErpAdapterError ───────────────────────────────────────────────────────────

/// Errors produced by [`ErpAdapter`] and [`ErpCommandSource`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum ErpAdapterError {
    /// The ERP response payload could not be deserialised or is semantically
    /// invalid.
    #[error("ERP payload error: {0}")]
    Payload(String),

    /// A transient transport error (network timeout, HTTP 5xx, broker
    /// disconnect).  The delivery worker will retry with exponential backoff.
    #[error("ERP transport error: {0}")]
    Transport(String),

    /// A permanent, non-retryable error (e.g. invalid configuration,
    /// authentication failure).  The delivery worker will dead-letter the
    /// message.
    #[error("ERP permanent error: {0}")]
    Permanent(String),
}

impl ErpAdapterError {
    /// Construct a [`Payload`](ErpAdapterError::Payload) variant.
    pub fn payload(e: impl std::fmt::Display) -> Self {
        Self::Payload(e.to_string())
    }

    /// Construct a [`Transport`](ErpAdapterError::Transport) variant.
    pub fn transport(e: impl std::fmt::Display) -> Self {
        Self::Transport(e.to_string())
    }

    /// Construct a [`Permanent`](ErpAdapterError::Permanent) variant.
    pub fn permanent(e: impl std::fmt::Display) -> Self {
        Self::Permanent(e.to_string())
    }

    /// Returns `true` for transient errors that warrant a retry.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transport(_))
    }
}

// ── ErpEventType ─────────────────────────────────────────────────────────────

/// Semantic classification of an outbound ERP process event.
///
/// The ERP uses this to decide which action to take — update an order status,
/// trigger a billing run, open a complaint ticket, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErpEventType {
    /// A new MaKo process was spawned (e.g. inbound UTILMD received).
    ProcessInitiated,
    /// The counterparty sent an APERAK accepting our UTILMD.
    AperakAccepted,
    /// The counterparty sent an APERAK rejecting our UTILMD.
    AperakRejected,
    /// No APERAK received within the regulatory SLA window (deadline expired).
    AperakTimeout,
    /// A CONTRL syntax acknowledgement was received.
    ContrlReceived,
    /// The process reached its terminal success state
    /// (e.g. Lieferbeginn/Lieferende confirmed).
    ProcessCompleted,
    /// A MaLo identification request was successfully resolved: the MaLo was
    /// found and the positive callback was delivered to the requesting LF.
    ///
    /// The `payload` field of the associated [`ErpEvent`] carries a BO4E
    /// `Marktlokation` JSON object with the resolved MaLo data.
    MaloIdentified,
    /// The process failed permanently (regulatory timeout, data error, …).
    ProcessFailed {
        /// Human-readable failure description.
        reason: Box<str>,
    },
}

impl ErpEventType {
    /// Short label for structured logging and metrics.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::ProcessInitiated => "process_initiated",
            Self::AperakAccepted => "aperak_accepted",
            Self::AperakRejected => "aperak_rejected",
            Self::AperakTimeout => "aperak_timeout",
            Self::ContrlReceived => "contrl_received",
            Self::ProcessCompleted => "process_completed",
            Self::MaloIdentified => "malo_identified",
            Self::ProcessFailed { .. } => "process_failed",
        }
    }

    /// CloudEvents 1.0 `type` attribute for this event.
    ///
    /// Follows the reverse-DNS prefix convention (`de.mako.<domain>.<action>`).
    /// Used by the `WebhookErpAdapter` to populate the `type` field of the
    /// CloudEvents envelope.
    #[must_use]
    pub fn cloud_event_type(&self) -> &'static str {
        match self {
            Self::ProcessInitiated => "de.mako.process.initiated",
            Self::AperakAccepted => "de.mako.aperak.accepted",
            Self::AperakRejected => "de.mako.aperak.rejected",
            Self::AperakTimeout => "de.mako.aperak.timeout",
            Self::ContrlReceived => "de.mako.contrl.received",
            Self::ProcessCompleted => "de.mako.process.completed",
            Self::MaloIdentified => "de.mako.malo.identified",
            Self::ProcessFailed { .. } => "de.mako.process.failed",
        }
    }
}

// ── ErpEvent ──────────────────────────────────────────────────────────────────

/// A structured process event delivered from `mako-engine` to the ERP.
///
/// The payload is always a **BO4E-typed JSON object** — the ERP adapter never
/// receives raw EDIFACT bytes or EDIFACT format-version identifiers.
///
/// On the wire (via `WebhookErpAdapter`) this struct is serialised as a
/// **[CloudEvents 1.0](https://cloudevents.io) structured-mode JSON** envelope
/// with `Content-Type: application/cloudevents+json`.  The BO4E payload lives
/// in the CloudEvents `data` field; `payload_schema` maps to `dataschema`;
/// `event_type` maps to the `type` attribute via [`ErpEventType::cloud_event_type`].
///
/// ## Idempotency
///
/// `idempotency_key` maps to the CloudEvents `id` attribute and is also sent
/// as `X-Idempotency-Key` for ERP middleware that keys on headers.  The ERP
/// **must** persist this key and return `HTTP 200 OK` for duplicate deliveries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErpEvent {
    /// Stable dedup key — store in the ERP to reject duplicate deliveries.
    ///
    /// Derived from the outbox `message_id`; stable across retries.
    pub idempotency_key: String,

    /// Semantic classification of this event.
    pub event_type: ErpEventType,

    /// The mako process that generated this event.
    pub process_id: ProcessId,

    /// Tenant (operator GLN) that owns this process.
    pub tenant_id: TenantId,

    /// BDEW business conversation identifier.
    pub conversation_id: ConversationId,

    /// The mako domain event that directly caused this ERP notification.
    pub causation_id: EventId,

    /// Prüfidentifikator of the process.
    pub pid: u32,

    /// BO4E JSON Schema URL that validates [`payload`](ErpEvent::payload).
    ///
    /// Examples:
    /// - `"https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202501.0.0/src/bo4e_schemas/bo/Marktlokation.json"`
    /// - `"https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202501.0.0/src/bo4e_schemas/bo/Messlokation.json"`
    ///
    /// `None` for events where no primary BO4E object is applicable
    /// (e.g. `ContrlReceived`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_schema: Option<String>,

    /// BO4E-typed payload.
    ///
    /// Deserialise using the ERP's own BO4E library.  Raw EDIFACT structures
    /// are never exposed here.  `null` when no payload is applicable.
    pub payload: serde_json::Value,

    /// Wall-clock time when the domain event was persisted.
    pub occurred_at: OffsetDateTime,

    /// Workflow family name that produced this event (e.g. `"gpke-sperrung"`).
    ///
    /// Carried through from `OutboxMessage::workflow_name`.  Emitted as the
    /// `makoworkflow` CloudEvents extension attribute by `WebhookErpAdapter`.
    /// `mdmd` maps this to `mdmrole` for role-scoped ERP subscriber fan-out.
    ///
    /// Empty string for events produced by legacy outbox messages that
    /// predate this field.
    pub workflow_name: Box<str>,
}

// ── ErpAdapter trait ──────────────────────────────────────────────────────────

/// Outbound notification sink — `mako-engine` calls this when a process event
/// should be reported to the ERP.
///
/// The payload is always a BO4E-typed JSON object; the adapter never receives
/// raw EDIFACT bytes or format-version identifiers.
///
/// ## Contract
///
/// - Must be **idempotent** on `event.idempotency_key`.  Called twice with the
///   same key must succeed without double-posting.
/// - Return [`ErpAdapterError::Transport`] for transient failures — the caller
///   will retry with exponential backoff.
/// - Return [`ErpAdapterError::Permanent`] for non-retryable failures — the
///   caller will dead-letter the event.
#[allow(async_fn_in_trait)]
pub trait ErpAdapter: Send + Sync + 'static {
    /// Deliver `event` to the ERP.
    async fn notify(&self, event: ErpEvent) -> Result<(), ErpAdapterError>;
}

/// Blanket `Arc` implementation so `ErpAdapter` can be shared across tasks.
impl<T: ErpAdapter> ErpAdapter for Arc<T> {
    async fn notify(&self, event: ErpEvent) -> Result<(), ErpAdapterError> {
        (**self).notify(event).await
    }
}

// ── InboundErpCommand ─────────────────────────────────────────────────────────

/// A BO4E business object received from the ERP, intended to trigger a mako
/// process.
///
/// `mako-engine` maps the BO4E payload to an internal `Command` via the
/// domain crate's command mapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundErpCommand {
    /// Stable dedup key — forwarded to [`InboxStore::accept`].
    ///
    /// The ERP must supply a stable, unique identifier per command so that
    /// retransmissions do not double-execute the workflow.
    ///
    /// [`InboxStore::accept`]: crate::inbox::InboxStore::accept
    pub idempotency_key: String,

    /// Tenant (operator GLN) that owns the target process.
    pub tenant_id: TenantId,

    /// BO4E JSON Schema URL — identifies the object type without inspecting
    /// `payload`.
    ///
    /// Example:
    /// `"https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202501.0.0/src/bo4e_schemas/bo/Vertrag.json"`
    pub payload_schema: String,

    /// BO4E-typed JSON payload.  `mako-engine` maps this to an internal
    /// `Command` via the registered domain command mapper.
    pub payload: serde_json::Value,
}

// ── ErpCommandSource trait ────────────────────────────────────────────────────

/// Inbound command source — `mako-engine` polls this for new BO4E objects
/// from the ERP.
///
/// Implement this for broker-based inbound flows (Kafka consumer, SFTP poll,
/// database change feed, …) to make the entire integration fully event-driven
/// — no synchronous REST round-trip required.
///
/// ## Contract
///
/// - [`next`](ErpCommandSource::next) must be **non-blocking** when idle —
///   return `Ok(None)` immediately when no command is available.
/// - [`ack`](ErpCommandSource::ack) must suppress re-delivery of `id` after
///   a successful ack (idempotent).
/// - [`nack`](ErpCommandSource::nack) should allow re-delivery of `id` after
///   an appropriate backoff.
#[allow(async_fn_in_trait)]
pub trait ErpCommandSource: Send + Sync + 'static {
    /// Return the next pending BO4E command, or `None` when the source is idle.
    async fn next(&self) -> Result<Option<InboundErpCommand>, ErpAdapterError>;

    /// Acknowledge successful processing of `id`.
    ///
    /// After a successful ack the source must not re-deliver `id`.
    async fn ack(&self, id: &str) -> Result<(), ErpAdapterError>;

    /// Negative-acknowledge — allow re-delivery of `id` after backoff.
    async fn nack(&self, id: &str, reason: &str) -> Result<(), ErpAdapterError>;
}

/// Blanket `Arc` implementation so `ErpCommandSource` can be shared across tasks.
impl<S: ErpCommandSource> ErpCommandSource for Arc<S> {
    async fn next(&self) -> Result<Option<InboundErpCommand>, ErpAdapterError> {
        (**self).next().await
    }
    async fn ack(&self, id: &str) -> Result<(), ErpAdapterError> {
        (**self).ack(id).await
    }
    async fn nack(&self, id: &str, reason: &str) -> Result<(), ErpAdapterError> {
        (**self).nack(id, reason).await
    }
}

// ── NoopErpAdapter ────────────────────────────────────────────────────────────

/// An [`ErpAdapter`] that succeeds immediately without notifying anything.
///
/// Use in unit tests and CI where no real ERP endpoint is available.
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Default)]
pub struct NoopErpAdapter;

#[cfg(feature = "testing")]
impl ErpAdapter for NoopErpAdapter {
    async fn notify(&self, _event: ErpEvent) -> Result<(), ErpAdapterError> {
        Ok(())
    }
}

// ── LogErpAdapter ─────────────────────────────────────────────────────────────

/// An [`ErpAdapter`] that logs every event at `info` level without delivering
/// it.
///
/// Useful as a development starting point — replace it with your concrete ERP
/// adapter in production.
#[derive(Debug, Clone, Default)]
pub struct LogErpAdapter;

impl ErpAdapter for LogErpAdapter {
    async fn notify(&self, event: ErpEvent) -> Result<(), ErpAdapterError> {
        tracing::info!(
            idempotency_key = %event.idempotency_key,
            event_type      = event.event_type.label(),
            process_id      = %event.process_id,
            tenant_id       = %event.tenant_id,
            pid             = event.pid,
            "ErpAdapter: event logged (no delivery configured)",
        );
        Ok(())
    }
}

// ── NoopErpCommandSource ──────────────────────────────────────────────────────

/// An [`ErpCommandSource`] that is always idle (returns `Ok(None)`).
///
/// Use in tests where no inbound ERP command flow is needed.
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Default)]
pub struct NoopErpCommandSource;

#[cfg(feature = "testing")]
impl ErpCommandSource for NoopErpCommandSource {
    async fn next(&self) -> Result<Option<InboundErpCommand>, ErpAdapterError> {
        Ok(None)
    }
    async fn ack(&self, _id: &str) -> Result<(), ErpAdapterError> {
        Ok(())
    }
    async fn nack(&self, _id: &str, _reason: &str) -> Result<(), ErpAdapterError> {
        Ok(())
    }
}

// ── ErpAdapterTestHarness ─────────────────────────────────────────────────────

/// A recording [`ErpAdapter`] for use in tests.
///
/// Records every [`ErpEvent`] delivered via [`notify`](ErpAdapter::notify) so
/// tests can assert on event types, ordering, and BO4E payload shapes.
///
/// ```rust,ignore
/// let harness = ErpAdapterTestHarness::new();
/// my_workflow.run_with_adapter(harness.adapter()).await?;
///
/// let events = harness.events();
/// assert_eq!(events[0].event_type, ErpEventType::ProcessInitiated);
/// assert_eq!(events[1].event_type, ErpEventType::AperakAccepted);
/// ```
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Default)]
pub struct ErpAdapterTestHarness {
    events: Arc<tokio::sync::Mutex<Vec<ErpEvent>>>,
}

#[cfg(feature = "testing")]
impl ErpAdapterTestHarness {
    /// Create a new empty harness.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a snapshot of all recorded events in delivery order.
    pub async fn events(&self) -> Vec<ErpEvent> {
        self.events.lock().await.clone()
    }

    /// Drain all recorded events, resetting the harness.
    pub async fn drain(&self) -> Vec<ErpEvent> {
        std::mem::take(&mut *self.events.lock().await)
    }
}

#[cfg(feature = "testing")]
impl ErpAdapter for ErpAdapterTestHarness {
    async fn notify(&self, event: ErpEvent) -> Result<(), ErpAdapterError> {
        self.events.lock().await.push(event);
        Ok(())
    }
}

// ── ErpCommandSourceTestHarness ───────────────────────────────────────────────

/// A controllable [`ErpCommandSource`] for use in tests.
///
/// Inject canned [`InboundErpCommand`] payloads and verify that the engine
/// processes them correctly.
///
/// ```text
/// let source = ErpCommandSourceTestHarness::new();
/// source.inject(InboundErpCommand {
///     idempotency_key: "order-42".into(),
///     tenant_id: TenantId::new(),
///     payload_schema: ".../Vertrag.json".into(),
///     payload: serde_json::json!({ "_typ": "VERTRAG", ... }),
/// }).await;
///
/// // The engine picks up the command on the next poll.
/// ```
#[cfg(feature = "testing")]
#[derive(Debug, Clone, Default)]
pub struct ErpCommandSourceTestHarness {
    queue: Arc<tokio::sync::Mutex<std::collections::VecDeque<InboundErpCommand>>>,
    acked: Arc<tokio::sync::Mutex<Vec<String>>>,
    nacked: Arc<tokio::sync::Mutex<Vec<(String, String)>>>,
}

#[cfg(feature = "testing")]
impl ErpCommandSourceTestHarness {
    /// Create a new empty harness.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a command to be returned by the next [`next`](ErpCommandSource::next) call.
    pub async fn inject(&self, cmd: InboundErpCommand) {
        self.queue.lock().await.push_back(cmd);
    }

    /// Return all acked command IDs.
    pub async fn acked(&self) -> Vec<String> {
        self.acked.lock().await.clone()
    }

    /// Return all nacked `(id, reason)` pairs.
    pub async fn nacked(&self) -> Vec<(String, String)> {
        self.nacked.lock().await.clone()
    }
}

#[cfg(feature = "testing")]
impl ErpCommandSource for ErpCommandSourceTestHarness {
    async fn next(&self) -> Result<Option<InboundErpCommand>, ErpAdapterError> {
        Ok(self.queue.lock().await.pop_front())
    }

    async fn ack(&self, id: &str) -> Result<(), ErpAdapterError> {
        self.acked.lock().await.push(id.to_owned());
        Ok(())
    }

    async fn nack(&self, id: &str, reason: &str) -> Result<(), ErpAdapterError> {
        self.nacked
            .lock()
            .await
            .push((id.to_owned(), reason.to_owned()));
        Ok(())
    }
}

// ── BO4E schema URL constants ─────────────────────────────────────────────────

/// BO4E schema URL base for v202501.0.0.
///
/// Use `bo4e_schema_url!(Marktlokation)` to construct typed schema URLs at
/// compile time.
pub const BO4E_V202501_BASE: &str =
    "https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202501.0.0/src/bo4e_schemas";

/// Construct a BO4E v202501.0.0 JSON Schema URL for a Business Object.
///
/// ```rust
/// use mako_engine::bo4e_schema_url;
/// assert!(bo4e_schema_url!("bo", "Marktlokation").contains("Marktlokation"));
/// ```
#[macro_export]
macro_rules! bo4e_schema_url {
    ($category:literal, $name:literal) => {
        concat!(
            "https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202501.0.0/src/bo4e_schemas/",
            $category,
            "/",
            $name,
            ".json",
        )
    };
}

/// BO4E JSON Schema URL for `Marktlokation`.
pub const BO4E_SCHEMA_MARKTLOKATION: &str = bo4e_schema_url!("bo", "Marktlokation");

/// BO4E JSON Schema URL for `Messlokation`.
pub const BO4E_SCHEMA_MESSLOKATION: &str = bo4e_schema_url!("bo", "Messlokation");

/// BO4E JSON Schema URL for `Vertrag`.
pub const BO4E_SCHEMA_VERTRAG: &str = bo4e_schema_url!("bo", "Vertrag");

/// BO4E JSON Schema URL for `Energiemenge`.
pub const BO4E_SCHEMA_ENERGIEMENGE: &str = bo4e_schema_url!("bo", "Energiemenge");

/// BO4E JSON Schema URL for `Rechnung`.
pub const BO4E_SCHEMA_RECHNUNG: &str = bo4e_schema_url!("bo", "Rechnung");

/// BO4E JSON Schema URL for `Zaehler`.
pub const BO4E_SCHEMA_ZAEHLER: &str = bo4e_schema_url!("bo", "Zaehler");

/// BO4E JSON Schema URL for `Geschaeftspartner`.
pub const BO4E_SCHEMA_GESCHAEFTSPARTNER: &str = bo4e_schema_url!("bo", "Geschaeftspartner");
