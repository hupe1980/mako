//! [`Workflow`] trait, [`EventPayload`], [`CommandPayload`], and [`CommandContext`].
//!
//! # Design contract
//!
//! Workflows are **pure state machines**:
//!
//! - [`Workflow::apply`] folds a domain event into the current state.
//! - [`Workflow::handle`] validates a command against the current state and
//!   returns the events to emit. It has no I/O, no side effects, and no
//!   clock access. The same state + command always produce the same events.
//!
//! All I/O (parsing raw bytes, calling external services) must happen
//! **before** the command is constructed and passed to the write path.
//! This keeps workflows deterministic and trivially replayable.
//!
//! # Serialization boundary
//!
//! Domain events must implement [`serde::Serialize`] and
//! [`serde::de::DeserializeOwned`] so the engine can persist them as JSON
//! inside [`EventEnvelope::payload`]. The [`EventPayload`] trait adds a
//! stable `event_type` discriminant for projection routing.
//!
//! # Write path
//!
//! The public write path is [`Process::execute`] / [`Process::execute_with`].
//! These delegate to the crate-internal `execute_command` function. Direct
//! use of `execute_command` is intentionally not part of the public API;
//! use [`Process`] instead.
//!
//! [`Process`]: crate::process::Process
//! [`Process::execute`]: crate::process::Process::execute
//! [`Process::execute_with`]: crate::process::Process::execute_with

use crate::{
    deadline::Deadline,
    envelope::{EventEnvelope, NewEvent},
    error::{EngineError, WorkflowError},
    event_store::{EventStore, ExpectedVersion},
    ids::{CausationId, ConversationId, CorrelationId, ProcessId, TenantId},
    outbox::PendingOutbox,
    version::{WorkflowId, WorkflowVersionPolicy},
};

// ── PendingDeadline ───────────────────────────────────────────────────────────

/// A deadline that a [`Workflow::handle`] function wishes to register,
/// expressed without the process-identity fields that are only known to the
/// engine's execution context.
///
/// The engine converts `PendingDeadline` into a fully-typed [`Deadline`] by
/// injecting `stream_id`, `process_id`, `tenant_id`, and `workflow_id` from
/// the active [`CommandContext`].  Workflows therefore stay pure (no I/O).
///
/// ## Usage
///
/// Return `PendingDeadline` inside [`WorkflowOutput`] when the command must
/// register a regulatory deadline alongside its events and outbox messages:
///
/// ```rust,ignore
/// use mako_engine::fristen::{APERAK_STROM_WINDOW_LABEL, aperak_strom_due_at};
/// use mako_engine::workflow::PendingDeadline;
///
/// let due = aperak_strom_due_at(received_at);
/// let dl = PendingDeadline::new(APERAK_STROM_WINDOW_LABEL, due);
/// Ok(WorkflowOutput::with_outbox_and_deadline(events, outbox, dl))
/// ```
///
/// [`Deadline`]: crate::deadline::Deadline
/// [`CommandContext`]: crate::workflow::CommandContext
#[derive(Debug, Clone)]
pub struct PendingDeadline {
    /// Deadline label (matches the `on_deadline` match arm in the workflow).
    pub label: String,
    /// Absolute UTC time at which the deadline fires.
    pub due_at: time::OffsetDateTime,
}

impl PendingDeadline {
    /// Create a new pending deadline with the given label and due time.
    #[must_use]
    pub fn new(label: impl Into<String>, due_at: time::OffsetDateTime) -> Self {
        Self {
            label: label.into(),
            due_at,
        }
    }
}

// ── WorkflowOutput ────────────────────────────────────────────────────────────

/// The combined output of [`Workflow::handle`]: domain events, optional
/// outbox messages, and optional deadlines, all atomically co-persisted.
///
/// Use [`WorkflowOutput::events`] or `From<Vec<E>>` when the command produces
/// only events (no outbox messages). This keeps existing `handle`
/// implementations concise: `Ok(vec![event].into())`.
///
/// When the command must also send an EDIFACT message, add the corresponding
/// [`PendingOutbox`] entries to `outbox`. The engine materialises them into
/// fully-typed [`OutboxMessage`] values with correct `causation_event_id` links
/// inside [`Process::execute_and_enqueue`].
///
/// When the command must register a regulatory deadline (e.g. APERAK 45-min
/// sending window per APERAK AHB 1.0 §2.4.1), add a [`PendingDeadline`].
/// The engine injects the process-identity fields from [`CommandContext`] and
/// persists the deadline atomically with the events.
///
/// [`OutboxMessage`]: crate::outbox::OutboxMessage
/// [`Process::execute_and_enqueue`]: crate::process::Process::execute_and_enqueue
#[derive(Debug, Clone)]
pub struct WorkflowOutput<E: EventPayload> {
    /// Domain events to persist in the event stream.
    pub events: Vec<E>,
    /// Outbox messages to enqueue atomically alongside the events.
    ///
    /// Empty in the vast majority of commands. Only non-empty when the command
    /// needs to trigger an outbound EDIFACT message (e.g. `DispatchAperak`).
    pub outbox: Vec<PendingOutbox>,
    /// Deadlines to register atomically alongside the events.
    ///
    /// Empty in most commands. Non-empty when the command starts a regulatory
    /// monitoring window (e.g. APERAK 45-min sending deadline).
    pub deadlines: Vec<PendingDeadline>,
}

