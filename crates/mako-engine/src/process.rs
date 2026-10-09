//! [`Process`] — ergonomic typed handle for a single MaKo process instance.
//!
//! Instead of threading `stream_id`, `workflow_id`, `tenant_id`, and a store
//! reference through every call to the write path, bind them once into a
//! `Process<W, S>` and call [`run`] / [`state`] directly.
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
//! let envelopes = process.run(my_command, RunOptions::new()).await?;
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
//! [`run`]: Process::run
//! [`state`]: Process::state

use std::marker::PhantomData;

use crate::{
    envelope::EventEnvelope,
    error::EngineError,
    event_store::AtomicAppend,
    event_store::EventStore,
    ids::{ProcessId, ProcessIdentity, StreamId, TenantId},
    snapshot::{NoSnapshot, Snapshot, SnapshotStore},
    version::WorkflowId,
    workflow::{CommandContext, Workflow, decide_and_append},
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
/// - `S` — the [`EventStore`] backend. `InMemoryEventStore` (requires `testing` feature) is the default
///   for tests; production deployments wrap a persistent backend in
///   [`Arc`][std::sync::Arc] and use `Process<W, Arc<MyStore>>`.
///
/// ## Clone semantics
///
/// If `S: Clone` (e.g. `InMemoryEventStore` (requires `testing` feature) or `Arc<…>`), `Process` is also
/// `Clone` and all clones share the same underlying storage.
#[expect(
    clippy::struct_field_names,
    reason = "`process_id` and `stream_id` name engine-layer concepts, not redundant prefixes"
)]
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
    /// Pass it to [`RunOptions::context`] when you need to propagate
    /// EDIFACT correlation metadata into the event stream. The returned
    /// context is passed to [`Process::run`] through the options.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let process = ctx.resume::<GpkeSupplierChangeWorkflow>(identity);
    /// let cmd_ctx = process.context_for_inbound(&utilmd_interchange_ref);
    /// process.run(command, RunOptions::new().context(cmd_ctx)).await?;
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
    /// process.run(command, RunOptions::new()).await?;
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

    // ── Running a command ────────────────────────────────────────────────────

    /// Run `command` and persist everything it produces in one atomic write.
    ///
    /// The one way a command reaches a process. The state is replayed, the
    /// workflow handles the command, and its events, outbox entries and
    /// deadlines — with the extra deadlines and correlation entries `opts`
    /// carries — land in one [`AtomicAppend::append`], or none does. Nothing a
    /// workflow returns can be dropped by the choice of call, because there is
    /// no other call.
    ///
    /// A [`VersionConflict`] is retried up to [`RunOptions::attempts`] times,
    /// each attempt on freshly replayed state after a jittered back-off: the
    /// conflicting write may change what `handle` decides. Every attempt
    /// carries the same [`CommandContext`], so the correlation root does not
    /// depend on the retry count.
    ///
    /// ```rust,ignore
    /// let ran = process.run(command, RunOptions::new()).await?;
    ///
    /// // A spawn: the process's Frist and business key in the same write.
    /// process
    ///     .run(command, RunOptions::new().deadlines(&frist).correlations(&keys))
    ///     .await?;
    /// ```
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] — every attempt lost the race.
    /// - [`EngineError::Workflow`] — the workflow rejected the command.
    /// - [`EngineError::UnownedDeadlineLabel`] — a deadline under a label the
    ///   workflow does not own.
    /// - [`EngineError::UnrecordedEffect`] — an outbox entry or deadline
    ///   without an event.
    /// - [`EngineError::Deserialization`] — a stored event cannot be decoded.
    /// - Storage errors from the store; nothing was written.
    ///
    /// [`VersionConflict`]: EngineError::VersionConflict
    /// [`AtomicAppend::append`]: crate::event_store::AtomicAppend::append
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, command, opts), fields(
            workflow = %self.workflow_id,
            process_id = %self.process_id,
            stream_id = %self.stream_id,
        ))
    )]
    pub async fn run<Snap>(
        &self,
        command: W::Command,
        opts: RunOptions<'_, Snap>,
    ) -> Result<RunOutcome, EngineError>
    where
        S: AtomicAppend,
        Snap: SnapshotAfterRun<W>,
    {
        let ran = self
            .decide_and_run(&mut move |_: &W::State| Some(command.clone()), opts)
            .await?;
        Ok(ran.unwrap_or_default())
    }

    /// Run the command [`Workflow::on_deadline`] decides for a fired deadline.
    ///
    /// The deadline-shaped entry to [`Process::run`]: the same write, the same
    /// options, the same retry — only the command is not the caller's but the
    /// workflow's own, decided on the current state. A label outside
    /// [`Workflow::DEADLINE_LABELS`] — a delivery window, or a label the
    /// workflow never registered — is answered `Ok(None)` without consulting
    /// the hook.
    ///
    /// Every attempt replays the state and asks `on_deadline` again: the write
    /// that caused a conflict may be the very answer that closed the window, and
    /// a decision taken on the stale state would act on it anyway.
    ///
    /// Returns `Ok(Some(_))` when the hook acted and `Ok(None)` when the label
    /// is not the workflow's or the window was already closed.
    ///
    /// # Errors
    ///
    /// As [`Process::run`].
    ///
    /// [`Workflow::on_deadline`]: crate::workflow::Workflow::on_deadline
    /// [`Workflow::DEADLINE_LABELS`]: crate::workflow::Workflow::DEADLINE_LABELS
    pub async fn run_timeout<Snap>(
        &self,
        deadline: &crate::deadline::Deadline,
        opts: RunOptions<'_, Snap>,
    ) -> Result<Option<RunOutcome>, EngineError>
    where
        S: AtomicAppend,
        Snap: SnapshotAfterRun<W>,
    {
        if !W::DEADLINE_LABELS.contains(&deadline.label()) {
            return Ok(None);
        }
        self.decide_and_run(
            &mut |state: &W::State| W::on_deadline(deadline, state),
            opts,
        )
        .await
    }

    /// The loop both entries share: decide on fresh state, append, retry a
    /// version conflict, then take the snapshot `opts` asks for.
    async fn decide_and_run<Snap>(
        &self,
        decide: &mut (impl FnMut(&W::State) -> Option<W::Command> + Send),
        opts: RunOptions<'_, Snap>,
    ) -> Result<Option<RunOutcome>, EngineError>
    where
        S: AtomicAppend,
        Snap: SnapshotAfterRun<W>,
    {
        // Pinned before the loop: every attempt shares one correlation root.
        let ctx = opts.context.clone().unwrap_or_else(|| {
            CommandContext::new(self.tenant_id, self.process_id, self.workflow_id.clone())
        });
        let attempts = opts.attempts.max(1);
        let mut attempt = 0;
        let decided = loop {
            // State is replayed from the log on every attempt. Never carry it
            // across attempts: the winning writer's events must be visible to
            // the retry, or it decides on a state that no longer exists.
            match decide_and_append::<W, S>(
                &self.store,
                &self.stream_id,
                &ctx,
                decide,
                opts.deadlines,
                opts.correlations,
            )
            .await
            {
                Err(e) if e.is_version_conflict() && attempt + 1 < attempts => {
                    // Back off before re-reading: several writers on one
                    // Marktlokation would otherwise spin against each other and
                    // spend the budget in a few milliseconds. Uniform in
                    // [0, 10 ms × attempt], capped at 80 ms, from the OS CSPRNG
                    // so two writers that collided do not re-collide.
                    let window_ms: u64 = (10 * (u64::from(attempt) + 1)).min(80);
                    let jitter_ms = rand::random::<u64>() % window_ms;
                    tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;
                    attempt += 1;
                }
                other => break other?,
            }
        };
        let Some((appended, deadlines)) = decided else {
            return Ok(None);
        };

        let snapshot_taken = match opts.snapshot {
            Some((snap_store, interval)) if interval > 0 => {
                // A snapshot accelerates reads; it is not part of the write's
                // durability. The write has committed, so a failed snapshot is
                // reported, never returned as the command's failure — a caller
                // retrying it would run the command twice.
                match snap_store.snapshot_after(self, interval).await {
                    Ok(taken) => taken,
                    Err(e) => {
                        tracing::warn!(
                            process_id = %self.process_id,
                            error = %e,
                            "snapshot after a committed command failed (non-fatal — replay is slower)",
                        );
                        false
                    }
                }
            }
            _ => false,
        };

        Ok(Some(RunOutcome {
            events: appended.events,
            outbox: appended.outbox,
            deadlines,
            snapshot_taken,
        }))
    }
}

