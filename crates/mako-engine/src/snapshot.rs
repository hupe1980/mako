//! Snapshotting support — a performance optimisation for long event streams.
//!
//! Snapshots are **never the source of truth**. They are always rebuildable
//! by replaying the full event stream. The engine must function correctly if
//! all snapshots are discarded.
//!
//! # Policy
//!
//! A common policy is to take a snapshot every N events (e.g. N = 100).
//! Use [`Snapshot::should_take`] to check whether enough events have
//! accumulated since the last snapshot.
//!
//! ```rust
//! use mako_engine::snapshot::Snapshot;
//!
//! // Take a snapshot every 100 events.
//! // second argument is the sequence_number of the last snapshot (0 = none).
//! assert!(!Snapshot::should_take(99,  0, 100));
//! assert!( Snapshot::should_take(100, 0, 100));
//! // Non-exact counts still trigger once the threshold is exceeded:
//! assert!( Snapshot::should_take(101, 0, 100));
//! // After a snapshot at seq 100, next triggers at 200+:
//! assert!(!Snapshot::should_take(101, 100, 100));
//! assert!( Snapshot::should_take(200, 100, 100));
//! // interval = 0 disables snapshotting (never returns true):
//! assert!(!Snapshot::should_take(1000, 0, 0));
//! ```
//!
//! # Using the store
//!
//! ```rust,ignore
//! use mako_engine::snapshot::{Snapshot, SnapshotStore};
//!
//! // After executing a command:
//! let event_count = process.event_count().await?;
//! let last_snap   = snap_store.load(process.stream_id()).await?
//!     .map_or(0, |s| s.sequence_number);
//! if Snapshot::should_take(event_count, last_snap, 100) {
//!     let state   = process.state().await?;
//!     let payload = serde_json::to_value(&state)?;
//!     let snap    = Snapshot::new(process.stream_id().clone(), event_count, 1, payload);
//!     snap_store.save(&snap).await?;
//! }
//! ```

use std::sync::Arc;

#[cfg(any(test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;

use serde_json::Value;

use crate::{error::EngineError, ids::StreamId};

// ── Snapshot ──────────────────────────────────────────────────────────────────

/// A point-in-time snapshot of an aggregate's state.
///
/// A snapshot carries the serialized state at a specific `sequence_number`.
/// During state reconstruction, the engine loads the snapshot (if any) and
/// then replays only the events that arrived after it.
///
/// ## Schema versioning
///
/// `state_schema_version` mirrors `EventEnvelope::schema_version`. Increment
/// it when the serialized state layout changes incompatibly. The engine must
/// discard snapshots whose schema version it does not recognise.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    /// The stream this snapshot covers.
    pub stream_id: StreamId,

    /// The sequence number of the last event incorporated into this snapshot.
    ///
    /// Events with a higher sequence number must still be replayed on top.
    pub sequence_number: u64,

    /// Schema version of the serialized `state` payload.
    pub state_schema_version: u32,

    /// The serialized aggregate state at `sequence_number`.
    ///
    /// Stored as [`serde_json::Value`] so the engine layer remains
    /// domain-agnostic; the domain crate deserializes it into the concrete
    /// `Workflow::State` type.
    pub state: Value,
}

impl Snapshot {
    /// Construct a new snapshot.
    #[must_use]
    pub fn new(
        stream_id: StreamId,
        sequence_number: u64,
        state_schema_version: u32,
        state: Value,
    ) -> Self {
        Self {
            stream_id,
            sequence_number,
            state_schema_version,
            state,
        }
    }

    /// Return `true` when a snapshot should be taken.
    ///
    /// Returns `true` when `event_count - last_snapshot_at >= interval`,
    /// i.e. at least `interval` new events have accumulated since the last
    /// snapshot. This avoids the exact-multiple trap: if snapshotting is
    /// skipped at count 100, it still triggers at 101, 102, etc. until a
    /// snapshot is taken.
    ///
    /// Set `last_snapshot_at` to `0` when no snapshot has ever been taken.
    ///
    /// Returns `false` when `interval` is `0` (snapshotting disabled).
    ///
    /// # Example
    ///
    /// ```
    /// use mako_engine::snapshot::Snapshot;
    ///
    /// // First snapshot: no prior snapshot (last_snapshot_at = 0).
    /// assert!(!Snapshot::should_take(99,  0, 100));
    /// assert!( Snapshot::should_take(100, 0, 100));
    /// assert!( Snapshot::should_take(101, 0, 100)); // non-exact still triggers
    ///
    /// // After a snapshot at seq 100, next triggers at 200+.
    /// assert!(!Snapshot::should_take(101, 100, 100));
    /// assert!( Snapshot::should_take(200, 100, 100));
    /// assert!( Snapshot::should_take(201, 100, 100));
    ///
    /// // interval = 0 disables snapshotting.
    /// assert!(!Snapshot::should_take(1000, 0, 0));
    /// ```
    #[must_use]
    pub fn should_take(event_count: u64, last_snapshot_at: u64, interval: u64) -> bool {
        if interval == 0 {
            return false;
        }
        event_count > 0 && event_count.saturating_sub(last_snapshot_at) >= interval
    }
}