impl<E: EventPayload> WorkflowOutput<E> {
    /// Construct an output with events and no outbox messages or deadlines.
    ///
    /// Equivalent to `events.into()`.
    #[must_use]
    pub fn events(events: Vec<E>) -> Self {
        Self {
            events,
            outbox: Vec::new(),
            deadlines: Vec::new(),
        }
    }

    /// Construct an output with both events and outbox messages.
    #[must_use]
    pub fn with_outbox(events: Vec<E>, outbox: Vec<PendingOutbox>) -> Self {
        Self {
            events,
            outbox,
            deadlines: Vec::new(),
        }
    }

    /// Construct an output with events, outbox messages, and a single deadline.
    #[must_use]
    pub fn with_outbox_and_deadline(
        events: Vec<E>,
        outbox: Vec<PendingOutbox>,
        deadline: PendingDeadline,
    ) -> Self {
        Self {
            events,
            outbox,
            deadlines: vec![deadline],
        }
    }

    /// Construct an output with events, outbox messages, and multiple deadlines.
    #[must_use]
    pub fn with_outbox_and_deadlines(
        events: Vec<E>,
        outbox: Vec<PendingOutbox>,
        deadlines: Vec<PendingDeadline>,
    ) -> Self {
        Self {
            events,
            outbox,
            deadlines,
        }
    }
}

impl<E: EventPayload> From<Vec<E>> for WorkflowOutput<E> {
    /// Convert a plain event list into a `WorkflowOutput` with no outbox or deadlines.
    ///
    /// Allows `handle` implementations to write `Ok(vec![…].into())` without
    /// constructing a `WorkflowOutput` explicitly.
    fn from(events: Vec<E>) -> Self {
        Self::events(events)
    }
}

impl<E: EventPayload> std::ops::Deref for WorkflowOutput<E> {
    type Target = [E];

    /// Deref to the events slice so callers can use `.len()`, indexing, and
    /// iteration on `WorkflowOutput` without destructuring.
    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

impl<E: EventPayload> IntoIterator for WorkflowOutput<E> {
    type Item = E;
    type IntoIter = std::vec::IntoIter<E>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.into_iter()
    }
}

impl<'a, E: EventPayload> IntoIterator for &'a WorkflowOutput<E> {
    type Item = &'a E;
    type IntoIter = std::slice::Iter<'a, E>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
    }
}

// ── EventPayload ──────────────────────────────────────────────────────────────

/// Marker trait for domain event types.
///
/// Implementors must be JSON-serializable and carry a stable `event_type`
/// string that the engine stores in [`EventEnvelope::event_type`] for
/// projection routing and observability.
///
/// # Example
///
/// ```rust,ignore
/// use mako_engine::workflow::EventPayload;
///
/// #[derive(serde::Serialize, serde::Deserialize)]
/// enum MyEvent { Created { name: String }, Closed }
///
/// impl EventPayload for MyEvent {
///     fn event_type(&self) -> &'static str {
///         match self {
///             Self::Created { .. } => "MyCreated",
///             Self::Closed       => "MyClosed",
///         }
///     }
/// }
/// ```
pub trait EventPayload:
    serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static
{
    /// A stable, unique name for this event variant.
    ///
    /// Used in [`EventEnvelope::event_type`]. Choose names that survive
    /// refactors (e.g. `"SupplierChangeInitiated"`, not `"Initiated"`).
    fn event_type(&self) -> &'static str;

    /// Schema version of this event's payload layout.
    ///
    /// Increment when the serialized payload structure changes in a
    /// backward-incompatible way. The engine stamps this value into
    /// [`EventEnvelope::schema_version`] so replay and upcasting tooling
    /// can identify which decoder to use.
    ///
    /// Defaults to `1`.
    fn schema_version(&self) -> u32 {
        1
    }
}

