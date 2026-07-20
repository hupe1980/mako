//! Outbox pattern for reliable at-least-once outbound message delivery.
//!
//! # Why the outbox?
//!
//! When a process transition generates an outbound EDIFACT message (e.g. an
//! APERAK acknowledgement), two writes must happen atomically:
//!
//! 1. Domain events are appended to the event store.
//! 2. The EDIFACT payload is queued for delivery to the AS4 endpoint.
//!
//! Without the outbox, a crash between steps 1 and 2 silently loses the
//! outbound message. With the outbox, both writes are part of the same
//! database transaction — a background delivery worker then delivers pending
//! messages, surviving crashes and transient AS4 failures transparently.
//!
//! # Usage
//!
//! ```rust,ignore
//! // After a command dispatch that should trigger an outbound APERAK:
//! let env = &aperak_envelopes[0];
//! let msg = OutboxMessage::new(
//!     process.stream_id().clone(),
//!     env.process_id,
//!     env.tenant_id,
//!     env.correlation_id,
//!     env.conversation_id,
//!     env.event_id,
//!     "APERAK",
//!     &recipient_gln,
//!     aperak_payload_json,
//! );
//! outbox_store.enqueue(&[msg]).await?;
//!
//! // Background delivery worker:
//! let pending = outbox_store.pending_now(50).await?;
//! for msg in pending {
//!     as4_client.send(&msg).await?;
//!     outbox_store.acknowledge(msg.message_id).await?;
//! }
//! ```
//!
//! # Atomicity contract
//!
//! `InMemoryOutboxStore` does **not** guarantee transactional atomicity
//! with `InMemoryEventStore`. Persistent backend crates
//! (`mako-event-store-slatedb`, `mako-event-store-postgres`) MUST enqueue
//! messages in the same database transaction as the event append.

use std::sync::Arc;

#[cfg(any(test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;

use time::OffsetDateTime;

use crate::{
    error::EngineError,
    ids::{ConversationId, CorrelationId, EventId, OutboxMessageId, ProcessId, StreamId, TenantId},
};

// ── PendingOutbox ─────────────────────────────────────────────────────────────

/// A lightweight outbox message specification produced by [`Workflow::handle`].
///
/// [`Workflow::handle`] is a pure function: it cannot know the store-assigned
/// fields (`event_id`, `stream_id`, `process_id`, etc.) of the events it is
/// about to emit. `PendingOutbox` carries only the information the domain
/// workflow can produce deterministically, without I/O or clock access.
///
/// The engine fills in the store-assigned fields after the event append
/// succeeds, converting `PendingOutbox` into a fully materialised
/// [`OutboxMessage`] inside [`SlateDbStore::append_with_outbox`].
///
/// # Example
///
/// ```rust,ignore
/// // Inside Workflow::handle, when DispatchAperak succeeds:
/// let outbox = vec![
///     PendingOutbox::new("APERAK", &state.sender_party_id().to_string(), aperak_payload)
///         .caused_by(0),  // caused by the first event in this batch
/// ];
/// Ok(WorkflowOutput { events, outbox })
/// ```
///
/// [`Workflow::handle`]: crate::workflow::Workflow::handle
/// [`SlateDbStore::append_with_outbox`]: crate::event_store::AtomicAppend::append_with_outbox
#[derive(Debug, Clone)]
pub struct PendingOutbox {
    /// EDIFACT or XML message type (e.g. `"APERAK"`, `"CONTRL"`, `"REMADV"`).
    pub message_type: Box<str>,
    /// GLN or EIC code of the intended recipient market participant.
    pub recipient: Box<str>,
    /// Domain-level message payload (JSON).
    ///
    /// Typically encodes the intent (e.g. positive/negative APERAK reason)
    /// rather than the final EDIFACT bytes. The delivery worker or AS4
    /// gateway is responsible for rendering the final wire format.
    pub payload: serde_json::Value,
    /// Do not deliver before this time.
    ///
    /// `None` means deliver immediately (as soon as the delivery worker runs).
    /// Must not use the wall clock inside `handle` — derive from domain data
    /// only (e.g. a schedule date carried in the command).
    pub deliver_after: Option<OffsetDateTime>,
    /// BO4E JSON Schema URL that describes the `payload` shape.
    ///
    /// Set this to the canonical BO4E schema URL when the payload is a
    /// BO4E-typed object (e.g. `Marktlokation`, `Messlokation`). Leave
    /// `None` for raw EDIFACT or untyped payloads.
    ///
    /// Example:
    /// `"https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202607.0.0/src/bo4e_schemas/bo/Marktlokation.json"`
    pub payload_schema: Option<Box<str>>,
    /// Zero-based index into the concurrent events batch that caused this
    /// outbound message.
    ///
    /// Used by the engine to set `causation_event_id` on the materialised
    /// [`OutboxMessage`] from the stamped [`EventEnvelope`] at the same index.
    /// Clamped to `events.len() - 1` when out-of-range.
    ///
    /// [`EventEnvelope`]: crate::envelope::EventEnvelope
    pub caused_by_event_index: usize,
}

