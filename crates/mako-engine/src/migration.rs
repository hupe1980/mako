//! In-flight process state migration across BDEW format-version boundaries.
//!
//! When an old BDEW format version (FV) is removed from the adapter registry
//! after its grace period, any process initiated under that FV can no longer
//! receive new events — the `ForwardCompatible` policy rejects FV mismatches
//! at dispatch time. [`StateMigration`] + [`MigrationRunner`] provide the
//! tooling to advance those processes to a newer FV *before* the old FV is
//! retired.
//!
//! # How it works
//!
//! 1. **Scan** all event streams via [`EventStore::list_streams`].
//! 2. **Filter** streams whose first event carries
//!    `workflow_id == migration.source_workflow_id()`.
//! 3. **Replay** each matched stream using `FromWorkflow::apply` to reconstruct
//!    the fully-folded state.
//! 4. **Migrate** the state via [`StateMigration::migrate`].
//! 5. **Snapshot** the migrated state under `target_workflow_id`'s schema version so
//!    the process can continue executing under the new workflow definition without
//!    replaying old incompatible events.
//!
//! # Deployment sequence
//!
//! 1. Deploy the new binary with **both** FVs still registered in the adapter
//!    registry.
//! 2. Run `MigrationRunner::run_and_update_registry(&registry)` — all in-flight
//!    processes are migrated and their routing-table entries are rewritten to use
//!    the new `workflow_id`. Inspect [`MigrationReport`] for errors.
//! 3. Remove the old FV from the adapter registry and redeploy.
//!
//! > **Note**: `MigrationRunner::run_and_update_registry` handles the
//! > `ProcessRegistry` update automatically (updating `ProcessIdentity.workflow_id`
//! > for every primary process-keyed entry). For conversation- or correlation-keyed
//! > entries (which are short-lived and self-expire) no action is needed.
//! > If you only want the snapshot migration without registry updates, use the
//! > simpler `MigrationRunner::run()` instead.
//!
//! # Example
//!
//! ```rust,ignore
//! use mako_engine::{
//!     migration::{MigrationRunner, StateMigration},
//!     version::WorkflowId,
//! };
//!
//! struct SupplierChangeFv2024ToFv2025;
//!
//! impl StateMigration for SupplierChangeFv2024ToFv2025 {
//!     type FromWorkflow = GpkeSupplierChangeWorkflowFv2024;
//!     type ToWorkflow   = GpkeSupplierChangeWorkflowFv2025;
//!
//!     fn source_workflow_id(&self) -> &WorkflowId { &FV2024_WORKFLOW_ID }
//!     fn target_workflow_id(&self)   -> &WorkflowId { &FV2025_WORKFLOW_ID }
//!
//!     fn migrate(
//!         &self,
//!         state: SupplierChangeStateFv2024,
//!     ) -> Result<SupplierChangeStateFv2025, String> {
//!         Ok(SupplierChangeStateFv2025::from_v2024(state))
//!     }
//! }
//!
//! let runner = MigrationRunner::new(
//!     SupplierChangeFv2024ToFv2025,
//!     event_store,
//!     snap_store,
//! );
//! let report = runner.run().await;
//! assert!(report.is_ok(), "migration errors: {:?}", report.errors);
//! ```
//!
//! [`ProcessRegistry`]: crate::registry::ProcessRegistry
//! [`ProcessIdentity`]: crate::ids::ProcessIdentity
//! [`EventStore::list_streams`]: crate::event_store::EventStore::list_streams

use crate::{
    event_store::EventStore,
    ids::{ProcessId, StreamId, TenantId},
    snapshot::{Snapshot, SnapshotStore},
    version::WorkflowId,
    workflow::Workflow,
};

// ── MigrationError ────────────────────────────────────────────────────────────

