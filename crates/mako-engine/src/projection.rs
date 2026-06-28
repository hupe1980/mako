//! [`Projection`] trait and [`ProjectionRunner`].
//!
//! Projections build read models from the event stream. They are:
//!
//! - **Asynchronous** — fed events independently of the write path
//! - **Disposable** — the read model can be dropped and rebuilt at any time
//! - **Eventually consistent** — they may lag behind the write head
//!
//! Projection failures must never affect event persistence.
//!
//! # Incremental catch-up
//!
//! Projections that track their cursor position can implement
//! [`Projection::last_sequence`] so [`ProjectionRunner::catch_up`] and
//! [`ProjectionRunner::catch_up_from_store`] feed only new events.
//!
//! # Store-backed streaming
//!
//! [`ProjectionRunner::run_from_store`] and
//! [`ProjectionRunner::catch_up_from_store`] load events directly from an
//! [`EventStore`] without requiring the caller to pre-load the entire
//! event slice into memory.
//!
//! # Multi-stream projections
//!
//! [`ProjectionRunner::run_all_streams`] and
//! [`ProjectionRunner::catch_up_all_streams`] drive a projection across
//! multiple streams simultaneously. This is required for process families
//! where a read model aggregates across many process instances — for
//! example, MABIS Bilanzkreisabrechnung aggregating events across thousands
//! of MaLo-level process streams for a single billing period.
//!
//! The [`GlobalProjectionCheckpoint`] records per-stream cursors so
//! incremental catch-up only feeds events newer than the last replay.
//!
//! ```rust,ignore
//! // Initial full replay across all process streams:
//! let checkpoint = ProjectionRunner::run_all_streams(
//!     &mut billing_proj,
//!     &store,
//!     &stream_ids,
//! ).await?;
//!
//! // Later: incremental update after new events arrive:
//! let checkpoint = ProjectionRunner::catch_up_all_streams(
//!     &mut billing_proj,
//!     &store,
//!     &stream_ids,
//!     &checkpoint,
//! ).await?;
//! ```
//!
//! To enumerate all process streams automatically, use
//! [`EventStore::list_streams`] with a prefix.  Pass `"process/"` to scan
//! all tenants, or `&format!("process/{tenant_id}/")` to scope to one tenant:
//!
//! ```rust,ignore
//! // All tenants:
//! let streams = store.list_streams(Some("process/")).await?;
//! // Single tenant:
//! let streams = store.list_streams(Some(&format!("process/{tenant_id}/"))).await?;
//! let checkpoint = ProjectionRunner::run_all_streams(
//!     &mut billing_proj, &store, &streams,
//! ).await?;
//! ```

use std::collections::BTreeMap;

use crate::{envelope::EventEnvelope, error::EngineError, event_store::EventStore, ids::StreamId};

// ── Projection trait ──────────────────────────────────────────────────────────

/// A read-model builder that consumes events and maintains queryable state.
///
/// # Contract
///
/// - `handle_event` is called for every event in stream order.
/// - `handle_event` must not panic on events it doesn't recognise (forward
///   compatibility: new event types appear when new domain features are
///   deployed before all projections are updated).
/// - The projection is rebuilt from scratch by replaying all events through
///   [`ProjectionRunner::run`]; implementations must tolerate this.
pub trait Projection {
    /// A stable human-readable name for this projection (used in logs/metrics).
    fn name(&self) -> &'static str;

    /// Process a single event, updating internal read-model state.
    fn handle_event(&mut self, envelope: &EventEnvelope);

    /// The sequence number of the last event this projection processed.
    ///
    /// Return `None` when the projection has not processed any events yet
    /// (i.e. it needs a full replay).
    ///
    /// Implement this method if your projection stores the cursor alongside
    /// the read model so [`ProjectionRunner::catch_up`] can perform
    /// incremental updates.
    ///
    /// Defaults to `None`.
    fn last_sequence(&self) -> Option<u64> {
        None
    }
}

// ── GlobalProjectionCheckpoint ────────────────────────────────────────────────

/// Per-stream sequence number cursors for multi-stream projections.
///
/// Returned by [`ProjectionRunner::run_all_streams`] and
/// [`ProjectionRunner::catch_up_all_streams`]. Pass an existing checkpoint
/// to `catch_up_all_streams` so only events newer than the last replay are
/// fed to the projection.
///
/// Persist this value (e.g. alongside the read model in a snapshot store) to
/// survive process restarts and avoid full replays on restart.
///
/// A cursor value of `0` for a stream means "never seen" (equivalent to
/// "replay from the beginning").
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlobalProjectionCheckpoint {
    /// Last-processed sequence number per stream identifier.
    ///
    /// Streams not present in this map have an implicit cursor of `0`.
    pub cursors: BTreeMap<StreamId, u64>,
}