// ── CommandPayload ────────────────────────────────────────────────────────────

/// Marker trait for domain command types.
///
/// Commands are transient — they are never persisted. Only `Send + 'static` is
/// required.
pub trait CommandPayload: Send + 'static {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// A versioned, deterministic domain workflow.
///
/// Workflows are the unit of business logic in the engine. Each BDEW process
/// variant (e.g. GPKE Lieferbeginn, WiM Gerätewechsel) is a separate
/// `Workflow` implementation in its domain crate.
///
/// ## State reconstruction
///
/// Before handling a command, the engine calls [`Workflow::apply`] on every
/// event in the stream to reconstruct the current state. This is the only
/// path to reading state — there is no "load current state" API.
///
/// ## Determinism
///
/// `handle` and `apply` must be deterministic and free of side effects. Do not
/// access clocks, RNGs, network, or file system inside them.
pub trait Workflow: Send + Sync + 'static {
    /// Domain-specific process state, reconstructed by replaying events.
    type State: Default + Clone + Send + Sync + 'static;

    /// Domain event type emitted by this workflow.
    type Event: EventPayload;

    /// Command type handled by this workflow.
    type Command: CommandPayload;

    /// Fold a domain event into the current state.
    ///
    /// This function must be total (no panics, no errors) and must produce a
    /// deterministic result.
    fn apply(state: Self::State, event: &Self::Event) -> Self::State;

    /// Validate `command` against `state` and return the events to emit.
    ///
    /// Return an empty [`WorkflowOutput`] (or `vec![].into()`) when the
    /// command is a no-op (already processed).
    ///
    /// Outbox messages in the returned [`WorkflowOutput::outbox`] will be
    /// atomically co-persisted with the events when the command is dispatched
    /// via [`Process::execute_and_enqueue`]. If dispatched via
    /// [`Process::execute`], the outbox field is silently ignored.
    ///
    /// # Errors
    ///
    /// Return a [`WorkflowError`] when the command is invalid for the current
    /// state or when domain validation fails.
    ///
    /// [`Process::execute`]: crate::process::Process::execute
    /// [`Process::execute_and_enqueue`]: crate::process::Process::execute_and_enqueue
    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError>;

    /// Schema version for serialized `Workflow::State` payloads.
    ///
    /// The engine stores this value in every [`Snapshot`] taken via
    /// [`Process::take_snapshot`]. Increment it when the serialized state
    /// layout changes in a backward-incompatible way, and add a migration
    /// arm to your snapshot loader.
    ///
    /// Defaults to `1`.
    ///
    /// [`Snapshot`]: crate::snapshot::Snapshot
    /// [`Process::take_snapshot`]: crate::process::Process::take_snapshot
    #[must_use]
    fn state_schema_version() -> u32 {
        1
    }

    /// Upcast a stored event payload from an older schema version.
    ///
    /// The engine calls this during state reconstruction for every loaded
    /// event, *before* deserializing the payload into `Self::Event`. The
    /// returned [`serde_json::Value`] is passed to the standard JSON
    /// deserializer.
    ///
    /// Override this when you bump [`EventPayload::schema_version`] on a
    /// variant — return a `Value` compatible with the new schema so old
    /// events replay correctly without a data migration.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// fn upcast(
    ///     event_type: &str,
    ///     from_version: u32,
    ///     mut payload: serde_json::Value,
    /// ) -> Result<serde_json::Value, EngineError> {
    ///     // v2 of SupplierChangeInitiated added a `document_type` field.
    ///     if event_type == "SupplierChangeInitiated" && from_version == 1 {
    ///         payload["document_type"] = serde_json::json!("E01");
    ///     }
    ///     Ok(payload)
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Return [`EngineError::Deserialization`] when the payload cannot be
    /// migrated to the current schema.
    fn upcast(
        _event_type: &str,
        _from_version: u32,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, EngineError> {
        Ok(payload)
    }

    /// Declares which BDEW format versions this workflow accepts for in-flight
    /// processes.
    ///
    /// The engine uses this policy to validate that an incoming message's
    /// format version is acceptable *before* constructing the command, surfacing
    /// missing adapter coverage at dispatch time rather than during runtime
    /// deserialization.
    ///
    /// The default returns [`WorkflowVersionPolicy::ForwardCompatible`] —
    /// accept messages in any format version.  This is the safe default for
    /// the majority of BDEW market-communication processes, which routinely
    /// span annual release boundaries (e.g. a GPKE Lieferbeginn process
    /// started in September may still receive APERAK replies in November under
    /// the new October FV).
    ///
    /// Override to `Pinned` only for strictly short-lived workflows that are
    /// guaranteed to complete within a single BDEW release cycle.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use mako_engine::version::WorkflowVersionPolicy;
    ///
    /// // Override to Pinned for a workflow with a 24h wall-clock SLA:
    /// fn version_policy() -> WorkflowVersionPolicy {
    ///     WorkflowVersionPolicy::Pinned
    /// }
    /// ```
    #[must_use]
    fn version_policy() -> WorkflowVersionPolicy {
        WorkflowVersionPolicy::ForwardCompatible
    }

    /// Map a fired deadline to a compensating command.
    ///
    /// Called by [`Process::execute_timeout`] when a registered deadline
    /// for this workflow's process becomes overdue. Return `Some(command)`
    /// to trigger a compensating action; return `None` to acknowledge the
    /// deadline as a no-op.
    ///
    /// This method must be **pure**: no I/O, no clock access, no global state.
    /// The same `(deadline, state)` must always produce the same `Option<Command>`.
    ///
    /// The full [`Deadline`] is provided (not just the label) so implementations
    /// can construct commands that require `deadline_id` (e.g. `TimeoutExpired`).
    ///
    /// # Why a dedicated hook instead of a normal command?
    ///
    /// A synthetic `TimeoutFired` command variant works but couples the workflow
    /// enum to infrastructure concerns. `on_deadline` keeps the domain command
    /// type clean and makes compensation logic explicit and testable in isolation:
    ///
    /// ```rust,ignore
    /// fn on_deadline(
    ///     deadline: &Deadline,
    ///     state: &Self::State,
    /// ) -> Option<Self::Command> {
    ///     match (deadline.label(), state) {
    ///         ("aperak-window", SupplierChangeState::Initiated(_) | SupplierChangeState::ValidationPassed(_)) => {
    ///             Some(SupplierChangeCommand::TimeoutExpired {
    ///                 deadline_id: deadline.deadline_id(),
    ///                 label: deadline.label().into(),
    ///             })
    ///         }
    ///         _ => None,
    ///     }
    /// }
    /// ```
    ///
    /// # Default
    ///
    /// Returns `None` for all deadlines — no automatic compensation. Override in
    /// any workflow that has deadline-triggered compensation requirements.
    ///
    /// [`Process::execute_timeout`]: crate::process::Process::execute_timeout
    fn on_deadline(_deadline: &Deadline, _state: &Self::State) -> Option<Self::Command> {
        None
    }
}