impl PendingOutbox {
    /// Construct a pending outbox message for immediate delivery.
    ///
    /// `caused_by_event_index` defaults to `0` (first event in the batch).
    /// Chain [`caused_by`] to change it.
    ///
    /// [`caused_by`]: PendingOutbox::caused_by
    #[must_use]
    pub fn new(
        message_type: impl Into<Box<str>>,
        recipient: impl Into<Box<str>>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            message_type: message_type.into(),
            recipient: recipient.into(),
            payload,
            deliver_after: None,
            payload_schema: None,
            caused_by_event_index: 0,
        }
    }

    /// Set the zero-based index of the event that caused this outbox message.
    #[must_use]
    pub fn caused_by(mut self, index: usize) -> Self {
        self.caused_by_event_index = index;
        self
    }

    /// Set a deferred delivery time (must be derived from domain data, not
    /// the wall clock, to preserve `Workflow::handle` purity).
    #[must_use]
    pub fn with_deliver_after(mut self, deliver_after: OffsetDateTime) -> Self {
        self.deliver_after = Some(deliver_after);
        self
    }

    /// Attach a BO4E JSON Schema URL to the payload.
    ///
    /// Use this when the payload is a BO4E-typed object so the ERP adapter
    /// can deserialise it into the correct type without inspecting the JSON.
    #[must_use]
    pub fn with_schema(mut self, schema_url: &'static str) -> Self {
        self.payload_schema = Some(schema_url.into());
        self
    }
}

// ── OutboxMessage ─────────────────────────────────────────────────────────────

/// An outbound message queued for delivery via AS4 or another channel.
///
/// The message carries both routing information (`recipient`, `message_type`)
/// and full correlation metadata so the delivery worker can trace every send
/// back to the domain event that caused it.
///
/// Construct with [`OutboxMessage::new`] and optionally chain
/// [`OutboxMessage::with_deliver_after`] for deferred delivery.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutboxMessage {
    /// Stable unique identifier for this outbox entry.
    pub message_id: OutboxMessageId,

    /// The process stream that produced this outbound message.
    pub stream_id: StreamId,

    /// The MaKo process instance.
    pub process_id: ProcessId,

    /// The tenant sending this message.
    pub tenant_id: TenantId,

    /// Propagated correlation root from the triggering event.
    pub correlation_id: CorrelationId,

    /// Business conversation this message belongs to (e.g. UTILMD ↔ APERAK).
    pub conversation_id: ConversationId,

    /// The persisted event that directly caused this outbound message.
    pub causation_event_id: EventId,

    /// EDIFACT or XML message type (e.g. `"APERAK"`, `"CONTRL"`, `"UTILMD"`).
    pub message_type: Box<str>,

    /// GLN or EIC code of the intended recipient market participant.
    pub recipient: Box<str>,

    /// Serialized message payload.
    ///
    /// Typically a JSON-encoded string of EDIFACT bytes or a structured
    /// JSON object for non-EDIFACT channels.
    pub payload: serde_json::Value,

    /// BO4E JSON Schema URL that validates `payload`, if present.
    ///
    /// `None` for raw EDIFACT or untyped payloads. Set by domain workflows
    /// via [`PendingOutbox::with_schema`] when the payload is a BO4E object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_schema: Option<Box<str>>,

    /// When this entry was created.
    pub created_at: OffsetDateTime,

    /// Do not deliver before this time.
    ///
    /// `None` means deliver immediately (as soon as the delivery worker runs).
    pub deliver_after: Option<OffsetDateTime>,

    /// Number of delivery attempts so far. Starts at `0`, incremented by
    /// [`OutboxStore::reschedule`].
    pub attempt_count: u32,

    /// Workflow family name that produced this message (e.g. `"gpke-sperrung"`).
    ///
    /// Stamped from the `EventEnvelope::workflow_id.name` at materialisation
    /// time.  Used by the `OutboxErpWorker` to populate the `makoworkflow`
    /// CloudEvents extension attribute, which `marktd` maps to `marktrole` for
    /// role-scoped ERP fan-out.
    ///
    /// Empty string for messages materialised before this field was introduced
    /// (backward-compatible deserialisation via `#[serde(default)]`).
    #[serde(default)]
    pub workflow_name: Box<str>,

    /// W3C `traceparent` of the request that caused this message.
    ///
    /// Captured from [`crate::trace_ctx`] at creation time and injected into
    /// outbound deliveries (ERP webhook header + CloudEvents `traceparent`
    /// extension), so a trace started by the inbound transport continues
    /// across the asynchronous outbox boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<Box<str>>,
}

