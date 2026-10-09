//! [`EventStore`] trait and the in-process `InMemoryEventStore` implementation.
//!
//! The engine defines only the trait. The production implementation is
//! `SlateDbStore` (crate `store_slatedb`), enabled by the `slatedb`
//! feature flag. `InMemoryEventStore` is included here for tests, spikes, and
//! development without external dependencies.

use std::sync::Arc;

#[cfg(any(test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "testing"))]
use time::OffsetDateTime;
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;

use crate::{
    envelope::{EventEnvelope, NewEvent},
    error::EngineError,
    ids::StreamId,
};

// ── ExpectedVersion ───────────────────────────────────────────────────────────

/// Optimistic concurrency control contract for [`AtomicAppend::append`].
///
/// The caller declares which sequence number they expect the stream to be at.
/// The store atomically checks this before writing; a mismatch means a
/// concurrent writer modified the stream and the caller must reload and retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedVersion {
    /// The stream must not exist yet (sequence number 0).
    NoStream,
    /// The stream must be at exactly this sequence number.
    Exact(u64),
    /// Skip the concurrency check entirely.
    ///
    /// **Do not use in production workflow append paths.** `Any` silently
    /// accepts any write regardless of concurrent modifications, which can
    /// produce duplicate or interleaved events in a stream.
    ///
    /// Legitimate uses: [`MigrationRunner`] (bulk admin rewrites),
    /// snapshot-accelerated store internals, and test scaffolding where the
    /// caller owns all write access by construction.
    ///
    /// For normal workflow event appends always use [`ExpectedVersion::NoStream`]
    /// (first event) or [`ExpectedVersion::Exact`] (subsequent events).
    ///
    /// [`MigrationRunner`]: crate::migration::MigrationRunner
    Any,
}

// ── AppendResult ──────────────────────────────────────────────────────────────

/// What a successful [`AtomicAppend::append`] wrote.
#[derive(Debug, Clone)]
pub struct AppendResult {
    /// The sequence number of the last event written in this batch.
    pub last_sequence: u64,
    /// The fully materialised envelopes as persisted by the store.
    ///
    /// Each envelope has its `event_id`, `sequence_number`, `stream_id`, and
    /// `timestamp` stamped by the store. Callers use these for return values
    /// and projection seeding without re-loading from storage.
    pub events: Vec<EventEnvelope>,
    /// The outbox entries enqueued in the same write, materialised against the
    /// stamped events (`causation_event_id` names the event that caused each).
    pub outbox: Vec<crate::outbox::OutboxMessage>,
}

// ── EventStore trait ──────────────────────────────────────────────────────────

/// The read side of an append-only, ordered event stream store.
///
/// Writing is [`AtomicAppend::append`]: one batch, one method, so no store can
/// persist the events of a command and drop what must land with them.
///
/// ## Implementation requirements
///
/// - **Ordered**: events within a stream are always returned in append order.
/// - **Atomic**: a multi-event append either fully succeeds or fully fails.
/// - **Optimistic concurrency**: detect concurrent writers via
///   [`ExpectedVersion`].
/// - **Append-only**: events are never modified or deleted through this API.
/// - **Sequence number ownership**: the store assigns `sequence_number`,
///   `event_id`, `stream_id`, and `timestamp` on each appended envelope.
///   Callers submit [`NewEvent`] values without these fields.
///
/// ## Blanket `Arc` implementation
///
/// `Arc<S>` implements `EventStore` whenever `S: EventStore`, so
/// `Process<W, Arc<MyStore>>` works without any extra wrapper type.
#[expect(
    async_fn_in_trait,
    reason = "RPIT-in-traits with Send bounds require AFIT; use #[allow] until"
)]
// the ecosystem settles on a stable pattern for Rust 1.85 MSRV.
pub trait EventStore: Send + Sync {
    /// Load all events from `stream_id` in sequence order.
    ///
    /// Returns an empty `Vec` when the stream does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] for underlying storage failures.
    #[must_use = "dropping a load Result silently discards a store error"]
    async fn load(&self, stream_id: &StreamId) -> Result<Vec<EventEnvelope>, EngineError>;