// ── CommandContext ─────────────────────────────────────────────────────────────

/// Contextual metadata attached to every command dispatch.
///
/// The engine stamps this information onto every event produced by the command.
/// Callers provide the process identity; the engine generates correlation IDs
/// automatically unless provided explicitly.
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// See [`CorrelationId`].
    pub correlation_id: CorrelationId,
    /// See [`ConversationId`].
    pub conversation_id: ConversationId,
    /// The MaKo process instance this command targets.
    pub process_id: ProcessId,
    /// The tenant that issued this command.
    pub tenant_id: TenantId,
    /// The workflow version to use for processing.
    pub workflow_id: WorkflowId,
    /// The immediate cause of this command, if driven by a prior event.
    pub causation_id: Option<CausationId>,
}

impl CommandContext {
    /// Construct a context with auto-generated correlation and conversation IDs.
    #[must_use]
    pub fn new(tenant_id: TenantId, process_id: ProcessId, workflow_id: WorkflowId) -> Self {
        Self {
            correlation_id: CorrelationId::new(),
            conversation_id: ConversationId::new(),
            process_id,
            tenant_id,
            workflow_id,
            causation_id: None,
        }
    }

    /// Set an explicit causation ID (e.g. the ID of the event that triggered
    /// this command).
    #[must_use]
    pub fn with_causation(mut self, id: CausationId) -> Self {
        self.causation_id = Some(id);
        self
    }

    /// Override the auto-generated correlation ID.
    ///
    /// Use this to propagate a correlation ID from an inbound EDIFACT message
    /// so all resulting events share the same root correlation.
    #[must_use]
    pub fn with_correlation(mut self, id: CorrelationId) -> Self {
        self.correlation_id = id;
        self
    }