// ── SnapshotStore ─────────────────────────────────────────────────────────────

/// Storage contract for aggregate snapshots.
///
/// Implementations live in separate backend crates
/// (`mako-event-store-slatedb`, `mako-event-store-redb`, etc.).
/// [`NoopSnapshotStore`] is provided here for tests and deployments that do not
/// need snapshotting.
///
/// ## Contract
///
/// - `save` must persist the snapshot durably before returning `Ok`.
/// - `load` must return the **most recent** snapshot for `stream_id`, or
///   `None` if no snapshot exists.
/// - A snapshot must never be returned for a `stream_id` if its
///   `state_schema_version` is not recognised by the caller — implementations
///   are encouraged to store schema version in a queryable column/key so
///   callers can filter by it.
///
/// ## Blanket `Arc` implementation
///
/// `Arc<S>` implements `SnapshotStore` whenever `S: SnapshotStore`, so
/// `Process<W, Arc<MyEventStore>>` can accept `Arc<MySnapshotStore>` without
/// any extra wrapper.
#[allow(async_fn_in_trait)]
pub trait SnapshotStore: Send + Sync {
    /// Persist `snapshot`, replacing any previous snapshot for the same stream.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Snapshot`] on storage failure.
    async fn save(&self, snapshot: &Snapshot) -> Result<(), EngineError>;

    /// Load the most recent snapshot for `stream_id`.
    ///
    /// Returns `None` when no snapshot exists (full replay required).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Snapshot`] on storage failure.
    async fn load(&self, stream_id: &StreamId) -> Result<Option<Snapshot>, EngineError>;
}

// ── Arc<S> blanket impl ───────────────────────────────────────────────────────

impl<S: SnapshotStore> SnapshotStore for Arc<S> {
    async fn save(&self, snapshot: &Snapshot) -> Result<(), EngineError> {
        self.as_ref().save(snapshot).await
    }

    async fn load(&self, stream_id: &StreamId) -> Result<Option<Snapshot>, EngineError> {
        self.as_ref().load(stream_id).await
    }
}

// ── NoopSnapshotStore ─────────────────────────────────────────────────────────

/// A [`SnapshotStore`] that never persists anything.
///
/// Every `load` returns `None` (full replay); every `save` succeeds silently.
///
/// # ⚠️ Data loss warning
///
/// `NoopSnapshotStore` **discards every snapshot silently**. Processes built
/// with this store perform full event replay on every state read. Do not use
/// in production — bind a `SlateDbStore::as_snapshot_store()` (requires `slatedb`
/// feature) instead.
///
/// The [`SnapshotStore`] implementation is only compiled when the `testing`
/// feature is enabled or inside `#[cfg(test)]`. Production binaries must call
/// `EngineBuilder::with_snapshot_store` with a durable backend.
#[derive(Debug, Clone, Copy, Default)]
#[must_use = "NoopSnapshotStore discards all snapshots silently — use a persistent SnapshotStore in production"]
pub struct NoopSnapshotStore;

#[cfg(any(test, feature = "testing"))]
impl SnapshotStore for NoopSnapshotStore {
    async fn save(&self, _snapshot: &Snapshot) -> Result<(), EngineError> {
        Ok(())
    }

    async fn load(&self, _stream_id: &StreamId) -> Result<Option<Snapshot>, EngineError> {
        Ok(None)
    }
}

// ── InMemorySnapshotStore ─────────────────────────────────────────────────────

/// An in-memory [`SnapshotStore`] for tests and development.
///
/// Stores the **most recent snapshot per stream**. Cloning shares the
/// underlying data via `Arc` — all clones see the same snapshots.
///
/// Use this with [`Process::take_snapshot`] and
/// [`Process::state_with_snapshot`] to verify snapshot-accelerated replay
/// without depending on an external storage backend.
///
/// Only available in `#[cfg(test)]` or with the `testing` feature enabled.
///
/// [`Process::take_snapshot`]: crate::process::Process::take_snapshot
/// [`Process::state_with_snapshot`]: crate::process::Process::state_with_snapshot
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default, Clone)]
pub struct InMemorySnapshotStore {
    inner: Arc<RwLock<HashMap<StreamId, Snapshot>>>,
}