// ── SnapshotAfterRun ──────────────────────────────────────────────────────────

/// A snapshot store [`RunOptions::snapshot`] can name.
///
/// Implemented for every [`SnapshotStore`] — which needs the state to
/// serialise — and for [`NoSnapshot`], which needs nothing, so a workflow
/// whose state is never snapshotted still runs.
#[expect(
    async_fn_in_trait,
    reason = "the trait is implemented and awaited within this workspace, where the future's Send bound is known"
)]
pub trait SnapshotAfterRun<W: Workflow> {
    /// Snapshot `process` into `self` when `interval` events have accumulated.
    ///
    /// # Errors
    ///
    /// As [`Process::take_snapshot`].
    async fn snapshot_after<S: EventStore>(
        &self,
        process: &Process<W, S>,
        interval: u64,
    ) -> Result<bool, EngineError>;
}

impl<W: Workflow> SnapshotAfterRun<W> for NoSnapshot {
    async fn snapshot_after<S: EventStore>(
        &self,
        _process: &Process<W, S>,
        _interval: u64,
    ) -> Result<bool, EngineError> {
        match *self {}
    }
}

impl<W, T> SnapshotAfterRun<W> for T
where
    W: Workflow,
    W::State: serde::Serialize,
    T: SnapshotStore,
{
    async fn snapshot_after<S: EventStore>(
        &self,
        process: &Process<W, S>,
        interval: u64,
    ) -> Result<bool, EngineError> {
        process.take_snapshot(self, interval).await
    }
}

