//! [`Process`] — ergonomic typed handle for a single MaKo process instance.
//!
//! Instead of threading `stream_id`, `workflow_id`, `tenant_id`, and a store
//! reference through every call to the write path, bind them once into a
//! `Process<W, S>` and call [`execute`] / [`state`] directly.
//!
//! # Starting a new process
//!
//! ```rust,ignore
//! use mako_engine::{
//!     event_store::InMemoryEventStore,
//!     ids::TenantId,
//!     process::Process,
//!     version::WorkflowId,
//! };
//!
//! let store = InMemoryEventStore::new();
//! let process = Process::<MyWorkflow, _>::new(
//!     store,
//!     TenantId::new(),
//!     WorkflowId::new("my-workflow", "FV2024-10-01"),
//! );
//!
//! let envelopes = process.execute(my_command).await?;
//! let current   = process.state().await?;
//! ```
//!
//! # Resuming an existing process
//!
//! ```rust,ignore
//! let process = Process::<MyWorkflow, _>::from_stream(
//!     store, stream_id, process_id, tenant_id, workflow_id,
//! );
//! ```
//!
//! [`execute`]: Process::execute
//! [`state`]: Process::state

use std::marker::PhantomData;

use crate::{
    envelope::EventEnvelope,
    error::EngineError,
    event_store::EventStore,
    ids::{ProcessId, ProcessIdentity, StreamId, TenantId},
    snapshot::{Snapshot, SnapshotStore},
    version::WorkflowId,
    workflow::{
        CommandContext, Workflow, execute_command, execute_command_and_collect,
        execute_command_with_snapshot,
    },
};

// ── Process ───────────────────────────────────────────────────────────────────

/// An ergonomic typed handle for a single MaKo process instance.
///
/// `Process` bundles the [`StreamId`], [`ProcessId`], [`TenantId`],
/// [`WorkflowId`], and event store into a single owned value so callers do not
/// need to pass them on every command dispatch.
///
/// ## Generic parameters
///
/// - `W` — the [`Workflow`] implementation. In practice this is a zero-size
///   marker struct; the type parameter carries the domain logic as associated
///   types.
/// - `S` — the [`EventStore`] backend. [`InMemoryEventStore`] is the default
///   for tests; production deployments wrap a persistent backend in
///   [`Arc`][std::sync::Arc] and use `Process<W, Arc<MyStore>>`.
///
/// ## Clone semantics
///
/// If `S: Clone` (e.g. [`InMemoryEventStore`] or `Arc<…>`), `Process` is also
/// `Clone` and all clones share the same underlying storage.
///
/// [`InMemoryEventStore`]: crate::event_store::InMemoryEventStore
#[allow(clippy::struct_field_names)] // `process_id` and `stream_id` are intentional: they
// describe engine-layer concepts, not redundant prefixes.
pub struct Process<W: Workflow, S: EventStore> {
    stream_id: StreamId,
    process_id: ProcessId,
    tenant_id: TenantId,
    workflow_id: WorkflowId,
    store: S,
    _phantom: PhantomData<fn() -> W>,
}

impl<W: Workflow, S: EventStore> Process<W, S> {
    /// Create a fresh process instance.
    ///
    /// Generates a new [`ProcessId`] and derives the [`StreamId`] from
    /// `tenant_id` and `process_id` (`process/{tenant_id}/{process_id}`).
    /// Use this when starting a new MaKo process
    /// (e.g. on receipt of the first inbound UTILMD Lieferbeginn).
    #[must_use]
    pub fn new(store: S, tenant_id: TenantId, workflow_id: WorkflowId) -> Self {
        let process_id = ProcessId::new();
        let stream_id = StreamId::for_process(tenant_id, &process_id);
        Self {
            stream_id,
            process_id,
            tenant_id,
            workflow_id,
            store,
            _phantom: PhantomData,
        }
    }

    /// Attach to an existing process stream.
    ///
    /// Use this on service restart or when routing an inbound message to an
    /// already-running process whose identifiers were previously persisted.
    #[must_use]
    pub fn from_stream(
        store: S,
        stream_id: StreamId,
        process_id: ProcessId,
        tenant_id: TenantId,
        workflow_id: WorkflowId,
    ) -> Self {
        Self {
            stream_id,
            process_id,
            tenant_id,
            workflow_id,
            store,
            _phantom: PhantomData,
        }
    }

    /// The event stream identifier for this process.
    #[must_use]
    pub fn stream_id(&self) -> &StreamId {
        &self.stream_id
    }

    /// The stable process identifier.
    #[must_use]
    pub fn process_id(&self) -> ProcessId {
        self.process_id
    }