    /// Load events from `stream_id` starting after `from_sequence` (exclusive).
    ///
    /// Useful for incremental projection catch-up: pass the projection's last
    /// processed sequence number to load only new events.
    ///
    /// Returns an empty `Vec` when no new events exist.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] for underlying storage failures.
    #[must_use = "dropping a load_from Result silently discards a store error"]
    async fn load_from(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> Result<Vec<EventEnvelope>, EngineError>;

    /// Return the current sequence number of `stream_id`.
    ///
    /// The sequence number equals the number of events in the stream (1-based
    /// after the first append). Returns `0` when the stream does not exist.
    ///
    /// Use this instead of `load(…).await?.len()` when you only need the
    /// count — backends can implement this as a cheap metadata query without
    /// transferring event payloads.
    ///
    /// **Required.** There is no default implementation — a fallback that
    /// loads all events defeats the O(1) metadata-query contract. Implementors
    /// must read the stored sequence counter directly (e.g. a `sv/{stream_id}`
    /// key in SlateDB) to avoid O(n) event-payload transfers.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] for underlying storage failures.
    #[must_use = "dropping a stream_version Result silently discards a store error"]
    async fn stream_version(&self, stream_id: &StreamId) -> Result<u64, EngineError>;

    /// Return all known stream identifiers in this store, optionally filtered
    /// by `prefix`.
    ///
    /// When `prefix` is `Some("process/")`, only streams whose identifiers
    /// start with `"process/"` are returned (e.g. all process-instance
    /// streams). When `prefix` is `None`, all streams are returned.
    ///
    /// This is the primary enumeration API for multi-stream projections: the
    /// caller discovers all relevant streams, then passes the list to
    /// [`crate::projection::ProjectionRunner::run_all_streams`] /
    /// [`crate::projection::ProjectionRunner::catch_up_all_streams`].
    ///
    /// The returned order is unspecified. Stable ordering can be achieved by
    /// sorting the `Vec` before use if deterministic replay is required.
    ///
    /// **Required.** There is no default implementation — a missing override
    /// silently returns no streams, causing multi-stream projections (e.g.
    /// MABIS billing aggregations) to process zero streams and return empty
    /// read models with no error signal.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] for underlying storage failures.
    #[must_use = "dropping a list_streams Result silently discards a store error"]
    async fn list_streams(&self, prefix: Option<&str>) -> Result<Vec<StreamId>, EngineError>;

    /// Paginated stream enumeration — equivalent to `list_streams` but returns
    /// at most `limit` entries starting after `cursor` (exclusive, UTF-8
    /// stream-ID order).
    ///
    /// # Parameters
    ///
    /// - `prefix` — optional key prefix to restrict the scan (same semantics as
    ///   `list_streams`).
    /// - `cursor` — if `Some(s)`, resume after stream ID `s`; `None` starts
    ///   from the beginning.
    /// - `limit` — maximum number of stream IDs to return per page.  A return
    ///   count strictly less than `limit` indicates the last page.
    ///
    /// # Page iteration pattern
    ///
    /// ```rust,ignore
    /// let mut cursor: Option<StreamId> = None;
    /// loop {
    ///     let page = store.list_streams_page(Some("process/"), cursor.as_ref(), 100).await?;
    ///     let done = page.len() < 100;
    ///     for id in &page { /* process */ }
    ///     cursor = page.into_iter().last();
    ///     if done { break; }
    /// }
    /// ```
    ///
    /// # Default implementation
    ///
    /// Falls back to `list_streams` + in-memory slicing for stores that do not
    /// provide a native cursor scan.
    ///
    /// # ⚠️ Override required for production stores
    ///
    /// This default loads **all** matching stream IDs into memory on every call,
    /// making `list_streams_page` loops O(n²) in total stream count.  Any
    /// production `EventStore` implementation (e.g. PostgreSQL, CockroachDB)
    /// **must** override this method with an efficient cursor-based scan.  The
    /// SlateDB store already provides such an override.  Failure to override
    /// this method will cause projection catch-up to degrade silently under
    /// deployments with > 10,000 active process streams.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] for underlying storage failures.
    #[must_use = "dropping a list_streams_page Result silently discards a store error"]
    async fn list_streams_page(
        &self,
        prefix: Option<&str>,
        cursor: Option<&StreamId>,
        limit: usize,
    ) -> Result<Vec<StreamId>, EngineError> {
        // Default: enumerate all + skip-after-cursor + take(limit).
        let all = self.list_streams(prefix).await?;
        let iter: Box<dyn Iterator<Item = StreamId>> = match cursor {
            None => Box::new(all.into_iter()),
            Some(c) => Box::new(all.into_iter().skip_while(move |id| id != c).skip(1)),
        };
        Ok(iter.take(limit).collect())
    }

    /// Fold over events in `stream_id` starting after `from_sequence`
    /// (exclusive), accumulating state without materialising the full
    /// `Vec<EventEnvelope>`.
    ///
    /// This is the memory-efficient alternative to `load_from` for large
    /// streams. Instead of returning all events as a Vec, it applies `f` to
    /// each event in order and returns the final accumulated value.
    ///
    /// `from_sequence = 0` folds from the beginning of the stream.
    ///
    /// ```rust,ignore
    /// // Reconstruct process state event-by-event without a Vec allocation:
    /// let state = store.fold_stream(
    ///     &stream_id, 0, W::State::default(),
    ///     |acc, env| Ok(acc.apply(env.event()))
    /// ).await?;
    /// ```
    ///
    /// **Required.** There is no default implementation — a fallback that
    /// materialises `load_from(...)` into a `Vec` defeats the purpose of this
    /// method for large MABIS billing streams (potentially thousands of events
    /// per billing period). Implementors must provide a cursor-based scan for
    /// constant-memory behaviour.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] for underlying storage failures.
    #[must_use = "dropping a fold_stream Result silently discards a store or fold error"]
    async fn fold_stream<T, F>(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
        initial: T,
        f: F,
    ) -> Result<T, EngineError>
    where
        T: Send,
        F: FnMut(T, EventEnvelope) -> Result<T, EngineError> + Send;
}

// ── Arc<S> blanket impl ───────────────────────────────────────────────────────

impl<S: EventStore> EventStore for Arc<S> {
    async fn load(&self, stream_id: &StreamId) -> Result<Vec<EventEnvelope>, EngineError> {
        self.as_ref().load(stream_id).await
    }