// ── RunOptions ────────────────────────────────────────────────────────────────

/// How [`Process::run`] runs a command: everything that is an option rather
/// than a different method.
///
/// ```rust,ignore
/// RunOptions::new()                              // 3 attempts, fresh context
///     .context(ctx)                              // propagate an inbound correlation
///     .deadlines(&spawn_frist)                   // caller-computed Fristen
///     .correlations(&business_keys)              // index the process in the same write
///     .snapshot(&snap_store, 100)                // snapshot every 100 events
/// ```
#[derive(Debug)]
#[must_use = "RunOptions does nothing until passed to Process::run"]
pub struct RunOptions<'a, Snap = NoSnapshot> {
    context: Option<CommandContext>,
    attempts: u32,
    deadlines: &'a [crate::deadline::Deadline],
    correlations: &'a [crate::event_store::CorrelationEntry],
    snapshot: Option<(&'a Snap, u64)>,
}

/// The attempts [`RunOptions::new`] allows a command before a version
/// conflict is returned.
pub const DEFAULT_ATTEMPTS: u32 = 3;

impl RunOptions<'_, NoSnapshot> {
    /// [`DEFAULT_ATTEMPTS`] attempts, a fresh [`CommandContext`], no extra
    /// deadlines or correlation entries, no snapshot.
    pub fn new() -> Self {
        Self {
            context: None,
            attempts: DEFAULT_ATTEMPTS,
            deadlines: &[],
            correlations: &[],
            snapshot: None,
        }
    }
}

impl Default for RunOptions<'_, NoSnapshot> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, Snap> RunOptions<'a, Snap> {
    /// Stamp the events with `ctx` — e.g. one from
    /// [`Process::context_for_inbound`] or [`CommandContext::from_envelope`] —
    /// instead of a fresh correlation root.
    pub fn context(mut self, ctx: CommandContext) -> Self {
        self.context = Some(ctx);
        self
    }