    /// Override the auto-generated conversation ID.
    ///
    /// Use this to link the outbound APERAK to the same conversation as the
    /// UTILMD that triggered it, so the full message exchange is traceable as
    /// a unit.
    #[must_use]
    pub fn with_conversation(mut self, id: ConversationId) -> Self {
        self.conversation_id = id;
        self
    }

    /// Build a context that is causally linked to a prior persisted event.
    ///
    /// Propagates `correlation_id`, `conversation_id`, `process_id`, and
    /// `tenant_id` from the envelope and sets the envelope's `event_id` as
    /// the `causation_id`. This is the canonical constructor for all commands
    /// that are triggered by a prior event (e.g. dispatching an APERAK in
    /// response to a received UTILMD).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ctx = CommandContext::from_envelope(&utilmd_envelope, workflow_id);
    /// process.execute_with(DispatchAperak { positive: true, reason: None }, ctx).await?;
    /// ```
    #[must_use]
    pub fn from_envelope(env: &EventEnvelope, workflow_id: WorkflowId) -> Self {
        Self {
            correlation_id: env.correlation_id,
            conversation_id: env.conversation_id,
            process_id: env.process_id,
            tenant_id: env.tenant_id,
            workflow_id,
            causation_id: Some(env.event_id.into()),
        }
    }

    /// Build a context for a deadline-triggered command.
    ///
    /// Propagates `process_id` and `tenant_id` from the deadline. Generates
    /// fresh `correlation_id` and `conversation_id` (deadline firings start
    /// a new tracing root).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ctx = CommandContext::from_deadline(&overdue_deadline, workflow_id);
    /// process.execute_with(HandleTimeout { label: overdue_deadline.label().into() }, ctx).await?;
    /// ```
    #[must_use]
    pub fn from_deadline(deadline: &crate::deadline::Deadline, workflow_id: WorkflowId) -> Self {
        Self::new(deadline.tenant_id(), deadline.process_id(), workflow_id)
    }

    /// Build a [`NewEvent`] from this context and a domain event payload.
    ///
    /// This is the canonical way to construct a `NewEvent` inside a transport
    /// adapter or test helper — it eliminates the nine-argument [`NewEvent::new`]
    /// call and ensures that correlation metadata is always propagated correctly.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Serialization`] when the event payload cannot be
    /// serialized to JSON.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Inside a MessageAdapter or test:
    /// let new_event = ctx.new_event(&SupplierChangeEvent::Activated)?;
    /// store.append(&stream_id, ExpectedVersion::Any, &[new_event]).await?;
    /// ```
    pub fn new_event<E: EventPayload>(&self, event: &E) -> Result<NewEvent, EngineError> {
        let payload =
            serde_json::to_value(event).map_err(|e| EngineError::Serialization(e.to_string()))?;
        Ok(NewEvent {
            correlation_id: self.correlation_id,
            causation_id: self.causation_id,
            conversation_id: self.conversation_id,
            process_id: self.process_id,
            tenant_id: self.tenant_id,
            workflow_id: self.workflow_id.clone(),
            event_type: event.event_type().into(),
            schema_version: event.schema_version(),
            payload,
        })
    }
}

// ── EventEnvelope convenience ──────────────────────────────────────────────────

impl EventEnvelope {
    /// Build a [`NewEvent`] causally linked to this envelope.
    ///
    /// Propagates `correlation_id`, `conversation_id`, `process_id`, and
    /// `tenant_id` from the envelope and sets `envelope.event_id` as the
    /// `causation_id` of the new event. Useful when generating a follow-up
    /// event (e.g. an APERAK trigger event) that must be traceable back to
    /// the UTILMD envelope that caused it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Serialization`] when the event payload cannot be
    /// serialized to JSON.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // After persisting a UTILMD receive event, trigger an APERAK:
    /// let aperak_new = utilmd_envelope.new_caused_event(
    ///     workflow_id,
    ///     &SupplierChangeEvent::AperakDispatched { positive: true, reason: None },
    /// )?;
    /// ```
    pub fn new_caused_event<E: EventPayload>(
        &self,
        workflow_id: WorkflowId,
        event: &E,
    ) -> Result<NewEvent, EngineError> {
        let payload =
            serde_json::to_value(event).map_err(|e| EngineError::Serialization(e.to_string()))?;
        Ok(NewEvent {
            correlation_id: self.correlation_id,
            causation_id: Some(self.event_id.into()),
            conversation_id: self.conversation_id,
            process_id: self.process_id,
            tenant_id: self.tenant_id,
            workflow_id,
            event_type: event.event_type().into(),
            schema_version: event.schema_version(),
            payload,
        })
    }
}