    async fn load_from(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> Result<Vec<EventEnvelope>, EngineError> {
        self.as_ref().load_from(stream_id, from_sequence).await
    }

    async fn stream_version(&self, stream_id: &StreamId) -> Result<u64, EngineError> {
        self.as_ref().stream_version(stream_id).await
    }

    async fn list_streams(&self, prefix: Option<&str>) -> Result<Vec<StreamId>, EngineError> {
        self.as_ref().list_streams(prefix).await
    }

    async fn list_streams_page(
        &self,
        prefix: Option<&str>,
        cursor: Option<&StreamId>,
        limit: usize,
    ) -> Result<Vec<StreamId>, EngineError> {
        self.as_ref().list_streams_page(prefix, cursor, limit).await
    }

    async fn fold_stream<T, F>(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
        initial: T,
        f: F,
    ) -> Result<T, EngineError>
    where
        T: Send,
        F: FnMut(T, EventEnvelope) -> Result<T, EngineError> + Send,
    {
        self.as_ref()
            .fold_stream(stream_id, from_sequence, initial, f)
            .await
    }
}

// ── Arc<S>: AtomicAppend blanket impl ────────────────────────────────────────

/// `Arc<S>` inherits the [`AtomicAppend`] contract from `S`, so
/// `Process<W, Arc<SlateDbStore>>` runs commands without unwrapping the `Arc`.
impl<S: AtomicAppend> AtomicAppend for Arc<S> {
    async fn append(&self, batch: AppendBatch<'_>) -> Result<AppendResult, EngineError> {
        self.as_ref().append(batch).await
    }
}

// ── AppendBatch ───────────────────────────────────────────────────────────────

/// One atomic write: events, and everything that must land with them.
///
/// A command's output is more than its events. The outbox entries are the
/// messages the events promise to send, the deadlines are the Fristen they
/// start, and the correlation entries are how the counterparty's reply finds
/// the process. Each is worthless without the events and the events are wrong
/// without each, so the four travel as one value into one method.
///
/// Built from the events up; every other part defaults to empty:
///
/// ```rust,ignore
/// store.append(
///     AppendBatch::new(&stream_id, ExpectedVersion::Exact(n), &events)
///         .outbox(&outbox)
///         .deadlines(&deadlines)
///         .correlations(&correlations),
/// ).await?;
/// ```
#[derive(Debug, Clone, Copy)]
#[must_use = "an AppendBatch writes nothing until it is passed to AtomicAppend::append"]
pub struct AppendBatch<'a> {
    /// The stream the events are appended to.
    pub stream_id: &'a StreamId,
    /// The optimistic-concurrency check the write must pass.
    pub expected_version: ExpectedVersion,
    /// The events, stamped by the store.
    pub events: &'a [NewEvent],
    /// Outbound messages, materialised against the stamped events.
    pub outbox: &'a [crate::outbox::PendingOutbox],
    /// Deadlines registered in the same write.
    pub deadlines: &'a [crate::deadline::Deadline],
    /// Correlation-index entries written in the same write.
    pub correlations: &'a [CorrelationEntry],
}

impl<'a> AppendBatch<'a> {
    /// A batch of `events` alone.
    pub fn new(
        stream_id: &'a StreamId,
        expected_version: ExpectedVersion,
        events: &'a [NewEvent],
    ) -> Self {
        Self {
            stream_id,
            expected_version,
            events,
            outbox: &[],
            deadlines: &[],
            correlations: &[],
        }
    }