    /// Allow `attempts` tries against a version conflict. `0` is read as `1`.
    pub fn attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }

    /// Register `deadlines` in the same write as the command's events, beside
    /// those `handle` returns. Each label must be one the workflow owns.
    pub fn deadlines(mut self, deadlines: &'a [crate::deadline::Deadline]) -> Self {
        self.deadlines = deadlines;
        self
    }

    /// Index the process under `correlations` in the same write — the spawn
    /// shape: a process its counterparty's reply cannot find is not running.
    pub fn correlations(
        mut self,
        correlations: &'a [crate::event_store::CorrelationEntry],
    ) -> Self {
        self.correlations = correlations;
        self
    }

    /// After the write, snapshot the state into `store` once `interval`
    /// events have accumulated since the last snapshot.
    pub fn snapshot<S2: SnapshotStore>(self, store: &'a S2, interval: u64) -> RunOptions<'a, S2> {
        RunOptions {
            context: self.context,
            attempts: self.attempts,
            deadlines: self.deadlines,
            correlations: self.correlations,
            snapshot: Some((store, interval)),
        }
    }
}

// ── RunOutcome ────────────────────────────────────────────────────────────────

/// What one [`Process::run`] wrote.
///
/// Dereferences to the persisted events, so `outcome.len()`, indexing and
/// iteration read the events directly.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    /// The persisted events, stamped by the store. Empty when the command
    /// changed nothing.
    pub events: Vec<EventEnvelope>,
    /// The outbox entries enqueued with them, as the store materialised them.
    pub outbox: Vec<crate::outbox::OutboxMessage>,
    /// The deadlines registered with them — the caller's and the workflow's.
    pub deadlines: Vec<crate::deadline::Deadline>,
    /// Whether a snapshot was taken after the write.
    pub snapshot_taken: bool,
}

impl std::ops::Deref for RunOutcome {
    type Target = [EventEnvelope];

    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

impl IntoIterator for RunOutcome {
    type Item = EventEnvelope;
    type IntoIter = std::vec::IntoIter<EventEnvelope>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.into_iter()
    }
}

impl<'a> IntoIterator for &'a RunOutcome {
    type Item = &'a EventEnvelope;
    type IntoIter = std::slice::Iter<'a, EventEnvelope>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
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
    use crate::event_store::AppendBatch;
    use crate::{
        envelope::NewEvent,
        error::WorkflowError,
        event_store::{ExpectedVersion, InMemoryEventStore},
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

    // ── run + state round-trip ────────────────────────────────────────────────

    #[tokio::test]
    async fn run_then_state_round_trip() {
        let p = make_process();

        p.run(CounterCommand::Increment { by: 3 }, RunOptions::new())
            .await
            .unwrap();
        p.run(CounterCommand::Increment { by: 7 }, RunOptions::new())
            .await
            .unwrap();

        let state = p.state().await.unwrap();
        assert_eq!(state.value, 10);
    }

    #[tokio::test]
    async fn event_count_matches_dispatched_commands() {
        let p = make_process();

        assert_eq!(p.event_count().await.unwrap(), 0);
        p.run(CounterCommand::Increment { by: 1 }, RunOptions::new())
            .await
            .unwrap();
        assert_eq!(p.event_count().await.unwrap(), 1);
        p.run(CounterCommand::Reset, RunOptions::new())
            .await
            .unwrap();
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

        p1.run(CounterCommand::Increment { by: 5 }, RunOptions::new())
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
            p.run(CounterCommand::Increment { by: i }, RunOptions::new())
                .await
                .unwrap();
        }

        let took = p.take_snapshot(&snap_store, 4).await.unwrap();
        assert!(took, "snapshot must be taken at event_count = 4");

        // Dispatch one more command after the snapshot.
        p.run(CounterCommand::Increment { by: 10 }, RunOptions::new())
            .await
            .unwrap();

        let state = p.state_with_snapshot(&snap_store).await.unwrap();
        // 1+2+3+4 = 10, plus the final +10 = 20.
        assert_eq!(state.value, 20);
    }