impl GlobalProjectionCheckpoint {
    /// Create an empty checkpoint (all streams will be fully replayed).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The last-processed sequence number for `stream_id`.
    ///
    /// Returns `0` when the stream has never been processed (signals a full
    /// replay is needed for that stream).
    #[must_use]
    pub fn cursor_for(&self, stream_id: &StreamId) -> u64 {
        self.cursors.get(stream_id).copied().unwrap_or(0)
    }

    /// Update the cursor for `stream_id` to `sequence` (if `sequence` is
    /// greater than the current cursor).
    pub fn advance(&mut self, stream_id: &StreamId, sequence: u64) {
        let entry = self.cursors.entry(stream_id.clone()).or_insert(0);
        if sequence > *entry {
            *entry = sequence;
        }
    }
}

// ── ProjectionRunner ──────────────────────────────────────────────────────────

/// Persist and load named [`GlobalProjectionCheckpoint`] values.
///
/// Implement this trait on your event store to enable
/// [`ProjectionRunner::catch_up_persistent`], which avoids full replays on
/// restart by persisting cursor progress after each catch-up cycle.
///
/// The SlateDB implementation stores one key per (projection, stream) pair
/// under the `cp/{name}/{stream_id}` key space (raw u64 LE — no JSON). This
/// bounds each `catch_up_persistent` cycle to O(changed_streams) writes
/// instead of O(total_streams), which matters for MABIS deployments tracking
/// tens of thousands of streams. Other backing stores may choose any suitable
/// serialisation.
#[allow(async_fn_in_trait)]
pub trait ProjectionCheckpointStore {
    /// Load a previously saved checkpoint by name.
    ///
    /// Returns an empty [`GlobalProjectionCheckpoint`] (all cursors zero) when
    /// no checkpoint has been persisted for `name` yet — this triggers a full
    /// replay from the beginning.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    async fn load_projection_checkpoint(
        &self,
        name: &str,
    ) -> Result<GlobalProjectionCheckpoint, EngineError>;

    /// Persist `checkpoint` under `name`, overwriting any previously stored
    /// value.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    async fn save_projection_checkpoint(
        &self,
        name: &str,
        checkpoint: &GlobalProjectionCheckpoint,
    ) -> Result<(), EngineError>;

    /// Persist only the cursors that advanced since `previous`.
    ///
    /// The default implementation ignores `previous` and saves the full
    /// `current` checkpoint. Override in storage backends that support
    /// per-key atomic writes (e.g. SlateDB `WriteBatch`) for O(changed)
    /// write cost instead of O(total streams).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    async fn advance_projection_cursors(
        &self,
        name: &str,
        _previous: &GlobalProjectionCheckpoint,
        current: &GlobalProjectionCheckpoint,
    ) -> Result<(), EngineError> {
        self.save_projection_checkpoint(name, current).await
    }
}

// ── ProjectionRunner ──────────────────────────────────────────────────────────

/// Drives one or more projections over a slice of events.
///
/// The runner is stateless — it simply iterates over events and calls
/// [`Projection::handle_event`] for each.
pub struct ProjectionRunner;

impl ProjectionRunner {
    /// Feed all `events` into `projection` in order (full replay).
    ///
    /// # Performance
    ///
    /// This method requires the caller to have already loaded all `events` into
    /// a `Vec`. For large event streams, prefer [`run_from_store`] /
    /// [`run_all_streams`] which use `fold_stream` internally and avoid
    /// allocating the full event slice.
    ///
    /// [`run_from_store`]: ProjectionRunner::run_from_store
    /// [`run_all_streams`]: ProjectionRunner::run_all_streams
    pub fn run<P: Projection>(projection: &mut P, events: &[EventEnvelope]) {
        for event in events {
            projection.handle_event(event);
        }
    }

    /// Feed all `events` into multiple projections simultaneously (single pass,
    /// full replay).
    ///
    /// # Performance
    ///
    /// Same caveat as [`run`]: the caller must supply a pre-loaded slice.
    /// For large streams, prefer [`run_all_streams`] which streams events
    /// directly from the store with O(1) working memory.
    ///
    /// [`run`]: ProjectionRunner::run
    /// [`run_all_streams`]: ProjectionRunner::run_all_streams
    pub fn run_all(projections: &mut [&mut dyn Projection], events: &[EventEnvelope]) {
        for event in events {
            for projection in projections.iter_mut() {
                projection.handle_event(event);
            }
        }
    }