    /// Enqueue `outbox` in the same write.
    pub fn outbox(mut self, outbox: &'a [crate::outbox::PendingOutbox]) -> Self {
        self.outbox = outbox;
        self
    }

    /// Register `deadlines` in the same write.
    pub fn deadlines(mut self, deadlines: &'a [crate::deadline::Deadline]) -> Self {
        self.deadlines = deadlines;
        self
    }

    /// Write `correlations` in the same write.
    pub fn correlations(mut self, correlations: &'a [CorrelationEntry]) -> Self {
        self.correlations = correlations;
        self
    }

    /// Refuse a batch no store may write, before anything is staged.
    ///
    /// An outbox entry names the event that caused it; with no event there is
    /// nothing to name, and the message would be sent with no record in the
    /// stream of why. A malformed correlation tag would corrupt the index key.
    ///
    /// # Errors
    ///
    /// [`EngineError::Outbox`] for an outbox entry without an event,
    /// [`EngineError::Registry`] for a malformed correlation tag.
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.events.is_empty() && !self.outbox.is_empty() {
            return Err(EngineError::outbox(
                "an outbox entry needs an event in the same batch to name as its cause",
            ));
        }
        for c in self.correlations {
            validate_correlation_tag(&c.tag)?;
        }
        Ok(())
    }
}

/// Validate a correlation-index tag before it becomes part of a key.
///
/// Rejects tags that would corrupt the `ci/{tenant}/{tag}/{process}` key
/// structure and leak across tags in a prefix scan:
///
/// - `\0` corrupts key scan boundaries on byte-string backends.
/// - An empty tag collapses the tag segment and makes lookups ambiguous.
/// - `/` creates a sub-prefix that leaks into or out of sibling tags.
/// - More than 128 bytes offers nothing and produces abnormally long keys.
pub(crate) fn validate_correlation_tag(tag: &str) -> Result<(), EngineError> {
    if tag.contains('\0') {
        return Err(EngineError::registry(
            "ci tag must not contain NUL bytes ('\\0')",
        ));
    }
    if tag.is_empty() {
        return Err(EngineError::registry("ci tag must not be empty"));
    }
    if tag.contains('/') {
        return Err(EngineError::registry(
            "ci tag must not contain '/' — use '-' or ':' as a separator",
        ));
    }
    if tag.len() > 128 {
        return Err(EngineError::registry(
            "ci tag exceeds the 128-byte maximum length",
        ));
    }
    Ok(())
}

// ── AtomicAppend trait ────────────────────────────────────────────────────────

