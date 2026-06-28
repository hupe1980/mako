//! The [`EventEnvelope`] — the standard wrapper for every persisted event.
//!
//! Infrastructure metadata (routing, tracing, auditing) lives in the envelope.
//! The business payload is opaque JSON, allowing each domain crate to own its
//! own schema without leaking it into the engine layer.
//!
//! # Write / read split
//!
//! - [`NewEvent`] is what the caller submits to [`EventStore::append`][crate::event_store::EventStore::append]. It
//!   contains everything the *caller* knows: context IDs, the typed payload.
//!   It intentionally **omits** `event_id`, `stream_id`, `sequence_number`,
//!   and `timestamp` — the store assigns those atomically during the append.
//!
//! - [`EventEnvelope`] is what the store returns and persists. It is the
//!   complete, immutable record.

use serde_json::Value;
use time::OffsetDateTime;

use crate::ids::{
    CausationId, ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId,
};
use crate::version::WorkflowId;

// ── NewEvent ──────────────────────────────────────────────────────────────────

/// A pending event ready to be appended to a stream.
///
/// The caller constructs a `NewEvent` for each domain event produced by a
/// workflow command. Fields that the store assigns (`event_id`,
/// `sequence_number`, `timestamp`, `stream_id`) are absent.
///
/// ## Idiomatic construction
///
/// Prefer [`CommandContext::new_event`][crate::workflow::CommandContext::new_event]
/// inside workflow handlers and transport adapters — it propagates all
/// correlation IDs from the command context automatically:
///
/// ```rust,ignore
/// // Inside a MessageAdapter or test:
/// let new_event = ctx.new_event(&SupplierChangeEvent::Initiated { .. })?;
/// store.append(&stream_id, ExpectedVersion::Any, &[new_event]).await?;
/// ```
///
/// Use [`EventEnvelope::new_caused_event`] when building a follow-up event
/// causally linked to a prior persisted event.
///
/// For test scaffolding that needs a `NewEvent` without a typed payload or
/// context, use [`NewEvent::new`].  The `#[non_exhaustive]` attribute
/// future-proofs callers against new optional fields being added without
/// requiring a semver-breaking change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NewEvent {
    /// Groups all events that originate from the same root command.
    pub correlation_id: CorrelationId,
    /// The event or command that directly caused this event, if any.
    pub causation_id: Option<CausationId>,
    /// Links events belonging to the same business conversation.
    pub conversation_id: ConversationId,
    /// Stable identifier for the MaKo process instance.
    pub process_id: ProcessId,
    /// Tenant that owns this event.
    pub tenant_id: TenantId,
    /// Workflow definition that produced this event.
    pub workflow_id: WorkflowId,
    /// Stable, human-readable type discriminant (e.g. `"SupplierChangeInitiated"`).
    pub event_type: Box<str>,
    /// Schema version of the serialized payload.
    pub schema_version: u32,
    /// The domain event payload, serialized as JSON.
    pub payload: Value,
}

impl NewEvent {
    /// Construct a `NewEvent` from its constituent parts.
    ///
    /// This is the escape hatch for callers that need full control over all
    /// fields (e.g. test scaffolding, migration tooling, storage-layer tests).
    /// In application code, prefer
    /// [`CommandContext::new_event`][crate::workflow::CommandContext::new_event]
    /// which propagates correlation metadata automatically.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        correlation_id: CorrelationId,
        causation_id: Option<CausationId>,
        conversation_id: ConversationId,
        process_id: ProcessId,
        tenant_id: TenantId,
        workflow_id: WorkflowId,
        event_type: impl Into<Box<str>>,
        schema_version: u32,
        payload: Value,
    ) -> Self {
        Self {
            correlation_id,
            causation_id,
            conversation_id,
            process_id,
            tenant_id,
            workflow_id,
            event_type: event_type.into(),
            schema_version,
            payload,
        }
    }
}

// ── EventEnvelope ─────────────────────────────────────────────────────────────