    /// The tenant that owns this process.
    #[must_use]
    pub fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }

    /// The workflow version under which this process was created.
    #[must_use]
    pub fn workflow_id(&self) -> &WorkflowId {
        &self.workflow_id
    }

    /// Return a serializable value bundle of all four process identifiers.
    ///
    /// Persist this to a routing table (e.g. keyed by `conversation_id` or
    /// `correlation_id`) so inbound messages can be routed to the correct
    /// running process without the caller needing to manage four separate
    /// fields.
    ///
    /// Use [`Process::from_identity`] to re-attach to the same process stream
    /// on a subsequent request.
    ///
    /// ```rust,ignore
    /// let id = process.identity();
    /// routing_table.insert(conv_id, id.clone());
    ///
    /// // Later, on a subsequent inbound message:
    /// let id = routing_table.get(&conv_id)?;
    /// let process = Process::<MyWorkflow, _>::from_identity(store, id);
    /// ```
    #[must_use]
    pub fn identity(&self) -> ProcessIdentity {
        ProcessIdentity::new(self.process_id, self.tenant_id, self.workflow_id.clone())
    }

    /// Build a [`CommandContext`] for an inbound EDIFACT message dispatch.
    ///
    /// Derives a **deterministic** [`CorrelationId`] from `interchange_ref`
    /// (UUID v5) so repeated dispatches of the same EDIFACT message — e.g.
    /// AS4 retransmissions or idempotent REST replays — produce the same
    /// correlation root. This makes EDIFACT-level idempotency observable in
    /// distributed traces without any extra dedup logic at the engine level.
    ///
    /// Use this instead of [`Process::execute`] when you need to propagate
    /// EDIFACT correlation metadata into the event stream. The returned
    /// context is passed to [`Process::execute_with`].
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let process = ctx.resume::<GpkeSupplierChangeWorkflow>(identity);
    /// let cmd_ctx = process.context_for_inbound(&utilmd_interchange_ref);
    /// process.execute_with(command, cmd_ctx).await?;
    /// ```
    ///
    /// [`CorrelationId`]: crate::ids::CorrelationId
    #[must_use]
    pub fn context_for_inbound(&self, interchange_ref: &str) -> CommandContext {
        CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone())
            .with_correlation(crate::ids::CorrelationId::from_interchange_ref(
                interchange_ref,
            ))
    }

    /// Attach to an existing process stream from a previously persisted
    /// [`ProcessIdentity`].
    ///
    /// This is the companion to [`Process::identity`]: look up the identity
    /// from your routing table and call `from_identity` to get a live
    /// `Process` handle bound to `store`.
    #[must_use]
    pub fn from_identity(store: S, identity: ProcessIdentity) -> Self {
        Self {
            stream_id: identity.stream_id().clone(),
            process_id: identity.process_id,
            tenant_id: identity.tenant_id,
            workflow_id: identity.workflow_id,
            store,
            _phantom: PhantomData,
        }
    }

    /// Return the number of events currently in the stream.
    ///
    /// Uses [`EventStore::stream_version`] for an efficient O(1) metadata
    /// query on backends that override it. Falls back to loading all events
    /// on stores that use the default implementation.
    ///
    /// Use this to decide whether to take a snapshot — e.g. with
    /// [`Snapshot::should_take`]:
    ///
    /// ```rust,ignore
    /// if Snapshot::should_take(process.event_count().await?, 100) {
    ///     process.take_snapshot(&snap_store, 100).await?;
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failures.
    ///
    /// [`Snapshot::should_take`]: crate::snapshot::Snapshot::should_take
    pub async fn event_count(&self) -> Result<u64, EngineError> {
        self.store.stream_version(&self.stream_id).await
    }

    /// Dispatch `command` using a freshly generated [`CommandContext`].
    ///
    /// A new [`CorrelationId`] and [`ConversationId`] are auto-generated for
    /// each call. To propagate tracing IDs from an inbound EDIFACT message
    /// across a multi-step command chain, use [`execute_with`].
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] when a concurrent writer raced ahead;
    ///   retry by calling `execute` again.
    /// - [`EngineError::Workflow`] when the workflow rejects the command.
    /// - [`EngineError::Deserialization`] when a stored event cannot be decoded.
    ///
    /// [`CorrelationId`]: crate::ids::CorrelationId
    /// [`ConversationId`]: crate::ids::ConversationId
    /// [`execute_with`]: Process::execute_with
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, command), fields(
            workflow = %self.workflow_id,
            process_id = %self.process_id,
            stream_id = %self.stream_id,
        ))
    )]
    pub async fn execute(&self, command: W::Command) -> Result<Vec<EventEnvelope>, EngineError> {
        let ctx = CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone());
        execute_command::<W, S>(&self.store, &self.stream_id, command, &ctx).await
    }

    /// Like [`execute`] but also returns the outbox messages produced by
    /// [`Workflow::handle`], fully stamped with the real IDs from the persisted
    /// event.
    ///
    /// The returned [`OutboxMessage`] entries have their `causation_event_id`
    /// set to the `event_id` of the first persisted event — identical to what
    /// `execute_and_enqueue` writes into the [`OutboxStore`] atomically.  This
    /// makes the messages ready to pass directly to the EDIFACT renderer
    /// without any manual ID stitching.
    ///
    /// Use this in E2E and integration tests that need to inspect or render
    /// outbox messages after a command is persisted, without the awkward
    /// `handle()` + `execute()` double invocation.
    ///
    /// [`execute`]: Process::execute
    /// [`OutboxMessage`]: crate::outbox::OutboxMessage
    /// [`OutboxStore`]: crate::outbox::OutboxStore
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] on storage or command handling failure.
    pub async fn execute_and_collect(
        &self,
        command: W::Command,
    ) -> Result<(Vec<EventEnvelope>, Vec<crate::outbox::OutboxMessage>), EngineError> {
        let ctx = CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone());
        let (events, pending) =
            execute_command_and_collect::<W, S>(&self.store, &self.stream_id, command, &ctx)
                .await?;

        // Stamp each PendingOutbox with the real IDs from the persisted event.
        // Using the first event's event_id as causation_event_id mirrors what
        // execute_and_enqueue writes into the OutboxStore atomically.
        let causation_event_id = events
            .first()
            .map_or_else(crate::ids::EventId::new, |e| e.event_id);

        let outbox = pending
            .into_iter()
            .map(|p| {
                crate::outbox::OutboxMessage::new(
                    self.stream_id.clone(),
                    self.process_id,
                    self.tenant_id,
                    ctx.correlation_id,
                    ctx.conversation_id,
                    causation_event_id,
                    p.message_type,
                    p.recipient,
                    p.payload,
                )
            })
            .collect();

        Ok((events, outbox))
    }

    /// Dispatch `command` with a caller-supplied [`CommandContext`].
    ///
    /// Use this when you need to thread a specific `correlation_id`,
    /// `conversation_id`, or `causation_id` through the command. For example,
    /// when dispatching an APERAK in response to a UTILMD, pass the
    /// `conversation_id` from the UTILMD envelope so both exchanges are
    /// traceable as a single business conversation.
    ///
    /// Build a context with:
    ///
    /// ```rust,ignore
    /// let ctx = CommandContext::new(tenant_id, process_id, workflow_id)
    ///     .with_causation(utilmd_event_id.into())  // From<EventId> for CausationId
    ///     .with_conversation(utilmd_conversation_id);
    /// process.execute_with(DispatchAperak { .. }, ctx).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// See [`Process::execute`] for the error contract.
    ///
    /// [`Process::execute`]: Process::execute
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, command, ctx), fields(
            workflow = %self.workflow_id,
            process_id = %self.process_id,
            correlation_id = %ctx.correlation_id,
        ))
    )]
    pub async fn execute_with(
        &self,
        command: W::Command,
        ctx: CommandContext,
    ) -> Result<Vec<EventEnvelope>, EngineError> {
        execute_command::<W, S>(&self.store, &self.stream_id, command, &ctx).await
    }

    /// Dispatch `command` using a snapshot store to accelerate state reconstruction.
    ///
    /// Equivalent to [`Process::execute`] but starts replay from the most recent
    /// snapshot rather than from sequence 0. For streams with thousands of events
    /// and a snapshot within the last 100 events, this reduces replay cost from
    /// O(n) to O(k) where k is the tail length since the last snapshot.
    ///
    /// When no snapshot exists or the schema version has changed, falls back to
    /// full O(n) replay — identical in cost to [`Process::execute`].
    ///
    /// # Errors
    ///
    /// Same contract as [`Process::execute`].
    pub async fn execute_snapshot<Snap>(
        &self,
        command: W::Command,
        snap_store: &Snap,
    ) -> Result<Vec<EventEnvelope>, EngineError>
    where
        W::State: serde::de::DeserializeOwned,
        Snap: SnapshotStore,
    {
        let ctx = CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone());
        execute_command_with_snapshot::<W, S, Snap>(
            &self.store,
            snap_store,
            &self.stream_id,
            command,
            &ctx,
        )
        .await
    }

    /// Reconstruct the current workflow state by replaying all persisted events.
    ///
    /// This is a **read-only** operation — it loads events but does not
    /// acquire any write lock or check optimistic concurrency. Use it to:
    ///
    /// - Inspect process status in tests without dispatching a command.
    /// - Build a diagnostic snapshot for observability or health checks.
    /// - Implement query-side read models that need the full typed state.
    ///
    /// For production read models, prefer a [`Projection`] that is updated
    /// incrementally rather than replaying the full stream on every query.
    ///
    /// To accelerate replay for long-lived streams, use
    /// [`Process::state_with_snapshot`] instead.
    ///
    /// # Errors
    ///
    /// - [`EngineError::Store`] on storage failures.
    /// - [`EngineError::Deserialization`] when a stored event cannot be decoded
    ///   into `W::Event` (schema migration required).
    ///
    /// [`Projection`]: crate::projection::Projection
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(
            workflow = %self.workflow_id,
            stream_id = %self.stream_id,
        ))
    )]
    pub async fn state(&self) -> Result<W::State, EngineError> {
        self.store
            .fold_stream(&self.stream_id, 0, W::State::default(), |acc, env| {
                let payload = W::upcast(&env.event_type, env.schema_version, env.payload)?;
                let event: W::Event = serde_json::from_value(payload)
                    .map_err(|e| EngineError::Deserialization(e.to_string()))?;
                Ok(W::apply(acc, &event))
            })
            .await
    }

    // ── Snapshot-aware state reconstruction ──────────────────────────────────

    /// Reconstruct current state using a snapshot as the starting point.
    ///
    /// Loads the most recent snapshot for this stream from `snap_store`. If
    /// one exists, deserializes it into `W::State` and then replays only
    /// events appended **after** the snapshot's `sequence_number`
    /// (O(k) instead of O(n)). Falls back to full replay when no snapshot
    /// exists.
    ///
    /// ## When to use
    ///
    /// Use this instead of [`Process::state`] for long-lived processes where
    /// the event count grows large. Pair it with [`Process::take_snapshot`]
    /// to keep the snapshot store current after each command.
    ///
    /// ## Schema version compatibility
    ///
    /// Snapshots whose `state` field cannot be deserialized into `W::State`
    /// (e.g. after a breaking state schema change) will return
    /// [`EngineError::Deserialization`]. In that case, fall back to
    /// [`Process::state`] (full replay) and take a fresh snapshot.
    ///
    /// # Errors
    ///
    /// - [`EngineError::Store`] on snapshot or event storage failures.
    /// - [`EngineError::Deserialization`] when the snapshot state or a tail
    ///   event cannot be decoded.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, snap_store), fields(
            workflow = %self.workflow_id,
            stream_id = %self.stream_id,
        ))
    )]
    pub async fn state_with_snapshot<Snap: SnapshotStore>(
        &self,
        snap_store: &Snap,
    ) -> Result<W::State, EngineError>
    where
        W::State: serde::de::DeserializeOwned,
    {
        let maybe_snap = snap_store.load(&self.stream_id).await?;

        let (initial_state, from_sequence) = match maybe_snap {
            Some(snap) => {
                if snap.state_schema_version == W::state_schema_version() {
                    let state = serde_json::from_value::<W::State>(snap.state)
                        .map_err(|e| EngineError::Deserialization(e.to_string()))?;
                    (state, snap.sequence_number)
                } else {
                    // Schema version mismatch: discard the stale snapshot and
                    // fall back to full replay. The caller should take a fresh
                    // snapshot after this reconstruction completes.
                    tracing::warn!(
                        expected = W::state_schema_version(),
                        actual   = snap.state_schema_version,
                        stream_id = %self.stream_id,
                        "snapshot schema version mismatch; falling back to full replay"
                    );
                    (W::State::default(), 0)
                }
            }
            None => (W::State::default(), 0),
        };

        let tail = self
            .store
            .fold_stream(&self.stream_id, from_sequence, initial_state, |acc, env| {
                let payload = W::upcast(&env.event_type, env.schema_version, env.payload)?;
                let event: W::Event = serde_json::from_value(payload)
                    .map_err(|e| EngineError::Deserialization(e.to_string()))?;
                Ok(W::apply(acc, &event))
            })
            .await?;
        Ok(tail)
    }

    /// Reconstruct current state and save a snapshot if the event-count
    /// threshold is reached.
    ///
    /// Checks [`Snapshot::should_take`] with `interval`. When at least
    /// `interval` new events have accumulated since the last snapshot,
    /// reconstructs state via full replay, serializes it, and calls
    /// [`SnapshotStore::save`].
    ///
    /// Returns `true` when a snapshot was taken, `false` when the threshold
    /// was not reached or `interval` is `0`.
    ///
    /// ## Integration pattern
    ///
    /// ```rust,ignore
    /// // After every successful command:
    /// process.execute(command).await?;
    /// process.take_snapshot(&snap_store, 100).await?;
    ///
    /// // On the read path — O(k) instead of O(n):
    /// let state = process.state_with_snapshot(&snap_store).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// - [`EngineError::Store`] on snapshot storage failures.
    /// - [`EngineError::Serialization`] when the state cannot be JSON-encoded.
    /// - [`EngineError::Deserialization`] when a stored event cannot be decoded.
    ///
    /// [`Snapshot::should_take`]: crate::snapshot::Snapshot::should_take
    pub async fn take_snapshot<Snap: SnapshotStore>(
        &self,
        snap_store: &Snap,
        interval: u64,
    ) -> Result<bool, EngineError>
    where
        W::State: serde::Serialize,
    {
        let count = self.event_count().await?;
        // Load the last snapshot (if any) to get its sequence number.
        let last_snap_seq = snap_store
            .load(&self.stream_id)
            .await?
            .map_or(0, |s| s.sequence_number);
        if !Snapshot::should_take(count, last_snap_seq, interval) {
            return Ok(false);
        }
        let state = self.state().await?;
        let payload =
            serde_json::to_value(&state).map_err(|e| EngineError::Serialization(e.to_string()))?;
        let snap = Snapshot::new(
            self.stream_id.clone(),
            count,
            W::state_schema_version(),
            payload,
        );
        snap_store.save(&snap).await?;
        Ok(true)
    }

    // ── Retry ─────────────────────────────────────────────────────────────────

    /// Dispatch `command` with automatic retry on [`EngineError::VersionConflict`].
    ///
    /// A version conflict occurs when a concurrent writer appended events
    /// between this process's read and its append attempt. On each conflict,
    /// the engine **reloads the complete event stream from the store and
    /// replays all events** to rebuild fresh state before re-handling the
    /// command. Stale in-memory state from a previous attempt is never
    /// carried forward — each retry always starts from a fully-rebuilt snapshot.
    ///
    /// Non-conflict errors (storage failures, workflow rejections) are
    /// returned immediately without retrying.
    ///
    /// A freshly-generated [`CommandContext`] is pinned before the first
    /// attempt and reused across all retries so all events share the same
    /// correlation root regardless of retry count. Use
    /// [`execute_with_retry_ctx`] to supply a specific context (e.g. one
    /// derived from an inbound EDIFACT envelope).
    ///
    /// ## When to use
    ///
    /// Use for commands where two inbound EDIFACT messages for the same
    /// process may arrive concurrently — e.g. a UTILMD and its APERAK
    /// processed on separate async tasks.
    ///
    /// ## Command cloning
    ///
    /// `W::Command` must implement [`Clone`] so it can be resubmitted on
    /// each retry without reconstructing it from scratch.
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] when all `max_attempts` are
    ///   exhausted without a successful append.
    /// - Any non-conflict [`EngineError`] returned by the workflow or storage.
    /// - [`EngineError::Store`] when `max_attempts` is `0`.
    ///
    /// [`execute_with_retry_ctx`]: Process::execute_with_retry_ctx
    pub async fn execute_with_retry(
        &self,
        command: W::Command,
        max_attempts: u32,
    ) -> Result<Vec<EventEnvelope>, EngineError>
    where
        W::Command: Clone,
    {
        if max_attempts == 0 {
            return Err(EngineError::store("max_attempts must be >= 1"));
        }
        // Pin context before the loop — all retry attempts share the same
        // correlation root for consistent distributed tracing.
        let ctx = CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone());
        self.execute_with_retry_ctx(command, ctx, max_attempts)
            .await
    }

    /// Dispatch `command` with a caller-supplied [`CommandContext`] and
    /// automatic retry on [`EngineError::VersionConflict`].
    ///
    /// Identical to [`execute_with_retry`] but threads the provided `ctx`
    /// (including its `correlation_id`, `conversation_id`, and `causation_id`)
    /// through every retry attempt. Use this when you need to propagate
    /// tracing IDs from an inbound EDIFACT envelope across a retried command.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ctx = CommandContext::from_envelope(&utilmd_envelope, workflow_id);
    /// process.execute_with_retry_ctx(HandleAperak { .. }, ctx, 3).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// See [`execute_with_retry`] for the error contract.
    ///
    /// # Panics
    ///
    /// Panics if `max_attempts` is 0 and the guard at the top of the function
    /// is somehow bypassed (unreachable in practice).
    ///
    /// [`execute_with_retry`]: Process::execute_with_retry
    pub async fn execute_with_retry_ctx(
        &self,
        command: W::Command,
        ctx: CommandContext,
        max_attempts: u32,
    ) -> Result<Vec<EventEnvelope>, EngineError>
    where
        W::Command: Clone,
    {
        if max_attempts == 0 {
            return Err(EngineError::store("max_attempts must be >= 1"));
        }
        let mut conflict_err: Option<EngineError> = None;
        for attempt in 0..max_attempts {
            // Each call to `execute_with` internally calls `fold_stream` from
            // sequence 0 (or from the most recent snapshot if one is available).
            // State is always freshly reconstructed from the event log on every
            // attempt — there is no stale state carried forward between retries.
            // Do NOT "optimise" this by caching state across attempts; doing so
            // would allow a winning concurrent writer's events to be invisible
            // to the retry, producing incorrect decisions and duplicate events.
            match self.execute_with(command.clone(), ctx.clone()).await {
                Ok(envs) => return Ok(envs),
                Err(e) if e.is_version_conflict() => {
                    conflict_err = Some(e);
                    // Brief jittered sleep to reduce thundering-herd under
                    // concurrent ERP commands targeting the same stream.
                    // Delay = uniform random in [0, 10ms * attempt], capped at 80ms.
                    // Uses the OS CSPRNG via rand so every retry gets independent
                    // entropy regardless of stream-ID prefix.
                    if attempt + 1 < max_attempts {
                        let entropy: u64 = rand::random();
                        let window_ms: u64 = (10 * (u64::from(attempt) + 1)).min(80);
                        let jitter_ms = if window_ms == 0 {
                            0
                        } else {
                            entropy % window_ms
                        };
                        tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;
                    }
                }
                Err(e) => return Err(e), // non-retriable — propagate immediately
            }
        }
        // At least one attempt ran (max_attempts >= 1), so conflict_err is Some.
        Err(conflict_err.expect("loop ran at least once"))
    }

    /// Execute `command` and atomically co-persist any [`PendingOutbox`] messages
    /// produced by [`Workflow::handle`].
    ///
    /// Like [`execute`], but requires `S: AtomicAppend`. When the workflow's
    /// `handle` returns outbox messages alongside events, both are written to
    /// storage in a single `WriteBatch`, eliminating the silent message-loss
    /// window that would exist with separate writes.
    ///
    /// When the handle returns no outbox messages, this degenerates to a plain
    /// `EventStore::append` (no performance cost).
    ///
    /// **Use this method instead of [`execute`] in all production code** that
    /// needs outbox delivery guarantees. Plain `execute` silently drops any
    /// outbox entries produced by the workflow handler — a crash between
    /// `execute` and a subsequent manual `OutboxStore::enqueue` call would
    /// lose the APERAK or UTILMD response permanently.
    ///
    /// For long event streams with periodic snapshots use
    /// [`execute_and_enqueue_snapshot`] to reduce O(n) replay cost to O(k).
    /// In concurrent environments where `VersionConflict` is expected, use
    /// [`execute_and_enqueue_with_retry`] to retry automatically.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use mako_engine::process::Process;
    /// use mako_engine::version::WorkflowId;
    /// use mako_engine::ids::TenantId;
    ///
    /// // SlateDbStore implements AtomicAppend — required for execute_and_enqueue.
    /// let store = Arc::new(SlateDbStore::open_in_memory().await?);
    /// let tenant_id = TenantId::from_party_id("9904231000007");
    /// let workflow_id = WorkflowId::new("gpke-supplier-change", fv);
    ///
    /// let process = Process::<GpkeSupplierChangeWorkflow, _>::new(
    ///     Arc::clone(&store),
    ///     tenant_id,
    ///     workflow_id,
    /// );
    ///
    /// // The workflow handle emits a PendingOutbox APERAK entry alongside the event.
    /// // execute_and_enqueue writes both in one WriteBatch — no partial-write window.
    /// let events = process
    ///     .execute_and_enqueue(GpkeCommand::ReceiveUtilmd { pid: 55001, payload })
    ///     .await?;
    ///
    /// assert!(!events.is_empty(), "at least one event was persisted");
    ///
    /// // The APERAK outbox entry is now visible to the outbox worker:
    /// let pending = store.peek_outbox(tenant_id, 10).await?;
    /// assert_eq!(pending.len(), 1, "APERAK enqueued atomically with the event");
    /// ```
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] — stream was modified concurrently;
    ///   retry with [`execute_and_enqueue_with_retry`].
    /// - [`EngineError::Workflow`] — the command was rejected by the workflow.
    /// - [`EngineError::Store`] / [`EngineError::Outbox`] — storage failure.
    ///
    /// [`PendingOutbox`]: crate::outbox::PendingOutbox
    /// [`execute`]: Process::execute
    /// [`execute_and_enqueue_snapshot`]: Process::execute_and_enqueue_snapshot
    /// [`execute_and_enqueue_with_retry`]: Process::execute_and_enqueue_with_retry
    pub async fn execute_and_enqueue(
        &self,
        command: W::Command,
    ) -> Result<Vec<EventEnvelope>, EngineError>
    where
        S: crate::event_store::AtomicAppend,
    {
        let ctx = CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone());
        crate::workflow::execute_command_atomic::<W, S>(&self.store, &self.stream_id, command, &ctx)
            .await
    }

    /// Like [`execute_and_enqueue`] but uses a snapshot to accelerate replay.
    ///
    /// Atomically persists events and outbox entries while starting state
    /// reconstruction from the most recent snapshot. For long streams with
    /// periodic snapshots this reduces replay cost from O(n) to O(k).
    ///
    /// [`execute_and_enqueue`]: Process::execute_and_enqueue
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] on storage or command handling failure.
    pub async fn execute_and_enqueue_snapshot<Snap>(
        &self,
        command: W::Command,
        snap_store: &Snap,
    ) -> Result<Vec<EventEnvelope>, EngineError>
    where
        W::State: serde::de::DeserializeOwned,
        S: crate::event_store::AtomicAppend,
        Snap: SnapshotStore,
    {
        let ctx = CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone());
        crate::workflow::execute_command_atomic_with_snapshot::<W, S, Snap>(
            &self.store,
            snap_store,
            &self.stream_id,
            command,
            &ctx,
        )
        .await
    }

    /// Dispatch the compensation command returned by [`Workflow::on_deadline`].
    ///
    /// Reconstructs the current process state, calls
    /// `W::on_deadline(deadline, &state)`, and — if the hook returns
    /// `Some(command)` — executes it via [`Process::execute_and_enqueue`],
    /// which atomically persists events **and** any outbox entries (e.g.
    /// APERAK Ablehnung) produced by the compensation handler.
    ///
    /// Returns `Ok(Some(events))` when compensation fired, `Ok(None)` when
    /// the hook returned `None` (deadline acknowledged as no-op).
    ///
    /// This is the canonical way to wire deadline firings to workflow
    /// compensation logic.  Any [`WorkflowOutput::with_outbox`] entries
    /// returned by `on_deadline` are guaranteed to be persisted atomically —
    /// there is no window where the event is stored but the outbox entry is
    /// lost.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // In the deadline worker:
    /// let overdue = ctx.deadline_store().due_now(50).await?;
    /// for deadline in overdue {
    ///     let identity = ctx.registry()
    ///         .lookup(deadline.tenant_id(), &RegistryKey::from_process(deadline.process_id()))
    ///         .await?
    ///         .expect("process must be registered");
    ///     let process = ctx.resume::<GpkeSupplierChangeWorkflow>(identity);
    ///     if let Some(events) = process.execute_timeout(&deadline).await? {
    ///         // compensation command was dispatched — APERAK Ablehnung enqueued
    ///         tracing::info!(events = events.len(), "timeout compensation applied");
    ///     }
    ///     ctx.deadline_store().cancel(deadline.deadline_id()).await?;
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Propagates [`EngineError::VersionConflict`], [`EngineError::Workflow`],
    /// and storage errors from `execute_and_enqueue`. Use
    /// [`execute_timeout_with_retry`] when `VersionConflict` retries are
    /// required.
    ///
    /// [`Workflow::on_deadline`]: crate::workflow::Workflow::on_deadline
    /// [`WorkflowOutput::with_outbox`]: crate::workflow::WorkflowOutput::with_outbox
    /// [`execute_timeout_with_retry`]: Process::execute_timeout_with_retry
    pub async fn execute_timeout(
        &self,
        deadline: &crate::deadline::Deadline,
    ) -> Result<Option<Vec<EventEnvelope>>, EngineError>
    where
        S: crate::event_store::AtomicAppend,
    {
        let state = self.state().await?;
        match W::on_deadline(deadline, &state) {
            None => Ok(None),
            Some(command) => self.execute_and_enqueue(command).await.map(Some),
        }
    }

    /// Like [`execute_timeout`] but retries on [`VersionConflict`] up to
    /// `max_attempts` times.
    ///
    /// Use this in production deadline workers where concurrent event appends
    /// are expected.  Outbox entries (e.g. APERAK Ablehnung) produced by the
    /// compensation handler are persisted atomically on every attempt.
    ///
    /// [`execute_timeout`]: Process::execute_timeout
    /// [`VersionConflict`]: crate::error::EngineError::VersionConflict
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] on storage or command handling failure.
    ///
    /// # Panics
    ///
    /// Panics if the deadline produces a command but the retry loop somehow
    /// exhausts without capturing an error (unreachable in practice).
    pub async fn execute_timeout_with_retry(
        &self,
        deadline: &crate::deadline::Deadline,
        max_attempts: u32,
    ) -> Result<Option<Vec<EventEnvelope>>, EngineError>
    where
        S: crate::event_store::AtomicAppend,
        W::Command: Clone,
    {
        let state = self.state().await?;
        match W::on_deadline(deadline, &state) {
            None => Ok(None),
            Some(command) => self
                .execute_and_enqueue_with_retry(command, max_attempts)
                .await
                .map(Some),
        }
    }

    /// Like [`execute_and_enqueue`] but retries on [`crate::error::EngineError::VersionConflict`] up to
    /// `max_attempts` times.
    ///
    /// [`execute_and_enqueue`]: Process::execute_and_enqueue
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] on storage or command handling failure.
    ///
    /// # Panics
    ///
    /// Panics if `max_attempts` is 0 and the guard is bypassed (unreachable).
    pub async fn execute_and_enqueue_with_retry(
        &self,
        command: W::Command,
        max_attempts: u32,
    ) -> Result<Vec<EventEnvelope>, EngineError>
    where
        S: crate::event_store::AtomicAppend,
        W::Command: Clone,
    {
        if max_attempts == 0 {
            return Err(EngineError::store("max_attempts must be >= 1"));
        }
        let ctx = CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone());
        let mut conflict_err: Option<EngineError> = None;
        for _ in 0..max_attempts {
            match crate::workflow::execute_command_atomic::<W, S>(
                &self.store,
                &self.stream_id,
                command.clone(),
                &ctx,
            )
            .await
            {
                Ok(envs) => return Ok(envs),
                Err(e) if e.is_version_conflict() => conflict_err = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(conflict_err.expect("loop ran at least once"))
    }

    /// Execute `command` atomically with outbox, then automatically snapshot
    /// if the event-count threshold is reached.
    ///
    /// Combines [`execute_and_enqueue`] with [`take_snapshot`]: after a
    /// successful write, checks whether `event_count % snapshot_interval == 0`
    /// and, if so, serialises and saves a snapshot via `snap_store`.
    ///
    /// Pass `snapshot_interval = 0` to disable auto-snapshotting; the call
    /// then behaves identically to [`execute_and_enqueue`].
    ///
    /// Returns `(events, snapshot_taken)` where `snapshot_taken` is `true` when
    /// a snapshot was written this call.
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] — stream was modified concurrently;
    ///   retry with [`execute_and_enqueue_with_retry`].
    /// - [`EngineError::Workflow`] — the command was rejected by the workflow.
    /// - [`EngineError::Store`] / [`EngineError::Outbox`] — storage failure.
    /// - [`EngineError::Serialization`] — state serialisation failed during snapshot.
    ///
    /// [`execute_and_enqueue`]: Process::execute_and_enqueue
    /// [`take_snapshot`]: Process::take_snapshot
    /// [`execute_and_enqueue_with_retry`]: Process::execute_and_enqueue_with_retry
    pub async fn execute_and_enqueue_with_snapshot<Snap>(
        &self,
        command: W::Command,
        snap_store: &Snap,
        snapshot_interval: u64,
    ) -> Result<(Vec<EventEnvelope>, bool), EngineError>
    where
        S: crate::event_store::AtomicAppend,
        Snap: crate::snapshot::SnapshotStore,
        W::State: serde::Serialize,
    {
        let events = self.execute_and_enqueue(command).await?;
        let snapped = if snapshot_interval > 0 {
            self.take_snapshot(snap_store, snapshot_interval).await?
        } else {
            false
        };
        Ok((events, snapped))
    }

    /// Like [`execute_and_enqueue_with_snapshot`] but retries on
    /// [`crate::error::EngineError::VersionConflict`] up to `max_attempts` times.
    ///
    /// [`execute_and_enqueue_with_retry`]: Process::execute_and_enqueue_with_retry
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] — stream was modified concurrently;
    ///   retry with [`execute_and_enqueue_with_snapshot_and_retry`].
    /// - [`EngineError::Workflow`] — the command was rejected by the workflow.
    /// - [`EngineError::Store`] / [`EngineError::Outbox`] — storage failure.
    /// - [`EngineError::Serialization`] — state serialisation failed during snapshot.
    ///
    /// # Panics
    ///
    /// Panics if `max_attempts` is 0 and the loop guard is bypassed (unreachable).
    ///
    /// [`execute_and_enqueue_with_snapshot`]: Process::execute_and_enqueue_with_snapshot
    /// [`execute_and_enqueue_with_snapshot_and_retry`]: Process::execute_and_enqueue_with_snapshot_and_retry
    pub async fn execute_and_enqueue_with_snapshot_and_retry<Snap>(
        &self,
        command: W::Command,
        max_attempts: u32,
        snap_store: &Snap,
        snapshot_interval: u64,
    ) -> Result<(Vec<EventEnvelope>, bool), EngineError>
    where
        S: crate::event_store::AtomicAppend,
        W::Command: Clone,
        Snap: crate::snapshot::SnapshotStore,
        W::State: serde::Serialize,
    {
        let events = self
            .execute_and_enqueue_with_retry(command, max_attempts)
            .await?;
        let snapped = if snapshot_interval > 0 {
            self.take_snapshot(snap_store, snapshot_interval).await?
        } else {
            false
        };
        Ok((events, snapped))
    }
}