/// The write side of an event store: one batch, written whole or not at all.
///
/// [`AtomicAppend::append`] is the only write a store offers, and it has no
/// default body. A store therefore cannot persist a command's events while
/// dropping its outbox entries, its deadlines or its correlation entries: there
/// is no narrower method to implement instead. For SlateDB the batch is one
/// SSI transaction.
///
/// A store that implements only an events-and-outbox write does not compile:
///
/// ```compile_fail
/// use mako_engine::{
///     envelope::{EventEnvelope, NewEvent},
///     error::EngineError,
///     event_store::{AppendResult, AtomicAppend, EventStore, ExpectedVersion},
///     ids::StreamId,
///     outbox::PendingOutbox,
/// };
///
/// struct ForgetsDeadlines;
///
/// impl EventStore for ForgetsDeadlines {
///     async fn load(&self, _: &StreamId) -> Result<Vec<EventEnvelope>, EngineError> { todo!() }
///     async fn load_from(&self, _: &StreamId, _: u64) -> Result<Vec<EventEnvelope>, EngineError> { todo!() }
///     async fn stream_version(&self, _: &StreamId) -> Result<u64, EngineError> { todo!() }
///     async fn list_streams(&self, _: Option<&str>) -> Result<Vec<StreamId>, EngineError> { todo!() }
///     async fn fold_stream<T: Send, F>(&self, _: &StreamId, _: u64, _: T, _: F) -> Result<T, EngineError>
///     where F: FnMut(T, EventEnvelope) -> Result<T, EngineError> + Send { todo!() }
/// }
///
/// // Events and outbox, and nothing for the deadlines: refused, because the
/// // trait's one method takes the whole batch and has no default.
/// impl AtomicAppend for ForgetsDeadlines {
///     async fn append_with_outbox(
///         &self,
///         _: &StreamId,
///         _: ExpectedVersion,
///         _: &[NewEvent],
///         _: &[PendingOutbox],
///     ) -> Result<AppendResult, EngineError> {
///         todo!()
///     }
/// }
/// ```
///
/// # Safety
///
/// Never write events first and the rest second — a crash between the two
/// produces a lost APERAK, a Frist that never fires, or a process its
/// counterparty's reply cannot find.
#[expect(
    async_fn_in_trait,
    reason = "the trait is implemented and awaited within this workspace, where the future's Send bound is known"
)]
pub trait AtomicAppend: EventStore {
    /// Write `batch` atomically: its events, outbox entries, deadlines and
    /// correlation entries all land, or none does.
    ///
    /// The store stamps the events and materialises each outbox entry against
    /// the event it names (`caused_by_event_index`), returning both.
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] — the optimistic concurrency check
    ///   failed; reload state and retry.
    /// - [`EngineError::Outbox`] / [`EngineError::Registry`] — the batch is
    ///   malformed ([`AppendBatch::validate`]); nothing was written.
    /// - [`EngineError::Store`], [`EngineError::Outbox`],
    ///   [`EngineError::Deadline`] or [`EngineError::Registry`] — storage
    ///   failure; nothing was written.
    #[must_use = "dropping an append Result silently discards a version-conflict or store error"]
    async fn append(&self, batch: AppendBatch<'_>) -> Result<AppendResult, EngineError>;
}

/// One correlation-index entry: "this business key resolves to this process".
///
/// Written in the same batch as the events that justify it — see
/// [`AppendBatch::correlations`].
#[derive(Debug, Clone)]
pub struct CorrelationEntry {
    /// Tenant the key is scoped to.
    pub tenant_id: crate::ids::TenantId,
    /// The business key — a MaLo, MeLo, Vorgangs- or Belegnummer.
    pub tag: String,
    /// Process the key resolves to.
    pub process_id: crate::ids::ProcessId,
    /// Full identity, so a lookup can rebuild a `Process` without a second read.
    pub identity: crate::ids::ProcessIdentity,
}

// ── InMemoryEventStore ────────────────────────────────────────────────────────

/// Internal state held behind the `Arc<Mutex<…>>`.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default)]
struct InMemoryState {
    /// Per-stream ordered event log (sequence-number indexed).
    streams: HashMap<StreamId, Vec<EventEnvelope>>,
    /// Global insertion-order log across all streams.
    ///
    /// This is the source for [`InMemoryEventStore::all_events`]. Keeping a
    /// separate flat list avoids collecting + sorting across stream-local
    /// sequence namespaces, which would produce an arbitrary ordering when
    /// multiple streams are active.
    global: Vec<EventEnvelope>,
    /// Outbox entries, in enqueue order.
    outbox: Vec<crate::outbox::OutboxMessage>,
    /// Registered deadlines, in registration order.
    deadlines: Vec<crate::deadline::Deadline>,
    /// Correlation-index entries, in write order.
    correlations: Vec<CorrelationEntry>,
}