/// Describes a failure to migrate a single process stream.
#[derive(Debug, Clone)]
pub struct MigrationError {
    /// The stream that could not be migrated.
    pub stream_id: StreamId,
    /// Human-readable failure reason.
    pub message: String,
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "migration error on stream {}: {}",
            self.stream_id, self.message
        )
    }
}

impl std::error::Error for MigrationError {}

// ── StateMigration ────────────────────────────────────────────────────────────

/// A typed, one-directional migration from one workflow version to another.
///
/// Implement this trait for each FV-to-FV transition your deployment requires.
/// The [`migrate`] function is called once per in-flight process stream and must
/// be **pure** (no I/O, no clock access, no global state).
///
/// ## Additive-only changes (same state type)
///
/// When both FVs share the same `State` type (only new optional fields added),
/// the migration can be a no-op:
///
/// ```rust,ignore
/// fn migrate(&self, state: SharedState) -> Result<SharedState, String> {
///     Ok(state)
/// }
/// ```
///
/// ## Structural changes (different state types)
///
/// When the state layout changed incompatibly (renamed variant, removed field,
/// changed discriminant), use different `FromWorkflow::State` and
/// `ToWorkflow::State` types. The Rust compiler will enforce that every field
/// is explicitly mapped:
///
/// ```rust,ignore
/// fn migrate(&self, old: OldState) -> Result<NewState, String> {
///     match old {
///         OldState::Initiated(data) => Ok(NewState::Initiated(NewInitiatedData {
///             ems_id:    data.ems_id,
///             malo_id:   data.malo_id,
///             initiated: data.initiated_at,  // renamed field
///         })),
///         OldState::Completed => Ok(NewState::Completed),
///     }
/// }
/// ```
///
/// [`migrate`]: StateMigration::migrate
pub trait StateMigration: Send + Sync + 'static {
    /// The old workflow definition whose events are stored in matched streams.
    type FromWorkflow: Workflow;
    /// The new workflow definition that continues execution after migration.
    type ToWorkflow: Workflow;

    /// The `WorkflowId` (name + old BDEW FV) that identifies streams to migrate.
    fn source_workflow_id(&self) -> &WorkflowId;

    /// The `WorkflowId` (name + new BDEW FV) stamped into the migrated snapshot.
    fn target_workflow_id(&self) -> &WorkflowId;

    /// Map the fully-replayed old state to the new state type.
    ///
    /// Called once per matched stream. Must be **pure**.
    ///
    /// # Errors
    ///
    /// Return `Err(human_readable_reason)` when the state cannot be migrated.
    /// The runner records the failure in [`MigrationReport::errors`] and
    /// continues with remaining streams — a single bad stream does not abort
    /// the entire migration.
    fn migrate(
        &self,
        state: <Self::FromWorkflow as Workflow>::State,
    ) -> Result<<Self::ToWorkflow as Workflow>::State, String>;
}

// ── MigrationReport ───────────────────────────────────────────────────────────

/// Summary produced by a [`MigrationRunner::run`] call.
#[derive(Debug, Default)]
pub struct MigrationReport {
    /// Number of streams successfully migrated and snapshotted.
    pub migrated: usize,
    /// Number of streams skipped (wrong `workflow_id`, empty, or already migrated).
    pub skipped: usize,
    /// Streams that encountered an error during replay, migration, or snapshot.
    pub errors: Vec<MigrationError>,
}

impl MigrationReport {
    /// Return `true` when no migration errors occurred.
    ///
    /// A `true` result does not imply that all processes were migrated — only
    /// that all matched processes succeeded. Check [`migrated`] and [`skipped`]
    /// to verify expected migration counts.
    ///
    /// [`migrated`]: MigrationReport::migrated
    /// [`skipped`]: MigrationReport::skipped
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Merge another report into this one (accumulate counts and errors).
    pub fn merge(&mut self, other: MigrationReport) {
        self.migrated += other.migrated;
        self.skipped += other.skipped;
        self.errors.extend(other.errors);
    }
}