impl<W: Workflow, S: EventStore + Clone> Clone for Process<W, S> {
    fn clone(&self) -> Self {
        Self {
            stream_id: self.stream_id.clone(),
            process_id: self.process_id,
            tenant_id: self.tenant_id,
            workflow_id: self.workflow_id.clone(),
            store: self.store.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<W: Workflow, S: EventStore + std::fmt::Debug> std::fmt::Debug for Process<W, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process")
            .field("stream_id", &self.stream_id)
            .field("process_id", &self.process_id)
            .field("workflow_id", &self.workflow_id)
            .finish_non_exhaustive()
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        envelope::NewEvent,
        error::WorkflowError,
        event_store::{EventStore, ExpectedVersion, InMemoryEventStore},
        ids::{ConversationId, CorrelationId, TenantId},
        snapshot::{InMemorySnapshotStore, NoopSnapshotStore},
        version::WorkflowId,
        workflow::{CommandPayload, EventPayload},
    };

    // ── Minimal test workflow ─────────────────────────────────────────────────

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
    enum CounterEvent {
        Incremented { by: u32 },
        Reset,
    }

    impl EventPayload for CounterEvent {
        fn event_type(&self) -> &'static str {
            match self {
                Self::Incremented { .. } => "Incremented",
                Self::Reset => "Reset",
            }
        }
    }