/// A fully in-memory [`EventStore`] and [`AtomicAppend`] for testing and
/// development.
///
/// Backed by logs protected by one `RwLock`, so an [`AppendBatch`] lands
/// whole:
/// - A per-stream `HashMap` for sequence-ordered access.
/// - A global flat `Vec` that preserves cross-stream insertion order.
/// - The outbox entries, deadlines and correlation entries each batch carried,
///   readable through [`InMemoryEventStore::outbox_messages`],
///   [`InMemoryEventStore::deadlines_for`] and
///   [`InMemoryEventStore::correlations`].
///
/// The store assigns `event_id`, `sequence_number`, `stream_id`, and
/// `timestamp` to each appended event — callers submit [`NewEvent`] values.
///
/// Cloning the store shares the underlying data via `Arc` — all clones see
/// the same events.
///
/// **Not suitable for production.** Use this for:
/// - Unit and integration tests
/// - Spikes and local development
/// - CI environments that must not depend on external services
///
/// Only available in `#[cfg(test)]` or with the `testing` feature enabled.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default, Clone)]
pub struct InMemoryEventStore {
    inner: Arc<RwLock<InMemoryState>>,
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryEventStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return all events across all streams in insertion order.
    ///
    /// Because sequence numbers are stream-local, this method uses the
    /// insertion-order global log rather than sorting by sequence number.
    ///
    /// **Test/development use only.** Production code should use
    /// [`EventStore::load`] or [`EventStore::load_from`] to read specific
    /// streams. Loading all events at once from a production store can OOM
    /// the process.
    ///
    /// Available only when the `testing` feature is enabled or in `cfg(test)`.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub async fn all_events(&self) -> Vec<EventEnvelope> {
        self.inner.read().await.global.clone()
    }

    /// Return all events for a specific stream in sequence order.
    ///
    /// **Test/development use only.** In production code, prefer
    /// [`EventStore::load`] which is part of the trait contract.
    ///
    /// Available only when the `testing` feature is enabled or in `cfg(test)`.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub async fn events_for(&self, stream_id: &StreamId) -> Vec<EventEnvelope> {
        self.inner
            .read()
            .await
            .streams
            .get(stream_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Every outbox entry enqueued so far, in enqueue order.
    #[must_use]
    pub async fn outbox_messages(&self) -> Vec<crate::outbox::OutboxMessage> {
        self.inner.read().await.outbox.clone()
    }

    /// Every deadline registered for `stream_id`, in registration order.
    #[must_use]
    pub async fn deadlines_for(&self, stream_id: &StreamId) -> Vec<crate::deadline::Deadline> {
        self.inner
            .read()
            .await
            .deadlines
            .iter()
            .filter(|d| d.stream_id() == stream_id)
            .cloned()
            .collect()
    }

    /// Every correlation-index entry written so far, in write order.
    #[must_use]
    pub async fn correlations(&self) -> Vec<CorrelationEntry> {
        self.inner.read().await.correlations.clone()
    }
}

#[cfg(any(test, feature = "testing"))]
impl AtomicAppend for InMemoryEventStore {
    async fn append(&self, batch: AppendBatch<'_>) -> Result<AppendResult, EngineError> {
        batch.validate()?;
        let AppendBatch {
            stream_id,
            expected_version,
            events: new_events,
            outbox,
            deadlines,
            correlations,
        } = batch;
        let mut inner = self.inner.write().await;

        let current = inner.streams.get(stream_id).map_or(0, |s| s.len() as u64);

        // Optimistic concurrency check.
        match expected_version {
            ExpectedVersion::NoStream => {
                if current != 0 {
                    return Err(EngineError::VersionConflict {
                        expected: 0,
                        actual: current,
                    });
                }
            }
            ExpectedVersion::Exact(v) => {
                if current != v {
                    return Err(EngineError::VersionConflict {
                        expected: v,
                        actual: current,
                    });
                }
            }
            ExpectedVersion::Any => {}
        }

        // Stamp each NewEvent with store-assigned fields.
        let now = OffsetDateTime::now_utc();
        let envelopes: Vec<EventEnvelope> = new_events
            .iter()
            .enumerate()
            .map(|(i, new)| {
                EventEnvelope::from_new(new.clone(), stream_id.clone(), current + i as u64 + 1, now)
            })
            .collect();
        let messages: Vec<crate::outbox::OutboxMessage> = outbox
            .iter()
            .map(|p| crate::outbox::materialise(p, stream_id, &envelopes, now))
            .collect();

        // Everything below is infallible, so the batch lands whole under the
        // one write lock.
        inner
            .streams
            .entry(stream_id.clone())
            .or_default()
            .extend_from_slice(&envelopes);
        inner.global.extend_from_slice(&envelopes);
        inner.outbox.extend_from_slice(&messages);
        inner.deadlines.extend_from_slice(deadlines);
        inner.correlations.extend_from_slice(correlations);

        Ok(AppendResult {
            last_sequence: current + new_events.len() as u64,
            events: envelopes,
            outbox: messages,
        })
    }
}