    #[tokio::test]
    async fn state_with_snapshot_falls_back_to_full_replay() {
        let p = make_process();
        p.run(CounterCommand::Increment { by: 42 }, RunOptions::new())
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

        p.run(CounterCommand::Increment { by: 1 }, RunOptions::new())
            .await
            .unwrap();
        p.run(CounterCommand::Increment { by: 1 }, RunOptions::new())
            .await
            .unwrap();
        p.run(CounterCommand::Increment { by: 1 }, RunOptions::new())
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
        p.run(CounterCommand::Increment { by: 5 }, RunOptions::new())
            .await
            .unwrap();
        p.run(CounterCommand::Increment { by: 3 }, RunOptions::new())
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

    // ── run: one write for the whole output ─────────────────────────────────

    /// A workflow whose `handle` returns an event, an outbox entry and a
    /// deadline — or, on `Bare`, the effects without an event.
    struct FullWorkflow;

    #[derive(Debug, Clone)]
    enum FullCommand {
        Spawn,
        Bare,
    }
    impl CommandPayload for FullCommand {}

    impl Workflow for FullWorkflow {
        type State = CounterState;
        type Event = CounterEvent;
        type Command = FullCommand;

        const DEADLINE_LABELS: &'static [&'static str] = &["antwort", "spawn-frist"];

        fn apply(state: CounterState, event: &CounterEvent) -> CounterState {
            CounterWorkflow::apply(state, event)
        }

        fn handle(
            _state: &CounterState,
            command: FullCommand,
        ) -> Result<crate::workflow::WorkflowOutput<CounterEvent>, WorkflowError> {
            let events = match command {
                FullCommand::Spawn => vec![CounterEvent::Incremented { by: 1 }],
                FullCommand::Bare => Vec::new(),
            };
            Ok(crate::workflow::WorkflowOutput::with_outbox_and_deadline(
                events,
                vec![crate::outbox::PendingOutbox::new(
                    "UTILMD",
                    "9900000000001",
                    serde_json::json!({}),
                )],
                crate::workflow::PendingDeadline::new("antwort", time::OffsetDateTime::UNIX_EPOCH),
            ))
        }
    }

    /// Events, outbox, the workflow's and the caller's deadlines, and the
    /// caller's correlation entries all land through the one `run`.
    #[tokio::test]
    async fn run_lands_events_outbox_deadlines_and_correlations_together() {
        let store = InMemoryEventStore::new();
        let p = Process::<FullWorkflow, _>::new(
            store.clone(),
            TenantId::new(),
            WorkflowId::new("full", "FV2025-10-01"),
        );
        let frist = crate::deadline::Deadline::new(
            p.stream_id().clone(),
            p.process_id(),
            p.tenant_id(),
            p.workflow_id().clone(),
            "spawn-frist",
            time::OffsetDateTime::UNIX_EPOCH,
        );
        let key = crate::event_store::CorrelationEntry {
            tenant_id: p.tenant_id(),
            tag: "51238696012".into(),
            process_id: p.process_id(),
            identity: p.identity(),
        };

        let ran = p
            .run(
                FullCommand::Spawn,
                RunOptions::new()
                    .deadlines(std::slice::from_ref(&frist))
                    .correlations(std::slice::from_ref(&key)),
            )
            .await
            .unwrap();

        assert_eq!(ran.len(), 1, "the event");
        assert_eq!(ran.outbox.len(), 1, "the outbox entry");
        assert_eq!(ran.outbox[0].causation_event_id, ran[0].event_id);
        assert_eq!(store.outbox_messages().await.len(), 1);
        let labels: Vec<String> = store
            .deadlines_for(p.stream_id())
            .await
            .iter()
            .map(|d| d.label().to_owned())
            .collect();
        assert_eq!(labels, ["spawn-frist", "antwort"], "both deadlines");
        assert_eq!(ran.deadlines.len(), 2);
        assert_eq!(store.correlations().await.len(), 1, "the business key");
    }