    #[derive(Debug, Clone)]
    enum CounterCommand {
        Increment { by: u32 },
        Reset,
    }

    impl CommandPayload for CounterCommand {}

    #[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
    struct CounterState {
        value: u32,
    }

    struct CounterWorkflow;

    impl Workflow for CounterWorkflow {
        type State = CounterState;
        type Event = CounterEvent;
        type Command = CounterCommand;

        fn apply(mut state: CounterState, event: &CounterEvent) -> CounterState {
            match event {
                CounterEvent::Incremented { by } => state.value += by,
                CounterEvent::Reset => state.value = 0,
            }
            state
        }

        fn handle(
            _state: &CounterState,
            command: CounterCommand,
        ) -> Result<crate::workflow::WorkflowOutput<CounterEvent>, WorkflowError> {
            Ok(match command {
                CounterCommand::Increment { by } => vec![CounterEvent::Incremented { by }].into(),
                CounterCommand::Reset => vec![CounterEvent::Reset].into(),
            })
        }
    }

    fn make_process() -> Process<CounterWorkflow, InMemoryEventStore> {
        Process::new(
            InMemoryEventStore::new(),
            TenantId::new(),
            WorkflowId::new("counter", "FV2024-10-01"),
        )
    }

    // ── execute + state round-trip ────────────────────────────────────────────