impl std::fmt::Display for MigrationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MigrationReport {{ migrated: {}, skipped: {}, errors: {} }}",
            self.migrated,
            self.skipped,
            self.errors.len(),
        )
    }
}

// ── IdentityMigration ─────────────────────────────────────────────────────────

/// A no-op [`StateMigration`] for workflows whose state schema did **not** change
/// between two BDEW format versions.
///
/// Use this when the FV transition only added new optional AHB rules, segment
/// cardinality changes, or code-list entries — no field was renamed, removed,
/// or made mandatory. The migrated snapshot repoints `workflow_id` to the new
/// FV while keeping the state value identical.
///
/// # Example
///
/// ```rust,ignore
/// use mako_engine::migration::IdentityMigration;
/// use mako_engine::version::WorkflowId;
/// use mako_gpke::lf_anmeldung::{GpkeLfAnmeldungWorkflow, WORKFLOW_NAME};
///
/// let migration = IdentityMigration::<GpkeLfAnmeldungWorkflow>::new(
///     WorkflowId::new(WORKFLOW_NAME, "FV2025-10-01"),
///     WorkflowId::new(WORKFLOW_NAME, "FV2026-10-01"),
/// );
/// ```
pub struct IdentityMigration<W> {
    source: WorkflowId,
    target: WorkflowId,
    _w: std::marker::PhantomData<fn() -> W>,
}

impl<W: Workflow> IdentityMigration<W> {
    /// Construct a new identity migration between two format versions.
    #[must_use]
    pub fn new(source: WorkflowId, target: WorkflowId) -> Self {
        Self {
            source,
            target,
            _w: std::marker::PhantomData,
        }
    }
}

impl<W> StateMigration for IdentityMigration<W>
where
    W: Workflow + Send + Sync + 'static,
    W::State: Clone,
{
    type FromWorkflow = W;
    type ToWorkflow = W;

    fn source_workflow_id(&self) -> &WorkflowId {
        &self.source
    }

    fn target_workflow_id(&self) -> &WorkflowId {
        &self.target
    }

    fn migrate(&self, state: W::State) -> Result<W::State, String> {
        Ok(state)
    }
}

// ── helpers ────────────────────────────────────────────────────────────────────

/// Parse `(tenant_id, process_id)` from a process stream identifier.
///
/// Stream IDs for process streams follow the format
/// `process/{tenant_uuid}/{process_uuid}` (includes a tenant discriminator
/// for single-tenant isolation).  Returns `None` when the format does not match.
fn parse_process_stream_id(stream_id: &str) -> Option<(TenantId, ProcessId)> {
    let rest = stream_id.strip_prefix("process/")?;
    let (tenant_str, process_str) = rest.split_once('/')?;
    let tenant_uuid = uuid::Uuid::parse_str(tenant_str).ok()?;
    let process_uuid = uuid::Uuid::parse_str(process_str).ok()?;
    Some((
        TenantId::from_uuid(tenant_uuid),
        ProcessId::from_uuid(process_uuid),
    ))
}

// ── MigrationRunner ───────────────────────────────────────────────────────────

/// Drives a [`StateMigration`] over every event stream in a store.
///
/// Constructed with separate [`EventStore`] and [`SnapshotStore`] handles so
/// the runner can operate against any backend combination (e.g. `SlateDbStore`
/// for events and `SlateDbSnapshotStore` for snapshots, or in-memory stores
/// during testing).
///
/// # Concurrency
///
/// `run()` processes streams **sequentially**. For deployments with thousands
/// of in-flight processes, wrapping the call in a dedicated migration task and
/// using a custom prefix filter via [`EventStore::list_streams`] (e.g. filtering
/// by stream-id prefix) can reduce the scan scope.
pub struct MigrationRunner<M, ES, SS> {
    migration: M,
    event_store: ES,
    snap_store: SS,
}