#[cfg(any(test, feature = "testing"))]
impl EventStore for InMemoryEventStore {
    async fn load(&self, stream_id: &StreamId) -> Result<Vec<EventEnvelope>, EngineError> {
        let inner = self.inner.read().await;
        Ok(inner.streams.get(stream_id).cloned().unwrap_or_default())
    }

    async fn load_from(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> Result<Vec<EventEnvelope>, EngineError> {
        let inner = self.inner.read().await;
        Ok(inner
            .streams
            .get(stream_id)
            .map(|events| {
                events
                    .iter()
                    .filter(|e| e.sequence_number > from_sequence)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    /// O(1) version check — reads the stream length from the HashMap without
    /// cloning any event payloads.
    async fn stream_version(&self, stream_id: &StreamId) -> Result<u64, EngineError> {
        let inner = self.inner.read().await;
        Ok(inner.streams.get(stream_id).map_or(0, |s| s.len() as u64))
    }

    /// Returns all known stream identifiers, optionally filtered by `prefix`.
    ///
    /// O(n) in the number of streams — scans the HashMap keys once.
    async fn list_streams(&self, prefix: Option<&str>) -> Result<Vec<StreamId>, EngineError> {
        let inner = self.inner.read().await;
        let ids = inner
            .streams
            .keys()
            .filter(|id| prefix.is_none_or(|p| id.as_str().starts_with(p)))
            .cloned()
            .collect();
        Ok(ids)
    }

    /// Paginated stream enumeration for `InMemoryEventStore`.
    ///
    /// Collects all matching keys, sorts them for deterministic order, then
    /// applies cursor + limit slicing.  O(n) — acceptable for test/dev stores.
    async fn list_streams_page(
        &self,
        prefix: Option<&str>,
        cursor: Option<&StreamId>,
        limit: usize,
    ) -> Result<Vec<StreamId>, EngineError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let inner = self.inner.read().await;
        let mut ids: Vec<StreamId> = inner
            .streams
            .keys()
            .filter(|id| prefix.is_none_or(|p| id.as_str().starts_with(p)))
            .cloned()
            .collect();
        // Sort for deterministic pagination order (HashMap is unordered).
        ids.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
        let iter: Box<dyn Iterator<Item = StreamId>> = match cursor {
            None => Box::new(ids.into_iter()),
            Some(c) => Box::new(ids.into_iter().skip_while(move |id| id != c).skip(1)),
        };
        Ok(iter.take(limit).collect())
    }

    /// Fold over events in `stream_id` starting after `from_sequence`
    /// (exclusive) without materialising the full `Vec<EventEnvelope>`.
    ///
    /// In-memory implementation: iterates the in-memory Vec slice.  This is
    /// O(N) memory but that is acceptable for the in-memory test store
    /// (production code uses `SlateDbStore::fold_stream` which is cursor-based).
    async fn fold_stream<T, F>(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
        initial: T,
        mut f: F,
    ) -> Result<T, EngineError>
    where
        T: Send,
        F: FnMut(T, EventEnvelope) -> Result<T, EngineError> + Send,
    {
        let inner = self.inner.read().await;
        let mut acc = initial;
        if let Some(events) = inner.streams.get(stream_id) {
            for env in events.iter().filter(|e| e.sequence_number > from_sequence) {
                acc = f(acc, env.clone())?;
            }
        }
        Ok(acc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ConversationId, CorrelationId, ProcessId, TenantId};
    use crate::version::WorkflowId;

    fn make_new_event() -> NewEvent {
        NewEvent {
            correlation_id: CorrelationId::new(),
            causation_id: None,
            conversation_id: ConversationId::new(),
            process_id: ProcessId::new(),
            tenant_id: TenantId::new(),
            workflow_id: WorkflowId::new("test", "FV2024-10-01"),
            event_type: "TestEvent".into(),
            schema_version: 1,
            payload: serde_json::json!({"test": true}),
        }
    }

    #[tokio::test]
    async fn append_and_load_roundtrip() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::new("test/s1");

        let result = store
            .append(AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &[make_new_event(), make_new_event()],
            ))
            .await
            .unwrap();

        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[0].sequence_number, 1);
        assert_eq!(result.events[1].sequence_number, 2);
        assert_eq!(result.last_sequence, 2);

        let loaded = store.load(&stream).await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn store_stamps_stream_id_and_sequence() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::new("test/stamp");

        let result = store
            .append(AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &[make_new_event()],
            ))
            .await
            .unwrap();

        let env = &result.events[0];
        assert_eq!(env.stream_id, stream);
        assert_eq!(env.sequence_number, 1);
    }

    #[tokio::test]
    async fn version_conflict_is_detected() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::new("test/s2");

        store
            .append(AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &[make_new_event()],
            ))
            .await
            .unwrap();