    #[tokio::test]
    async fn execute_then_state_round_trip() {
        let p = make_process();

        p.execute(CounterCommand::Increment { by: 3 })
            .await
            .unwrap();
        p.execute(CounterCommand::Increment { by: 7 })
            .await
            .unwrap();

        let state = p.state().await.unwrap();
        assert_eq!(state.value, 10);
    }

    #[tokio::test]
    async fn event_count_matches_dispatched_commands() {
        let p = make_process();

        assert_eq!(p.event_count().await.unwrap(), 0);
        p.execute(CounterCommand::Increment { by: 1 })
            .await
            .unwrap();
        assert_eq!(p.event_count().await.unwrap(), 1);
        p.execute(CounterCommand::Reset).await.unwrap();
        assert_eq!(p.event_count().await.unwrap(), 2);
    }

    // ── identity round-trip ───────────────────────────────────────────────────

    #[tokio::test]
    async fn identity_round_trip_via_from_identity() {
        let store = InMemoryEventStore::new();
        let p1 = Process::<CounterWorkflow, _>::new(
            store.clone(),
            TenantId::new(),
            WorkflowId::new("counter", "FV2024-10-01"),
        );

        p1.execute(CounterCommand::Increment { by: 5 })
            .await
            .unwrap();

        let identity = p1.identity();
        assert_eq!(*identity.stream_id(), *p1.stream_id());
        assert_eq!(identity.process_id, p1.process_id());

        // Re-attach from identity and confirm state is visible.
        let p2 = Process::<CounterWorkflow, _>::from_identity(store, identity);
        let state = p2.state().await.unwrap();
        assert_eq!(state.value, 5);
    }