#[cfg(any(test, feature = "testing"))]
impl InMemorySnapshotStore {
    /// Create an empty snapshot store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return `true` when no snapshots are stored.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

#[cfg(any(test, feature = "testing"))]
impl SnapshotStore for InMemorySnapshotStore {
    async fn save(&self, snapshot: &Snapshot) -> Result<(), EngineError> {
        self.inner
            .write()
            .await
            .insert(snapshot.stream_id.clone(), snapshot.clone());
        Ok(())
    }

    async fn load(&self, stream_id: &StreamId) -> Result<Option<Snapshot>, EngineError> {
        Ok(self.inner.read().await.get(stream_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_take_at_exact_multiples() {
        // No previous snapshot (last_snapshot_at = 0).
        assert!(!Snapshot::should_take(0, 0, 100));
        assert!(!Snapshot::should_take(99, 0, 100));
        assert!(Snapshot::should_take(100, 0, 100));
        // Non-exact still triggers ("at-least" semantics):
        assert!(Snapshot::should_take(101, 0, 100));
        assert!(Snapshot::should_take(200, 0, 100));
        // After snapshot at 100, next triggers at 200+:
        assert!(!Snapshot::should_take(101, 100, 100));
        assert!(Snapshot::should_take(200, 100, 100));
        assert!(Snapshot::should_take(250, 100, 100));
    }

    #[test]
    fn should_take_interval_one() {
        // Every event triggers a snapshot (last_snapshot_at = prev count).
        for i in 1u64..=5 {
            assert!(Snapshot::should_take(i, i - 1, 1));
        }
    }

    #[tokio::test]
    async fn noop_store_always_returns_none() {
        let store = NoopSnapshotStore;
        let stream = StreamId::new("process/test");
        assert!(store.load(&stream).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn noop_store_save_succeeds() {
        let store = NoopSnapshotStore;
        let snap = Snapshot::new(
            StreamId::new("process/test"),
            10,
            1,
            serde_json::json!({"status": "Active"}),
        );
        assert!(store.save(&snap).await.is_ok());
    }

    // ── InMemorySnapshotStore ─────────────────────────────────────────────────

    #[tokio::test]
    async fn in_memory_store_round_trip() {
        let store = InMemorySnapshotStore::new();
        let stream = StreamId::new("process/abc");
        let snap = Snapshot::new(stream.clone(), 5, 1, serde_json::json!({"x": 1}));

        store.save(&snap).await.unwrap();

        let loaded = store
            .load(&stream)
            .await
            .unwrap()
            .expect("snapshot must exist");
        assert_eq!(loaded.sequence_number, 5);
        assert_eq!(loaded.state_schema_version, 1);
        assert_eq!(loaded.state, serde_json::json!({"x": 1}));
    }

    #[tokio::test]
    async fn in_memory_store_overwrite_keeps_latest() {
        let store = InMemorySnapshotStore::new();
        let stream = StreamId::new("process/abc");

        store
            .save(&Snapshot::new(
                stream.clone(),
                5,
                1,
                serde_json::json!({"seq": 5}),
            ))
            .await
            .unwrap();
        store
            .save(&Snapshot::new(
                stream.clone(),
                10,
                1,
                serde_json::json!({"seq": 10}),
            ))
            .await
            .unwrap();

        let loaded = store
            .load(&stream)
            .await
            .unwrap()
            .expect("snapshot must exist");
        assert_eq!(
            loaded.sequence_number, 10,
            "second save must overwrite first"
        );
    }

    #[tokio::test]
    async fn in_memory_store_separate_streams_isolated() {
        let store = InMemorySnapshotStore::new();
        let stream1 = StreamId::new("process/aaa");
        let stream2 = StreamId::new("process/bbb");

        store
            .save(&Snapshot::new(
                stream1.clone(),
                3,
                1,
                serde_json::json!(null),
            ))
            .await
            .unwrap();

        assert!(
            store.load(&stream2).await.unwrap().is_none(),
            "unrelated stream must not return stream1's snapshot"
        );
    }

    #[tokio::test]
    async fn in_memory_store_is_empty_initially() {
        let store = InMemorySnapshotStore::new();
        assert!(store.is_empty().await);

        let stream = StreamId::new("process/test");
        store
            .save(&Snapshot::new(stream, 1, 1, serde_json::json!({})))
            .await
            .unwrap();
        assert!(!store.is_empty().await);
    }

    #[tokio::test]
    async fn in_memory_store_clone_shares_data() {
        let store1 = InMemorySnapshotStore::new();
        let store2 = store1.clone();
        let stream = StreamId::new("process/shared");

        store1
            .save(&Snapshot::new(
                stream.clone(),
                7,
                1,
                serde_json::json!({"y": 2}),
            ))
            .await
            .unwrap();

        let loaded = store2
            .load(&stream)
            .await
            .unwrap()
            .expect("clone must see the same snapshot");
        assert_eq!(loaded.sequence_number, 7);
    }
}
