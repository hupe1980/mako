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
//! The one write path is [`Process::run`] — or [`Process::run_timeout`], where
//! the command is the one [`Workflow::on_deadline`] decides. Both persist the
//! whole [`WorkflowOutput`] — events, outbox, deadlines — in one
//! [`AtomicAppend::append`].
//!
//! [`Process::run`]: crate::process::Process::run
//! [`Process::run_timeout`]: crate::process::Process::run_timeout
//! [`AtomicAppend::append`]: crate::event_store::AtomicAppend::append

use crate::{
    deadline::Deadline,
    envelope::{EventEnvelope, NewEvent},
    error::{EngineError, WorkflowError},
    event_store::{AppendBatch, AppendResult, AtomicAppend, ExpectedVersion},
    ids::{CausationId, ConversationId, CorrelationId, ProcessId, TenantId},
    outbox::PendingOutbox,
    version::WorkflowId,
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
/// use mako_fristen::{APERAK_STROM_WINDOW_LABEL, aperak_strom_due_at};
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
/// inside [`Process::run`].
///
/// When the command must register a regulatory deadline (e.g. APERAK 45-min
/// sending window per APERAK AHB 1.1 §2.4.1), add a [`PendingDeadline`].
/// The engine injects the process-identity fields from [`CommandContext`] and
/// persists the deadline atomically with the events.
///
/// [`OutboxMessage`]: crate::outbox::OutboxMessage
/// [`Process::run`]: crate::process::Process::run
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
/// Commands are transient — they are never persisted. `Clone` is required
/// because [`Process::run`] hands the same command to `handle` again, on
/// freshly replayed state, after a version conflict.
///
/// [`Process::run`]: crate::process::Process::run
pub trait CommandPayload: Clone + Send + 'static {}

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

    /// Every business-deadline label this workflow registers and decides.
    ///
    /// The one table registration and matching share. The engine refuses to
    /// persist a deadline whose label is neither listed here nor a delivery
    /// window ([`mako_fristen::is_delivery_window`]) — see
    /// [`EngineError::UnownedDeadlineLabel`] — and
    /// [`Process::run_timeout`] consults [`Workflow::on_deadline`] only for
    /// a label listed here. A delivery window is never listed: it is retired
    /// when its message is delivered, and one that fires is an alert about that
    /// message, not a step of the process.
    ///
    /// Defaults to none, for a workflow that registers no Frist.
    ///
    /// [`Process::run_timeout`]: crate::process::Process::run_timeout
    const DEADLINE_LABELS: &'static [&'static str] = &[];

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
    /// The whole output — events, outbox entries and deadlines — is persisted
    /// in one atomic write by [`Process::run`]. An outbox entry or deadline
    /// with no event beside it is refused
    /// ([`EngineError::UnrecordedEffect`]).
    ///
    /// # Errors
    ///
    /// Return a [`WorkflowError`] when the command is invalid for the current
    /// state or when domain validation fails.
    ///
    /// [`Process::run`]: crate::process::Process::run
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

    /// Map a fired deadline to a compensating command.
    ///
    /// Called by [`Process::run_timeout`] when a registered deadline
    /// for this workflow's process becomes overdue — and only for a label in
    /// [`Workflow::DEADLINE_LABELS`]. Return `Some(command)` to act; return
    /// `None` to acknowledge the deadline as a no-op.
    ///
    /// This is the one guard a timeout passes. Deadlines are never cancelled
    /// when a process moves on, so every one fires — on the healthy path too —
    /// and the hook must act only where the window the label names is still
    /// open in `state`.
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
    /// [`Process::run_timeout`]: crate::process::Process::run_timeout
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
    /// process.run(DispatchAperak { positive: true, reason: None }, RunOptions::new().context(ctx)).await?;
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
    /// process.run_timeout(&overdue_deadline, RunOptions::new().context(ctx)).await?;
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
    /// store.append(AppendBatch::new(&stream_id, ExpectedVersion::Any, &[new_event])).await?;
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

// ── decide_and_append ─────────────────────────────────────────────────────────