    #[test]
    fn process_identity_is_serializable() {
        let p = make_process();
        let id = p.identity();
        let json = serde_json::to_string(&id).expect("ProcessIdentity must be serializable");
        let back: ProcessIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(*back.stream_id(), *id.stream_id());
        assert_eq!(back.process_id, id.process_id);
    }

    // ── snapshot-accelerated state reconstruction ─────────────────────────────

    #[tokio::test]
    async fn take_snapshot_and_state_with_snapshot() {
        let snap_store = InMemorySnapshotStore::new();
        let p = make_process();

        // Dispatch 4 commands; the interval is 4.
        for i in 1u32..=4 {
            p.execute(CounterCommand::Increment { by: i })
                .await
                .unwrap();
        }

        let took = p.take_snapshot(&snap_store, 4).await.unwrap();
        assert!(took, "snapshot must be taken at event_count = 4");

        // Dispatch one more command after the snapshot.
        p.execute(CounterCommand::Increment { by: 10 })
            .await
            .unwrap();

        let state = p.state_with_snapshot(&snap_store).await.unwrap();
        // 1+2+3+4 = 10, plus the final +10 = 20.
        assert_eq!(state.value, 20);
    }

    #[tokio::test]
    async fn state_with_snapshot_falls_back_to_full_replay() {
        let p = make_process();
        p.execute(CounterCommand::Increment { by: 42 })
            .await
            .unwrap();

        // NoopSnapshotStore always returns None → full replay.
        let state = p.state_with_snapshot(&NoopSnapshotStore).await.unwrap();
        assert_eq!(state.value, 42);
    }

