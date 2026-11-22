//! The [`EventEnvelope`] — the standard wrapper for every persisted event.
//!
//! Infrastructure metadata (routing, tracing, auditing) lives in the envelope.
//! The business payload is opaque JSON, allowing each domain crate to own its
//! own schema without leaking it into the engine layer.
//!
//! # Write / read split
//!
//! - [`NewEvent`] is what the caller submits to [`AtomicAppend::append`][crate::event_store::AtomicAppend::append]. It
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

/// What an event says about the market message it is — for a reader's
/// history of a process, never for deciding anything.
///
/// Stated by [`EventPayload::verlauf`](crate::workflow::EventPayload::verlauf)
/// and stamped on the envelope at append, so a history read needs no
/// workflow's types. An event that is no message states nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Verlauf {
    /// Whether the message arrived or was sent.
    pub richtung: Richtung,
    /// The message's Prüfidentifikator, where it carries one.
    pub pid: Option<u32>,
    /// The message's document number (`BGM` C106), where known.
    pub message_ref: Option<String>,
}

/// The direction of a market message, as the operator's side sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Richtung {
    /// Received from a counterparty.
    Eingang,
    /// Sent to a counterparty.
    Ausgang,
}

/// The answer a market message gives: its code and the list that defines it.
///
/// A code means something only in the Entscheidungsbaum or Codeliste that
/// publishes it — `A02` is four unrelated refusals across `E_0607`, `E_0622`,
/// `E_0249` and `E_0250` — so the two travel together. Stated by
/// [`EventPayload::antwort`](crate::workflow::EventPayload::antwort) and
/// stamped on the envelope beside the [`Verlauf`], so a history read can resolve
/// the meaning without any workflow's types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Antwortcode {
    /// DE 9013 — the code (`A02`, `E15`, `Z12`, …).
    pub code: String,
    /// DE 1131 / DE 1082 — the Codeliste on the wire (`E_0622`, `S_0090`,
    /// `G_0051`), or the tree the workflow answers from; `None` where the
    /// message names neither.
    pub codeliste: Option<String>,
}

impl Antwortcode {
    /// A code and the list it is drawn from.
    #[must_use]
    pub fn new(code: impl Into<String>, codeliste: Option<impl Into<String>>) -> Self {
        Self {
            code: code.into(),
            codeliste: codeliste.map(Into::into),
        }
    }
}

/// Who caused an event — stamped on every envelope, with no default.
///
/// The fact every figure about intervention counts over: an event appended
/// without it cannot exist, so a process's history always says whether a
/// rule, an inbound message or a timer acted, or a person. No variant lets an
/// agent act alone — an agent only proposes, and a person approves.
///
/// The principal is personal data: it stays in the event store and leaves
/// only as its [`Eingriffsklasse`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "art", rename_all = "snake_case")]
pub enum Akteur {
    /// An inbound message, a fired deadline or a module's own follow-up.
    System,
    /// A person acting directly — a command sent from the Desk or the API.
    Person {
        /// The authenticated principal (pseudonymous: the OIDC `sub` or a
        /// configured name).
        principal: String,
    },
    /// A person approving a proposal, relayed by the service that holds the
    /// proposal.
    Freigabe {
        /// The approving principal.
        principal: String,
        /// Where the approved proposal came from.
        vorschlag: Vorschlag,
        /// Whether the approved body equals the proposed one.
        unveraendert: bool,
        /// The authenticated service that relayed the approval — set by the
        /// door from the caller's own identity, never from what the caller
        /// asserts.
        dienst: String,
    },
}

/// What a service asserts when it relays a person's approval
/// (`Mako-Akteur`): the approval alone. The relaying service is the door's to
/// name ([`Freigabeauftrag::durch`]), so an assertion cannot carry it, and
/// an unknown field is refused rather than dropped.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Freigabeauftrag {
    /// The approving principal.
    pub principal: String,
    /// Where the approved proposal came from.
    pub vorschlag: Vorschlag,
    /// Whether the approved body equals the proposed one.
    pub unveraendert: bool,
}

/// The longest principal or proposal name an assertion may carry.
pub const MAX_NAME_LEN: usize = 256;

impl Freigabeauftrag {
    /// The actor of an approval `dienst` relayed.
    ///
    /// # Errors
    ///
    /// An empty or over-long ([`MAX_NAME_LEN`]) principal or proposal name.
    pub fn durch(self, dienst: &str) -> Result<Akteur, &'static str> {
        let name = match &self.vorschlag {
            Vorschlag::Regel { name } => name,
            Vorschlag::Agent { skill } => skill,
        };
        for value in [&self.principal, name] {
            if value.trim().is_empty() {
                return Err("an approval names its principal and its proposal");
            }
            if value.len() > MAX_NAME_LEN {
                return Err("a principal or proposal name is longer than 256 bytes");
            }
        }
        Ok(Akteur::Freigabe {
            principal: self.principal,
            vorschlag: self.vorschlag,
            unveraendert: self.unveraendert,
            dienst: dienst.to_owned(),
        })
    }
}