    /// An outbox entry or deadline without an event is refused, not dropped.
    #[tokio::test]
    async fn an_effect_without_an_event_is_refused() {
        let store = InMemoryEventStore::new();
        let p = Process::<FullWorkflow, _>::new(
            store.clone(),
            TenantId::new(),
            WorkflowId::new("full", "FV2025-10-01"),
        );
        let err = p
            .run(FullCommand::Bare, RunOptions::new())
            .await
            .expect_err("an unrecorded effect is refused");
        assert!(
            matches!(err, EngineError::UnrecordedEffect { .. }),
            "{err:?}"
        );
        assert!(store.outbox_messages().await.is_empty());
        assert!(store.deadlines_for(p.stream_id()).await.is_empty());
    }

    /// `attempts(0)` still runs the command once.
    #[tokio::test]
    async fn zero_attempts_run_once() {
        let p = make_process();
        let envs = p
            .run(
                CounterCommand::Increment { by: 99 },
                RunOptions::new().attempts(0),
            )
            .await
            .unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(p.state().await.unwrap().value, 99);
    }

    /// A snapshot named in the options is taken after the write.
    #[tokio::test]
    async fn run_takes_the_snapshot_it_is_asked_for() {
        let snap_store = InMemorySnapshotStore::new();
        let p = make_process();
        let first = p
            .run(
                CounterCommand::Increment { by: 1 },
                RunOptions::new().snapshot(&snap_store, 2),
            )
            .await
            .unwrap();
        assert!(!first.snapshot_taken, "one event is below the interval");
        let second = p
            .run(
                CounterCommand::Increment { by: 1 },
                RunOptions::new().snapshot(&snap_store, 2),
            )
            .await
            .unwrap();
        assert!(second.snapshot_taken);
        assert_eq!(p.state_with_snapshot(&snap_store).await.unwrap().value, 2);
    }

    // ── run with an explicit context ─────────────────────────────────────────