    /// Feed only events newer than the projection's cursor into `projection`.
    ///
    /// Queries [`Projection::last_sequence`] to determine the starting point.
    /// If the projection returns `None`, all `events` are fed (same as [`run`]).
    ///
    /// `events` must be sorted by `sequence_number` in ascending order (which
    /// is the contract for all slices returned by [`EventStore::load`] /
    /// [`EventStore::load_from`]).
    ///
    /// This is a binary-search–accelerated variant: it finds the first event
    /// past the cursor in O(log n) then feeds the tail in O(k) where k is the
    /// number of new events.
    ///
    /// [`run`]: ProjectionRunner::run
    /// [`EventStore::load`]: crate::event_store::EventStore::load
    /// [`EventStore::load_from`]: crate::event_store::EventStore::load_from
    pub fn catch_up<P: Projection>(projection: &mut P, events: &[EventEnvelope]) {
        let from = projection.last_sequence().unwrap_or(0);
        if from == 0 {
            Self::run(projection, events);
            return;
        }
        // Binary search for the first event with sequence_number > from.
        let start = events.partition_point(|e| e.sequence_number <= from);
        for event in &events[start..] {
            projection.handle_event(event);
        }
    }

    /// Full replay of `stream_id` into `projection` without pre-loading the
    /// event slice into a `Vec`.
    ///
    /// Uses [`EventStore::fold_stream`] internally so production backends can
    /// stream events with cursor-based pagination rather than loading all
    /// events at once.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    /// Returns [`EngineError::Deserialization`] when the fold closure returns
    /// an error (propagated from the store).
    pub async fn run_from_store<P, S>(
        projection: &mut P,
        store: &S,
        stream_id: &StreamId,
    ) -> Result<(), EngineError>
    where
        P: Projection + Send,
        S: EventStore,
    {
        store
            .fold_stream(stream_id, 0, (), |(), env| {
                projection.handle_event(&env);
                Ok(())
            })
            .await
    }

    /// Incremental catch-up of `stream_id` into `projection` without
    /// pre-loading the event slice into a `Vec`.
    ///
    /// Queries [`Projection::last_sequence`] to determine the starting point.
    /// If the projection returns `None`, performs a full replay (same as
    /// [`run_from_store`]).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    ///
    /// [`run_from_store`]: ProjectionRunner::run_from_store
    pub async fn catch_up_from_store<P, S>(
        projection: &mut P,
        store: &S,
        stream_id: &StreamId,
    ) -> Result<(), EngineError>
    where
        P: Projection + Send,
        S: EventStore,
    {
        let from = projection.last_sequence().unwrap_or(0);
        store
            .fold_stream(stream_id, from, (), |(), env| {
                projection.handle_event(&env);
                Ok(())
            })
            .await
    }

    // ── Multi-stream ─────────────────────────────────────────────────────────

    /// Full replay of multiple `stream_ids` into `projection`.
    ///
    /// Events from each stream are fed in sequence order within that stream.
    /// Streams are processed in the order given by `stream_ids` — if
    /// cross-stream event ordering matters, sort `stream_ids` accordingly
    /// or use a single global-sequence backend.
    ///
    /// Returns a [`GlobalProjectionCheckpoint`] recording the last-processed
    /// sequence number for every stream. Pass this to
    /// [`catch_up_all_streams`] for subsequent incremental updates.
    ///
    /// # Production workers: use `catch_up_persistent` instead
    ///
    /// `run_all_streams` performs a **full replay from sequence 0** every
    /// time it is called. In a long-running background worker this becomes
    /// prohibitively expensive as the event log grows. Use
    /// [`catch_up_persistent`] instead — it loads and saves a durable
    /// checkpoint so only events appended since the last run are fed to the
    /// projection.
    ///
    /// This method is appropriate for one-shot diagnostic tools, tests, or
    /// the very first population of a new projection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure for any stream.
    ///
    /// [`catch_up_all_streams`]: ProjectionRunner::catch_up_all_streams
    /// [`catch_up_persistent`]:  ProjectionRunner::catch_up_persistent
    #[must_use = "pass the returned checkpoint to subsequent catch_up_all_streams calls; \
                  dropping it silently restarts replay from the beginning"]
    pub async fn run_all_streams<P, S>(
        projection: &mut P,
        store: &S,
        stream_ids: &[StreamId],
    ) -> Result<GlobalProjectionCheckpoint, EngineError>
    where
        P: Projection + Send,
        S: EventStore,
    {
        let mut checkpoint = GlobalProjectionCheckpoint::new();
        for stream_id in stream_ids {
            let last_seq = store
                .fold_stream(stream_id, 0, 0u64, |_, env| {
                    let seq = env.sequence_number;
                    projection.handle_event(&env);
                    Ok(seq)
                })
                .await?;
            if last_seq > 0 {
                checkpoint.advance(stream_id, last_seq);
            }
        }
        Ok(checkpoint)
    }