    #[tokio::test]
    async fn take_snapshot_skipped_between_intervals() {
        let snap_store = InMemorySnapshotStore::new();
        let p = make_process();

        p.execute(CounterCommand::Increment { by: 1 })
            .await
            .unwrap();
        p.execute(CounterCommand::Increment { by: 1 })
            .await
            .unwrap();
        p.execute(CounterCommand::Increment { by: 1 })
            .await
            .unwrap();

        // 3 events, interval = 4 → must not take.
        let took = p.take_snapshot(&snap_store, 4).await.unwrap();
        assert!(!took);
        assert!(snap_store.is_empty().await);
    }

    /// Regression test for when a persisted snapshot carries a
    /// `state_schema_version` that does not match the current workflow's
    /// `state_schema_version()`, `state_with_snapshot` must silently discard
    /// the stale snapshot and fall back to full event replay.
    ///
    /// This guards against silent data corruption when state layout changes
    /// incompatibly — e.g. after adding a new required field to `CounterState`.
    #[tokio::test]
    async fn stale_snapshot_schema_version_falls_back_to_full_replay() {
        // CounterWorkflow uses state_schema_version() == 1 (the default).
        // We simulate a "migrated" workflow by injecting a snapshot whose
        // state_schema_version is bumped to 99, representing a schema that the
        // current workflow code does not understand.
        let snap_store = InMemorySnapshotStore::new();
        let p = make_process();

        // Dispatch some events so there is something to replay.
        p.execute(CounterCommand::Increment { by: 5 })
            .await
            .unwrap();
        p.execute(CounterCommand::Increment { by: 3 })
            .await
            .unwrap();

        // Manually save a stale snapshot with schema_version = 99.
        // The state payload is intentionally wrong — it should never be used.
        let stale = crate::snapshot::Snapshot::new(
            p.stream_id().clone(),
            2,                                    // sequence_number after 2 events
            99,                                   // ← unknown schema version
            serde_json::json!({ "value": 9999 }), // ← wrong value; must not be read
        );
        snap_store.save(&stale).await.unwrap();

        // state_with_snapshot must discard the stale snapshot and replay all
        // events from sequence 0, producing the correct state (5+3=8).
        let current_state = p.state_with_snapshot(&snap_store).await.unwrap();
        assert_eq!(
            current_state.value, 8,
            "stale snapshot must be discarded; full replay must yield correct state"
        );
    }