    #[tokio::test]
    async fn run_with_explicit_context_propagates_ids() {
        use crate::ids::{ConversationId, CorrelationId};
        let p = make_process();

        let corr = CorrelationId::new();
        let conv = ConversationId::new();
        let ctx = CommandContext::new(p.tenant_id(), p.process_id(), p.workflow_id().clone())
            .with_correlation(corr)
            .with_conversation(conv);

        let envs = p
            .run(
                CounterCommand::Increment { by: 1 },
                RunOptions::new().context(ctx),
            )
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
            if event_type == "Tagged"
                && from_version == 1
                && let Some(obj) = payload.as_object_mut()
            {
                obj.entry("label")
                    .or_insert_with(|| serde_json::Value::String(String::new()));
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
            .append(AppendBatch::new(
                p.stream_id(),
                ExpectedVersion::Any,
                &[raw],
            ))
            .await
            .expect("inject v1 event");

        // Replay via the v2 workflow — `upcast` must fill in `label: ""`.
        let state = p.state().await.expect("state must replay without error");
        assert_eq!(state.total, 7, "count must be accumulated");
        assert_eq!(
            state.last_label, "",
            "missing v1 label must default to empty string"
        );

        // Also verify that a normally-run v2 event round-trips correctly.
        p.run(
            TagCommand {
                count: 3,
                label: "hello".into(),
            },
            RunOptions::new(),
        )
        .await
        .unwrap();
        let state2 = p.state().await.unwrap();
        assert_eq!(state2.total, 10);
        assert_eq!(state2.last_label, "hello");
    }

    // ── One timeout guard ─────────────────────────────────────────────────────

    #[cfg(feature = "slatedb")]
    mod one_guard {
        use super::*;
        use crate::store_slatedb::SlateDbStore;
        use std::sync::Arc;

        /// A workflow with one window, open while the counter is zero, whose hook
        /// would answer *any* label — so a test can tell the engine's filter from
        /// the hook's.
        struct WindowWorkflow;

        #[derive(Debug, Clone)]
        enum WindowCommand {
            Arm { label: &'static str },
            Lapse,
        }

        impl CommandPayload for WindowCommand {}

        impl Workflow for WindowWorkflow {
            type State = CounterState;
            type Event = CounterEvent;
            type Command = WindowCommand;

            const DEADLINE_LABELS: &'static [&'static str] = &["window"];

            fn on_deadline(
                _deadline: &crate::deadline::Deadline,
                state: &CounterState,
            ) -> Option<WindowCommand> {
                (state.value == 0).then_some(WindowCommand::Lapse)
            }

            fn apply(state: CounterState, event: &CounterEvent) -> CounterState {
                CounterWorkflow::apply(state, event)
            }

            fn handle(
                _state: &CounterState,
                command: WindowCommand,
            ) -> Result<crate::workflow::WorkflowOutput<CounterEvent>, WorkflowError> {
                Ok(match command {
                    WindowCommand::Arm { label } => crate::workflow::WorkflowOutput {
                        events: vec![CounterEvent::Reset],
                        outbox: Vec::new(),
                        deadlines: vec![crate::workflow::PendingDeadline::new(
                            label,
                            time::OffsetDateTime::UNIX_EPOCH,
                        )],
                    },
                    WindowCommand::Lapse => vec![CounterEvent::Incremented { by: 1 }].into(),
                })
            }
        }

        async fn window_process() -> Process<WindowWorkflow, Arc<SlateDbStore>> {
            Process::new(
                Arc::new(
                    SlateDbStore::open_in_memory()
                        .await
                        .expect("in-memory store"),
                ),
                TenantId::new(),
                WorkflowId::new("window", "FV2024-10-01"),
            )
        }

        fn fired(
            p: &Process<WindowWorkflow, Arc<SlateDbStore>>,
            label: &str,
        ) -> crate::deadline::Deadline {
            crate::deadline::Deadline::new(
                p.stream_id().clone(),
                p.process_id(),
                p.tenant_id(),
                p.workflow_id().clone(),
                label,
                time::OffsetDateTime::UNIX_EPOCH,
            )
        }

        /// The hook decides an owned label once, and the closed window then
        /// decides nothing.
        #[tokio::test]
        async fn an_owned_label_reaches_the_hook_until_its_window_closes() {
            let p = window_process().await;
            let acted = p
                .run_timeout(&fired(&p, "window"), RunOptions::new().attempts(3))
                .await
                .unwrap();
            assert!(acted.is_some_and(|e| e.len() == 1));
            let again = p
                .run_timeout(&fired(&p, "window"), RunOptions::new().attempts(3))
                .await
                .unwrap();
            assert!(again.is_none(), "the window closed on the first firing");
        }

        /// A label outside `DEADLINE_LABELS` never reaches the hook — not a
        /// delivery window, not a stray label — however eager the hook is.
        #[tokio::test]
        async fn an_unowned_label_never_reaches_the_hook() {
            let p = window_process().await;
            for label in [mako_fristen::APERAK_STROM_WINDOW_LABEL, "stray"] {
                let acted = p
                    .run_timeout(&fired(&p, label), RunOptions::new().attempts(1))
                    .await
                    .unwrap();
                assert!(acted.is_none(), "{label} must not reach on_deadline");
            }
            assert_eq!(p.state().await.unwrap().value, 0);
        }

        /// A deadline under a label the workflow does not own is refused, not
        /// stored; a delivery window is accepted.
        #[tokio::test]
        async fn an_unowned_label_cannot_be_registered() {
            let p = window_process().await;
            let err = p
                .run(WindowCommand::Arm { label: "stray" }, RunOptions::new())
                .await
                .expect_err("a stray label must be refused");
            assert!(
                matches!(&err, EngineError::UnownedDeadlineLabel { label, .. } if &**label == "stray"),
                "{err:?}"
            );
            assert!(p.state().await.unwrap() == CounterState::default());

            p.run(
                WindowCommand::Arm {
                    label: mako_fristen::APERAK_STROM_WINDOW_LABEL,
                },
                RunOptions::new(),
            )
            .await
            .expect("a delivery window is owned by the outbox, not refused");
            p.run(WindowCommand::Arm { label: "window" }, RunOptions::new())
                .await
                .expect("an owned label registers");
        }
    }
}