impl<M, ES, SS> MigrationRunner<M, ES, SS>
where
    M: StateMigration,
    <M::FromWorkflow as Workflow>::State: serde::de::DeserializeOwned,
    <M::ToWorkflow as Workflow>::State: serde::Serialize,
    ES: EventStore,
    SS: SnapshotStore,
{
    /// Construct a new runner.
    #[must_use]
    pub fn new(migration: M, event_store: ES, snap_store: SS) -> Self {
        Self {
            migration,
            event_store,
            snap_store,
        }
    }

    /// Scan all event streams and migrate those that match `source_workflow_id`.
    ///
    /// - Streams with no events or a different `workflow_id` are counted in
    ///   [`MigrationReport::skipped`].
    /// - Streams that fail (replay error, `migrate()` returning `Err`, or
    ///   snapshot write failure) are recorded in [`MigrationReport::errors`]
    ///   and do **not** abort the run.
    ///
    /// If `list_streams` itself fails, a single error entry covering
    /// `"(list_streams)"` is returned immediately.
    pub async fn run(&self) -> MigrationReport {
        let streams = match self.event_store.list_streams(None).await {
            Ok(s) => s,
            Err(e) => {
                return MigrationReport {
                    errors: vec![MigrationError {
                        stream_id: StreamId::new("(list_streams)"),
                        message: format!("list_streams failed: {e}"),
                    }],
                    ..Default::default()
                };
            }
        };

        let mut report = MigrationReport::default();

        for stream_id in streams {
            match self.migrate_stream(&stream_id).await {
                Ok(true) => report.migrated += 1,
                Ok(false) => report.skipped += 1,
                Err(err) => report.errors.push(err),
            }
        }

        report
    }

    /// Like [`run`] but also updates [`ProcessRegistry`] entries after each
    /// successful migration.
    ///
    /// For every migrated stream the runner:
    ///
    /// 1. Parses `(tenant_id, process_id)` from the stream ID
    ///    (`process/{tenant_id}/{process_id}`).
    /// 2. Looks up `RegistryKey::from_process(process_id)` for that tenant.
    /// 3. Rewrites the stored [`ProcessIdentity`] with the new `workflow_id`
    ///    and updated `stream_id` (which embeds the tenant discriminator).
    ///
    /// Entries for conversation- or correlation-based routing keys are
    /// typically short-lived (they exist only during an active EDIFACT
    /// exchange) and do not need updating here.
    ///
    /// Registry update failures are recorded as warnings in the returned
    /// [`MigrationReport`] but do **not** roll back the snapshot that was
    /// already written.
    ///
    /// [`run`]: MigrationRunner::run
    /// [`ProcessRegistry`]: crate::registry::ProcessRegistry
    /// [`ProcessIdentity`]: crate::ids::ProcessIdentity
    pub async fn run_and_update_registry<R>(&self, registry: &R) -> MigrationReport
    where
        R: crate::registry::ProcessRegistry,
    {
        let streams = match self.event_store.list_streams(None).await {
            Ok(s) => s,
            Err(e) => {
                return MigrationReport {
                    errors: vec![MigrationError {
                        stream_id: StreamId::new("(list_streams)"),
                        message: format!("list_streams failed: {e}"),
                    }],
                    ..Default::default()
                };
            }
        };

        let mut report = MigrationReport::default();

        for stream_id in streams {
            match self.migrate_stream(&stream_id).await {
                Ok(false) => {
                    report.skipped += 1;
                }
                Ok(true) => {
                    report.migrated += 1;
                    // Best-effort registry update: parse tenant + process from
                    // the stream ID and rewrite the primary process-keyed entry.
                    if let Some((tenant_id, process_id)) =
                        parse_process_stream_id(stream_id.as_str())
                    {
                        let key = crate::registry::RegistryKey::from_process(process_id);
                        match registry.lookup(tenant_id, &key).await {
                            Ok(Some(mut identity)) => {
                                // Rebind workflow_id to the new version; the
                                // stream_id is already correct (unchanged by
                                // migration).
                                identity.workflow_id = self.migration.target_workflow_id().clone();
                                if let Err(e) = registry.register(tenant_id, &key, identity).await {
                                    report.errors.push(MigrationError {
                                        stream_id: stream_id.clone(),
                                        message: format!(
                                            "registry update failed for process {process_id}: {e}"
                                        ),
                                    });
                                }
                            }
                            Ok(None) => {
                                // No direct-process registry entry — process
                                // might only be accessible via conversation/
                                // correlation keys; nothing to do here.
                            }
                            Err(e) => {
                                report.errors.push(MigrationError {
                                    stream_id: stream_id.clone(),
                                    message: format!(
                                        "registry lookup failed for process {process_id}: {e}"
                                    ),
                                });
                            }
                        }
                    } else {
                        tracing::warn!(
                            stream_id = stream_id.as_str(),
                            "run_and_update_registry: cannot parse tenant/process from \
                             stream_id — registry update skipped for this stream",
                        );
                    }
                }
                Err(err) => {
                    report.errors.push(err);
                }
            }
        }

        report
    }

    /// Attempt to migrate a single stream.
    ///
    /// Returns `Ok(true)` when the stream was migrated, `Ok(false)` when it
    /// was skipped.
    async fn migrate_stream(&self, stream_id: &StreamId) -> Result<bool, MigrationError> {
        // Load all events. We need them for both the workflow_id check (peek
        // the first event) and the fold. A cursor-based alternative would
        // require two round-trips; load_all is acceptable for a one-time
        // migration operation.
        let events = self
            .event_store
            .load(stream_id)
            .await
            .map_err(|e| MigrationError {
                stream_id: stream_id.clone(),
                message: format!("event load failed: {e}"),
            })?;

        let Some(first) = events.first() else {
            // Empty stream — nothing to migrate.
            return Ok(false);
        };

        if &first.workflow_id != self.migration.source_workflow_id() {
            // Different workflow — skip.
            return Ok(false);
        }

        // Fold state using FromWorkflow.
        let mut state = <M::FromWorkflow as Workflow>::State::default();
        let last_seq = events.last().map_or(0, |e| e.sequence_number);

        for env in events {
            let payload = M::FromWorkflow::upcast(&env.event_type, env.schema_version, env.payload)
                .map_err(|e| MigrationError {
                    stream_id: stream_id.clone(),
                    message: format!("upcast failed on seq {}: {e}", env.sequence_number),
                })?;
            let event: <M::FromWorkflow as Workflow>::Event = serde_json::from_value(payload)
                .map_err(|e| MigrationError {
                    stream_id: stream_id.clone(),
                    message: format!("event deserialize failed: {e}"),
                })?;
            state = M::FromWorkflow::apply(state, &event);
        }

        // Apply the user-supplied migration function.
        let new_state = self
            .migration
            .migrate(state)
            .map_err(|msg| MigrationError {
                stream_id: stream_id.clone(),
                message: msg,
            })?;

        // Serialize the migrated state.
        let payload = serde_json::to_value(&new_state).map_err(|e| MigrationError {
            stream_id: stream_id.clone(),
            message: format!("state serialization failed: {e}"),
        })?;

        // Save a snapshot under the new workflow's schema version. Future
        // Process::<ToWorkflow>::state_with_snapshot() calls will load this
        // snapshot and skip the old (incompatible) events entirely.
        let snap = Snapshot::new(
            stream_id.clone(),
            last_seq,
            M::ToWorkflow::state_schema_version(),
            payload,
        );
        self.snap_store
            .save(&snap)
            .await
            .map_err(|e| MigrationError {
                stream_id: stream_id.clone(),
                message: format!("snapshot save failed: {e}"),
            })?;

        Ok(true)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        envelope::NewEvent,
        event_store::{ExpectedVersion, InMemoryEventStore},
        ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId},
        snapshot::NoopSnapshotStore,
        version::WorkflowId,
        workflow::{CommandPayload, EventPayload, Workflow},
    };

    // ── Minimal "counter" workflows for test purposes ─────────────────────────

    #[derive(Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct CounterStateV1 {
        count: u32,
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    enum CounterEventV1 {
        Incremented,
    }

    impl EventPayload for CounterEventV1 {
        fn event_type(&self) -> &'static str {
            "Incremented"
        }
    }

    #[derive(Clone)]
    enum CounterCommandV1 {}

    impl CommandPayload for CounterCommandV1 {}

    struct CounterWorkflowV1;
    impl Workflow for CounterWorkflowV1 {
        type State = CounterStateV1;
        type Event = CounterEventV1;
        type Command = CounterCommandV1;

        fn handle(
            _state: &Self::State,
            _cmd: Self::Command,
        ) -> Result<crate::workflow::WorkflowOutput<Self::Event>, crate::error::WorkflowError>
        {
            unreachable!("not used in migration tests")
        }

        fn apply(mut state: Self::State, event: &Self::Event) -> Self::State {
            match event {
                CounterEventV1::Incremented => state.count += 1,
            }
            state
        }
    }

    // V2 adds a `label` field.
    #[derive(Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct CounterStateV2 {
        count: u32,
        label: String,
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    enum CounterEventV2 {
        Incremented,
    }

    impl EventPayload for CounterEventV2 {
        fn event_type(&self) -> &'static str {
            "Incremented"
        }
    }

    #[derive(Clone)]
    enum CounterCommandV2 {}

    impl CommandPayload for CounterCommandV2 {}

    struct CounterWorkflowV2;
    impl Workflow for CounterWorkflowV2 {
        type State = CounterStateV2;
        type Event = CounterEventV2;
        type Command = CounterCommandV2;

        fn handle(
            _state: &Self::State,
            _cmd: Self::Command,
        ) -> Result<crate::workflow::WorkflowOutput<Self::Event>, crate::error::WorkflowError>
        {
            unreachable!("not used in migration tests")
        }

        fn apply(mut state: Self::State, event: &Self::Event) -> Self::State {
            match event {
                CounterEventV2::Incremented => state.count += 1,
            }
            state
        }

        fn state_schema_version() -> u32 {
            2
        }
    }

    // ── Migration implementation ──────────────────────────────────────────────

    struct V1ToV2;

    impl StateMigration for V1ToV2 {
        type FromWorkflow = CounterWorkflowV1;
        type ToWorkflow = CounterWorkflowV2;

        fn source_workflow_id(&self) -> &WorkflowId {
            static WID: std::sync::OnceLock<WorkflowId> = std::sync::OnceLock::new();
            WID.get_or_init(|| WorkflowId::new("counter", "FV2024-10-01"))
        }

        fn target_workflow_id(&self) -> &WorkflowId {
            static WID: std::sync::OnceLock<WorkflowId> = std::sync::OnceLock::new();
            WID.get_or_init(|| WorkflowId::new("counter", "FV2025-04-01"))
        }

        fn migrate(&self, state: CounterStateV1) -> Result<CounterStateV2, String> {
            Ok(CounterStateV2 {
                count: state.count,
                label: "migrated".into(),
            })
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_increment_event(workflow_id: WorkflowId) -> NewEvent {
        let pid = ProcessId::new();
        let tid = TenantId::new();
        NewEvent {
            correlation_id: CorrelationId::new(),
            causation_id: None,
            conversation_id: ConversationId::new(),
            process_id: pid,
            tenant_id: tid,
            workflow_id,
            event_type: "Incremented".into(),
            schema_version: 1,
            payload: serde_json::to_value(CounterEventV1::Incremented).unwrap(),
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn migrate_matching_stream() {
        let store = InMemoryEventStore::default();
        let snaps = crate::snapshot::InMemorySnapshotStore::new();
        let sid = StreamId::new("process/counter-001");
        let wid_v1 = WorkflowId::new("counter", "FV2024-10-01");

        // Append 3 increment events under the V1 workflow.
        store
            .append(
                &sid,
                ExpectedVersion::Any,
                &[
                    make_increment_event(wid_v1.clone()),
                    make_increment_event(wid_v1.clone()),
                    make_increment_event(wid_v1.clone()),
                ],
            )
            .await
            .unwrap();

        let runner = MigrationRunner::new(V1ToV2, store, snaps.clone());
        let report = runner.run().await;

        assert!(report.is_ok(), "errors: {:?}", report.errors);
        assert_eq!(report.migrated, 1);
        assert_eq!(report.skipped, 0);

        // The snapshot should encode the migrated V2 state.
        let snap = snaps.load(&sid).await.unwrap().expect("snapshot saved");
        assert_eq!(snap.state_schema_version, 2);
        let state: CounterStateV2 = serde_json::from_value(snap.state).unwrap();
        assert_eq!(state.count, 3);
        assert_eq!(state.label, "migrated");
    }

    #[tokio::test]
    async fn skip_non_matching_stream() {
        let store = InMemoryEventStore::default();
        let snaps = NoopSnapshotStore;
        let sid = StreamId::new("process/counter-other");
        let wid_v2 = WorkflowId::new("counter", "FV2025-04-01"); // already V2

        store
            .append(&sid, ExpectedVersion::Any, &[make_increment_event(wid_v2)])
            .await
            .unwrap();

        let runner = MigrationRunner::new(V1ToV2, store, snaps);
        let report = runner.run().await;

        assert!(report.is_ok());
        assert_eq!(report.migrated, 0);
        assert_eq!(report.skipped, 1);
    }

    #[tokio::test]
    async fn skip_empty_stream() {
        let store = InMemoryEventStore::default();
        let snaps = NoopSnapshotStore;
        // list_streams returns an empty list for an unused store.
        let runner = MigrationRunner::new(V1ToV2, store, snaps);
        let report = runner.run().await;

        assert!(report.is_ok());
        assert_eq!(report.migrated, 0);
        assert_eq!(report.skipped, 0);
    }

    #[tokio::test]
    async fn migration_fn_error_is_recorded_not_fatal() {
        struct FailingMigration;

        impl StateMigration for FailingMigration {
            type FromWorkflow = CounterWorkflowV1;
            type ToWorkflow = CounterWorkflowV2;
            fn source_workflow_id(&self) -> &WorkflowId {
                static WID: std::sync::OnceLock<WorkflowId> = std::sync::OnceLock::new();
                WID.get_or_init(|| WorkflowId::new("counter", "FV2024-10-01"))
            }

            fn target_workflow_id(&self) -> &WorkflowId {
                static WID: std::sync::OnceLock<WorkflowId> = std::sync::OnceLock::new();
                WID.get_or_init(|| WorkflowId::new("counter", "FV2025-04-01"))
            }

            fn migrate(&self, _state: CounterStateV1) -> Result<CounterStateV2, String> {
                Err("intentional test failure".into())
            }
        }

        let store = InMemoryEventStore::default();
        let snaps = NoopSnapshotStore;
        let sid = StreamId::new("process/failing");
        let wid_v1 = WorkflowId::new("counter", "FV2024-10-01");

        store
            .append(&sid, ExpectedVersion::Any, &[make_increment_event(wid_v1)])
            .await
            .unwrap();

        let runner = MigrationRunner::new(FailingMigration, store, snaps);
        let report = runner.run().await;

        assert!(!report.is_ok());
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.migrated, 0);
        assert!(
            report.errors[0]
                .message
                .contains("intentional test failure")
        );
    }
}