/// A single persisted event, wrapped in engine-level metadata.
///
/// The envelope separates **infrastructure concerns** (identity, ordering,
/// tracing) from **domain concerns** (the business event payload). The payload
/// is stored as [`serde_json::Value`] so the engine remains domain-agnostic.
///
/// ## Sequence numbers
///
/// `sequence_number` is 1-based and monotonically increasing **per stream**.
/// It is assigned by the [`EventStore`] during [`EventStore::append`] and
/// must not be set by callers.
///
/// ## Immutability
///
/// Once persisted, an envelope is never modified. Corrections are modelled as
/// new events.
///
/// [`EventStore`]: crate::event_store::EventStore
/// [`EventStore::append`]: crate::event_store::EventStore::append
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventEnvelope {
    /// Globally unique event instance identifier (assigned by store).
    pub event_id: EventId,

    /// The stream this event belongs to (e.g. `process/xxxxxxxx`).
    pub stream_id: StreamId,

    /// 1-based monotonic position within the stream (assigned by store).
    pub sequence_number: u64,

    /// Wall-clock time at which the event was appended to the store (assigned by store).
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,

    /// Groups all events that originate from the same root command.
    pub correlation_id: CorrelationId,

    /// The event or command that directly caused this event, if any.
    pub causation_id: Option<CausationId>,

    /// Links events belonging to the same business conversation
    /// (e.g. a UTILMD exchange and its APERAK acknowledgement).
    pub conversation_id: ConversationId,

    /// Stable identifier for the MaKo process instance that owns this stream.
    pub process_id: ProcessId,

    /// Tenant that owns this event (market participant or deployment tenant).
    pub tenant_id: TenantId,

    /// Identifies the workflow definition (name + BDEW format version) that
    /// produced this event.
    pub workflow_id: WorkflowId,

    /// Stable, human-readable type discriminant for the domain event
    /// (e.g. `"SupplierChangeInitiated"`).
    ///
    /// Used for projection routing and observability without deserializing
    /// the full payload.
    pub event_type: Box<str>,

    /// Schema version of the serialized payload. Increment when the payload
    /// structure changes to enable upcasting during replay.
    pub schema_version: u32,

    /// The domain event payload, serialized as JSON.
    pub payload: Value,
}

impl EventEnvelope {
    /// Deserialize the payload into a typed domain event.
    ///
    /// Clones the [`serde_json::Value`] payload and deserializes `T` from it
    /// directly.  Use [`EventEnvelope::decode_owned`] to consume the envelope
    /// and avoid the clone when the envelope is no longer needed.
    ///
    /// # Errors
    ///
    /// Returns a [`serde_json::Error`] when deserialization fails.
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.payload.clone())
    }

    /// Deserialize the payload into a typed domain event, consuming the
    /// envelope to avoid an extra clone.
    ///
    /// Prefer this over [`EventEnvelope::decode`] when you no longer need
    /// access to the envelope after decoding (e.g. in one-shot projection
    /// handlers that transform each event exactly once).
    ///
    /// # Errors
    ///
    /// Returns a [`serde_json::Error`] when deserialization fails.
    pub fn decode_owned<T: serde::de::DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.payload)
    }

    /// Construct an envelope from a [`NewEvent`] with store-assigned fields.
    ///
    /// Called internally by [`EventStore`] implementations during append.
    ///
    /// [`EventStore`]: crate::event_store::EventStore
    #[must_use]
    pub fn from_new(
        new: NewEvent,
        stream_id: StreamId,
        sequence_number: u64,
        timestamp: OffsetDateTime,
    ) -> Self {
        Self {
            event_id: EventId::new(),
            stream_id,
            sequence_number,
            timestamp,
            correlation_id: new.correlation_id,
            causation_id: new.causation_id,
            conversation_id: new.conversation_id,
            process_id: new.process_id,
            tenant_id: new.tenant_id,
            workflow_id: new.workflow_id,
            event_type: new.event_type,
            schema_version: new.schema_version,
            payload: new.payload,
        }
    }
}