    /// Incremental catch-up of multiple `stream_ids` into `projection`.
    ///
    /// For each stream, queries `checkpoint` for the last-processed sequence
    /// number and feeds only events newer than that cursor.
    ///
    /// Returns an updated [`GlobalProjectionCheckpoint`] reflecting the new
    /// cursors after this catch-up pass. Pass the returned checkpoint to the
    /// next `catch_up_all_streams` call — do not reuse the input checkpoint.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure for any stream.
    #[must_use = "pass the returned checkpoint to the next catch_up_all_streams call; \
                  dropping it silently discards incremental progress"]
    pub async fn catch_up_all_streams<P, S>(
        projection: &mut P,
        store: &S,
        stream_ids: &[StreamId],
        checkpoint: &GlobalProjectionCheckpoint,
    ) -> Result<GlobalProjectionCheckpoint, EngineError>
    where
        P: Projection + Send,
        S: EventStore,
    {
        let mut updated = checkpoint.clone();
        for stream_id in stream_ids {
            let from = checkpoint.cursor_for(stream_id);
            let last_seq = store
                .fold_stream(stream_id, from, from, |_, env| {
                    let seq = env.sequence_number;
                    projection.handle_event(&env);
                    Ok(seq)
                })
                .await?;
            if last_seq > from {
                updated.advance(stream_id, last_seq);
            }
        }
        Ok(updated)
    }

    /// Discover all streams matching `prefix` and replay them into `projection`.
    ///
    /// Convenience wrapper around [`EventStore::list_streams`] +
    /// [`run_all_streams`]. Useful when the full set of streams is not known
    /// at compile time.
    ///
    /// # Production workers: use `catch_up_persistent` instead
    ///
    /// This function performs a **full replay from sequence 0** every call.
    /// For persistent background workers, use [`catch_up_persistent`] so only
    /// events appended since the last checkpoint are processed.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failures.
    ///
    /// [`run_all_streams`]:     ProjectionRunner::run_all_streams
    /// [`catch_up_persistent`]: ProjectionRunner::catch_up_persistent
    pub async fn run_matching_streams<P, S>(
        projection: &mut P,
        store: &S,
        prefix: Option<&str>,
    ) -> Result<GlobalProjectionCheckpoint, EngineError>
    where
        P: Projection + Send,
        S: EventStore,
    {
        let streams = store.list_streams(prefix).await?;
        Self::run_all_streams(projection, store, &streams).await
    }

    /// Incremental catch-up of all streams matching `prefix`.
    ///
    /// Convenience wrapper for the common pattern of discovering streams and
    /// then calling `catch_up_all_streams`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failures.
    pub async fn catch_up_matching_streams<P, S>(
        projection: &mut P,
        store: &S,
        prefix: Option<&str>,
        checkpoint: &GlobalProjectionCheckpoint,
    ) -> Result<GlobalProjectionCheckpoint, EngineError>
    where
        P: Projection + Send,
        S: EventStore,
    {
        let streams = store.list_streams(prefix).await?;
        Self::catch_up_all_streams(projection, store, &streams, checkpoint).await
    }