// ── execute_command ───────────────────────────────────────────────────────────

/// Dispatch a command through a workflow and persist the resulting events.
///
/// This is the crate-internal write-path entry point. The public API is
/// [`Process::execute`] / [`Process::execute_with`].
///
/// It performs, in order:
///
/// 1. **Load** all events from `stream_id` via `store`.
/// 2. **Reconstruct state** by folding events through [`Workflow::apply`].
/// 3. **Handle** the command via [`Workflow::handle`] (pure, no I/O).
/// 4. **Build** [`NewEvent`] values from each domain event + `ctx`.
/// 5. **Append** atomically with optimistic concurrency
///    (`ExpectedVersion::Exact(current_sequence)`).
///
/// Returns the persisted envelopes (with store-assigned IDs and sequence
/// numbers). Returns an empty `Vec` when the workflow produced no events.
///
/// # Errors
///
/// - [`EngineError::VersionConflict`] when a concurrent writer raced ahead.
/// - [`EngineError::Workflow`] when the workflow rejects the command.
/// - [`EngineError::Deserialization`] when a stored event cannot be decoded.
///
/// [`Process::execute`]: crate::process::Process::execute
/// [`Process::execute_with`]: crate::process::Process::execute_with
pub(crate) async fn execute_command<W, S>(
    store: &S,
    stream_id: &crate::ids::StreamId,
    command: W::Command,
    ctx: &CommandContext,
) -> Result<Vec<EventEnvelope>, EngineError>
where
    W: Workflow,
    S: EventStore,
{
    execute_command_and_collect::<W, S>(store, stream_id, command, ctx)
        .await
        .map(|(envelopes, _outbox)| envelopes)
}

/// Like [`execute_command`] but also returns the [`PendingOutbox`] entries
/// produced by [`Workflow::handle`].
///
/// Use this when the caller needs to inspect or render the outbox messages
/// produced by the command — for example, in E2E tests that render EDIFACT
/// wire bytes from the workflow's outbox.  Avoids calling `handle()` a second
/// time just to recover the outbox that `execute_command` silently discards.
///
/// [`PendingOutbox`]: crate::outbox::PendingOutbox
pub(crate) async fn execute_command_and_collect<W, S>(
    store: &S,
    stream_id: &crate::ids::StreamId,
    command: W::Command,
    ctx: &CommandContext,
) -> Result<(Vec<EventEnvelope>, Vec<PendingOutbox>), EngineError>
where
    W: Workflow,
    S: EventStore,
{
    // ── 1 + 2. Stream-fold: reconstruct state without materialising a Vec ─────
    //
    // `fold_stream` feeds `EventEnvelope` values one-at-a-time; the engine
    // never holds more than one envelope in memory during replay.  Because
    // the envelope is owned, `env.payload` is moved into `W::upcast` without
    // a clone — no extra heap allocation per event.
    let (state, current_sequence) = store
        .fold_stream(
            stream_id,
            0,
            (W::State::default(), 0u64),
            |(acc, _), env| {
                let seq = env.sequence_number;
                // env.payload is moved here — no clone required.
                let payload = W::upcast(&env.event_type, env.schema_version, env.payload)?;
                let event: W::Event = serde_json::from_value(payload)
                    .map_err(|e| EngineError::Deserialization(e.to_string()))?;
                Ok((W::apply(acc, &event), seq))
            },
        )
        .await?;

    // ── 3. Handle command (pure) ──────────────────────────────────────────────
    let output = W::handle(&state, command)?;

    if output.events.is_empty() {
        return Ok((Vec::new(), output.outbox));
    }

    // ── 4. Build NewEvent values (caller-known metadata) ──────────────────────
    let new_events: Result<Vec<NewEvent>, EngineError> = output
        .events
        .iter()
        .map(|event| ctx.new_event(event))
        .collect();
    let new_events = new_events?;

    // ── 5. Persist with optimistic concurrency ────────────────────────────────
    // The store assigns event_id, sequence_number, stream_id, and timestamp.
    // Outbox messages (output.outbox) are intentionally ignored here — use
    // execute_command_atomic when atomic dual-writes are required.
    let result = store
        .append(
            stream_id,
            ExpectedVersion::Exact(current_sequence),
            &new_events,
        )
        .await?;

    Ok((result.events, output.outbox))
}