impl OutboxMessage {
    /// Construct a new outbox message.
    ///
    /// `message_id` and `created_at` are generated automatically.
    /// `attempt_count` is initialized to `0`.
    ///
    /// Call [`OutboxMessage::with_deliver_after`] to schedule deferred
    /// delivery.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        stream_id: StreamId,
        process_id: ProcessId,
        tenant_id: TenantId,
        correlation_id: CorrelationId,
        conversation_id: ConversationId,
        causation_event_id: EventId,
        message_type: impl Into<Box<str>>,
        recipient: impl Into<Box<str>>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            message_id: OutboxMessageId::new(),
            stream_id,
            process_id,
            tenant_id,
            correlation_id,
            conversation_id,
            causation_event_id,
            message_type: message_type.into(),
            recipient: recipient.into(),
            payload,
            payload_schema: None,
            created_at: OffsetDateTime::now_utc(),
            deliver_after: None,
            attempt_count: 0,
            workflow_name: "".into(),
            trace_context: crate::trace_ctx::current().map(Into::into),
        }
    }

    /// Set a deferred delivery time.
    ///
    /// The message will not appear in [`OutboxStore::pending`] results until
    /// `now >= deliver_after`.
    #[must_use]
    pub fn with_deliver_after(mut self, deliver_after: OffsetDateTime) -> Self {
        self.deliver_after = Some(deliver_after);
        self
    }
}

// ── OutboxStore ───────────────────────────────────────────────────────────────

/// Storage contract for outbox messages.
///
/// ## Atomicity requirement
///
/// In production deployments, calls to [`OutboxStore::enqueue`] MUST be
/// atomic with the corresponding [`EventStore::append`] — both writes MUST
/// succeed or both MUST fail. Implement this by sharing the same database
/// transaction across both operations.
///
/// ## Delivery worker contract
///
/// The delivery worker loop should:
/// 1. Call [`OutboxStore::pending_now`] to retrieve ready messages.
/// 2. Attempt delivery to the AS4 endpoint.
/// 3. On success: call [`OutboxStore::acknowledge`] to remove the message.
/// 4. On transient failure: call [`OutboxStore::reschedule`] with an
///    exponential back-off delay.
///
/// ## Blanket `Arc` implementation
///
/// `Arc<S>` implements `OutboxStore` whenever `S: OutboxStore`, so you can
/// share a store across a delivery worker and command handlers without
/// additional wrapper types.
///
/// [`EventStore::append`]: crate::event_store::EventStore::append
#[allow(async_fn_in_trait)]
pub trait OutboxStore: Send + Sync {
    /// Persist `messages` durably, ready for delivery.
    ///
    /// In a persistent backend this MUST be called within the same
    /// transaction as the event append.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Outbox`] on storage failure.
    #[must_use = "dropping an enqueue Result silently loses outbound EDIFACT messages"]
    async fn enqueue(&self, messages: &[OutboxMessage]) -> Result<(), EngineError>;

    /// Return up to `limit` messages ready for delivery as of `now`.
    ///
    /// A message is ready when `deliver_after` is `None` or `<= now`.
    /// Results are ordered **oldest-first** by `created_at`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Outbox`] on storage failure.
    #[must_use = "dropping a pending Result silently discards outbox delivery work"]
    async fn pending(
        &self,
        limit: usize,
        now: OffsetDateTime,
    ) -> Result<Vec<OutboxMessage>, EngineError>;