    // ── execute_with_retry ────────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_with_retry_succeeds_on_first_attempt() {
        let p = make_process();
        let envs = p
            .execute_with_retry(CounterCommand::Increment { by: 99 }, 3)
            .await
            .unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(p.state().await.unwrap().value, 99);
    }

    #[tokio::test]
    async fn execute_with_retry_returns_err_on_zero_attempts() {
        let p = make_process();
        let err = p
            .execute_with_retry(CounterCommand::Increment { by: 1 }, 0)
            .await
            .unwrap_err();
        assert!(
            matches!(err, EngineError::Store { ref message, .. } if message.contains("max_attempts")),
            "expected Store error about max_attempts, got: {err:?}",
        );
    }

    // ── execute_with (explicit context) ──────────────────────────────────────

    #[tokio::test]
    async fn execute_with_explicit_context_propagates_ids() {
        use crate::ids::{ConversationId, CorrelationId};
        let p = make_process();

        let corr = CorrelationId::new();
        let conv = ConversationId::new();
        let ctx = CommandContext::new(p.tenant_id(), p.process_id(), p.workflow_id().clone())
            .with_correlation(corr)
            .with_conversation(conv);

        let envs = p
            .execute_with(CounterCommand::Increment { by: 1 }, ctx)
            .await
            .unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].correlation_id, corr);
        assert_eq!(envs[0].conversation_id, conv);
    }

    // ── upcast / schema-migration ─────────────────────────────────────────────
    //
    // A v2 workflow adds a `label: String` field to its single event.
    // Old (v1) events stored without `label` must be migrated by `upcast`.
    //
    // `#[serde(untagged)]` is used so the serialized payload is the flat
    // inner struct `{"count": N, "label": "..."}` rather than the externally-
    // tagged `{"Tagged": {"count": N}}` form.  This matches the common
    // real-world pattern where each `EventPayload::event_type()` discriminant
    // IS the variant selector stored in the envelope, and the payload holds
    // only the fields.

    #[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
    struct TagState {
        total: u32,
        last_label: String,
    }

    /// v1 schema (legacy): `{ "count": u32 }` — `label` field absent.
    /// v2 schema: `{ "count": u32, "label": String }`.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
    #[serde(untagged)]
    enum TagEvent {
        Tagged { count: u32, label: String },
    }

    impl EventPayload for TagEvent {
        fn event_type(&self) -> &'static str {
            "Tagged"
        }
        fn schema_version(&self) -> u32 {
            2
        }
    }

    #[derive(Debug, Clone)]
    struct TagCommand {
        count: u32,
        label: String,
    }
    impl CommandPayload for TagCommand {}

    struct TagWorkflow;

    impl Workflow for TagWorkflow {
        type State = TagState;
        type Event = TagEvent;
        type Command = TagCommand;

        fn apply(mut state: TagState, event: &TagEvent) -> TagState {
            let TagEvent::Tagged { count, label } = event;
            state.total += count;
            state.last_label = label.clone();
            state
        }

        fn handle(
            _state: &TagState,
            cmd: TagCommand,
        ) -> Result<crate::workflow::WorkflowOutput<TagEvent>, WorkflowError> {
            Ok(vec![TagEvent::Tagged {
                count: cmd.count,
                label: cmd.label,
            }]
            .into())
        }

        /// Migrate v1 `Tagged` events (missing `label`) to v2.
        ///
        /// v1 payload: `{"count": N}` (no `label` field)
        /// v2 payload: `{"count": N, "label": ""}` (default empty string)
        ///
        /// Because the event uses `#[serde(untagged)]`, the envelope payload
        /// is the flat struct — variant discrimination comes from `event_type`.
        fn upcast(
            event_type: &str,
            from_version: u32,
            mut payload: serde_json::Value,
        ) -> Result<serde_json::Value, EngineError> {
            if event_type == "Tagged" && from_version == 1 {
                if let Some(obj) = payload.as_object_mut() {
                    obj.entry("label")
                        .or_insert_with(|| serde_json::Value::String(String::new()));
                }
            }
            Ok(payload)
        }
    }

    /// Inject a raw v1 event (no `label` field) directly into the store and
    /// confirm that `state()` replays it correctly via `upcast`.
    #[tokio::test]
    async fn upcast_v1_event_adds_default_label() {
        let store = InMemoryEventStore::new();
        let p = Process::<TagWorkflow, _>::new(
            store.clone(), // shares the underlying Arc<RwLock<_>>
            TenantId::new(),
            WorkflowId::new("tag", "FV2025-10-01"),
        );

        // v1 payload: flat struct fields, no `label` (untagged serde repr).
        let v1_payload = serde_json::json!({ "count": 7 });
        let raw = NewEvent {
            correlation_id: CorrelationId::new(),
            causation_id: None,
            conversation_id: ConversationId::new(),
            process_id: p.process_id(),
            tenant_id: p.tenant_id(),
            workflow_id: p.workflow_id().clone(),
            event_type: "Tagged".into(),
            schema_version: 1, // ← schema_version 1 (old format)
            payload: v1_payload,
        };
        store
            .append(p.stream_id(), ExpectedVersion::Any, &[raw])
            .await
            .expect("inject v1 event");

        // Replay via the v2 workflow — `upcast` must fill in `label: ""`.
        let state = p.state().await.expect("state must replay without error");
        assert_eq!(state.total, 7, "count must be accumulated");
        assert_eq!(
            state.last_label, "",
            "missing v1 label must default to empty string"
        );

        // Also verify that a normally-executed v2 event round-trips correctly.
        p.execute(TagCommand {
            count: 3,
            label: "hello".into(),
        })
        .await
        .unwrap();
        let state2 = p.state().await.unwrap();
        assert_eq!(state2.total, 10);
        assert_eq!(state2.last_label, "hello");
    }
}