/// Like [`execute_command`] but atomically co-persists any [`PendingOutbox`]
/// messages produced by [`Workflow::handle`].
///
/// Requires `S: AtomicAppend`.  All internal logic is identical to
/// `execute_command`; the only difference is the persistence call at the end.
pub(crate) async fn execute_command_atomic<W, S>(
    store: &S,
    stream_id: &crate::ids::StreamId,
    command: W::Command,
    ctx: &CommandContext,
) -> Result<Vec<EventEnvelope>, EngineError>
where
    W: Workflow,
    S: crate::event_store::AtomicAppend,
{
    // ── 1 + 2. Stream-fold: reconstruct state without materialising a Vec ─────
    let (state, current_sequence) = store
        .fold_stream(
            stream_id,
            0,
            (W::State::default(), 0u64),
            |(acc, _), env| {
                let seq = env.sequence_number;
                let payload = W::upcast(&env.event_type, env.schema_version, env.payload)?;
                let event: W::Event = serde_json::from_value(payload)
                    .map_err(|e| EngineError::Deserialization(e.to_string()))?;
                Ok((W::apply(acc, &event), seq))
            },
        )
        .await?;

    // ── 3. Handle command (pure) ──────────────────────────────────────────────
    let output = W::handle(&state, command)?;

    if output.events.is_empty() {
        return Ok(Vec::new());
    }

    // ── 4. Build NewEvent values ──────────────────────────────────────────────
    let new_events: Result<Vec<NewEvent>, EngineError> = output
        .events
        .iter()
        .map(|event| ctx.new_event(event))
        .collect();
    let new_events = new_events?;

    // ── 5. Persist events + outbox atomically ─────────────────────────────────
    let result = store
        .append_with_outbox(
            stream_id,
            ExpectedVersion::Exact(current_sequence),
            &new_events,
            &output.outbox,
        )
        .await?;

    Ok(result.events)
}

/// Like [`execute_command_atomic`] but co-persists `deadlines` in the same
/// atomic write as events and outbox entries.
///
/// On [`SlateDbStore`] all three sets of writes land in a single SSI
/// transaction, eliminating the non-atomic window between event persistence
/// and deadline registration. On stores that use the default
/// [`AtomicAppend::append_with_outbox_and_deadlines`] fallback, deadlines are
/// **not** persisted here — callers must register them separately via
/// [`DeadlineStore::register`].
///
/// This is the canonical implementation path for commands that must register
/// a regulatory deadline (GPKE 24h, WiM 5 WT, GeLi Gas / WiM Gas 10 WT,
/// MABIS 1 WT).
///
/// [`SlateDbStore`]: crate::store_slatedb::SlateDbStore
/// [`DeadlineStore::register`]: crate::deadline::DeadlineStore::register
pub(crate) async fn execute_command_atomic_with_deadlines<W, S>(
    store: &S,
    stream_id: &crate::ids::StreamId,
    command: W::Command,
    ctx: &CommandContext,
    deadlines: &[crate::deadline::Deadline],
) -> Result<Vec<EventEnvelope>, EngineError>
where
    W: Workflow,
    S: crate::event_store::AtomicAppend,
{
    let (state, current_sequence) = store
        .fold_stream(
            stream_id,
            0,
            (W::State::default(), 0u64),
            |(acc, _), env| {
                let seq = env.sequence_number;
                let payload = W::upcast(&env.event_type, env.schema_version, env.payload)?;
                let event: W::Event = serde_json::from_value(payload)
                    .map_err(|e| EngineError::Deserialization(e.to_string()))?;
                Ok((W::apply(acc, &event), seq))
            },
        )
        .await?;

    let output = W::handle(&state, command)?;

    if output.events.is_empty() {
        return Ok(Vec::new());
    }

    let new_events: Result<Vec<NewEvent>, EngineError> = output
        .events
        .iter()
        .map(|event| ctx.new_event(event))
        .collect();
    let new_events = new_events?;

    // Merge externally-supplied deadlines with any PendingDeadline values
    // returned by the workflow's handle function.
    let mut all_deadlines: Vec<crate::deadline::Deadline> = deadlines.to_vec();
    for pd in &output.deadlines {
        all_deadlines.push(crate::deadline::Deadline::new(
            stream_id.clone(),
            ctx.process_id,
            ctx.tenant_id,
            ctx.workflow_id.clone(),
            pd.label.as_str(),
            pd.due_at,
        ));
    }

    let result = store
        .append_with_outbox_and_deadlines(
            stream_id,
            ExpectedVersion::Exact(current_sequence),
            &new_events,
            &output.outbox,
            &all_deadlines,
        )
        .await?;

    Ok(result.events)
}