    /// Return up to `limit` messages ready for delivery right now.
    ///
    /// Convenience wrapper around [`OutboxStore::pending`] that uses
    /// `OffsetDateTime::now_utc()` as the reference time.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Outbox`] on storage failure.
    async fn pending_now(&self, limit: usize) -> Result<Vec<OutboxMessage>, EngineError> {
        self.pending(limit, OffsetDateTime::now_utc()).await
    }

    /// Remove a message from the outbox after successful delivery.
    ///
    /// Calling this with an unknown `id` is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Outbox`] on storage failure.
    #[must_use = "dropping an acknowledge Result silently hides a store error"]
    async fn acknowledge(&self, id: OutboxMessageId) -> Result<(), EngineError>;

    /// Reschedule a message for a future delivery attempt.
    ///
    /// Implementations MUST increment `attempt_count` on the stored record.
    /// Calling this with an unknown `id` is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Outbox`] on storage failure.
    #[must_use = "dropping a reschedule Result silently hides a store error"]
    async fn reschedule(
        &self,
        id: OutboxMessageId,
        deliver_after: OffsetDateTime,
    ) -> Result<(), EngineError>;

    /// Return the total number of messages currently in the outbox.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Outbox`] on storage failure.
    #[must_use = "dropping a len Result silently discards a store error"]
    async fn len(&self) -> Result<usize, EngineError>;

    /// Return `true` when the outbox contains no messages.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Outbox`] on storage failure.
    async fn is_empty(&self) -> Result<bool, EngineError> {
        Ok(self.len().await? == 0)
    }
}

// ── Arc<S> blanket impl ───────────────────────────────────────────────────────

impl<S: OutboxStore> OutboxStore for Arc<S> {
    async fn enqueue(&self, messages: &[OutboxMessage]) -> Result<(), EngineError> {
        self.as_ref().enqueue(messages).await
    }

    async fn pending(
        &self,
        limit: usize,
        now: OffsetDateTime,
    ) -> Result<Vec<OutboxMessage>, EngineError> {
        self.as_ref().pending(limit, now).await
    }

    async fn acknowledge(&self, id: OutboxMessageId) -> Result<(), EngineError> {
        self.as_ref().acknowledge(id).await
    }

    async fn reschedule(
        &self,
        id: OutboxMessageId,
        deliver_after: OffsetDateTime,
    ) -> Result<(), EngineError> {
        self.as_ref().reschedule(id, deliver_after).await
    }

    async fn len(&self) -> Result<usize, EngineError> {
        self.as_ref().len().await
    }
}

// ── NoopOutboxStore ───────────────────────────────────────────────────────────

/// An [`OutboxStore`] that silently discards all messages.
///
/// Every `enqueue` succeeds without storing anything. `pending` always
/// returns an empty list. Use this as the default when outbox delivery is
/// managed elsewhere or not required.
///
/// # ⚠️ Data loss warning
///
/// `NoopOutboxStore` **discards every outbound message silently**. No APERAK,
/// MSCONS, or UTILMD will ever be delivered to the AS4 endpoint. Do not use
/// in production.
///
/// This type is available in all build configurations so it can serve as a
/// default type parameter in [`EngineBuilder`]. However, `EngineBuilder::new`
/// (which wires this as the default) is only available with the `testing`
/// feature or in `cfg(test)`. Production code must call
/// [`EngineBuilder::with_stores`] instead.
///
/// [`EngineBuilder`]: crate::builder::EngineBuilder
/// [`EngineBuilder::with_stores`]: crate::builder::EngineBuilder::with_stores
#[derive(Debug, Clone, Copy, Default)]
#[must_use = "NoopOutboxStore discards all outbound messages silently — use a persistent OutboxStore in production"]
#[cfg_attr(
    not(any(test, feature = "testing")),
    deprecated = "NoopOutboxStore must not be instantiated in production builds; use a durable OutboxStore instead"
)]
pub struct NoopOutboxStore;

#[cfg(any(test, feature = "testing"))]
impl OutboxStore for NoopOutboxStore {
    async fn enqueue(&self, _messages: &[OutboxMessage]) -> Result<(), EngineError> {
        Ok(())
    }