impl Akteur {
    /// The class an outbound message and the KPI report carry — never the
    /// principal.
    #[must_use]
    pub fn klasse(&self) -> Eingriffsklasse {
        match self {
            Self::System => Eingriffsklasse::System,
            Self::Person { .. } => Eingriffsklasse::Person,
            Self::Freigabe {
                unveraendert: true, ..
            } => Eingriffsklasse::Freigabe,
            Self::Freigabe {
                unveraendert: false,
                ..
            } => Eingriffsklasse::FreigabeGeaendert,
        }
    }
}

/// The origin of a proposal a person approved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "art", rename_all = "snake_case", deny_unknown_fields)]
pub enum Vorschlag {
    /// A tabulated rule (an Entscheidungsbaum answer, a cause table row).
    Regel {
        /// The rule's name.
        name: String,
    },
    /// An agent's draft.
    Agent {
        /// The skill that drafted it.
        skill: String,
    },
}

/// An actor's class, ordered from least to most human: a process is counted
/// by the greatest class among its events.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Eingriffsklasse {
    /// No person acted.
    #[serde(rename = "system")]
    System,
    /// A person approved a proposal unchanged.
    #[serde(rename = "freigabe")]
    Freigabe,
    /// A person approved a proposal after changing it.
    #[serde(rename = "freigabe-geaendert")]
    FreigabeGeaendert,
    /// A person acted directly.
    #[serde(rename = "person")]
    Person,
}

impl Eingriffsklasse {
    /// Every class, least human first.
    pub const ALL: [Self; 4] = [
        Self::System,
        Self::Freigabe,
        Self::FreigabeGeaendert,
        Self::Person,
    ];

    /// The wire name — the CloudEvent `makoakteur` extension value and the
    /// metric's `eingriff` label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Freigabe => "freigabe",
            Self::FreigabeGeaendert => "freigabe-geaendert",
            Self::Person => "person",
        }
    }

    /// The class a wire name names; `None` for any other string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

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
/// let new_event = ctx.new_event(&SupplierChangeEvent::Initiated { .. }, Akteur::System)?;
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
    /// The market message this event is, if it is one.
    pub verlauf: Option<Verlauf>,
    /// The answer the message gives, if it is an answer carrying a code.
    pub antwort: Option<Antwortcode>,
    /// Who caused the event.
    pub akteur: Akteur,
    /// The message passed between two of the deployment's own Marktrollen —
    /// delivered in-process, not over AS4.
    pub intern: bool,
}

impl NewEvent {
    /// Construct a `NewEvent` from its constituent parts.
    ///
    /// This is the escape hatch for callers that need full control over all
    /// fields (e.g. test scaffolding, migration tooling, storage-layer tests).
    /// In application code, prefer
    /// [`CommandContext::new_event`][crate::workflow::CommandContext::new_event]
    /// which propagates correlation metadata automatically.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a separate input of this operation; a parameter struct would exist for this one call"
    )]
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
        akteur: Akteur,
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
            verlauf: None,
            antwort: None,
            akteur,
            intern: false,
        }
    }

    /// The market message this event is.
    #[must_use]
    pub fn with_verlauf(mut self, verlauf: Verlauf) -> Self {
        self.verlauf = Some(verlauf);
        self
    }

    /// The answer the message gives.
    #[must_use]
    pub fn with_antwort(mut self, antwort: Antwortcode) -> Self {
        self.antwort = Some(antwort);
        self
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
/// It is assigned by the [`EventStore`] during [`AtomicAppend::append`] and
/// must not be set by callers.
///
/// ## Immutability
///
/// Once persisted, an envelope is never modified. Corrections are modelled as
/// new events.
///
/// [`EventStore`]: crate::event_store::EventStore
/// [`AtomicAppend::append`]: crate::event_store::AtomicAppend::append
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

    /// The market message this event is, if it is one — what a history
    /// shows. Absent on an event that is no message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verlauf: Option<Verlauf>,

    /// The answer the message gives — its code and Codeliste. Absent on an
    /// event that is no answer, or one whose message carries no code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antwort: Option<Antwortcode>,

    /// Who caused the event. Required: an envelope without it does not load.
    pub akteur: Akteur,
    /// The message passed between two of the deployment's own Marktrollen —
    /// delivered in-process, not over AS4.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub intern: bool,
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
            verlauf: new.verlauf,
            antwort: new.antwort,
            akteur: new.akteur,
            intern: new.intern,
        }
    }
}