        let err = store
            .append(AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &[make_new_event()],
            ))
            .await
            .unwrap_err();

        assert!(matches!(err, EngineError::VersionConflict { .. }));
    }

    #[tokio::test]
    async fn load_from_returns_tail_only() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::new("test/s3");
        let events: Vec<_> = (0..5).map(|_| make_new_event()).collect();

        store
            .append(AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &events,
            ))
            .await
            .unwrap();

        let tail = store.load_from(&stream, 3).await.unwrap();
        assert_eq!(tail.len(), 2, "expected events 4 and 5");
        assert_eq!(tail[0].sequence_number, 4);
        assert_eq!(tail[1].sequence_number, 5);
    }

    #[tokio::test]
    async fn all_events_preserves_insertion_order_across_streams() {
        let store = InMemoryEventStore::new();
        let s1 = StreamId::new("test/order-s1");
        let s2 = StreamId::new("test/order-s2");

        store
            .append(AppendBatch::new(
                &s1,
                ExpectedVersion::NoStream,
                &[make_new_event()],
            ))
            .await
            .unwrap();
        store
            .append(AppendBatch::new(
                &s2,
                ExpectedVersion::NoStream,
                &[make_new_event()],
            ))
            .await
            .unwrap();
        store
            .append(AppendBatch::new(
                &s1,
                ExpectedVersion::Exact(1),
                &[make_new_event()],
            ))
            .await
            .unwrap();

        let all = store.all_events().await;
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].stream_id, s1);
        assert_eq!(all[1].stream_id, s2);
        assert_eq!(all[2].stream_id, s1);
    }

    #[tokio::test]
    async fn arc_wrapper_delegates_correctly() {
        let store = Arc::new(InMemoryEventStore::new());
        let stream = StreamId::new("test/arc-s1");

        store
            .append(AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &[make_new_event()],
            ))
            .await
            .unwrap();

        let loaded = store.load(&stream).await.unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[tokio::test]
    async fn fold_stream_accumulates_without_full_vec() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::new("test/fold-s1");
        let events: Vec<_> = (0..4).map(|_| make_new_event()).collect();

        store
            .append(AppendBatch::new(
                &stream,
                ExpectedVersion::NoStream,
                &events,
            ))
            .await
            .unwrap();

        // Fold from the beginning: count events.
        let count = store
            .fold_stream(&stream, 0, 0usize, |acc, _| Ok(acc + 1))
            .await
            .unwrap();
        assert_eq!(count, 4);

        // Fold from sequence 2: count only tail events.
        let tail_count = store
            .fold_stream(&stream, 2, 0usize, |acc, _| Ok(acc + 1))
            .await
            .unwrap();
        assert_eq!(tail_count, 2);
    }
}