    async fn pending(
        &self,
        _limit: usize,
        _now: OffsetDateTime,
    ) -> Result<Vec<OutboxMessage>, EngineError> {
        Ok(Vec::new())
    }

    async fn acknowledge(&self, _id: OutboxMessageId) -> Result<(), EngineError> {
        Ok(())
    }

    async fn reschedule(
        &self,
        _id: OutboxMessageId,
        _deliver_after: OffsetDateTime,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn len(&self) -> Result<usize, EngineError> {
        Ok(0)
    }
}

// ── InMemoryOutboxStore ───────────────────────────────────────────────────────

/// An in-memory [`OutboxStore`] for tests and development.
///
/// Backed by a `HashMap` protected by a `RwLock`. Cloning shares the
/// underlying data via `Arc` — all clones see the same outbox state.
///
/// **Not production-safe.** Use this for:
/// - Unit and integration tests
/// - Local development and examples
/// - Verifying the outbox delivery loop without an external message broker
///
/// Only available in `#[cfg(test)]` or with the `testing` feature enabled.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default, Clone)]
pub struct InMemoryOutboxStore {
    inner: Arc<RwLock<HashMap<OutboxMessageId, OutboxMessage>>>,
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryOutboxStore {
    /// Create an empty outbox store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "testing"))]
impl OutboxStore for InMemoryOutboxStore {
    async fn enqueue(&self, messages: &[OutboxMessage]) -> Result<(), EngineError> {
        let mut map = self.inner.write().await;
        for msg in messages {
            map.insert(msg.message_id, msg.clone());
        }
        Ok(())
    }

    async fn pending(
        &self,
        limit: usize,
        now: OffsetDateTime,
    ) -> Result<Vec<OutboxMessage>, EngineError> {
        let map = self.inner.read().await;
        let mut ready: Vec<_> = map
            .values()
            .filter(|m| m.deliver_after.is_none_or(|d| d <= now))
            .cloned()
            .collect();
        // Stable ordering: oldest first so the delivery worker processes in
        // creation order, preserving causal ordering across messages.
        ready.sort_by_key(|m| m.created_at);
        ready.truncate(limit);
        Ok(ready)
    }

    async fn acknowledge(&self, id: OutboxMessageId) -> Result<(), EngineError> {
        self.inner.write().await.remove(&id);
        Ok(())
    }

    async fn reschedule(
        &self,
        id: OutboxMessageId,
        deliver_after: OffsetDateTime,
    ) -> Result<(), EngineError> {
        let mut map = self.inner.write().await;
        if let Some(msg) = map.get_mut(&id) {
            msg.deliver_after = Some(deliver_after);
            msg.attempt_count += 1;
        }
        Ok(())
    }

    async fn len(&self) -> Result<usize, EngineError> {
        Ok(self.inner.read().await.len())
    }
}

// ── Outbox idempotency key ────────────────────────────────────────────────────

/// Compute a deterministic idempotency key for an outbound message.
///
/// The key is a UUID v5 (SHA-1 over a stable namespace) derived from the
/// combination of process id, workflow step name, recipient partner id, and
/// format version. Identical inputs always produce the same UUID.
///
/// # Usage
///
/// Store the key alongside the outbox entry and use it as a unique constraint
/// in persistent backends so that re-dispatching the same command (e.g. after
/// a retry) does not produce duplicate outbound messages:
///
/// ```rust
/// use mako_engine::outbox::outbox_idempotency_key;
/// use mako_engine::ids::ProcessId;
///
/// let process_id = ProcessId::new();
/// let key = outbox_idempotency_key(process_id, "DispatchAperak", "4012345000023", "FV2025-10-01");
/// println!("idempotency key: {key}");
/// ```
///
/// # Key derivation
///
/// The key is `UUID_v5(MAKO_ENGINE_OUTBOX_NS, "{process_id}|{step}|{partner}|{fv}")`.
///
/// `MAKO_ENGINE_OUTBOX_NS` is a fixed namespace UUID (RFC 4122 §4.3, SHA-1
/// variant) that scopes all mako-engine outbox keys to avoid collisions with
/// UUIDs from other namespaces.
#[must_use]
pub fn outbox_idempotency_key(
    process_id: ProcessId,
    step: &str,
    recipient: &str,
    format_version: &str,
) -> uuid::Uuid {
    // A fixed namespace UUID for mako-engine outbox keys.
    // Generated once by uuid::Uuid::new_v4() and hardcoded for stability.
    // Changing this constant invalidates all existing keys — treat as immutable.
    const MAKO_ENGINE_OUTBOX_NS: uuid::Uuid = uuid::Uuid::from_bytes([
        0xd4, 0x7a, 0x2c, 0x9e, 0x5b, 0x31, 0x47, 0xf2, 0x89, 0x0a, 0x1e, 0x6c, 0x8a, 0x3d, 0x5f,
        0x04,
    ]);
    let name = format!("{process_id}|{step}|{recipient}|{format_version}");
    uuid::Uuid::new_v5(&MAKO_ENGINE_OUTBOX_NS, name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ConversationId, CorrelationId, EventId, ProcessId, TenantId};

    fn make_msg() -> OutboxMessage {
        OutboxMessage::new(
            StreamId::new("process/test"),
            ProcessId::new(),
            TenantId::new(),
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            "APERAK",
            "4012345000023",
            serde_json::json!({"positive": true}),
        )
    }

    #[tokio::test]
    async fn enqueue_appears_in_pending() {
        let store = InMemoryOutboxStore::new();
        let msg = make_msg();
        let id = msg.message_id;

        store.enqueue(&[msg]).await.unwrap();

        assert_eq!(store.len().await.unwrap(), 1);
        let pending = store.pending_now(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message_id, id);
    }

    #[tokio::test]
    async fn acknowledge_removes_message() {
        let store = InMemoryOutboxStore::new();
        let msg = make_msg();
        let id = msg.message_id;

        store.enqueue(&[msg]).await.unwrap();
        store.acknowledge(id).await.unwrap();

        assert!(store.is_empty().await.unwrap());
    }

    #[tokio::test]
    async fn deferred_message_not_in_pending_yet() {
        let store = InMemoryOutboxStore::new();
        let future = OffsetDateTime::now_utc() + time::Duration::hours(1);
        let msg = make_msg().with_deliver_after(future);

        store.enqueue(&[msg]).await.unwrap();

        let pending = store.pending_now(10).await.unwrap();
        assert!(
            pending.is_empty(),
            "deferred message must not appear before its time"
        );
    }

    #[tokio::test]
    async fn deferred_message_appears_after_deadline() {
        let store = InMemoryOutboxStore::new();
        let past = OffsetDateTime::now_utc() - time::Duration::seconds(1);
        let msg = make_msg().with_deliver_after(past);

        store.enqueue(&[msg]).await.unwrap();

        let pending = store.pending_now(10).await.unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn reschedule_increments_attempt_count() {
        let store = InMemoryOutboxStore::new();
        let msg = make_msg();
        let id = msg.message_id;
        let new_time = OffsetDateTime::now_utc() + time::Duration::minutes(5);

        store.enqueue(&[msg]).await.unwrap();
        store.reschedule(id, new_time).await.unwrap();

        let inner = store.inner.read().await;
        let stored = inner.get(&id).unwrap();
        assert_eq!(stored.attempt_count, 1);
        assert_eq!(stored.deliver_after, Some(new_time));
    }

    #[tokio::test]
    async fn pending_ordered_oldest_first() {
        let store = InMemoryOutboxStore::new();
        store.enqueue(&[make_msg()]).await.unwrap();
        store.enqueue(&[make_msg()]).await.unwrap();

        let pending = store.pending_now(10).await.unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending[0].created_at <= pending[1].created_at);
    }

    #[test]
    fn outbox_idempotency_key_is_stable_and_deterministic() {
        let pid = ProcessId::new();
        let step = "ReceiveAperak";
        let partner = "4012345000023";
        let fv = "FV2025-10-01";

        let k1 = outbox_idempotency_key(pid, step, partner, fv);
        let k2 = outbox_idempotency_key(pid, step, partner, fv);
        assert_eq!(k1, k2, "same inputs must produce the same key");
        assert_eq!(k1.to_string().len(), 36, "UUID string is 36 chars");

        // Different step → different key.
        let k3 = outbox_idempotency_key(pid, "ReceiveContrl", partner, fv);
        assert_ne!(k1, k3, "different step must produce different key");

        // Different FV → different key.
        let k4 = outbox_idempotency_key(pid, step, partner, "FV2026-10-01");
        assert_ne!(k1, k4, "different FV must produce different key");
    }
}