/// Replay `stream_id`, let `decide` pick a command from the state, handle it,
/// and persist the whole output in one [`AtomicAppend::append`].
///
/// The crate-internal core of [`Process::run`] and [`Process::run_timeout`].
/// In order:
///
/// 1. **Replay** the stream through [`Workflow::apply`].
/// 2. **Decide**: `decide(&state)` returns the command, or `None` to write
///    nothing (a deadline whose window is already closed).
/// 3. **Handle** the command via [`Workflow::handle`] (pure, no I/O).
/// 4. **Append** events, outbox entries, the caller's and the workflow's
///    deadlines, and the caller's correlation entries in one batch, guarded by
///    `ExpectedVersion::Exact(current_sequence)`.
///
/// A command that produces no event writes nothing — not the caller's
/// deadlines, not its correlation entries — because nothing happened that they
/// could belong to. One that produces an outbox entry or a deadline without an
/// event is refused ([`EngineError::UnrecordedEffect`]).
///
/// [`Process::run`]: crate::process::Process::run
/// [`Process::run_timeout`]: crate::process::Process::run_timeout
/// [`AtomicAppend::append`]: crate::event_store::AtomicAppend::append
pub(crate) async fn decide_and_append<W, S>(
    store: &S,
    stream_id: &crate::ids::StreamId,
    ctx: &CommandContext,
    decide: &mut (impl FnMut(&W::State) -> Option<W::Command> + Send),
    deadlines: &[Deadline],
    correlations: &[crate::event_store::CorrelationEntry],
) -> Result<Option<(AppendResult, Vec<Deadline>)>, EngineError>
where
    W: Workflow,
    S: AtomicAppend,
{
    // `fold_stream` feeds one owned envelope at a time, so the payload moves
    // into `W::upcast` without a clone.
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

    let Some(command) = decide(&state) else {
        return Ok(None);
    };
    let output = W::handle(&state, command)?;

    if output.events.is_empty() {
        if !output.outbox.is_empty() || !output.deadlines.is_empty() {
            return Err(EngineError::UnrecordedEffect {
                workflow: ctx.workflow_id.name.as_ref().into(),
            });
        }
        return Ok(Some((
            AppendResult {
                last_sequence: current_sequence,
                events: Vec::new(),
                outbox: Vec::new(),
            },
            Vec::new(),
        )));
    }

    let new_events = output
        .events
        .iter()
        .map(|event| ctx.new_event(event))
        .collect::<Result<Vec<NewEvent>, EngineError>>()?;
    let all_deadlines = materialise_deadlines::<W>(stream_id, ctx, deadlines, &output.deadlines)?;

    let result = store
        .append(
            AppendBatch::new(
                stream_id,
                ExpectedVersion::Exact(current_sequence),
                &new_events,
            )
            .outbox(&output.outbox)
            .deadlines(&all_deadlines)
            .correlations(correlations),
        )
        .await?;
    Ok(Some((result, all_deadlines)))
}

/// Does `W` own the deadline `label`?
///
/// Owned means listed in [`Workflow::DEADLINE_LABELS`], or a delivery window
/// ([`mako_fristen::is_delivery_window`]) that the outbox worker retires on
/// delivery.
#[must_use]
pub fn owns_deadline_label<W: Workflow>(label: &str) -> bool {
    W::DEADLINE_LABELS.contains(&label) || mako_fristen::is_delivery_window(label)
}

/// The deadlines one command persists: the caller's, plus the
/// [`PendingDeadline`]s `handle` returned, stamped with the process identity.
///
/// # Errors
///
/// [`EngineError::UnownedDeadlineLabel`] when any of them carries a label `W`
/// does not own — such a deadline would fire into a workflow that never
/// decides it.
fn materialise_deadlines<W: Workflow>(
    stream_id: &crate::ids::StreamId,
    ctx: &CommandContext,
    supplied: &[Deadline],
    pending: &[PendingDeadline],
) -> Result<Vec<Deadline>, EngineError> {
    let mut all: Vec<Deadline> = supplied.to_vec();
    all.extend(pending.iter().map(|pd| {
        Deadline::new(
            stream_id.clone(),
            ctx.process_id,
            ctx.tenant_id,
            ctx.workflow_id.clone(),
            pd.label.as_str(),
            pd.due_at,
        )
    }));
    if let Some(unowned) = all.iter().find(|d| !owns_deadline_label::<W>(d.label())) {
        return Err(EngineError::UnownedDeadlineLabel {
            workflow: ctx.workflow_id.name.as_ref().into(),
            label: unowned.label().into(),
        });
    }
    Ok(all)
}

// ── Business-key occupancy ────────────────────────────────────────────────────

/// Whether a process in this state still occupies its business key.
///
/// MaKo processes are correlated by a business key — usually the MaLo — through
/// the process registry's correlation index. That index is **append-only**:
/// an entry is written when a process spawns and nothing removes it when the
/// process finishes. So "is there an entry for this MaLo?" cannot answer "is a
/// process still running for this MaLo?" — only the replayed state can.
///
/// A duplicate guard that refuses on entry presence therefore blocks a business
/// key permanently. That is wrong for every MaKo process family: an Anmeldung
/// the NB rejected is finished, and the corrected Anmeldung that follows is a
/// normal flow, not a retry.
///
/// Implement this on a workflow's [`Workflow::State`] so the decision sits next
/// to the state definition. Write the impl as an **exhaustive match** rather
/// than a catch-all, so adding a state variant forces a deliberate choice
/// instead of silently inheriting whichever default the catch-all happened to
/// pick.
pub trait OccupiesBusinessKey {
    /// `true` while a *new* process for the same business key must be refused.
    ///
    /// Return `false` for the pre-start state and for every terminal state
    /// (completed, rejected, timed out). When a state's meaning is genuinely
    /// ambiguous, return `true`: refusing a legitimate process surfaces as a
    /// `409` the operator can see, whereas admitting a duplicate one puts a
    /// second live process on the same MaLo, which the market cannot untangle.
    fn occupies_business_key(&self) -> bool;
}