/// Reconstruct `(W::State, current_sequence)` using an optional snapshot as a
/// starting point.
///
/// When a snapshot with matching schema version exists, replay starts from
/// `snap.sequence_number` (O(k) tail scan). Otherwise falls back to full replay.
async fn reconstruct_with_snapshot<W, S, Snap>(
    store: &S,
    snap_store: &Snap,
    stream_id: &crate::ids::StreamId,
) -> Result<(W::State, u64), EngineError>
where
    W: Workflow,
    W::State: serde::de::DeserializeOwned,
    S: EventStore,
    Snap: crate::snapshot::SnapshotStore,
{
    let maybe_snap = snap_store.load(stream_id).await?;
    let (initial_state, from_sequence) = match &maybe_snap {
        Some(snap) if snap.state_schema_version == W::state_schema_version() => {
            let state = serde_json::from_value::<W::State>(snap.state.clone())
                .map_err(|e| EngineError::Deserialization(e.to_string()))?;
            (state, snap.sequence_number)
        }
        #[allow(unused_variables)]
        Some(snap) => {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                expected = W::state_schema_version(),
                actual   = snap.state_schema_version,
                stream_id = %stream_id,
                "snapshot schema version mismatch; falling back to full replay"
            );
            (W::State::default(), 0)
        }
        None => (W::State::default(), 0),
    };
    store
        .fold_stream(
            stream_id,
            from_sequence,
            (initial_state, from_sequence),
            |(acc, _), env| {
                let seq = env.sequence_number;
                let payload = W::upcast(&env.event_type, env.schema_version, env.payload)?;
                let event: W::Event = serde_json::from_value(payload)
                    .map_err(|e| EngineError::Deserialization(e.to_string()))?;
                Ok((W::apply(acc, &event), seq))
            },
        )
        .await
}

/// Like [`execute_command`] but uses a snapshot store to skip full replay.
///
/// When a valid snapshot exists, only tail events since the snapshot are
/// replayed — O(k) instead of O(n). Falls back to full replay when no snapshot
/// exists or the schema version has changed.
pub(crate) async fn execute_command_with_snapshot<W, S, Snap>(
    store: &S,
    snap_store: &Snap,
    stream_id: &crate::ids::StreamId,
    command: W::Command,
    ctx: &CommandContext,
) -> Result<Vec<EventEnvelope>, EngineError>
where
    W: Workflow,
    W::State: serde::de::DeserializeOwned,
    S: EventStore,
    Snap: crate::snapshot::SnapshotStore,
{
    let (state, current_sequence) =
        reconstruct_with_snapshot::<W, S, Snap>(store, snap_store, stream_id).await?;

    let output = W::handle(&state, command)?;
    if output.events.is_empty() {
        return Ok(Vec::new());
    }
    let new_events: Result<Vec<NewEvent>, EngineError> = output
        .events
        .iter()
        .map(|event| ctx.new_event(event))
        .collect();
    let new_events = new_events?;
    let result = store
        .append(
            stream_id,
            ExpectedVersion::Exact(current_sequence),
            &new_events,
        )
        .await?;
    Ok(result.events)
}

/// Like [`execute_command_atomic`] but uses a snapshot store to skip full replay.
///
/// Atomically co-persists outbox messages alongside events while using a
/// snapshot as the starting point for state reconstruction.
pub(crate) async fn execute_command_atomic_with_snapshot<W, S, Snap>(
    store: &S,
    snap_store: &Snap,
    stream_id: &crate::ids::StreamId,
    command: W::Command,
    ctx: &CommandContext,
) -> Result<Vec<EventEnvelope>, EngineError>
where
    W: Workflow,
    W::State: serde::de::DeserializeOwned,
    S: crate::event_store::AtomicAppend,
    Snap: crate::snapshot::SnapshotStore,
{
    let (state, current_sequence) =
        reconstruct_with_snapshot::<W, S, Snap>(store, snap_store, stream_id).await?;

    let output = W::handle(&state, command)?;
    if output.events.is_empty() {
        return Ok(Vec::new());
    }
    let new_events: Result<Vec<NewEvent>, EngineError> = output
        .events
        .iter()
        .map(|event| ctx.new_event(event))
        .collect();
    let new_events = new_events?;
    let result = store
        .append_with_outbox(
            stream_id,
            ExpectedVersion::Exact(current_sequence),
            &new_events,
            &output.outbox,
        )
        .await?;
    Ok(result.events)
}