    /// Incremental, persistent catch-up for all streams matching `prefix`.
    ///
    /// Loads the named checkpoint from `store`, performs an incremental
    /// catch-up of every matching stream, then saves the updated checkpoint
    /// back atomically.  On the next call, only events appended since the last
    /// run are processed — avoiding full replays across restarts.
    ///
    /// This is the preferred entry point for background projection workers
    /// that must survive process restarts.
    ///
    /// # Key space
    ///
    /// The SlateDB implementation stores `cp/{checkpoint_name}/{stream_id}` →
    /// `u64 LE` (8 bytes) per stream. Each cycle only writes the streams
    /// whose cursors advanced, giving O(changed_streams) write cost instead
    /// of O(total_streams).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on any storage failure (checkpoint load,
    /// event scan, or checkpoint save).
    pub async fn catch_up_persistent<P, S>(
        projection: &mut P,
        store: &S,
        prefix: Option<&str>,
        checkpoint_name: &str,
    ) -> Result<GlobalProjectionCheckpoint, EngineError>
    where
        P: Projection + Send,
        S: EventStore + ProjectionCheckpointStore,
    {
        let checkpoint = store.load_projection_checkpoint(checkpoint_name).await?;
        let streams = store.list_streams(prefix).await?;
        let updated = Self::catch_up_all_streams(projection, store, &streams, &checkpoint).await?;
        store
            .advance_projection_cursors(checkpoint_name, &checkpoint, &updated)
            .await?;
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        envelope::NewEvent,
        event_store::{ExpectedVersion, InMemoryEventStore},
        ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId},
        version::WorkflowId,
    };
    use serde_json::json;

    /// A simple counter projection that counts events and tracks its cursor.
    struct Counter {
        count: usize,
        last: Option<u64>,
    }

    impl Counter {
        fn new() -> Self {
            Self {
                count: 0,
                last: None,
            }
        }
    }

    impl Projection for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }

        fn handle_event(&mut self, envelope: &EventEnvelope) {
            self.count += 1;
            self.last = Some(envelope.sequence_number);
        }

        fn last_sequence(&self) -> Option<u64> {
            self.last
        }
    }

    fn make_event() -> NewEvent {
        NewEvent {
            correlation_id: CorrelationId::new(),
            causation_id: None,
            conversation_id: ConversationId::new(),
            process_id: ProcessId::new(),
            tenant_id: TenantId::new(),
            workflow_id: WorkflowId::new("test", "FV2024-10-01"),
            event_type: "TestEvent".into(),
            schema_version: 1,
            payload: json!({}),
        }
    }

    #[tokio::test]
    async fn run_from_store_full_replay() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::new("proj/s1");

        store
            .append(
                &stream,
                ExpectedVersion::NoStream,
                &[make_event(), make_event(), make_event()],
            )
            .await
            .unwrap();

        let mut proj = Counter::new();
        ProjectionRunner::run_from_store(&mut proj, &store, &stream)
            .await
            .unwrap();

        assert_eq!(proj.count, 3);
        assert_eq!(proj.last, Some(3));
    }

    #[tokio::test]
    async fn catch_up_from_store_incremental() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::new("proj/s2");

        store
            .append(
                &stream,
                ExpectedVersion::NoStream,
                &[make_event(), make_event()],
            )
            .await
            .unwrap();

        let mut proj = Counter::new();
        // Full replay first.
        ProjectionRunner::run_from_store(&mut proj, &store, &stream)
            .await
            .unwrap();
        assert_eq!(proj.count, 2);

        // Append two more events.
        store
            .append(
                &stream,
                ExpectedVersion::Exact(2),
                &[make_event(), make_event()],
            )
            .await
            .unwrap();

        // Incremental catch-up should feed only the two new events.
        ProjectionRunner::catch_up_from_store(&mut proj, &store, &stream)
            .await
            .unwrap();
        assert_eq!(proj.count, 4);
        assert_eq!(proj.last, Some(4));
    }

    // ── Multi-stream tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn run_all_streams_aggregates_across_multiple_streams() {
        let store = InMemoryEventStore::new();
        let s1 = StreamId::new("process/ms-s1");
        let s2 = StreamId::new("process/ms-s2");
        let s3 = StreamId::new("process/ms-s3");

        // 2 events in s1, 3 in s2, 1 in s3.
        store
            .append(
                &s1,
                ExpectedVersion::NoStream,
                &[make_event(), make_event()],
            )
            .await
            .unwrap();
        store
            .append(
                &s2,
                ExpectedVersion::NoStream,
                &[make_event(), make_event(), make_event()],
            )
            .await
            .unwrap();
        store
            .append(&s3, ExpectedVersion::NoStream, &[make_event()])
            .await
            .unwrap();

        let mut proj = Counter::new();
        let cp = ProjectionRunner::run_all_streams(
            &mut proj,
            &store,
            &[s1.clone(), s2.clone(), s3.clone()],
        )
        .await
        .unwrap();

        assert_eq!(proj.count, 6, "all 6 events across 3 streams must be fed");
        assert_eq!(cp.cursor_for(&s1), 2);
        assert_eq!(cp.cursor_for(&s2), 3);
        assert_eq!(cp.cursor_for(&s3), 1);
    }

    #[tokio::test]
    async fn catch_up_all_streams_feeds_only_new_events() {
        let store = InMemoryEventStore::new();
        let s1 = StreamId::new("process/cu-s1");
        let s2 = StreamId::new("process/cu-s2");

        store
            .append(
                &s1,
                ExpectedVersion::NoStream,
                &[make_event(), make_event()],
            )
            .await
            .unwrap();
        store
            .append(&s2, ExpectedVersion::NoStream, &[make_event()])
            .await
            .unwrap();

        let mut proj = Counter::new();
        let cp = ProjectionRunner::run_all_streams(&mut proj, &store, &[s1.clone(), s2.clone()])
            .await
            .unwrap();
        assert_eq!(proj.count, 3);
        assert_eq!(cp.cursor_for(&s1), 2);
        assert_eq!(cp.cursor_for(&s2), 1);

        // Add one event to each stream.
        store
            .append(&s1, ExpectedVersion::Exact(2), &[make_event()])
            .await
            .unwrap();
        store
            .append(
                &s2,
                ExpectedVersion::Exact(1),
                &[make_event(), make_event()],
            )
            .await
            .unwrap();

        let cp2 = ProjectionRunner::catch_up_all_streams(
            &mut proj,
            &store,
            &[s1.clone(), s2.clone()],
            &cp,
        )
        .await
        .unwrap();

        assert_eq!(proj.count, 6, "3 new events added across both streams");
        assert_eq!(cp2.cursor_for(&s1), 3, "s1 advanced from 2 to 3");
        assert_eq!(cp2.cursor_for(&s2), 3, "s2 advanced from 1 to 3");
    }

    #[tokio::test]
    async fn run_matching_streams_uses_prefix_filter() {
        let store = InMemoryEventStore::new();
        let proc1 = StreamId::new("process/match-p1");
        let proc2 = StreamId::new("process/match-p2");
        let partner = StreamId::new("partner/match-pp1"); // should NOT be included

        store
            .append(&proc1, ExpectedVersion::NoStream, &[make_event()])
            .await
            .unwrap();
        store
            .append(
                &proc2,
                ExpectedVersion::NoStream,
                &[make_event(), make_event()],
            )
            .await
            .unwrap();
        store
            .append(&partner, ExpectedVersion::NoStream, &[make_event()])
            .await
            .unwrap();

        let mut proj = Counter::new();
        let _ = ProjectionRunner::run_matching_streams(&mut proj, &store, Some("process/match-"))
            .await
            .unwrap();

        // Only the 3 events from proc1 + proc2 should have been fed.
        assert_eq!(
            proj.count, 3,
            "partner stream must be excluded by prefix filter"
        );
    }

    #[tokio::test]
    async fn global_projection_checkpoint_serde_roundtrip() {
        let mut cp = GlobalProjectionCheckpoint::new();
        cp.advance(&StreamId::new("p/1"), 5);
        cp.advance(&StreamId::new("p/2"), 3);

        let json = serde_json::to_string(&cp).unwrap();
        let cp2: GlobalProjectionCheckpoint = serde_json::from_str(&json).unwrap();

        assert_eq!(cp2.cursor_for(&StreamId::new("p/1")), 5);
        assert_eq!(cp2.cursor_for(&StreamId::new("p/2")), 3);
        assert_eq!(cp2.cursor_for(&StreamId::new("p/never")), 0);
    }

    #[tokio::test]
    async fn list_streams_with_prefix() {
        let store = InMemoryEventStore::new();
        let s1 = StreamId::new("process/ls-a");
        let s2 = StreamId::new("process/ls-b");
        let other = StreamId::new("partner/ls-c");

        store
            .append(&s1, ExpectedVersion::NoStream, &[make_event()])
            .await
            .unwrap();
        store
            .append(&s2, ExpectedVersion::NoStream, &[make_event()])
            .await
            .unwrap();
        store
            .append(&other, ExpectedVersion::NoStream, &[make_event()])
            .await
            .unwrap();

        let mut streams = store.list_streams(Some("process/")).await.unwrap();
        streams.sort_by_key(|s| s.as_str().to_owned()); // deterministic order
        assert_eq!(streams.len(), 2);
        assert!(streams.iter().any(|s| s.as_str() == "process/ls-a"));
        assert!(streams.iter().any(|s| s.as_str() == "process/ls-b"));

        let all = store.list_streams(None).await.unwrap();
        assert_eq!(all.len(), 3);
    }
}
