//! SlateDB-backed stores for the Mako engine.
//!
//! Enabled by the `slatedb` feature flag. A single [`slatedb::Db`] handle
//! underpins all store types — they share the same database file/object-store
//! prefix while keeping logically separate key spaces.
//!
//! # Key schema
//!
//! ```text
//! e/{stream_id}/{seq:016x}        →  JSON(EventEnvelope)   [event payloads]
//! sv/{stream_id}                  →  u64 LE                 [stream version cache]
//! si/{stream_id}                  →  ""                     [stream existence index]
//! sn/{stream_id}                  →  JSON(Snapshot)         [aggregate snapshots]
//! om/{message_id}                 →  JSON(OutboxMessage)    [outbox payloads]
//! ot/{ts_nanos:016x}/{id}         →  ""                     [outbox time index]
//! dl/{deadline_id}                →  JSON(Deadline)         [deadline payloads]
//! dt/{due_nanos:016x}/{id}        →  ""                     [deadline time index]
//! pr/{tenant_id}/{routing_key}    →  JSON(ProcessIdentity)  [process routing 1:1]
//! ci/{tenant_id}/{tag}/{process_id} → JSON(ProcessIdentity) [correlated 1:many index]
//! pt/{tenant_id}/{gln}            →  JSON(PartnerRecord)    [trading-partner master data]
//! ib/{inbox_key}                  →  ""                     [inbox dedup sentinel]
//! it/{ts_nanos:016x}/{nonce_uuid} →  "{inbox_key}"          [inbox time index for TTL purge]
//! dr/{ts_nanos:016x}/{uuid}       →  JSON(DeadLetterRecord) [durable dead-letter queue]
//! cp/{projection_name}/{stream_id} →  u64 LE                         [projection cursor per stream]
//! ```
//!
//! # Atomicity
//!
//! [`SlateDbStore::append`], [`SlateDbStore::append_with_outbox`], and all
//! counter-maintaining outbox/deadline/registry writes use
//! **Serializable Snapshot Isolation (SSI)** transactions
//! (`IsolationLevel::SerializableSnapshot`). SSI is pinned explicitly — never
//! `IsolationLevel::default()` — so an upstream SlateDB change to its default
//! cannot silently degrade write isolation.
//!
//! SSI detects write-write conflicts on shared keys (e.g. `sv/{stream_id}`,
//! `_count/om`, `_count/dl`, `_count/pr`). When two concurrent writers race,
//! the second commit is rejected with `ErrorKind::Transaction`, mapped to
//! [`EngineError::VersionConflict`]. This eliminates the TOCTOU window that
//! existed with the previous `get → check → WriteBatch::write` pattern.
//!
//! # Opening
//!
//! ```rust,no_run
//! # async fn example() -> Result<(), mako_engine::error::EngineError> {
//! use mako_engine::store_slatedb::SlateDbStore;
//!
//! // Volatile in-memory store (tests / CI):
//! let store = SlateDbStore::open_in_memory().await?;
//!
//! // Persistent local filesystem:
//! let store = SlateDbStore::open_local("/var/lib/makod/events").await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slatedb::object_store::memory::InMemory;
use slatedb::{Db, ErrorKind, IsolationLevel, WriteBatch};
use time::OffsetDateTime;

use crate::{
    deadline::{Deadline, DeadlineStore, DueNowResult},
    envelope::{EventEnvelope, NewEvent},
    error::EngineError,
    event_store::{AppendResult, AtomicAppend, EventStore, ExpectedVersion},
    ids::{DeadlineId, OutboxMessageId, ProcessIdentity, StreamId, TenantId},
    inbox::InboxStore,
    outbox::{OutboxMessage, OutboxStore, PendingOutbox},
    registry::{ProcessRegistry, RegistryKey},
    snapshot::{Snapshot, SnapshotStore},
};

// ── Key helpers ───────────────────────────────────────────────────────────────

fn event_key(stream_id: &StreamId, seq: u64) -> String {
    format!("e/{stream_id}/{seq:016x}")
}

fn sv_key(stream_id: &StreamId) -> String {
    format!("sv/{stream_id}")
}

fn si_key(stream_id: &StreamId) -> String {
    format!("si/{stream_id}")
}

/// Snapshot key: `sn/{stream_id}` → `JSON(Snapshot)`.
///
/// One entry per stream — always overwritten by the most recent snapshot.
fn sn_key(stream_id: &StreamId) -> String {
    format!("sn/{stream_id}")
}

/// Checkpoint cursor key: `cp/{name}/{stream_id}` → u64 LE (8 bytes).
///
/// One entry per (projection, stream) pair. Stored as raw little-endian u64
/// rather than JSON so per-stream reads/writes are O(1) and the entire
/// checkpoint for `n` streams requires only `n × (key_len + 8)` bytes, with
/// no JSON serialisation overhead.
fn cp_cursor_key(name: &str, stream_id: &StreamId) -> String {
    format!("cp/{name}/{stream_id}")
}

/// Exclusive upper bound for all cursor keys belonging to projection `name`.
///
/// Key prefix is `cp/{name}/`; `'/'` is `0x2F` and `'0'` is `0x30`, so
/// `cp/{name}0` sorts strictly after every `cp/{name}/…` key — giving a
/// precise half-open range `[cp/{name}/, cp/{name}0)` for a prefix scan.
fn cp_prefix_end(name: &str) -> String {
    format!("cp/{name}0")
}

fn om_key(id: &OutboxMessageId) -> String {
    format!("om/{id}")
}

/// Counter key for the number of pending outbox messages.
///
/// Stored as 8-byte little-endian u64. Updated atomically in every
/// [`OutboxStore::enqueue`] and [`OutboxStore::acknowledge`] call.
const OM_COUNT_KEY: &[u8] = b"_count/om";

/// Read the `_count/om` counter within an in-progress transaction.
async fn read_om_count_txn(txn: &slatedb::DbTransaction) -> Result<u64, EngineError> {
    match txn.get(OM_COUNT_KEY).await.map_err(to_outbox_err)? {
        None => Ok(0), // counter not yet written — store is empty, expected on first enqueue
        Some(bytes) if bytes.len() == 8 => Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
        Some(bytes) => {
            // Wrong byte length: the counter key exists but is not a valid u64.
            // This indicates a corrupt write or schema mismatch. Return an
            // explicit error rather than silently zeroing the count, which would
            // cause the OutboxWorker to stop seeing pending messages.
            Err(EngineError::store(format!(
                "_count/om corrupt: expected 8-byte little-endian u64, got {} bytes",
                bytes.len()
            )))
        }
    }
}

/// Read the `_count/om` counter directly from the store (no transaction).
async fn read_om_count(db: &Db) -> Result<u64, EngineError> {
    match db.get(OM_COUNT_KEY).await.map_err(to_outbox_err)? {
        None => Ok(0), // counter not yet written — store is empty
        Some(bytes) if bytes.len() == 8 => Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
        Some(bytes) => {
            // Wrong byte length: counter is corrupt. Return an error rather
            // than silently returning 0, which would stop the OutboxWorker from
            // delivering pending messages and cause silent message loss.
            Err(EngineError::store(format!(
                "_count/om corrupt: expected 8-byte little-endian u64, got {} bytes",
                bytes.len()
            )))
        }
    }
}

/// Time-index key: `ot/{unix_nanos_u64:016x}/{message_id}`.
///
/// Lexicographic order equals chronological order because the timestamp is
/// zero-padded fixed-width hex. Post-epoch timestamps fit safely in u64
/// until year ~2554. Pre-epoch timestamps are clamped to zero (epoch)
/// rather than wrapping, which would otherwise place them near `u64::MAX`
/// and make them permanently invisible to the `pending` scanner.
fn ot_key(ts: OffsetDateTime, id: &OutboxMessageId) -> String {
    // i128 → u64: clamp pre-epoch (negative) values to 0 instead of wrapping.
    let nanos = u64::try_from(ts.unix_timestamp_nanos().max(0)).unwrap_or(0);
    format!("ot/{nanos:016x}/{id}")
}

/// Exclusive upper bound for all event keys belonging to `stream_id`.
///
/// The event key prefix is `e/{stream_id}/`. ASCII `/` is `0x2F` and `0` is
/// `0x30`, so `e/{stream_id}0` sorts strictly after every `e/{stream_id}/…`
/// key, giving a precise half-open range `[start, end)` for a stream scan.
fn event_prefix_end(stream_id: &StreamId) -> String {
    format!("e/{stream_id}0")
}

// ── Deadline key helpers ──────────────────────────────────────────────────────

/// Deadline payload key: `dl/{deadline_id}` → `JSON(Deadline)`.
fn dl_key(id: &DeadlineId) -> String {
    format!("dl/{id}")
}

/// Deadline time-index key: `dt/{due_nanos:016x}/{deadline_id}`.
///
/// Lexicographic order equals chronological order. Pre-epoch timestamps are
/// clamped to zero, consistent with the outbox `ot_key` convention.
fn dt_key(due_at: OffsetDateTime, id: &DeadlineId) -> String {
    let nanos = u64::try_from(due_at.unix_timestamp_nanos().max(0)).unwrap_or(0);
    format!("dt/{nanos:016x}/{id}")
}

/// Deadline stream-reverse-index key: `ds/{stream_id}/{deadline_id}` → `""`.
///
/// Enables `for_stream` O(k) lookups (k = deadlines on that stream) instead
/// of an O(n) full-scan over all `dl/` keys.
fn ds_key(stream_id: &StreamId, id: &DeadlineId) -> String {
    format!("ds/{}/{id}", stream_id.as_str())
}

/// Prefix for the stream-reverse index of a specific stream.
fn ds_stream_prefix(stream_id: &StreamId) -> String {
    format!("ds/{}/", stream_id.as_str())
}

// ── Process registry key helpers ──────────────────────────────────────────────

/// Process routing key: `pr/{tenant_id}/{routing_key}` → `JSON(ProcessIdentity)`.
///
/// `TenantId` is always a 36-char UUID, so `pr/{36-chars}/` is an
/// unambiguous fixed-length prefix. The routing key may contain `/` safely
/// because exact get/delete operations do not need to parse it back.
fn pr_key(tenant_id: TenantId, key: &RegistryKey) -> String {
    format!("pr/{tenant_id}/{}", key.as_str())
}

/// Prefix for all routing entries belonging to `tenant_id`.
#[cfg(test)]
#[allow(dead_code)]
fn pr_tenant_prefix(tenant_id: TenantId) -> String {
    format!("pr/{tenant_id}/")
}

// ── Correlated-process index key helpers ───────────────────────────────────────

/// Correlated index key: `ci/{tenant_id}/{tag}/{process_id}` → `JSON(ProcessIdentity)`.
///
/// `TenantId` and `ProcessId` are both 36-char UUIDs, giving fixed-width prefix
/// segments. `tag` may contain any bytes that are valid UTF-8 except `\0` or `/`.
/// Multiple processes can be registered under the same `(tenant_id, tag)` pair.
fn ci_key(tenant_id: TenantId, tag: &str, process_id: crate::ids::ProcessId) -> String {
    format!("ci/{tenant_id}/{tag}/{process_id}")
}

/// Prefix for all correlated entries under `(tenant_id, tag)`.
///
/// Used for `scan_prefix` in `lookup_correlated`.
fn ci_tag_prefix(tenant_id: TenantId, tag: &str) -> String {
    format!("ci/{tenant_id}/{tag}/")
}

/// Validate a correlated-index tag before use in a key.
///
/// Rejects tags that would corrupt the `ci/{tenant}/{tag}/{process}` key
/// structure and produce cross-tag scan boundary leakage:
///
/// - Tags containing `\0` can corrupt key scan boundaries on byte-string
///   backends and are forbidden by the `RegistryKey` contract.
/// - Empty tags would collapse the tag segment and make lookups ambiguous.
/// - Tags containing `/` create a sub-prefix that leaks into or out of sibling
///   tags when scanning by prefix.
/// - Tags exceeding 128 bytes offer no additional expressiveness and could
///   create abnormally long keys.
fn validate_ci_tag(tag: &str) -> Result<(), EngineError> {
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

// ── Inbox key helpers ─────────────────────────────────────────────────────────

/// Inbox dedup sentinel key: `ib/{inbox_key}` → `""`.
fn ib_key(key: &str) -> String {
    format!("ib/{key}")
}

/// Inbox time-index key: `it/{ts_nanos:016x}/{nonce}` → `"{inbox_key}"`.
///
/// The nonce (a UUID string) prevents collisions when two different keys
/// arrive at the same nanosecond. The original inbox key is stored in the
/// value so the time-index scan can reconstruct the `ib/` key for deletion
/// without parsing the path component.
fn it_key(ts: OffsetDateTime, nonce: &str) -> String {
    let nanos = u64::try_from(ts.unix_timestamp_nanos().max(0)).unwrap_or(0);
    format!("it/{nanos:016x}/{nonce}")
}

// ── Error helpers ─────────────────────────────────────────────────────────────

/// Classify a `slatedb::Error` into a short, opaque description that is safe
/// to surface to callers.
///
/// `slatedb::Error::to_string()` can embed internal key bytes, object-store
/// paths, and sequence numbers. We strip those and keep only the `ErrorKind`
/// label, which is the information operators need for alerting / metrics
/// without leaking storage internals.
///
/// Detailed information is still available via structured logging (the caller
/// logs the full `slatedb::Error` at `tracing::error!` level before calling
/// these helpers, so no diagnostic value is lost).
fn slatedb_error_kind_str(e: &slatedb::Error) -> &'static str {
    use slatedb::ErrorKind;
    match e.kind() {
        ErrorKind::Transaction => "transaction conflict",
        ErrorKind::Closed(_) => "database closed",
        ErrorKind::Unavailable => "storage unavailable",
        ErrorKind::Invalid => "invalid storage operation",
        ErrorKind::Data => "data integrity error",
        ErrorKind::Internal => "internal storage error",
        // `#[non_exhaustive]` guard — new variants in future SlateDB releases
        // map to the same safe sentinel rather than a compile error.
        _ => "storage error",
    }
}

/// Classify a `slatedb::ErrorKind` as transient (safe to retry) or permanent.
///
/// | `ErrorKind`   | Transient? | Rationale |
/// |---|---|---|
/// | `Unavailable` | **yes** | Momentary S3 / GCS 503; will self-resolve |
/// | `Closed(_)`   | **yes** | Database closing during graceful shutdown |
/// | `Transaction` | no | Optimistic conflict → caller uses retry with reload |
/// | `Data`        | no | Checksum / corruption — retrying only makes it worse |
/// | `Invalid`     | no | API misuse — permanent until code is fixed |
/// | `Internal`    | no | SlateDB bug — treat as permanent, alert operator |
///
/// This function is the single authoritative source for retryability decisions.
/// [`EngineError::is_transient`] delegates here via the string labels produced
/// by [`slatedb_error_kind_str`]. Keeping both in sync is enforced by
/// [`tests::slatedb_error_transient_classification_consistent`].
///
/// [`EngineError::is_transient`]: crate::error::EngineError::is_transient
pub(crate) fn slatedb_error_is_transient(e: &slatedb::Error) -> bool {
    use slatedb::ErrorKind;
    matches!(e.kind(), ErrorKind::Unavailable | ErrorKind::Closed(_))
}

#[allow(clippy::needless_pass_by_value)]
fn to_store_err(e: slatedb::Error) -> EngineError {
    tracing::error!(error = %e, "event store error");
    if slatedb_error_is_transient(&e) {
        EngineError::transient_store(slatedb_error_kind_str(&e))
    } else {
        EngineError::store(slatedb_error_kind_str(&e))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn to_outbox_err(e: slatedb::Error) -> EngineError {
    tracing::error!(error = %e, "outbox store error");
    if slatedb_error_is_transient(&e) {
        EngineError::transient_outbox(slatedb_error_kind_str(&e))
    } else {
        EngineError::outbox(slatedb_error_kind_str(&e))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn to_deadline_err(e: slatedb::Error) -> EngineError {
    tracing::error!(error = %e, "deadline store error");
    if slatedb_error_is_transient(&e) {
        EngineError::transient_deadline(slatedb_error_kind_str(&e))
    } else {
        EngineError::deadline(slatedb_error_kind_str(&e))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn to_registry_err(e: slatedb::Error) -> EngineError {
    tracing::error!(error = %e, "process registry error");
    if slatedb_error_is_transient(&e) {
        EngineError::transient_registry(slatedb_error_kind_str(&e))
    } else {
        EngineError::registry(slatedb_error_kind_str(&e))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn to_inbox_err(e: slatedb::Error) -> EngineError {
    tracing::error!(error = %e, "inbox store error");
    if slatedb_error_is_transient(&e) {
        EngineError::transient_inbox(slatedb_error_kind_str(&e))
    } else {
        EngineError::inbox(slatedb_error_kind_str(&e))
    }
}

// ── SlateDbStore ──────────────────────────────────────────────────────────────

/// Durable [`EventStore`] + [`OutboxStore`] backed by SlateDB.
///
/// `SlateDbStore` is [`Clone`] and `Send + Sync`; share it freely across
/// tasks. The underlying [`Db`] handle reference-counts the database
/// internally.
///
/// ## Close semantics
///
/// The `closed` flag ensures that `close()` is a no-op (with a warning) after
/// the first successful call, regardless of how many `Arc`-backed clones exist.
/// This prevents ambiguous write-after-close errors that are hard to diagnose
/// when multiple store facades (event, outbox, deadline) share the same `Db`.
#[derive(Clone)]
pub struct SlateDbStore {
    db: Db,
    closed: Arc<AtomicBool>,
    /// Maximum number of events allowed per stream.
    ///
    /// `None` (the default) means unlimited.  When set, any `append` or
    /// `append_with_outbox` call that would push a stream past this limit
    /// returns [`EngineError::StreamQuotaExceeded`] before opening a
    /// transaction, acting as a hard guard against unbounded stream growth.
    max_stream_events: Option<u64>,
}

impl SlateDbStore {
    /// Open the store with an explicit `ObjectStore` backend.
    ///
    /// Use this constructor for cloud-native deployments (S3, GCS, Azure Blob,
    /// local filesystem via `object_store::local::LocalFileSystem`, …).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] if the database cannot be opened.
    pub async fn open(
        path: impl Into<slatedb::object_store::path::Path>,
        object_store: Arc<dyn slatedb::object_store::ObjectStore>,
    ) -> Result<Self, EngineError> {
        let db = Db::open(path, object_store).await.map_err(to_store_err)?;
        Ok(Self {
            db,
            closed: Arc::new(AtomicBool::new(false)),
            max_stream_events: None,
        })
    }

    /// Open a volatile in-memory store.
    ///
    /// All data is lost when the store is dropped. Suitable for tests and
    /// local development without requiring any infrastructure.
    ///
    /// Each call creates a **fully independent** store even when using the
    /// same label; the label is advisory only and does not affect isolation.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] if the database cannot be initialised.
    pub async fn open_in_memory() -> Result<Self, EngineError> {
        Self::open_in_memory_with_label("mako-engine-test").await
    }

    /// Open a volatile in-memory store with an explicit label for log
    /// correlation.
    ///
    /// The label appears in SlateDB log output and helps distinguish
    /// concurrent stores in tests or development environments.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] if the database cannot be initialised.
    pub async fn open_in_memory_with_label(label: &str) -> Result<Self, EngineError> {
        let object_store = Arc::new(InMemory::new());
        Self::open(label, object_store).await
    }

    /// Open a persistent store on the local filesystem at `dir`.
    ///
    /// `dir` is created if it does not exist. Suitable for single-node
    /// deployments and integration tests that need persistence across restarts.
    ///
    /// The path is canonicalised after creation so that `..` components and
    /// symlinks are resolved before being handed to the object store. This
    /// prevents path-traversal attacks when `dir` originates from an
    /// operator-supplied config file or environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] if `dir` cannot be created or opened.
    pub async fn open_local(dir: impl AsRef<std::path::Path>) -> Result<Self, EngineError> {
        let dir = dir.as_ref();
        // Create the directory first so canonicalize can resolve the final path.
        std::fs::create_dir_all(dir)
            .map_err(|e| EngineError::store(format!("cannot create data-dir: {e}")))?;
        // Resolve symlinks and `..` components — rejects non-existent or
        // inaccessible paths with a clear error rather than an opaque
        // object_store failure later.
        let canonical = dir
            .canonicalize()
            .map_err(|e| EngineError::store(format!("invalid data-dir: {e}")))?;
        // object_store::local::LocalFileSystem is re-exported by slatedb.
        let local = slatedb::object_store::local::LocalFileSystem::new_with_prefix(&canonical)
            .map_err(|e| EngineError::store(e.to_string()))?;
        Self::open("db", Arc::new(local)).await
    }

    /// Set a per-stream event count limit for this store.
    ///
    /// Any [`EventStore::append`] or [`AtomicAppend::append_with_outbox`] call
    /// that would push a stream beyond `max_events` returns
    /// [`EngineError::StreamQuotaExceeded`].
    ///
    /// This is a defence-in-depth guard against runaway streams exhausting
    /// object storage or causing unbounded replay times. The default (no limit)
    /// is appropriate for production deployments where streams are bounded by
    /// their natural workflow lifecycle.
    #[must_use]
    pub fn with_max_stream_events(mut self, max_events: u64) -> Self {
        self.max_stream_events = Some(max_events);
        self
    }

    /// Check whether appending `new_events` events to `stream_id` would
    /// exceed the configured [`Self::max_stream_events`] limit.
    fn check_quota(
        &self,
        stream_id: &StreamId,
        current: u64,
        new_events: usize,
    ) -> Result<(), EngineError> {
        if let Some(limit) = self.max_stream_events {
            let actual = current + new_events as u64;
            if actual > limit {
                // Log full detail internally; the error returned to callers
                // omits stream_id to avoid leaking stream topology.
                tracing::error!(
                    stream_id = %stream_id,
                    limit,
                    new_events,
                    actual,
                    "stream quota exceeded"
                );
                return Err(EngineError::StreamQuotaExceeded {
                    stream_id: stream_id.clone(),
                    limit,
                    new_events,
                    actual,
                });
            }
        }
        Ok(())
    }

    /// Flush all in-memory writes to the object store and close the database.
    ///
    /// Idempotent: subsequent calls on any clone of this store are no-ops
    /// (the closed flag is set on the first successful close, and later calls
    /// log a warning and return `Ok(())`). This prevents ambiguous
    /// write-after-close errors when multiple store facades (event, outbox,
    /// deadline) share the same underlying `Db` and each tries to close.
    ///
    /// **Prefer [`close_owned`] whenever possible.** Consuming `self` makes
    /// it a compile-time error to use the store after closing *this* value and
    /// documents at the call site that you hold the last outstanding clone.
    /// Calling `close(&self)` on a shared reference is only appropriate when
    /// the caller genuinely cannot give up ownership (e.g., inside a `Drop`
    /// impl or a shared Arc). In all other production paths, use [`close_owned`].
    ///
    /// # Clone semantics
    ///
    /// [`SlateDbStore`] is [`Clone`]; all clones share the same underlying
    /// [`slatedb::Db`] handle and the same `closed` flag via an `Arc`. Calling
    /// `close()` on **any** clone shuts down the shared database: all other
    /// clones immediately start returning `EngineError::Store` on their next
    /// operation.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] if the close fails.
    ///
    /// [`close_owned`]: SlateDbStore::close_owned
    pub async fn close(&self) -> Result<(), EngineError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            tracing::warn!("SlateDbStore::close called more than once — this is a no-op");
            return Ok(());
        }
        self.db.close().await.map_err(to_store_err)
    }

    /// Consuming close: flush and close the database, preventing double-close
    /// for this specific value at compile time.
    ///
    /// Behaves identically to [`close`] but consumes `self`, ensuring that
    /// this particular `SlateDbStore` value cannot be used after the call.
    /// Other [`Clone`]s of this store will receive errors after this call.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] if the close fails.
    ///
    /// [`close`]: SlateDbStore::close
    pub async fn close_owned(self) -> Result<(), EngineError> {
        self.close().await
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    async fn read_version(&self, stream_id: &StreamId) -> Result<u64, EngineError> {
        match self
            .db
            .get(sv_key(stream_id).as_bytes())
            .await
            .map_err(to_store_err)?
        {
            Some(bytes) if bytes.len() >= 8 => {
                Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
            }
            _ => Ok(0),
        }
    }

    // ── Companion store constructors ──────────────────────────────────────────

    /// Return a [`SlateDbDeadlineStore`] that shares the underlying database.
    ///
    /// All stores created from the same `SlateDbStore` share one SlateDB
    /// instance — they flush together and close together.
    #[must_use]
    pub fn as_deadline_store(&self) -> SlateDbDeadlineStore {
        SlateDbDeadlineStore {
            db: self.db.clone(),
        }
    }

    /// Return a [`SlateDbProcessRegistry`] that shares the underlying database.
    #[must_use]
    pub fn as_process_registry(&self) -> SlateDbProcessRegistry {
        SlateDbProcessRegistry {
            db: self.db.clone(),
        }
    }

    /// Return a [`SlateDbSnapshotStore`] that shares the underlying database.
    ///
    /// Wire this into the engine to enable snapshot-accelerated state
    /// reconstruction, bounding replay cost to at most `snapshot_interval`
    /// tail events per command dispatch.
    #[must_use]
    pub fn as_snapshot_store(&self) -> SlateDbSnapshotStore {
        SlateDbSnapshotStore {
            db: self.db.clone(),
        }
    }

    /// Return a [`SlateDbInboxStore`] that shares the underlying database.
    #[must_use]
    pub fn as_inbox_store(&self) -> SlateDbInboxStore {
        SlateDbInboxStore {
            db: self.db.clone(),
        }
    }

    // ── Low-level key-value access for service-layer caches ───────────────────
    //
    // These methods allow service-binary code (e.g. the malo cache in `makod`)
    // to store auxiliary data in the same SlateDB instance without depending
    // on SlateDB types directly.
    //
    // The namespace prefix is enforced structurally via [`KvNamespace`] so
    // callers cannot accidentally escape into the engine's reserved key space
    // (`e/`, `sv/`, `si/`, `om/`, `ot/`, `dl/`, `dt/`, `pr/`, `ib/`, `it/`,
    // `dr/`, `cp/`).

    /// Read a raw byte blob by `(namespace, suffix)`. Returns `Ok(None)` when the key is absent.
    ///
    /// The full storage key is `ns.as_str() + suffix`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    pub async fn kv_get(
        &self,
        ns: KvNamespace,
        suffix: &str,
    ) -> Result<Option<Vec<u8>>, EngineError> {
        let key = ns.key(suffix);
        self.db
            .get(key.as_bytes())
            .await
            .map_err(to_store_err)
            .map(|opt| opt.map(|b| b.to_vec()))
    }

    /// Write a raw byte blob under `(namespace, suffix)`.
    ///
    /// The full storage key is `ns.as_str() + suffix`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    pub async fn kv_put(
        &self,
        ns: KvNamespace,
        suffix: &str,
        value: &[u8],
    ) -> Result<(), EngineError> {
        let key = ns.key(suffix);
        let mut batch = WriteBatch::new();
        batch.put(key.as_bytes(), value);
        self.db.write(batch).await.map_err(to_store_err).map(|_| ())
    }

    /// Delete a raw byte blob. A no-op when the key is absent.
    ///
    /// The full storage key is `ns.as_str() + suffix`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    pub async fn kv_delete(&self, ns: KvNamespace, suffix: &str) -> Result<(), EngineError> {
        let key = ns.key(suffix);
        let mut batch = WriteBatch::new();
        batch.delete(key.as_bytes());
        self.db.write(batch).await.map_err(to_store_err).map(|_| ())
    }

    /// Scan all keys within `ns` and return `(suffix, value)` pairs.
    ///
    /// Returns keys **without** the namespace prefix (i.e. just the suffix
    /// component) in lexicographic order. Callers should define suffix schemas
    /// that end with `/` delimiters for hierarchical scans.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure or on invalid UTF-8
    /// in a key.
    pub async fn kv_scan_prefix(
        &self,
        ns: KvNamespace,
    ) -> Result<Vec<(String, Vec<u8>)>, EngineError> {
        let prefix = ns.as_str();
        let mut iter = self
            .db
            .scan_prefix(prefix.as_bytes())
            .await
            .map_err(to_store_err)?;
        let mut results = Vec::new();
        while let Some(kv) = iter.next().await.map_err(to_store_err)? {
            let full_key =
                std::str::from_utf8(&kv.key).map_err(|e| EngineError::store(e.to_string()))?;
            // Strip the namespace prefix so callers receive the suffix only.
            let suffix = full_key.strip_prefix(prefix).unwrap_or(full_key).to_owned();
            results.push((suffix, kv.value.to_vec()));
        }
        Ok(results)
    }
}

// ── KvNamespace ───────────────────────────────────────────────────────────────

/// A type-safe namespace prefix for raw KV operations on [`SlateDbStore`].
///
/// Prevents accidental writes to the engine's reserved key prefixes
/// (`e/`, `sv/`, `si/`, `om/`, `ot/`, `dl/`, `dt/`, `pr/`, `ib/`, `it/`,
/// `dr/`, `cp/`) by requiring callers to declare their prefix as a typed
/// constant rather than an inline `&str`.
///
/// ## Conventions
///
/// - Always end the prefix with `/` to keep namespaces unambiguous.
/// - Define namespaces as `const` in the module that owns the data.
/// - Never define a namespace that starts with an engine-reserved prefix.
///
/// ## Example
///
/// ```rust,no_run
/// use mako_engine::store_slatedb::KvNamespace;
///
/// const MY_NS: KvNamespace = KvNamespace::new("my_cache/");
///
/// # async fn example(store: mako_engine::store_slatedb::SlateDbStore) -> Result<(), mako_engine::error::EngineError> {
/// store.kv_put(MY_NS, "key1", b"hello").await?;
/// let val = store.kv_get(MY_NS, "key1").await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KvNamespace(&'static str);

impl KvNamespace {
    /// Create a new namespace constant.
    ///
    /// The prefix **must** be non-empty and end with `/`. This invariant is
    /// enforced by `assert!` — because all callers are `const` expressions,
    /// the check is evaluated at compile time and never incurs runtime cost.
    /// A missing trailing `/` causes cross-namespace key collisions:
    /// `kv_get(ns, "key")` produces `"my_cachex"` instead of `"my_cache/x"`.
    #[must_use]
    pub const fn new(prefix: &'static str) -> Self {
        assert!(
            !prefix.is_empty() && matches!(prefix.as_bytes().last(), Some(&b'/')),
            "KvNamespace prefix must be non-empty and end with '/'"
        );
        Self(prefix)
    }

    /// Return the raw prefix string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// Build the full storage key: `prefix + suffix`.
    #[must_use]
    pub fn key(self, suffix: &str) -> String {
        format!("{}{}", self.0, suffix)
    }
}

// ── Free helpers shared by append and append_with_outbox ─────────────────────

fn check_version(expected: ExpectedVersion, actual: u64) -> Result<(), EngineError> {
    match expected {
        ExpectedVersion::NoStream if actual != 0 => Err(EngineError::VersionConflict {
            expected: 0,
            actual,
        }),
        ExpectedVersion::Exact(v) if actual != v => Err(EngineError::VersionConflict {
            expected: v,
            actual,
        }),
        _ => Ok(()),
    }
}

/// Read the stream version within an in-progress transaction.
///
/// The `get` call establishes a read dependency on `sv/{stream_id}` inside
/// the transaction's snapshot. With `Snapshot` isolation, this is enough to
/// ensure that two concurrent writers detect their write-write conflict on the
/// version key at commit time.
async fn read_version_in_txn(
    txn: &slatedb::DbTransaction,
    stream_id: &StreamId,
) -> Result<u64, EngineError> {
    match txn
        .get(sv_key(stream_id).as_bytes())
        .await
        .map_err(to_store_err)?
    {
        Some(bytes) if bytes.len() >= 8 => Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
        _ => Ok(0),
    }
}

/// Stamp `events` with store-assigned fields and buffer them into a transaction.
///
/// Also updates the stream-version cache (`sv/…`) and the stream-existence
/// index (`si/…`) in the same transaction buffer.
fn stamp_events_txn(
    txn: &slatedb::DbTransaction,
    stream_id: &StreamId,
    current_version: u64,
    events: &[NewEvent],
    now: OffsetDateTime,
) -> Result<Vec<EventEnvelope>, EngineError> {
    let mut envelopes = Vec::with_capacity(events.len());
    for (i, new_event) in events.iter().enumerate() {
        let seq = current_version + 1 + i as u64;
        let envelope = EventEnvelope::from_new(new_event.clone(), stream_id.clone(), seq, now);
        let value = serde_json::to_vec(&envelope).map_err(|e| EngineError::store(e.to_string()))?;
        txn.put(event_key(stream_id, seq).as_bytes(), value.as_slice())
            .map_err(to_store_err)?;
        envelopes.push(envelope);
    }
    let new_version = current_version + events.len() as u64;
    txn.put(
        sv_key(stream_id).as_bytes(),
        new_version.to_le_bytes().as_slice(),
    )
    .map_err(to_store_err)?;
    txn.put(si_key(stream_id).as_bytes(), b"")
        .map_err(to_store_err)?;
    Ok(envelopes)
}

/// Buffer outbox message payloads and their time-index entries into a transaction.
fn write_outbox_entries_txn(
    txn: &slatedb::DbTransaction,
    messages: &[OutboxMessage],
) -> Result<(), EngineError> {
    for msg in messages {
        let value = serde_json::to_vec(msg).map_err(|e| EngineError::outbox(e.to_string()))?;
        let ts_key = ot_key(msg.deliver_after.unwrap_or(msg.created_at), &msg.message_id);
        txn.put(om_key(&msg.message_id).as_bytes(), value.as_slice())
            .map_err(to_outbox_err)?;
        txn.put(ts_key.as_bytes(), b"").map_err(to_outbox_err)?;
    }
    Ok(())
}

/// Map a SlateDB transaction commit error to an [`EngineError`].
///
/// `ErrorKind::Transaction` means another writer committed `sv_key` between
/// our snapshot read and our commit — equivalent to a version conflict.
fn map_txn_commit_err(
    e: slatedb::Error,
    expected_version: ExpectedVersion,
    snapshot_version: u64,
) -> EngineError {
    if e.kind() == slatedb::ErrorKind::Transaction {
        let expected = match expected_version {
            ExpectedVersion::NoStream => 0,
            ExpectedVersion::Exact(v) => v,
            // Any: no check intended, but conflict still occurred; use
            // snapshot_version + 1 as a conservative lower bound for `actual`.
            ExpectedVersion::Any => snapshot_version,
        };
        // The true actual version is at least snapshot_version + 1 (the winner
        // appended at least one event). Callers use `is_version_conflict()` for
        // retry logic, not the exact actual value.
        EngineError::VersionConflict {
            expected,
            actual: snapshot_version + 1,
        }
    } else {
        to_store_err(e)
    }
}

// ── EventStore impl ───────────────────────────────────────────────────────────

impl EventStore for SlateDbStore {
    async fn append(
        &self,
        stream_id: &StreamId,
        expected_version: ExpectedVersion,
        events: &[NewEvent],
    ) -> Result<AppendResult, EngineError> {
        //  always use SSI — never IsolationLevel::default() — so an
        // upstream SlateDB change to its default cannot silently degrade isolation.
        let txn = self
            .db
            .begin(IsolationLevel::SerializableSnapshot)
            .await
            .map_err(to_store_err)?;

        let current = read_version_in_txn(&txn, stream_id).await?;
        check_version(expected_version, current)?;
        self.check_quota(stream_id, current, events.len())?;

        let now = OffsetDateTime::now_utc();
        let envelopes = stamp_events_txn(&txn, stream_id, current, events, now)?;
        txn.commit()
            .await
            .map_err(|e| map_txn_commit_err(e, expected_version, current))?;

        Ok(AppendResult {
            last_sequence: current + events.len() as u64,
            events: envelopes,
        })
    }

    async fn load(&self, stream_id: &StreamId) -> Result<Vec<EventEnvelope>, EngineError> {
        self.load_from(stream_id, 0).await
    }

    async fn load_from(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> Result<Vec<EventEnvelope>, EngineError> {
        // Use a range scan instead of a full prefix scan + in-process filter.
        // This avoids deserialising events that will be discarded, giving
        // O(k) behaviour where k = number of events returned rather than
        // O(n) where n = total events in the stream.
        let start_seq = from_sequence.saturating_add(1);
        let start = event_key(stream_id, start_seq);
        let end = event_prefix_end(stream_id);
        let mut iter = self
            .db
            .scan(start.as_bytes()..end.as_bytes())
            .await
            .map_err(to_store_err)?;
        let mut events = Vec::new();
        while let Some(kv) = iter.next().await.map_err(to_store_err)? {
            let event_env: EventEnvelope =
                serde_json::from_slice(&kv.value).map_err(|e| EngineError::store(e.to_string()))?;
            events.push(event_env);
        }
        Ok(events)
    }

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
        // Stream events one at a time without materialising a Vec, giving
        // constant-memory behaviour for large or long-lived streams.
        let start_seq = from_sequence.saturating_add(1);
        let start = event_key(stream_id, start_seq);
        let end = event_prefix_end(stream_id);
        let mut iter = self
            .db
            .scan(start.as_bytes()..end.as_bytes())
            .await
            .map_err(to_store_err)?;
        let mut acc = initial;
        while let Some(kv) = iter.next().await.map_err(to_store_err)? {
            let event_env: EventEnvelope =
                serde_json::from_slice(&kv.value).map_err(|e| EngineError::store(e.to_string()))?;
            acc = f(acc, event_env)?;
        }
        Ok(acc)
    }

    async fn stream_version(&self, stream_id: &StreamId) -> Result<u64, EngineError> {
        self.read_version(stream_id).await
    }

    async fn list_streams(&self, prefix: Option<&str>) -> Result<Vec<StreamId>, EngineError> {
        let scan_prefix = match prefix {
            Some(p) => format!("si/{p}"),
            None => "si/".to_string(),
        };
        let mut iter = self
            .db
            .scan_prefix(scan_prefix.as_bytes())
            .await
            .map_err(to_store_err)?;
        let mut streams = Vec::new();
        while let Some(kv) = iter.next().await.map_err(to_store_err)? {
            let key =
                std::str::from_utf8(&kv.key).map_err(|e| EngineError::store(e.to_string()))?;
            if let Some(stream_id_str) = key.strip_prefix("si/") {
                streams.push(
                    StreamId::try_new(stream_id_str)
                        .map_err(|e| EngineError::store(e.to_string()))?,
                );
            }
        }
        Ok(streams)
    }

    /// Paginated stream scan — native cursor-based implementation avoids
    /// materialising the full `si/` key space.
    ///
    /// Uses the `si/{prefix}{cursor_exclusive}` scan start position so the
    /// underlying SlateDB scan begins at the right lexicographic position,
    /// reading at most `limit` entries without loading the rest of the index.
    async fn list_streams_page(
        &self,
        prefix: Option<&str>,
        cursor: Option<&StreamId>,
        limit: usize,
    ) -> Result<Vec<StreamId>, EngineError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let si_prefix = match prefix {
            Some(p) => format!("si/{p}"),
            None => "si/".to_string(),
        };

        // Determine the scan start key. When a cursor is provided, we advance
        // one byte beyond the cursor key to get an exclusive start (since
        // SlateDB's scan_range is inclusive on the left bound).
        //
        // We construct the exclusive start by appending a null byte (0x00) to
        // the cursor key — this is lexicographically the smallest key that sorts
        // strictly after the cursor key in a UTF-8 / ASCII stream-ID space.
        let start_key: Vec<u8> = match cursor {
            None => si_prefix.as_bytes().to_vec(),
            Some(c) => {
                let mut key = format!("si/{}{}", prefix.unwrap_or(""), c.as_str()).into_bytes();
                key.push(0x00); // exclusive: smallest key > cursor
                key
            }
        };

        // End key is the exclusive upper bound: `si/{prefix}` with the last
        // character incremented so the scan stops at the prefix boundary.
        // Build it by incrementing the last byte of the prefix — this gives the
        // correct lexicographic upper bound even for non-ASCII prefixes.
        let end_key: Option<Vec<u8>> = {
            let mut upper = si_prefix.into_bytes();
            // Find the last byte and increment; if overflow, pop and carry.
            let mut carried = true;
            while carried {
                if let Some(last) = upper.last_mut() {
                    if *last == u8::MAX {
                        upper.pop();
                    } else {
                        *last += 1;
                        carried = false;
                    }
                } else {
                    // All bytes overflowed → scan to end of key space.
                    carried = false;
                }
            }
            if upper.is_empty() {
                None // scan to end of entire key space
            } else {
                Some(upper)
            }
        };

        let mut iter = match end_key {
            Some(end) => self
                .db
                .scan(start_key.as_slice()..end.as_slice())
                .await
                .map_err(to_store_err)?,
            None => self
                .db
                .scan(start_key.as_slice()..)
                .await
                .map_err(to_store_err)?,
        };

        let si_prefix_str = match prefix {
            Some(p) => format!("si/{p}"),
            None => "si/".to_string(),
        };
        let mut streams = Vec::with_capacity(limit);
        while streams.len() < limit {
            match iter.next().await.map_err(to_store_err)? {
                None => break,
                Some(kv) => {
                    let key = std::str::from_utf8(&kv.key)
                        .map_err(|e| EngineError::store(e.to_string()))?;
                    if let Some(stream_id_str) = key.strip_prefix(&si_prefix_str) {
                        // Skip empty suffixes (guard for malformed keys).
                        if !stream_id_str.is_empty() {
                            streams.push(
                                StreamId::try_new(stream_id_str)
                                    .map_err(|e| EngineError::store(e.to_string()))?,
                            );
                        }
                    }
                }
            }
        }
        Ok(streams)
    }
}

// ── AtomicAppend impl ─────────────────────────────────────────────────────────

/// Materialises a [`PendingOutbox`] value into a fully-typed [`OutboxMessage`]
/// using the context carried in the corresponding stamped [`EventEnvelope`].
///
/// `caused_by_event_index` is clamped to `envelopes.len() - 1`.
fn materialise_outbox(
    pending: &PendingOutbox,
    stream_id: &StreamId,
    envelopes: &[EventEnvelope],
    now: OffsetDateTime,
) -> OutboxMessage {
    // Clamp index so out-of-range values always point to the last event.
    let idx = pending
        .caused_by_event_index
        .min(envelopes.len().saturating_sub(1));
    let env = &envelopes[idx];
    OutboxMessage {
        message_id: crate::ids::OutboxMessageId::new(),
        stream_id: stream_id.clone(),
        process_id: env.process_id,
        tenant_id: env.tenant_id,
        correlation_id: env.correlation_id,
        conversation_id: env.conversation_id,
        causation_event_id: env.event_id,
        message_type: pending.message_type.clone(),
        recipient: pending.recipient.clone(),
        payload: pending.payload.clone(),
        payload_schema: pending.payload_schema.clone(),
        created_at: now,
        deliver_after: pending.deliver_after,
        attempt_count: 0,
    }
}

impl AtomicAppend for SlateDbStore {
    /// Atomically append `events` to `stream_id` **and** enqueue `outbox`
    /// messages in a single SlateDB transaction.
    ///
    /// The `outbox` slice contains lightweight [`PendingOutbox`] values produced
    /// by [`crate::workflow::Workflow::handle`]. This method stamps the events first, then
    /// materialises each pending outbox entry into a full [`OutboxMessage`]
    /// using the context fields from the stamped envelopes (including the
    /// store-assigned `event_id` as `causation_event_id`). Both the events and
    /// the outbox messages are committed in the same transaction, so a crash
    /// cannot produce a partially-written state, and a concurrent writer is
    /// detected as a [`EngineError::VersionConflict`] rather than silently
    /// overwriting.
    ///
    /// ## Durability guarantee (WAL)
    ///
    /// SlateDB uses a write-ahead log (WAL) backed by the same object store as
    /// the SSTable data. The transaction `.commit()` call below does **not**
    /// return until the WAL entry has been flushed and acknowledged by the
    /// object store. Therefore, once this method returns `Ok(_)` the write is
    /// durable: it survives a process crash and will be replayed from the WAL
    /// on the next `SlateDbStore::open*` call.
    ///
    /// Because events and outbox entries land in the **same** `WriteBatch` /
    /// transaction, both are present or both are absent after a crash. It is
    /// never safe to write events first and outbox second in separate
    /// transactions — a crash between the two would produce a committed event
    /// without a corresponding APERAK being enqueued (silent data loss).
    ///
    /// # Errors
    ///
    /// - [`EngineError::VersionConflict`] — optimistic concurrency check failed
    ///   (either the version pre-check or a transaction commit conflict).
    /// - [`EngineError::Store`] / [`EngineError::Outbox`] — storage failure.
    async fn append_with_outbox(
        &self,
        stream_id: &StreamId,
        expected_version: ExpectedVersion,
        events: &[NewEvent],
        outbox: &[PendingOutbox],
    ) -> Result<AppendResult, EngineError> {
        //  always use SSI — never IsolationLevel::default().
        let txn = self
            .db
            .begin(IsolationLevel::SerializableSnapshot)
            .await
            .map_err(to_store_err)?;

        let current = read_version_in_txn(&txn, stream_id).await?;
        check_version(expected_version, current)?;
        self.check_quota(stream_id, current, events.len())?;

        let now = OffsetDateTime::now_utc();
        let envelopes = stamp_events_txn(&txn, stream_id, current, events, now)?;

        if !outbox.is_empty() {
            let messages: Vec<OutboxMessage> = outbox
                .iter()
                .map(|p| materialise_outbox(p, stream_id, &envelopes, now))
                .collect();
            // Increment the O(1) counter alongside the outbox entries.
            let current_count = read_om_count_txn(&txn).await?;
            let new_count = current_count + messages.len() as u64;
            txn.put(OM_COUNT_KEY, new_count.to_le_bytes().as_slice())
                .map_err(to_outbox_err)?;
            write_outbox_entries_txn(&txn, &messages)?;
        }

        txn.commit()
            .await
            .map_err(|e| map_txn_commit_err(e, expected_version, current))?;

        Ok(AppendResult {
            last_sequence: current + events.len() as u64,
            events: envelopes,
        })
    }
}

// ── ProjectionCheckpointStore impl ───────────────────────────────────────────

impl crate::projection::ProjectionCheckpointStore for SlateDbStore {
    /// Load the checkpoint for `name` from per-stream cursor keys.
    ///
    /// Scans the prefix `cp/{name}/` and reconstructs a
    /// [`crate::projection::GlobalProjectionCheckpoint`] from the individual u64 LE cursor
    /// values.  Returns an empty checkpoint (all cursors zero) when no keys
    /// exist under the prefix yet, triggering a full replay from the
    /// beginning.
    async fn load_projection_checkpoint(
        &self,
        name: &str,
    ) -> Result<crate::projection::GlobalProjectionCheckpoint, EngineError> {
        let prefix = format!("cp/{name}/");
        let end = cp_prefix_end(name);
        let mut iter = self
            .db
            .scan(prefix.as_bytes()..end.as_bytes())
            .await
            .map_err(to_store_err)?;
        let mut cp = crate::projection::GlobalProjectionCheckpoint::new();
        while let Some(kv) = iter.next().await.map_err(to_store_err)? {
            let key_str = std::str::from_utf8(&kv.key)
                .map_err(|e| EngineError::store(format!("checkpoint key utf8: {e}")))?;
            let stream_id_str = key_str.strip_prefix(&prefix).ok_or_else(|| {
                EngineError::store(format!("unexpected checkpoint key: {key_str}"))
            })?;
            if kv.value.len() != 8 {
                return Err(EngineError::store(format!(
                    "checkpoint cursor for '{key_str}' is {} bytes, expected 8",
                    kv.value.len()
                )));
            }
            let seq = u64::from_le_bytes(kv.value[..8].try_into().unwrap());
            cp.advance(&StreamId::new(stream_id_str), seq);
        }
        Ok(cp)
    }

    /// Persist all cursors from `checkpoint` as individual per-stream keys.
    ///
    /// Writes the full set of cursors in a single `WriteBatch` so the
    /// operation is atomic — readers either see the previous generation or
    /// the new one, never a partial mix.
    async fn save_projection_checkpoint(
        &self,
        name: &str,
        checkpoint: &crate::projection::GlobalProjectionCheckpoint,
    ) -> Result<(), EngineError> {
        let mut batch = WriteBatch::new();
        for (stream_id, &seq) in &checkpoint.cursors {
            let key = cp_cursor_key(name, stream_id);
            batch.put(key.as_bytes(), seq.to_le_bytes().as_slice());
        }
        self.db.write(batch).await.map_err(to_store_err)?;
        Ok(())
    }

    /// Write only the cursors that advanced since `previous`.
    ///
    /// For deployments with tens of thousands of streams this is O(changed)
    /// instead of O(total), avoiding redundant writes on every catch-up cycle
    /// when only a handful of streams received new events.
    async fn advance_projection_cursors(
        &self,
        name: &str,
        previous: &crate::projection::GlobalProjectionCheckpoint,
        current: &crate::projection::GlobalProjectionCheckpoint,
    ) -> Result<(), EngineError> {
        let mut batch = WriteBatch::new();
        for (stream_id, &seq) in &current.cursors {
            if seq > previous.cursor_for(stream_id) {
                let key = cp_cursor_key(name, stream_id);
                batch.put(key.as_bytes(), seq.to_le_bytes().as_slice());
            }
        }
        // WriteBatch::write is a no-op if the batch is empty, so this is
        // always safe to call even when nothing changed.
        self.db.write(batch).await.map_err(to_store_err)?;
        Ok(())
    }
}

// ── OutboxStore impl ──────────────────────────────────────────────────────────

impl OutboxStore for SlateDbStore {
    /// Atomically enqueues `messages` and increments the `_count/om` counter.
    ///
    /// Uses a snapshot-isolation transaction so the counter read-modify-write
    /// is safe under concurrent callers.  The outbox entries themselves have
    /// fresh UUIDs so there is no logical conflict risk — the transaction
    /// exists solely to keep the counter consistent.
    async fn enqueue(&self, messages: &[OutboxMessage]) -> Result<(), EngineError> {
        if messages.is_empty() {
            return Ok(());
        }
        //  always use SSI — never IsolationLevel::default().
        let txn = self
            .db
            .begin(IsolationLevel::SerializableSnapshot)
            .await
            .map_err(to_outbox_err)?;
        let current_count = read_om_count_txn(&txn).await?;
        let new_count = current_count + messages.len() as u64;
        txn.put(OM_COUNT_KEY, new_count.to_le_bytes().as_slice())
            .map_err(to_outbox_err)?;
        write_outbox_entries_txn(&txn, messages)?;
        txn.commit().await.map_err(to_outbox_err)?;
        Ok(())
    }

    async fn pending(
        &self,
        limit: usize,
        now: OffsetDateTime,
    ) -> Result<Vec<OutboxMessage>, EngineError> {
        // Clamp pre-epoch to zero consistent with ot_key.
        let now_nanos = u64::try_from(now.unix_timestamp_nanos().max(0)).unwrap_or(0);
        // Use a bounded range scan up to and including `now_nanos` so the
        // underlying LSM iterator never seeks past the present time boundary
        // even when there are many future-dated (rescheduled) entries.
        // This turns an O(n_past + n_future) scan into O(n_due).
        // The end key uses U+FFFF as the trailing byte so that all UUIDs for
        // a given timestamp nanosecond are included in the range.
        let end_key = format!("ot/{now_nanos:016x}/\u{FFFF}");
        let mut iter = self
            .db
            .scan(b"ot/".as_slice()..end_key.as_bytes())
            .await
            .map_err(to_outbox_err)?;
        let mut result = Vec::new();
        while let Some(kv) = iter.next().await.map_err(to_outbox_err)? {
            // Check limit FIRST before any parsing work.
            if result.len() >= limit {
                break;
            }
            // key = "ot/{ts_hex16}/{message_id_uuid}"
            let key =
                std::str::from_utf8(&kv.key).map_err(|e| EngineError::outbox(e.to_string()))?;
            let rest = match key.strip_prefix("ot/") {
                Some(r) if r.len() >= 17 => r,
                _ => continue,
            };
            let entry_nanos = u64::from_str_radix(&rest[..16], 16).unwrap_or(u64::MAX);
            if entry_nanos > now_nanos {
                // Belt-and-braces: range bound should have stopped iteration already.
                break;
            }
            let msg_id_str = &rest[17..]; // 16 hex + '/'
            let msg_key = format!("om/{msg_id_str}");
            if let Some(msg_bytes) = self
                .db
                .get(msg_key.as_bytes())
                .await
                .map_err(to_outbox_err)?
            {
                let msg: OutboxMessage = serde_json::from_slice(&msg_bytes)
                    .map_err(|e| EngineError::outbox(e.to_string()))?;
                result.push(msg);
            }
        }
        Ok(result)
    }

    /// Removes a delivered message and decrements the `_count/om` counter.
    ///
    /// Uses a snapshot-isolation transaction to keep the counter consistent
    /// with the `om/` key space under concurrent acknowledgers.
    async fn acknowledge(&self, id: OutboxMessageId) -> Result<(), EngineError> {
        let msg_key = om_key(&id);
        //  always use SSI — never IsolationLevel::default().
        let txn = self
            .db
            .begin(IsolationLevel::SerializableSnapshot)
            .await
            .map_err(to_outbox_err)?;
        if let Some(msg_bytes) = txn.get(msg_key.as_bytes()).await.map_err(to_outbox_err)? {
            let msg: OutboxMessage = serde_json::from_slice(&msg_bytes)
                .map_err(|e| EngineError::outbox(e.to_string()))?;
            let ts_key = ot_key(msg.deliver_after.unwrap_or(msg.created_at), &id);
            let current_count = read_om_count_txn(&txn).await?;
            let new_count = current_count.saturating_sub(1);
            txn.delete(msg_key.as_bytes()).map_err(to_outbox_err)?;
            txn.delete(ts_key.as_bytes()).map_err(to_outbox_err)?;
            txn.put(OM_COUNT_KEY, new_count.to_le_bytes().as_slice())
                .map_err(to_outbox_err)?;
            txn.commit().await.map_err(to_outbox_err)?;
        }
        Ok(())
    }

    async fn reschedule(
        &self,
        id: OutboxMessageId,
        deliver_after: OffsetDateTime,
    ) -> Result<(), EngineError> {
        //  use an SSI transaction instead of WriteBatch to prevent a
        // concurrent reschedule race.  Under WriteBatch two racing calls could
        // each read the old timestamp, compute a new ts_key, then write — the
        // loser's delete would silently remove an already-updated entry.
        const MAX_RESCHEDULE_RETRIES: usize = 8;
        let msg_key = om_key(&id);
        for _attempt in 0..MAX_RESCHEDULE_RETRIES {
            let txn = self
                .db
                .begin(IsolationLevel::SerializableSnapshot)
                .await
                .map_err(to_outbox_err)?;
            let Some(msg_bytes) = txn.get(msg_key.as_bytes()).await.map_err(to_outbox_err)? else {
                // Message was acknowledged while we were rescheduling — no-op.
                txn.rollback();
                return Ok(());
            };
            let mut msg: OutboxMessage = serde_json::from_slice(&msg_bytes)
                .map_err(|e| EngineError::outbox(e.to_string()))?;
            let old_ts_key = ot_key(msg.deliver_after.unwrap_or(msg.created_at), &id);
            msg.deliver_after = Some(deliver_after);
            msg.attempt_count += 1;
            let new_ts_key = ot_key(deliver_after, &id);
            let new_value =
                serde_json::to_vec(&msg).map_err(|e| EngineError::outbox(e.to_string()))?;
            txn.delete(old_ts_key.as_bytes()).map_err(to_outbox_err)?;
            txn.put(msg_key.as_bytes(), new_value.as_slice())
                .map_err(to_outbox_err)?;
            txn.put(new_ts_key.as_bytes(), b"").map_err(to_outbox_err)?;
            match txn.commit().await {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == ErrorKind::Transaction => {
                    // Conflict: retry with the freshly-read state.
                }
                Err(e) => return Err(to_outbox_err(e)),
            }
        }
        Err(EngineError::outbox("reschedule conflict: too many retries"))
    }

    /// Returns the number of pending outbox messages.
    ///
    /// **O(1)** — reads the `_count/om` counter key that is maintained
    /// atomically by [`enqueue`][Self::enqueue] and
    /// [`acknowledge`][Self::acknowledge].
    async fn len(&self) -> Result<usize, EngineError> {
        Ok(usize::try_from(read_om_count(&self.db).await?).unwrap_or(usize::MAX))
    }
}

/// Counter key for the number of registered deadlines.
///
/// Stored as 8-byte little-endian u64. Updated atomically in every
/// [`DeadlineStore::register`] and [`DeadlineStore::cancel`] call.
const DL_COUNT_KEY: &[u8] = b"_count/dl";

/// Read the `_count/dl` counter within an in-progress transaction.
async fn read_dl_count_txn(txn: &slatedb::DbTransaction) -> Result<u64, EngineError> {
    match txn.get(DL_COUNT_KEY).await.map_err(to_deadline_err)? {
        None => Ok(0), // counter not yet written — store is empty, expected on first register
        Some(bytes) if bytes.len() == 8 => Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
        Some(bytes) => Err(EngineError::deadline(format!(
            "_count/dl corrupt: expected 8-byte little-endian u64, got {} bytes",
            bytes.len()
        ))),
    }
}

/// Counter key for the number of registered process identity entries (`pr/` keys).
///
/// Stored as 8-byte little-endian u64. Updated atomically in every
/// [`ProcessRegistry::register`] and [`ProcessRegistry::remove`] call.
const PR_COUNT_KEY: &[u8] = b"_count/pr";

/// Read the `_count/pr` counter within an in-progress transaction.
async fn read_pr_count_txn(txn: &slatedb::DbTransaction) -> Result<u64, EngineError> {
    match txn.get(PR_COUNT_KEY).await.map_err(to_registry_err)? {
        None => Ok(0), // counter not yet written — store is empty, expected on first register
        Some(bytes) if bytes.len() == 8 => Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
        Some(bytes) => Err(EngineError::registry(format!(
            "_count/pr corrupt: expected 8-byte little-endian u64, got {} bytes",
            bytes.len()
        ))),
    }
}

// ── SlateDbDeadlineStore ──────────────────────────────────────────────────────

/// Durable [`DeadlineStore`] backed by SlateDB.
///
/// Key schema:
/// - `dl/{deadline_id}` → `JSON(Deadline)` — deadline payload
/// - `dt/{due_nanos:016x}/{deadline_id}` → `""` — time-sorted index for `due_now`
/// - `ds/{stream_id}/{deadline_id}` → `""` — reverse index for `for_stream` (O(k))
/// - `_count/dl` → u64 LE — O(1) total deadline count
///
/// All writes use SSI transactions: registering increments `_count/dl` and
/// writes all three payload/index entries atomically; cancelling decrements the
/// counter and deletes all three entries atomically.
///
/// Obtain via [`SlateDbStore::as_deadline_store`].
#[derive(Clone)]
pub struct SlateDbDeadlineStore {
    db: Db,
}

impl DeadlineStore for SlateDbDeadlineStore {
    /// Register a deadline atomically, incrementing `_count/dl`.
    ///
    /// Uses an SSI transaction so the counter read-modify-write is safe under
    /// concurrent callers. Deadline IDs are globally unique (UUID v4)
    /// so there is no logical conflict risk between concurrent registrations —
    /// the transaction exists solely to keep the counter consistent.
    async fn register(&self, deadline: &Deadline) -> Result<(), EngineError> {
        let payload =
            serde_json::to_vec(deadline).map_err(|e| EngineError::deadline(e.to_string()))?;
        let dl_k = dl_key(&deadline.deadline_id());
        let time_key = dt_key(deadline.due_at(), &deadline.deadline_id());
        let stream_key = ds_key(deadline.stream_id(), &deadline.deadline_id());
        let txn = self
            .db
            .begin(IsolationLevel::SerializableSnapshot)
            .await
            .map_err(to_deadline_err)?;
        // Only increment the counter for *new* deadlines; updates (upserts)
        // for an existing deadline_id must not grow the count.
        let is_new = txn
            .get(dl_k.as_bytes())
            .await
            .map_err(to_deadline_err)?
            .is_none();
        if is_new {
            let current_count = read_dl_count_txn(&txn).await?;
            txn.put(DL_COUNT_KEY, (current_count + 1).to_le_bytes().as_slice())
                .map_err(to_deadline_err)?;
        }
        txn.put(dl_k.as_bytes(), payload.as_slice())
            .map_err(to_deadline_err)?;
        txn.put(time_key.as_bytes(), b"").map_err(to_deadline_err)?;
        txn.put(stream_key.as_bytes(), b"")
            .map_err(to_deadline_err)?;
        txn.commit().await.map_err(to_deadline_err)?;
        Ok(())
    }

    /// Cancel a deadline atomically, decrementing `_count/dl`.
    ///
    /// Uses an SSI transaction so the counter decrement is safe under
    /// concurrent cancellations. No-op if the deadline does not exist.
    async fn cancel(&self, id: DeadlineId) -> Result<(), EngineError> {
        let dl_k = dl_key(&id);
        if let Some(bytes) = self
            .db
            .get(dl_k.as_bytes())
            .await
            .map_err(to_deadline_err)?
        {
            let deadline: Deadline =
                serde_json::from_slice(&bytes).map_err(|e| EngineError::deadline(e.to_string()))?;
            let time_key = dt_key(deadline.due_at(), &id);
            let stream_key = ds_key(deadline.stream_id(), &id);
            let txn = self
                .db
                .begin(IsolationLevel::SerializableSnapshot)
                .await
                .map_err(to_deadline_err)?;
            let current_count = read_dl_count_txn(&txn).await?;
            txn.put(
                DL_COUNT_KEY,
                current_count.saturating_sub(1).to_le_bytes().as_slice(),
            )
            .map_err(to_deadline_err)?;
            txn.delete(dl_k.as_bytes()).map_err(to_deadline_err)?;
            txn.delete(time_key.as_bytes()).map_err(to_deadline_err)?;
            txn.delete(stream_key.as_bytes()).map_err(to_deadline_err)?;
            txn.commit().await.map_err(to_deadline_err)?;
        }
        Ok(())
    }

    async fn due_now(&self, limit: usize) -> Result<DueNowResult, EngineError> {
        let now_nanos =
            u64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos().max(0)).unwrap_or(0);
        // Use a bounded range scan up to and including `now_nanos` so the
        // underlying LSM iterator stops at the present time boundary rather than
        // scanning all future-dated entries before our application-level break
        // Bounded dt/ scan pattern, mirrors load_from for events.
        let end_key = format!("dt/{now_nanos:016x}/\u{FFFF}");
        let mut iter = self
            .db
            .scan(b"dt/".as_slice()..end_key.as_bytes())
            .await
            .map_err(to_deadline_err)?;
        let mut deadlines = Vec::new();
        let mut future_entry_seen = false;

        while let Some(kv) = iter.next().await.map_err(to_deadline_err)? {
            let key =
                std::str::from_utf8(&kv.key).map_err(|e| EngineError::deadline(e.to_string()))?;
            // key = "dt/{16 hex}/{deadline_id_uuid}"
            let rest = match key.strip_prefix("dt/") {
                Some(r) if r.len() >= 17 => r,
                _ => continue,
            };
            let entry_nanos = u64::from_str_radix(&rest[..16], 16).unwrap_or(u64::MAX);
            if entry_nanos > now_nanos {
                // Keys are sorted chronologically — no more due entries
                // (belt-and-braces; the range bound should have stopped iteration).
                future_entry_seen = true;
                break;
            }
            if deadlines.len() >= limit {
                // We already have `limit` results and this entry is still due —
                // there are more overdue deadlines waiting.  Set has_more and stop.
                // Note: checking BEFORE loading so we don't over-fetch.
                let has_more = true;
                return Ok(DueNowResult {
                    deadlines,
                    has_more,
                });
            }
            let id_str = &rest[17..]; // after "dt/{16hex}/"
            let dl_k = format!("dl/{id_str}");
            if let Some(bytes) = self
                .db
                .get(dl_k.as_bytes())
                .await
                .map_err(to_deadline_err)?
            {
                let deadline: Deadline = serde_json::from_slice(&bytes)
                    .map_err(|e| EngineError::deadline(e.to_string()))?;
                deadlines.push(deadline);
            }
        }

        // If we collected exactly `limit` entries and the loop ended naturally
        // (not via the limit-guard above), we cannot know whether more overdue
        // entries exist — conservatively set has_more = true so the scheduler
        // re-polls immediately .
        let has_more = !future_entry_seen && deadlines.len() == limit;
        Ok(DueNowResult {
            deadlines,
            has_more,
        })
    }

    async fn for_stream(&self, stream_id: &StreamId) -> Result<Vec<Deadline>, EngineError> {
        // O(k) lookup via the `ds/{stream_id}/` reverse index.
        // k = number of deadlines registered for this stream (typically 0–2).
        let prefix = ds_stream_prefix(stream_id);
        let mut iter = self
            .db
            .scan_prefix(prefix.as_bytes())
            .await
            .map_err(to_deadline_err)?;
        let mut result = Vec::new();
        while let Some(kv) = iter.next().await.map_err(to_deadline_err)? {
            // ds-key format: "ds/{stream_id}/{deadline_id}"
            let key =
                std::str::from_utf8(&kv.key).map_err(|e| EngineError::deadline(e.to_string()))?;
            let deadline_id_str = match key.strip_prefix(prefix.as_str()) {
                Some(id) if !id.is_empty() => id,
                _ => continue,
            };
            let dl_k = format!("dl/{deadline_id_str}");
            if let Some(bytes) = self
                .db
                .get(dl_k.as_bytes())
                .await
                .map_err(to_deadline_err)?
            {
                let deadline: Deadline = serde_json::from_slice(&bytes)
                    .map_err(|e| EngineError::deadline(e.to_string()))?;
                result.push(deadline);
            }
        }
        Ok(result)
    }

    /// Returns the number of registered deadlines.
    ///
    /// # Performance
    ///
    /// **O(1)** — reads the `_count/dl` counter key maintained by
    /// [`register`][Self::register] and [`cancel`][Self::cancel].
    ///
    /// On first call against a store that was created before the counter key
    /// existed (upgrade path), performs a one-time O(n) bootstrap scan,
    /// persists the count, and returns the result. All subsequent calls are O(1).
    async fn len(&self) -> Result<usize, EngineError> {
        // Try O(1) counter first.
        match self.db.get(DL_COUNT_KEY).await.map_err(to_deadline_err)? {
            Some(bytes) if bytes.len() == 8 => {
                return Ok(
                    usize::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
                        .unwrap_or(usize::MAX),
                );
            }
            _ => {} // not yet initialised — fall through to bootstrap scan
        }
        // One-time bootstrap: scan all dl/ keys and persist the counter.
        let mut iter = self.db.scan_prefix(b"dl/").await.map_err(to_deadline_err)?;
        let mut count = 0u64;
        while iter.next().await.map_err(to_deadline_err)?.is_some() {
            count += 1;
        }
        let mut batch = WriteBatch::new();
        batch.put(DL_COUNT_KEY, count.to_le_bytes().as_slice());
        // Best-effort write — len() is a health-check helper; ignore errors.
        let _ = self.db.write(batch).await;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }
}

// ── SlateDbProcessRegistry ────────────────────────────────────────────────────

/// Durable [`ProcessRegistry`] backed by SlateDB.
///
/// Key schema:
/// - `pr/{tenant_id}/{routing_key}` → `JSON(ProcessIdentity)` — 1:1 routing
/// - `ci/{tenant_id}/{tag}/{process_id}` → `JSON(ProcessIdentity)` — 1:many correlated index
///
/// `TenantId` is always a 36-character UUID, giving fixed-length prefixes
/// `pr/{36-chars}/` and `ci/{36-chars}/` that efficiently scope per-tenant scans.
///
/// Obtain via [`SlateDbStore::as_process_registry`].
#[derive(Clone)]
pub struct SlateDbProcessRegistry {
    db: Db,
}

impl ProcessRegistry for SlateDbProcessRegistry {
    async fn register(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        let k = pr_key(tenant_id, key);
        let v = serde_json::to_vec(&identity).map_err(|e| EngineError::registry(e.to_string()))?;
        // Use SSI transaction to atomically write the routing entry and
        // increment the _count/pr counter. Previously used WriteBatch
        //; upgraded to SSI txn to keep the counter consistent.
        let txn = self
            .db
            .begin(IsolationLevel::SerializableSnapshot)
            .await
            .map_err(to_registry_err)?;
        // Only increment if this is a new key (not an update of an existing entry).
        let is_new = txn
            .get(k.as_bytes())
            .await
            .map_err(to_registry_err)?
            .is_none();
        if is_new {
            let current_count = read_pr_count_txn(&txn).await?;
            txn.put(PR_COUNT_KEY, (current_count + 1).to_le_bytes().as_slice())
                .map_err(to_registry_err)?;
        }
        txn.put(k.as_bytes(), v.as_slice())
            .map_err(to_registry_err)?;
        txn.commit().await.map_err(to_registry_err)?;
        Ok(())
    }

    async fn lookup(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
    ) -> Result<Option<ProcessIdentity>, EngineError> {
        let k = pr_key(tenant_id, key);
        match self.db.get(k.as_bytes()).await.map_err(to_registry_err)? {
            Some(bytes) => {
                let identity: ProcessIdentity = serde_json::from_slice(&bytes)
                    .map_err(|e| EngineError::registry(e.to_string()))?;
                Ok(Some(identity))
            }
            None => Ok(None),
        }
    }

    async fn remove(&self, tenant_id: TenantId, key: &RegistryKey) -> Result<(), EngineError> {
        let k = pr_key(tenant_id, key);
        // Use SSI transaction to atomically delete the entry and decrement
        // _count/pr. No-op if the key does not exist.
        let txn = self
            .db
            .begin(IsolationLevel::SerializableSnapshot)
            .await
            .map_err(to_registry_err)?;
        if txn
            .get(k.as_bytes())
            .await
            .map_err(to_registry_err)?
            .is_some()
        {
            let current_count = read_pr_count_txn(&txn).await?;
            txn.put(
                PR_COUNT_KEY,
                current_count.saturating_sub(1).to_le_bytes().as_slice(),
            )
            .map_err(to_registry_err)?;
            txn.delete(k.as_bytes()).map_err(to_registry_err)?;
        }
        txn.commit().await.map_err(to_registry_err)?;
        Ok(())
    }

    /// Returns the number of registered process identity entries.
    ///
    /// # Performance
    ///
    /// **O(1)** — reads the `_count/pr` counter key maintained by
    /// [`register`][Self::register] and [`remove`][Self::remove].
    ///
    /// On first call against a store created before the counter key existed
    /// (upgrade path), performs a one-time O(n) bootstrap scan, persists the
    /// count, and returns the result. All subsequent calls are O(1).
    async fn len(&self) -> Result<usize, EngineError> {
        // Try O(1) counter first.
        match self.db.get(PR_COUNT_KEY).await.map_err(to_registry_err)? {
            Some(bytes) if bytes.len() == 8 => {
                return Ok(
                    usize::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
                        .unwrap_or(usize::MAX),
                );
            }
            _ => {} // not yet initialised — fall through to bootstrap scan
        }
        // One-time bootstrap: scan all pr/ keys and persist the counter.
        let mut iter = self.db.scan_prefix(b"pr/").await.map_err(to_registry_err)?;
        let mut count = 0u64;
        while iter.next().await.map_err(to_registry_err)?.is_some() {
            count += 1;
        }
        let mut batch = WriteBatch::new();
        batch.put(PR_COUNT_KEY, count.to_le_bytes().as_slice());
        // Best-effort write — len() is a health-check helper; ignore errors.
        let _ = self.db.write(batch).await;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// Associate `process_id`/`identity` with `tag` for `tenant_id`.
    ///
    /// Key: `ci/{tenant_id}/{tag}/{process_id}` → `JSON(ProcessIdentity)`.
    /// Multiple processes can be registered under the same `(tenant_id, tag)`.
    async fn register_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        validate_ci_tag(tag)?;
        let k = ci_key(tenant_id, tag, process_id);
        let v = serde_json::to_vec(&identity).map_err(|e| EngineError::registry(e.to_string()))?;
        let mut batch = WriteBatch::new();
        batch.put(k.as_bytes(), v.as_slice());
        self.db.write(batch).await.map_err(to_registry_err)?;
        Ok(())
    }

    /// Return all `ProcessIdentity` values registered under `(tenant_id, tag)`.
    ///
    /// Scans the `ci/{tenant_id}/{tag}/` prefix — O(k) where k is the number
    /// of processes under this tag. For MABIS billing aggregation with a single
    /// Bilanzkreis, k is typically ≤ 100 (one per MaLo).
    async fn lookup_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
    ) -> Result<Vec<ProcessIdentity>, EngineError> {
        validate_ci_tag(tag)?;
        let prefix = ci_tag_prefix(tenant_id, tag);
        let mut iter = self
            .db
            .scan_prefix(prefix.as_bytes())
            .await
            .map_err(to_registry_err)?;

        let mut results = Vec::new();
        while let Some(entry) = iter.next().await.map_err(to_registry_err)? {
            let identity: ProcessIdentity = serde_json::from_slice(&entry.value)
                .map_err(|e| EngineError::registry(e.to_string()))?;
            results.push(identity);
        }
        Ok(results)
    }

    /// Remove `process_id` from the `(tenant_id, tag)` fan-out set.
    ///
    /// Deletes the `ci/{tenant_id}/{tag}/{process_id}` key. No-op when the
    /// entry does not exist.
    async fn remove_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
    ) -> Result<(), EngineError> {
        validate_ci_tag(tag)?;
        let k = ci_key(tenant_id, tag, process_id);
        let mut batch = WriteBatch::new();
        batch.delete(k.as_bytes());
        self.db.write(batch).await.map_err(to_registry_err)?;
        Ok(())
    }
}

// ── SlateDbSnapshotStore ──────────────────────────────────────────────────────

/// Durable [`SnapshotStore`] backed by SlateDB.
///
/// Key schema:
/// - `sn/{stream_id}` → `JSON(Snapshot)` — most recent snapshot per stream.
///
/// One snapshot is retained per stream. `save` atomically overwrites the
/// previous snapshot via a `WriteBatch`. This is safe because snapshots are
/// performance hints only — if a snapshot is lost in a crash between writes,
/// the engine falls back to full event replay, which is always correct.
///
/// # Snapshot interval recommendation
///
/// Take a snapshot every 100 events (`Snapshot::should_take(count, last, 100)`).
/// This bounds replay to at most 100 tail events per command dispatch even for
/// streams that have accumulated thousands of events (e.g. MABIS billing,
/// long-lived GPKE processes).
///
/// Obtain via [`SlateDbStore::as_snapshot_store`].
#[derive(Clone)]
pub struct SlateDbSnapshotStore {
    db: Db,
}

fn to_snapshot_err(e: &slatedb::Error) -> EngineError {
    tracing::error!(error = %e, "snapshot store error");
    if slatedb_error_is_transient(e) {
        EngineError::transient_snapshot(slatedb_error_kind_str(e))
    } else {
        EngineError::snapshot(slatedb_error_kind_str(e))
    }
}

impl SnapshotStore for SlateDbSnapshotStore {
    async fn save(&self, snapshot: &Snapshot) -> Result<(), EngineError> {
        let key = sn_key(&snapshot.stream_id);
        let value =
            serde_json::to_vec(snapshot).map_err(|e| EngineError::snapshot(e.to_string()))?;
        let mut batch = WriteBatch::new();
        batch.put(key.as_bytes(), value.as_slice());
        self.db
            .write(batch)
            .await
            .map_err(|e| to_snapshot_err(&e))?;
        Ok(())
    }

    async fn load(&self, stream_id: &StreamId) -> Result<Option<Snapshot>, EngineError> {
        let key = sn_key(stream_id);
        match self
            .db
            .get(key.as_bytes())
            .await
            .map_err(|e| to_snapshot_err(&e))?
        {
            None => Ok(None),
            Some(bytes) => {
                let snapshot: Snapshot = serde_json::from_slice(&bytes)
                    .map_err(|e| EngineError::snapshot(e.to_string()))?;
                Ok(Some(snapshot))
            }
        }
    }
}

// ── SlateDbInboxStore ─────────────────────────────────────────────────────────

/// Durable [`InboxStore`] backed by SlateDB with TTL support.
///
/// Key schema:
/// - `ib/{inbox_key}` → `""` — dedup sentinel (existence = seen)
/// - `it/{ts_nanos:016x}/{nonce_uuid}` → `"{inbox_key}"` — time-sorted index for TTL purge
///
/// The time index enables [`purge_expired`] to range-delete all entries older
/// than a configurable TTL window without scanning the full `ib/` namespace.
///
/// # Atomicity of `accept`
///
/// `accept` uses a **Serializable Snapshot Isolation (SSI) transaction**
/// (`IsolationLevel::SerializableSnapshot`) to eliminate the TOCTOU race
/// between the existence check and the write.
///
/// Protocol:
/// 1. Begin an SSI transaction. The transaction takes a snapshot of the
///    database at the current LSN.
/// 2. Read `ib/{key}` within the transaction. Under SSI, this registers the
///    key in the read set — any concurrent write to that key by another
///    transaction will cause a conflict on commit.
/// 3. If the key exists → return `false` (duplicate detected).
/// 4. Write `ib/{key}` and the time-index entry within the same transaction.
/// 5. Commit. On `ErrorKind::Transaction` (SSI conflict), retry from step 1.
///    The retry will observe the concurrent writer's commit and return `false`.
///
/// This makes `accept` **linearisable** across all concurrent callers — both
/// within a single `makod` instance (multiple Tokio tasks) and across multiple
/// `makod` instances sharing the same SlateDB storage path. No in-process
/// mutex is required.
///
/// The maximum retry count is bounded by `MAX_ACCEPT_RETRIES` (8). Exceeding
/// this limit (virtually impossible under realistic AS4 traffic) returns
/// `EngineError::inbox("accept conflict: too many retries")`.
///
/// # AS4 retry window
///
/// BDEW AS4 senders retry unacknowledged messages for up to 72 hours. Set the
/// TTL to at least 96 hours (72h + 24h safety margin) to survive the full
/// retry window across process restarts.
///
/// # TTL purge
///
/// Call [`purge_expired`] periodically (e.g. in a daily cron or on startup)
/// to reclaim storage. The `accept` method itself never purges.
///
/// Obtain via [`SlateDbStore::as_inbox_store`].
///
/// [`purge_expired`]: SlateDbInboxStore::purge_expired
/// [`accept`]: SlateDbInboxStore::accept
#[derive(Clone)]
pub struct SlateDbInboxStore {
    db: Db,
}

impl SlateDbInboxStore {
    /// Delete all inbox entries whose registration timestamp is older than
    /// `before`.
    ///
    /// Scans the `it/` time index up to `before`, deletes both the time-index
    /// entry and the corresponding `ib/` sentinel in a single `WriteBatch`
    /// per scanned entry.
    ///
    /// Recommended TTL: `OffsetDateTime::now_utc() - Duration::hours(96)`
    /// (72h AS4 retry window + 24h safety margin).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Inbox`] on storage failure.
    pub async fn purge_expired(&self, before: OffsetDateTime) -> Result<usize, EngineError> {
        let cutoff_nanos = u64::try_from(before.unix_timestamp_nanos().max(0)).unwrap_or(0);
        let mut iter = self.db.scan_prefix(b"it/").await.map_err(to_inbox_err)?;
        let mut purged = 0usize;
        // Collect all deletions into a single batch per 1000 entries to avoid
        // O(n) round-trips.
        let mut batch = WriteBatch::new();

        while let Some(kv) = iter.next().await.map_err(to_inbox_err)? {
            let key_str =
                std::str::from_utf8(&kv.key).map_err(|e| EngineError::inbox(e.to_string()))?;
            // key_str = "it/{16 hex}/{nonce_uuid}"
            let rest = match key_str.strip_prefix("it/") {
                Some(r) if r.len() >= 17 => r,
                _ => continue,
            };
            let entry_nanos = u64::from_str_radix(&rest[..16], 16).unwrap_or(u64::MAX);
            if entry_nanos >= cutoff_nanos {
                break;
            }
            // value = the original inbox key
            let inbox_key =
                std::str::from_utf8(&kv.value).map_err(|e| EngineError::inbox(e.to_string()))?;
            let ib_k = ib_key(inbox_key);
            batch.delete(kv.key.as_ref());
            batch.delete(ib_k.as_bytes());
            purged += 1;
            // Flush every 1000 deletions to bound memory usage.
            if purged % 1000 == 0 {
                self.db
                    .write(std::mem::replace(&mut batch, WriteBatch::new()))
                    .await
                    .map_err(to_inbox_err)?;
            }
        }
        // Flush any remaining deletions. Use WriteBatch::is_empty() rather than
        // `purged % 1000 != 0` to avoid the off-by-one where a final batch that
        // lands on an exact multiple of 1000 is silently skipped.
        if !batch.is_empty() {
            self.db.write(batch).await.map_err(to_inbox_err)?;
        }
        Ok(purged)
    }
}

impl InboxStore for SlateDbInboxStore {
    async fn accept(&self, key: &str) -> Result<bool, EngineError> {
        const MAX_ACCEPT_RETRIES: usize = 8;
        if key.len() > crate::inbox::MAX_INBOX_KEY_LEN {
            return Err(EngineError::inbox(format!(
                "inbox key is {} bytes, exceeds maximum of {}",
                key.len(),
                crate::inbox::MAX_INBOX_KEY_LEN,
            )));
        }
        let ib_k = ib_key(key);

        // Use a Serializable Snapshot Isolation (SSI) transaction to make
        // the existence-check + write atomic and linearisable — both within
        // a single process and across multiple concurrent `makod` instances
        // sharing the same SlateDB storage.
        //
        // Under SSI, reading `ib_k` registers it in the transaction's read
        // set. If a concurrent transaction commits a write to the same key
        // before we commit, SlateDB detects the conflict and returns
        // `ErrorKind::Transaction`. We retry; the next read will see the
        // committed write and return `false` (duplicate).
        //
        // A retry loop is safe because conflicts imply forward progress by a
        // competing acceptor; under bounded concurrency the loop terminates.
        for _attempt in 0..MAX_ACCEPT_RETRIES {
            let txn = self
                .db
                .begin(IsolationLevel::SerializableSnapshot)
                .await
                .map_err(to_inbox_err)?;

            if txn
                .get(ib_k.as_bytes())
                .await
                .map_err(to_inbox_err)?
                .is_some()
            {
                // Key already exists — duplicate message. The read itself is
                // sufficient; no need to commit the read-only transaction.
                txn.rollback();
                return Ok(false);
            }

            let now = OffsetDateTime::now_utc();
            let nonce = uuid::Uuid::new_v4().to_string();
            let time_key = it_key(now, &nonce);
            txn.put(ib_k.as_bytes(), b"").map_err(to_inbox_err)?;
            txn.put(time_key.as_bytes(), key.as_bytes())
                .map_err(to_inbox_err)?;

            match txn.commit().await {
                Ok(_) => return Ok(true),
                Err(e) if e.kind() == ErrorKind::Transaction => {
                    // Conflict: a concurrent acceptor committed to the same
                    // key between our read and our commit. Retry — the next
                    // iteration will observe the concurrent write and return
                    // false.
                }
                Err(e) => return Err(to_inbox_err(e)),
            }
        }
        Err(EngineError::inbox(
            "accept conflict: too many retries (concurrent accept storm)",
        ))
    }
}

// ── SlateDbDeadLetterSink ─────────────────────────────────────────────────────

/// Capacity of the in-memory dead-letter buffer.
///
/// 512 entries is far more than enough for any realistic rejection burst.
/// If the buffer fills (> 512 concurrent unprocessed rejections), new entries
/// are dropped with an `tracing::error!` log and a `channel_overflow` metric.
const DL_BUFFER_CAPACITY: usize = 512;

/// Internal entry queued from the synchronous `reject()` call to the async writer.
struct DlEntry {
    label: String,
    detail: String,
}

/// A [`DeadLetterSink`] that buffers rejections in a bounded in-memory queue
/// and persists them to SlateDB via a paired [`SlateDbDeadLetterWorker`] task.
///
/// ## Key improvement over the previous `rt.spawn()` approach
///
/// The previous implementation called `rt.spawn()` inside the synchronous
/// `reject()` method. If the Tokio runtime was shutting down at the exact
/// moment a dead-letter was recorded, `spawn()` would silently drop the future
/// and the DLQ entry would be permanently lost — violating BNetzA traceability
/// requirements.
///
/// This implementation uses a bounded `tokio::sync::mpsc` channel. `reject()`
/// is a non-blocking `try_send` into the buffer; the background
/// [`SlateDbDeadLetterWorker`] drains the buffer to SlateDB. Because the
/// worker is started at daemon startup (not inside `reject()`), it is immune
/// to the runtime-shutdown race.
///
/// ## Wiring
///
/// ```rust,no_run
/// # async fn example(store: mako_engine::store_slatedb::SlateDbStore) {
/// let (dl_sink, dl_worker) = store.as_dead_letter_sink();
/// let dl_handle = tokio::spawn(dl_worker.run());
///
/// // Wire dl_sink into EngineBuilder::with_dead_letter_sink(dl_sink).
///
/// // On graceful shutdown (after wait_for_shutdown()):
/// // 1. Call dl_sink.signal_shutdown() — closes the channel.
/// // 2. Await dl_handle — worker drains remaining entries and exits.
/// // 3. Call store.close_owned().
/// # }
/// ```
///
/// [`DeadLetterSink`]: crate::dead_letter::DeadLetterSink
#[derive(Clone)]
pub struct SlateDbDeadLetterSink {
    inner: std::sync::Arc<DlSinkInner>,
}

struct DlSinkInner {
    /// `None` after `signal_shutdown()` is called.
    tx: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<DlEntry>>>,
}

impl std::fmt::Debug for SlateDbDeadLetterSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let open = self.inner.tx.lock().map(|g| g.is_some()).unwrap_or(false);
        f.debug_struct("SlateDbDeadLetterSink")
            .field("channel_open", &open)
            .finish_non_exhaustive()
    }
}

impl SlateDbDeadLetterSink {
    /// Close the dead-letter channel and signal the background worker to drain
    /// remaining buffered entries and exit.
    ///
    /// After this call, `reject()` is a **silent no-op** — no new entries are
    /// queued. Call this during graceful shutdown **before** awaiting the
    /// [`SlateDbDeadLetterWorker`] handle and **before** `store.close_owned()`.
    pub fn signal_shutdown(&self) {
        // Dropping the Sender closes the mpsc channel.  The worker's `recv()`
        // loop will drain remaining entries and then return `None`, exiting cleanly.
        if let Ok(mut guard) = self.inner.tx.lock() {
            *guard = None;
        }
    }
}

impl crate::dead_letter::DeadLetterSink for SlateDbDeadLetterSink {
    fn reject(&self, reason: &crate::dead_letter::DeadLetterReason) {
        let label = reason.label();
        let detail = reason.to_string();

        // Metrics (sync, no I/O). Per-PID label for unknown-PID alerts.
        let metric_label: std::borrow::Cow<'static, str> = match reason {
            crate::dead_letter::DeadLetterReason::UnknownPid(pid) => {
                format!("unknown_pid:{pid}").into()
            }
            _ => label.into(),
        };
        crate::metrics::EngineMetrics::global().dead_letter_recorded(&metric_label);

        // Structured log (sync, never blocks).
        tracing::warn!(
            reason = label,
            detail = %detail,
            "dead letter: enqueueing for durable DLQ persistence",
        );

        // Non-blocking enqueue into the bounded buffer.
        let guard = self.inner.tx.lock().unwrap();
        let Some(tx) = &*guard else {
            // signal_shutdown() was called; silently discard.
            return;
        };
        if let Err(_full) = tx.try_send(DlEntry {
            label: label.to_owned(),
            detail,
        }) {
            tracing::error!(
                reason = label,
                capacity = DL_BUFFER_CAPACITY,
                "dead letter channel full; entry dropped — check for mass-rejection storm",
            );
            crate::metrics::EngineMetrics::global().dead_letter_recorded("channel_overflow");
        }
    }
}

/// Background task that persists dead-letter entries from the in-memory buffer
/// to SlateDB.
///
/// Obtained alongside [`SlateDbDeadLetterSink`] via
/// [`SlateDbStore::as_dead_letter_sink`].
///
/// ## Usage
///
/// ```rust,no_run
/// # async fn example(store: mako_engine::store_slatedb::SlateDbStore) {
/// let (dl_sink, dl_worker) = store.as_dead_letter_sink();
/// let dl_handle = tokio::spawn(dl_worker.run());
/// // ...
/// # }
/// ```
pub struct SlateDbDeadLetterWorker {
    rx: tokio::sync::mpsc::Receiver<DlEntry>,
    db: Db,
}

impl std::fmt::Debug for SlateDbDeadLetterWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlateDbDeadLetterWorker")
            .finish_non_exhaustive()
    }
}

impl SlateDbDeadLetterWorker {
    /// Run the background writer until the channel is closed.
    ///
    /// The channel is closed when [`SlateDbDeadLetterSink::signal_shutdown`] is
    /// called (or when all `SlateDbDeadLetterSink` clones are dropped). The
    /// worker drains any remaining buffered entries before returning.
    ///
    /// Returns the total number of entries persisted.
    pub async fn run(mut self) -> u64 {
        let mut count = 0u64;
        while let Some(entry) = self.rx.recv().await {
            Self::persist_entry(&self.db, entry).await;
            count += 1;
        }
        count
    }

    async fn persist_entry(db: &Db, entry: DlEntry) {
        let now = OffsetDateTime::now_utc();
        let uuid = uuid::Uuid::new_v4().to_string();
        let key = dr_key(now, &uuid);
        let record = DeadLetterRecord {
            rejected_at: now.to_string(),
            reason_label: entry.label,
            reason_detail: entry.detail,
        };
        if let Ok(bytes) = serde_json::to_vec(&record) {
            let mut batch = WriteBatch::new();
            batch.put(key.as_bytes(), &bytes);
            if let Err(e) = db.write(batch).await {
                tracing::error!(
                    error = %e,
                    "SlateDbDeadLetterWorker: failed to persist dead-letter entry; \
                     entry is lost — check storage health",
                );
            }
        }
    }
}

/// A structured record of a single rejected inbound message.
///
/// Serialised as JSON and stored under `dr/{ts_nanos:016x}/{uuid}`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DeadLetterRecord {
    /// ISO 8601 timestamp of rejection (UTC).
    pub rejected_at: String,
    /// Rejection category (maps to [`DeadLetterReason::label`]).
    ///
    /// [`DeadLetterReason::label`]: crate::dead_letter::DeadLetterReason::label
    pub reason_label: String,
    /// Human-readable description of the rejection.
    pub reason_detail: String,
}

fn dr_key(ts: OffsetDateTime, uuid: &str) -> String {
    // Consistent with ot_key / dt_key / it_key: use unix_timestamp_nanos() so
    // lexicographic order equals chronological order.  Pre-epoch timestamps are
    // clamped to zero (only relevant in tests; all regulatory messages are 2000+).
    let nanos = u64::try_from(ts.unix_timestamp_nanos().max(0)).unwrap_or(0);
    format!("dr/{nanos:016x}/{uuid}")
}

impl SlateDbStore {
    /// Create a dead-letter sink backed by a buffered async writer.
    ///
    /// Returns `(sink, worker)` where:
    /// - `sink` implements [`DeadLetterSink`] and is `Clone + Send + Sync`.
    /// - `worker` must be spawned: `tokio::spawn(worker.run())`.
    ///
    /// ## Graceful shutdown sequence
    ///
    /// ```text
    /// 1. wait_for_shutdown().await
    /// 2. dl_sink.signal_shutdown()       — closes the channel
    /// 3. dl_handle.await.ok()            — worker drains remaining entries
    /// 4. store.close_owned().await       — safe to close storage
    /// ```
    ///
    /// [`DeadLetterSink`]: crate::dead_letter::DeadLetterSink
    #[must_use]
    pub fn as_dead_letter_sink(&self) -> (SlateDbDeadLetterSink, SlateDbDeadLetterWorker) {
        let (tx, rx) = tokio::sync::mpsc::channel(DL_BUFFER_CAPACITY);
        let inner = std::sync::Arc::new(DlSinkInner {
            tx: std::sync::Mutex::new(Some(tx)),
        });
        (
            SlateDbDeadLetterSink { inner },
            SlateDbDeadLetterWorker {
                rx,
                db: self.db.clone(),
            },
        )
    }

    /// Return up to `limit` dead-letter records in **reverse-chronological**
    /// order (most-recent first).
    ///
    /// Scans the entire `dr/` key space (write-once, typically small).
    /// Suitable for operator dashboards and integration tests.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::DeadLetter`] on storage failures.
    pub async fn list_dead_letters(
        &self,
        limit: usize,
    ) -> Result<Vec<DeadLetterRecord>, EngineError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        // Keys sort chronologically; reverse to give most-recent-first ordering.
        let mut iter = self
            .db
            .scan_prefix(b"dr/")
            .await
            .map_err(|e| EngineError::dead_letter(slatedb_error_kind_str(&e)))?;

        let mut all: Vec<Vec<u8>> = Vec::new();
        while let Some(kv) = iter
            .next()
            .await
            .map_err(|e| EngineError::dead_letter(slatedb_error_kind_str(&e)))?
        {
            all.push(kv.value.to_vec());
        }

        all.reverse();
        all.truncate(limit);

        let mut records = Vec::with_capacity(all.len());
        for bytes in all {
            let record: DeadLetterRecord = serde_json::from_slice(&bytes)
                .map_err(|e| EngineError::dead_letter(e.to_string()))?;
            records.push(record);
        }

        Ok(records)
    }

    /// Return a [`SlateDbPartnerStore`] that shares the underlying database.
    #[must_use]
    pub fn as_partner_store(&self) -> SlateDbPartnerStore {
        SlateDbPartnerStore {
            db: self.db.clone(),
        }
    }
}

// ── SlateDbPartnerStore ───────────────────────────────────────────────────────

/// Durable [`crate::partner::PartnerStore`] backed by SlateDB.
///
/// Key schema:
/// - `pt/{tenant_id}/{gln}` → `JSON(PartnerRecord)`
///
/// `TenantId` is always a 36-character UUID and GLN is always 13 digits,
/// giving a fixed-width `pt/{36-chars}/{13-chars}` key that bounds efficient
/// per-tenant scans.
///
/// Upsert reads the existing record, merges via
/// [`crate::partner::PartnerRecord::merge_from_partin`], and writes the merged result within
/// the same snapshot-isolation transaction to prevent write races.
///
/// Obtain via [`SlateDbStore::as_partner_store`].
#[derive(Clone)]
pub struct SlateDbPartnerStore {
    db: Db,
}

fn pt_key(tenant_id: TenantId, gln: &crate::types::MarktpartnerCode) -> String {
    format!("pt/{tenant_id}/{gln}")
}

fn pt_tenant_prefix(tenant_id: TenantId) -> String {
    format!("pt/{tenant_id}/")
}

fn to_partner_err(e: &slatedb::Error) -> EngineError {
    tracing::error!(error = %e, "partner store error");
    if slatedb_error_is_transient(e) {
        EngineError::transient_partner(slatedb_error_kind_str(e))
    } else {
        EngineError::partner(slatedb_error_kind_str(e))
    }
}

impl crate::partner::PartnerStore for SlateDbPartnerStore {
    async fn upsert(
        &self,
        tenant_id: TenantId,
        record: &crate::partner::PartnerRecord,
    ) -> Result<(), EngineError> {
        const MAX_UPSERT_RETRIES: usize = 8;
        let key = pt_key(tenant_id, &record.gln);

        // Read-then-merge inside a serializable-snapshot transaction to
        // prevent two concurrent PARTIN upserts from racing.
        for _attempt in 0..MAX_UPSERT_RETRIES {
            let txn = self
                .db
                .begin(IsolationLevel::SerializableSnapshot)
                .await
                .map_err(|e| to_partner_err(&e))?;

            let merged = match txn
                .get(key.as_bytes())
                .await
                .map_err(|e| to_partner_err(&e))?
            {
                Some(bytes) => {
                    let mut existing: crate::partner::PartnerRecord =
                        serde_json::from_slice(&bytes)
                            .map_err(|e| EngineError::partner(e.to_string()))?;
                    existing.merge_from_partin(record.clone());
                    existing
                }
                None => record.clone(),
            };

            let v = serde_json::to_vec(&merged).map_err(|e| EngineError::partner(e.to_string()))?;
            txn.put(key.as_bytes(), v.as_slice())
                .map_err(|e| to_partner_err(&e))?;

            match txn.commit().await {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == ErrorKind::Transaction => {}
                Err(e) => return Err(to_partner_err(&e)),
            }
        }
        Err(EngineError::partner(
            "partner upsert conflict: too many retries (concurrent update storm)",
        ))
    }

    async fn get(
        &self,
        tenant_id: TenantId,
        gln: &crate::types::MarktpartnerCode,
    ) -> Result<Option<crate::partner::PartnerRecord>, EngineError> {
        let key = pt_key(tenant_id, gln);
        match self
            .db
            .get(key.as_bytes())
            .await
            .map_err(|e| to_partner_err(&e))?
        {
            None => Ok(None),
            Some(bytes) => {
                let record: crate::partner::PartnerRecord = serde_json::from_slice(&bytes)
                    .map_err(|e| EngineError::partner(e.to_string()))?;
                Ok(Some(record))
            }
        }
    }

    async fn remove(
        &self,
        tenant_id: TenantId,
        gln: &crate::types::MarktpartnerCode,
    ) -> Result<(), EngineError> {
        let key = pt_key(tenant_id, gln);
        let mut batch = WriteBatch::new();
        batch.delete(key.as_bytes());
        self.db.write(batch).await.map_err(|e| to_partner_err(&e))?;
        Ok(())
    }

    async fn list(
        &self,
        tenant_id: TenantId,
    ) -> Result<Vec<crate::partner::PartnerRecord>, EngineError> {
        let prefix = pt_tenant_prefix(tenant_id);
        let mut iter = self
            .db
            .scan_prefix(prefix.as_bytes())
            .await
            .map_err(|e| to_partner_err(&e))?;

        let mut results = Vec::new();
        while let Some(entry) = iter.next().await.map_err(|e| to_partner_err(&e))? {
            let record: crate::partner::PartnerRecord = serde_json::from_slice(&entry.value)
                .map_err(|e| EngineError::partner(e.to_string()))?;
            results.push(record);
        }
        Ok(results)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use time::{Duration, OffsetDateTime};
    // Trait methods are called via method syntax; the explicit imports ensure
    // the compiler resolves them in a `--no-default-features` context.
    #[allow(unused_imports)]
    use crate::{
        deadline::Deadline,
        envelope::NewEvent,
        event_store::{AtomicAppend, EventStore, ExpectedVersion},
        ids::{
            ConversationId, CorrelationId, EventId, OutboxMessageId, ProcessId, StreamId, TenantId,
        },
        inbox::InboxStore,
        outbox::{OutboxMessage, OutboxStore, PendingOutbox},
        registry::RegistryKey,
        version::WorkflowId,
    };

    ///  SlateDB's default isolation level is `Snapshot`, **not** SSI.
    ///
    /// This test documents the current upstream default so that any future
    /// upgrade that changes it will produce a visible failure and a prompt to
    /// re-evaluate whether the explicit `IsolationLevel::SerializableSnapshot`
    /// calls in this module are still necessary.
    #[test]
    fn isolation_level_default_is_snapshot_not_ssi() {
        assert_eq!(
            IsolationLevel::default(),
            IsolationLevel::Snapshot,
            "SlateDB changed IsolationLevel::default() — check whether explicit \
             SerializableSnapshot calls in store_slatedb.rs are still required",
        );
        // Double-check that SSI is a distinct, stronger variant.
        assert_ne!(
            IsolationLevel::SerializableSnapshot,
            IsolationLevel::Snapshot,
            "SerializableSnapshot and Snapshot must not be the same variant",
        );
    }

    ///  `slatedb_error_is_transient` and `EngineError::is_transient` must
    /// agree for all `ErrorKind` values we classify as transient.
    ///
    /// Typed constructors set `transient: true/false` directly. This test guards
    /// against the two sides drifting when new `ErrorKind` variants are added.
    #[test]
    fn slatedb_error_transient_classification_consistent() {
        use crate::error::EngineError;

        // Transient constructors.
        assert!(EngineError::transient_store("storage unavailable").is_transient());
        assert!(EngineError::transient_outbox("outbox unavailable").is_transient());
        assert!(EngineError::transient_deadline("deadline unavailable").is_transient());
        assert!(EngineError::transient_registry("registry unavailable").is_transient());
        assert!(EngineError::transient_inbox("inbox unavailable").is_transient());
        assert!(EngineError::transient_snapshot("snapshot unavailable").is_transient());
        assert!(EngineError::transient_partner("partner unavailable").is_transient());
        assert!(EngineError::transient_dead_letter("dead-letter unavailable").is_transient());

        // Permanent constructors.
        assert!(!EngineError::store("data integrity error").is_transient());
        assert!(!EngineError::outbox("outbox conflict").is_transient());
        assert!(!EngineError::dead_letter("serialization error").is_transient());

        // Always-transient / always-permanent variants.
        assert!(
            EngineError::Transport {
                endpoint: "http://test".into(),
                message: "connection refused".into(),
            }
            .is_transient()
        );
        assert!(
            !EngineError::PartnerUnknown {
                recipient: "0123456789012".into()
            }
            .is_transient()
        );
    }

    // ── Helpers ────────────────────────────────────────────────────────────
    // These are all called from `#[tokio::test]` functions; Rust's dead-code
    // analysis doesn't cross the async-test boundary for helper fns.
    async fn make_store() -> SlateDbStore {
        SlateDbStore::open_in_memory_with_label("test")
            .await
            .unwrap()
    }

    fn make_stream(name: &str) -> StreamId {
        StreamId::new(name)
    }

    fn make_event(seq_hint: u64) -> NewEvent {
        let wf = WorkflowId::new("test-workflow", "FV2025-10-01");
        NewEvent {
            correlation_id: CorrelationId::new(),
            causation_id: None,
            conversation_id: ConversationId::new(),
            process_id: ProcessId::new(),
            tenant_id: TenantId::new(),
            workflow_id: wf,
            event_type: format!("TestEvent{seq_hint}").into(),
            schema_version: 1,
            payload: serde_json::json!({ "n": seq_hint }),
        }
    }

    fn make_outbox_msg(deliver_in: Duration) -> OutboxMessage {
        let now = OffsetDateTime::now_utc();
        OutboxMessage {
            message_id: OutboxMessageId::new(),
            stream_id: StreamId::new("test/stream"),
            process_id: ProcessId::new(),
            tenant_id: TenantId::new(),
            correlation_id: CorrelationId::new(),
            conversation_id: ConversationId::new(),
            causation_event_id: EventId::new(),
            message_type: "APERAK".into(),
            recipient: "9900000000001".into(),
            payload: serde_json::json!({ "type": "APERAK" }),
            payload_schema: None,
            created_at: now,
            deliver_after: Some(now + deliver_in),
            attempt_count: 0,
        }
    }

    // ── Event store tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn append_load_round_trip() {
        let store = make_store().await;
        let stream = make_stream("test/round-trip");

        let events = vec![make_event(1), make_event(2), make_event(3)];
        let result = store
            .append(&stream, ExpectedVersion::NoStream, &events)
            .await
            .unwrap();

        assert_eq!(result.last_sequence, 3);
        assert_eq!(result.events.len(), 3);

        let loaded = store.load(&stream).await.unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].sequence_number, 1);
        assert_eq!(loaded[1].sequence_number, 2);
        assert_eq!(loaded[2].sequence_number, 3);
        assert_eq!(loaded[0].event_type.as_ref(), "TestEvent1");
        assert_eq!(loaded[2].event_type.as_ref(), "TestEvent3");
    }

    #[tokio::test]
    async fn version_conflict_on_concurrent_append() {
        let store = make_store().await;
        let stream = make_stream("test/conflict");

        // First append at NoStream succeeds.
        store
            .append(&stream, ExpectedVersion::NoStream, &[make_event(1)])
            .await
            .unwrap();

        // Second append at the same version must fail.
        let err = store
            .append(&stream, ExpectedVersion::Exact(0), &[make_event(2)])
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::VersionConflict {
                    expected: 0,
                    actual: 1
                }
            ),
            "expected VersionConflict, got {err:?}",
        );
    }

    /// Two tasks race to be the first writer on the same stream.
    /// Exactly one must succeed; the other must receive `VersionConflict`.
    /// The final stream version must be 1 (not 2 — only one event appended).
    #[tokio::test]
    async fn concurrent_race_produces_exactly_one_conflict() {
        use std::sync::Arc;
        let store = Arc::new(make_store().await);
        let stream = make_stream("test/race");

        // Seed version 0 so both tasks can target ExpectedVersion::Exact(0).
        store
            .append(&stream, ExpectedVersion::NoStream, &[make_event(0)])
            .await
            .unwrap();

        let s1 = Arc::clone(&store);
        let st1 = stream.clone();
        let s2 = Arc::clone(&store);
        let st2 = stream.clone();

        // Launch both tasks; they race to append at version 1 (Exact(1)).
        let t1 = tokio::spawn(async move {
            s1.append(&st1, ExpectedVersion::Exact(1), &[make_event(10)])
                .await
        });
        let t2 = tokio::spawn(async move {
            s2.append(&st2, ExpectedVersion::Exact(1), &[make_event(20)])
                .await
        });

        let r1 = t1.await.expect("task 1 panicked");
        let r2 = t2.await.expect("task 2 panicked");

        // Classify outcomes.
        let successes = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
        let conflicts = [&r1, &r2]
            .iter()
            .filter(|r| matches!(r, Err(EngineError::VersionConflict { .. })))
            .count();

        assert_eq!(
            successes, 1,
            "exactly one task should succeed; r1={r1:?}, r2={r2:?}"
        );
        assert_eq!(
            conflicts, 1,
            "exactly one task should get VersionConflict; r1={r1:?}, r2={r2:?}"
        );

        // Final version: initial seed (1 event) + the one successful append (1 event) = 2.
        assert_eq!(
            store.stream_version(&stream).await.unwrap(),
            2,
            "final stream version must be 2 (seed + one winner)",
        );
    }

    #[tokio::test]
    async fn stream_version_tracks_appends() {
        let store = make_store().await;
        let stream = make_stream("test/version");

        assert_eq!(store.stream_version(&stream).await.unwrap(), 0);

        store
            .append(&stream, ExpectedVersion::NoStream, &[make_event(1)])
            .await
            .unwrap();
        assert_eq!(store.stream_version(&stream).await.unwrap(), 1);

        store
            .append(
                &stream,
                ExpectedVersion::Exact(1),
                &[make_event(2), make_event(3)],
            )
            .await
            .unwrap();
        assert_eq!(store.stream_version(&stream).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn load_from_skips_earlier_events() {
        let store = make_store().await;
        let stream = make_stream("test/load-from");

        let events: Vec<NewEvent> = (1..=10).map(make_event).collect();
        store
            .append(&stream, ExpectedVersion::NoStream, &events)
            .await
            .unwrap();

        // Load from sequence 7 should return events 8, 9, 10.
        let tail = store.load_from(&stream, 7).await.unwrap();
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].sequence_number, 8);
        assert_eq!(tail[2].sequence_number, 10);
    }

    #[tokio::test]
    async fn fold_stream_streams_without_vec() {
        let store = make_store().await;
        let stream = make_stream("test/fold");

        let events: Vec<NewEvent> = (1..=5).map(make_event).collect();
        store
            .append(&stream, ExpectedVersion::NoStream, &events)
            .await
            .unwrap();

        let sum = store
            .fold_stream(&stream, 0, 0u64, |acc, env| Ok(acc + env.sequence_number))
            .await
            .unwrap();

        // 1 + 2 + 3 + 4 + 5 = 15
        assert_eq!(sum, 15);
    }

    #[tokio::test]
    async fn fold_stream_from_sequence_filters_correctly() {
        let store = make_store().await;
        let stream = make_stream("test/fold-from");

        let events: Vec<NewEvent> = (1..=5).map(make_event).collect();
        store
            .append(&stream, ExpectedVersion::NoStream, &events)
            .await
            .unwrap();

        // fold from sequence 3: should see events 4 and 5 only.
        let sum = store
            .fold_stream(&stream, 3, 0u64, |acc, env| Ok(acc + env.sequence_number))
            .await
            .unwrap();

        // 4 + 5 = 9
        assert_eq!(sum, 9);
    }

    #[tokio::test]
    async fn list_streams_with_prefix_filter() {
        let store = make_store().await;

        store
            .append(
                &make_stream("process/alpha"),
                ExpectedVersion::NoStream,
                &[make_event(1)],
            )
            .await
            .unwrap();
        store
            .append(
                &make_stream("process/beta"),
                ExpectedVersion::NoStream,
                &[make_event(1)],
            )
            .await
            .unwrap();
        store
            .append(
                &make_stream("other/gamma"),
                ExpectedVersion::NoStream,
                &[make_event(1)],
            )
            .await
            .unwrap();

        let process_streams = store.list_streams(Some("process/")).await.unwrap();
        assert_eq!(process_streams.len(), 2);
        assert!(
            process_streams
                .iter()
                .any(|s| s.as_str() == "process/alpha")
        );
        assert!(process_streams.iter().any(|s| s.as_str() == "process/beta"));

        let all_streams = store.list_streams(None).await.unwrap();
        assert_eq!(all_streams.len(), 3);
    }

    // ── Outbox store tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn pending_returns_due_messages_in_order() {
        let store = make_store().await;

        let past = make_outbox_msg(Duration::hours(-2));
        let now = make_outbox_msg(Duration::ZERO);
        let future = make_outbox_msg(Duration::hours(1));

        store
            .enqueue(&[past.clone(), now.clone(), future.clone()])
            .await
            .unwrap();

        let cutoff = OffsetDateTime::now_utc() + Duration::milliseconds(50);
        let pending = store.pending(10, cutoff).await.unwrap();

        assert_eq!(pending.len(), 2, "only past and now-due messages expected");
        // Oldest deliver_after first (sorted by ot/ key).
        assert!(
            pending[0].deliver_after <= pending[1].deliver_after,
            "pending messages must be in chronological order",
        );
        // Future message not returned.
        assert!(!pending.iter().any(|m| m.message_id == future.message_id));
    }

    #[tokio::test]
    async fn acknowledge_removes_message_and_time_index() {
        let store = make_store().await;
        let msg = make_outbox_msg(Duration::hours(-1));
        let id = msg.message_id;
        store.enqueue(&[msg]).await.unwrap();

        let before = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
        assert_eq!(before.len(), 1);

        store.acknowledge(id).await.unwrap();

        let after = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
        assert_eq!(
            after.len(),
            0,
            "message must not be visible after acknowledge"
        );
        assert_eq!(store.len().await.unwrap(), 0, "om/ entry must be deleted");
    }

    #[tokio::test]
    async fn reschedule_moves_time_index_and_increments_attempt_count() {
        let store = make_store().await;
        let msg = make_outbox_msg(Duration::hours(-1));
        let id = msg.message_id;
        store.enqueue(&[msg]).await.unwrap();

        // Move deliver_after 1 hour into the future.
        let future = OffsetDateTime::now_utc() + Duration::hours(1);
        store.reschedule(id, future).await.unwrap();

        // Message must no longer be visible as due.
        let pending = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
        assert_eq!(pending.len(), 0, "rescheduled message must not be due yet");

        // Fetch the raw message to verify attempt_count.
        let msg_key = om_key(&id);
        let bytes = store.db.get(msg_key.as_bytes()).await.unwrap().unwrap();
        let updated: OutboxMessage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(updated.attempt_count, 1);
        assert_eq!(updated.deliver_after, Some(future));
    }

    #[tokio::test]
    async fn append_with_outbox_is_atomic() {
        let store = make_store().await;
        let stream = make_stream("test/atomic");

        let event = make_event(1);
        let pending_outbox = PendingOutbox::new(
            "APERAK",
            "9900000000001",
            serde_json::json!({ "type": "APERAK" }),
        );

        store
            .append_with_outbox(
                &stream,
                ExpectedVersion::NoStream,
                &[event],
                &[pending_outbox],
            )
            .await
            .unwrap();

        // Both the event and the materialised outbox message must be visible.
        let events = store.load(&stream).await.unwrap();
        assert_eq!(events.len(), 1);

        let pending = store.pending(10, OffsetDateTime::now_utc()).await.unwrap();
        assert_eq!(pending.len(), 1);
        // Verify the outbox message has the correct causation linkage.
        assert_eq!(pending[0].causation_event_id, events[0].event_id);
        assert_eq!(pending[0].message_type.as_ref(), "APERAK");
    }

    #[tokio::test]
    async fn ot_key_pre_epoch_clamps_to_epoch() {
        // Negative timestamps must not wrap to near u64::MAX.
        let pre_epoch = OffsetDateTime::UNIX_EPOCH - Duration::hours(1);
        let id = OutboxMessageId::new();
        let key = ot_key(pre_epoch, &id);
        let epoch_key = ot_key(OffsetDateTime::UNIX_EPOCH, &id);
        // Both should sort at or before the epoch key (pre_epoch clamped to 0).
        assert!(
            key <= epoch_key,
            "pre-epoch key {key} must sort at or before epoch key {epoch_key}",
        );
    }

    // ── SlateDbDeadlineStore tests ────────────────────────────────────

    async fn make_deadline_store() -> (SlateDbStore, SlateDbDeadlineStore) {
        let s = SlateDbStore::open_in_memory_with_label("deadlines-test")
            .await
            .unwrap();
        let ds = s.as_deadline_store();
        (s, ds)
    }

    fn make_deadline(due_at: OffsetDateTime) -> Deadline {
        use crate::ids::ProcessId;
        Deadline::new(
            StreamId::new("process/test"),
            ProcessId::new(),
            TenantId::new(),
            WorkflowId::new("test-workflow", "FV2025-10-01"),
            "aperak-response-window",
            due_at,
        )
    }

    #[tokio::test]
    async fn deadline_register_and_cancel() {
        let (_, ds) = make_deadline_store().await;
        let d = make_deadline(OffsetDateTime::now_utc() + Duration::days(1));
        let id = d.deadline_id();

        ds.register(&d).await.unwrap();
        assert_eq!(ds.len().await.unwrap(), 1);

        ds.cancel(id).await.unwrap();
        assert_eq!(ds.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn deadline_due_now_only_returns_overdue() {
        let (_, ds) = make_deadline_store().await;
        let past = make_deadline(OffsetDateTime::now_utc() - Duration::seconds(1));
        let future = make_deadline(OffsetDateTime::now_utc() + Duration::days(5));

        ds.register(&past).await.unwrap();
        ds.register(&future).await.unwrap();

        let result = ds.due_now(100).await.unwrap();
        assert_eq!(result.deadlines.len(), 1);
        assert!(!result.has_more);
    }

    #[tokio::test]
    async fn deadline_due_now_has_more_when_over_limit() {
        let (_, ds) = make_deadline_store().await;
        let past = OffsetDateTime::now_utc() - Duration::seconds(1);
        for _ in 0..5 {
            ds.register(&make_deadline(past)).await.unwrap();
        }

        let r = ds.due_now(3).await.unwrap();
        assert_eq!(r.deadlines.len(), 3);
        assert!(r.has_more);

        let r2 = ds.due_now(10).await.unwrap();
        assert_eq!(r2.deadlines.len(), 5);
        assert!(!r2.has_more);
    }

    #[tokio::test]
    async fn deadline_for_stream_filters_correctly() {
        let (_, ds) = make_deadline_store().await;
        let stream1 = StreamId::new("process/aaa");
        let stream2 = StreamId::new("process/bbb");

        let wf = WorkflowId::new("test-workflow", "FV2025-10-01");
        let d1 = Deadline::new(
            stream1.clone(),
            ProcessId::new(),
            TenantId::new(),
            wf.clone(),
            "label",
            OffsetDateTime::now_utc() + Duration::days(1),
        );
        let d2 = Deadline::new(
            stream2.clone(),
            ProcessId::new(),
            TenantId::new(),
            wf,
            "label",
            OffsetDateTime::now_utc() + Duration::days(1),
        );
        ds.register(&d1).await.unwrap();
        ds.register(&d2).await.unwrap();

        let for1 = ds.for_stream(&stream1).await.unwrap();
        assert_eq!(for1.len(), 1);
        assert_eq!(for1[0].stream_id(), &stream1);

        let for2 = ds.for_stream(&stream2).await.unwrap();
        assert_eq!(for2.len(), 1);
        assert_eq!(for2[0].stream_id(), &stream2);
    }

    #[tokio::test]
    async fn deadline_register_upserts_same_id() {
        use crate::ids::ProcessId;
        let (_, ds) = make_deadline_store().await;
        // Register two distinct deadlines; the total should be 2 (not a collision).
        let d1 = make_deadline(OffsetDateTime::now_utc() + Duration::days(5));
        let d2 = make_deadline(OffsetDateTime::now_utc() + Duration::days(10));
        ds.register(&d1).await.unwrap();
        ds.register(&d2).await.unwrap();
        assert_eq!(ds.len().await.unwrap(), 2);
        // Re-registering the same deadline (upsert) must not grow the count.
        ds.register(&d1).await.unwrap();
        // Payload is overwritten; time-index gets an extra entry for the old key.
        // The payload count stays the same (old + new time-index entries both exist,
        // but both payload keys map to the same deadline_id). In practice the test
        // verifies that re-registering d1 doesn't corrupt the store.
        let _ = ProcessId::new(); // suppress unused import warning
        assert_eq!(
            ds.len().await.unwrap(),
            2,
            "upsert must not grow the payload count"
        );
    }

    // ── SlateDbProcessRegistry tests ──────────────────────────────────

    fn make_reg_store(base: &SlateDbStore) -> SlateDbProcessRegistry {
        base.as_process_registry()
    }

    fn make_identity() -> ProcessIdentity {
        use crate::{ids::ProcessId, version::WorkflowId};
        ProcessIdentity::new(
            ProcessId::new(),
            TenantId::new(),
            WorkflowId::new("test", "FV2024-10-01"),
        )
    }

    #[tokio::test]
    async fn registry_register_and_lookup() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let tenant = TenantId::new();
        let key = RegistryKey::parse("conv:abc").expect("valid key");
        let id = make_identity();

        reg.register(tenant, &key, id.clone()).await.unwrap();
        let found = reg
            .lookup(tenant, &key)
            .await
            .unwrap()
            .expect("must be found");
        assert_eq!(found.process_id, id.process_id);
    }

    #[tokio::test]
    async fn registry_lookup_returns_none_for_unknown() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        assert!(
            reg.lookup(
                TenantId::new(),
                &RegistryKey::parse("nope").expect("valid key")
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[tokio::test]
    async fn registry_remove_clears_mapping() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let tenant = TenantId::new();
        let key = RegistryKey::parse("k1").expect("valid key");

        reg.register(tenant, &key, make_identity()).await.unwrap();
        reg.remove(tenant, &key).await.unwrap();
        assert!(reg.lookup(tenant, &key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn registry_upsert_overwrites_existing() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let tenant = TenantId::new();
        let key = RegistryKey::parse("k1").expect("valid key");
        let id1 = make_identity();
        let id2 = make_identity();

        reg.register(tenant, &key, id1).await.unwrap();
        reg.register(tenant, &key, id2.clone()).await.unwrap();

        let found = reg.lookup(tenant, &key).await.unwrap().unwrap();
        assert_eq!(found.process_id, id2.process_id);
        assert_eq!(reg.len().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn registry_tenant_isolation() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let t1 = TenantId::new();
        let t2 = TenantId::new();
        let key = RegistryKey::parse("k1").expect("valid key");

        reg.register(t1, &key, make_identity()).await.unwrap();
        assert!(reg.contains(t1, &key).await.unwrap());
        assert!(!reg.contains(t2, &key).await.unwrap());
    }

    // ── Correlated-process index tests ────────────────────────────────

    #[tokio::test]
    async fn correlated_register_and_lookup() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let tenant = TenantId::new();
        let id1 = make_identity();
        let id2 = make_identity();

        reg.register_correlated(tenant, "DE0001234567890", id1.process_id, id1.clone())
            .await
            .unwrap();
        reg.register_correlated(tenant, "DE0001234567890", id2.process_id, id2.clone())
            .await
            .unwrap();

        let mut results = reg
            .lookup_correlated(tenant, "DE0001234567890")
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        results.sort_by_key(|id| id.process_id.to_string());
        let mut expected = vec![id1.process_id, id2.process_id];
        expected.sort_by_key(std::string::ToString::to_string);
        assert_eq!(
            results.iter().map(|id| id.process_id).collect::<Vec<_>>(),
            expected,
        );
    }

    #[tokio::test]
    async fn correlated_lookup_empty_for_unknown_tag() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let results = reg
            .lookup_correlated(TenantId::new(), "unknown-malo")
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn correlated_remove_single_entry() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let tenant = TenantId::new();
        let id1 = make_identity();
        let id2 = make_identity();

        reg.register_correlated(tenant, "malo-1", id1.process_id, id1.clone())
            .await
            .unwrap();
        reg.register_correlated(tenant, "malo-1", id2.process_id, id2.clone())
            .await
            .unwrap();
        reg.remove_correlated(tenant, "malo-1", id1.process_id)
            .await
            .unwrap();

        let results = reg.lookup_correlated(tenant, "malo-1").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].process_id, id2.process_id);
    }

    #[tokio::test]
    async fn correlated_tenant_isolation() {
        let (store, _) = make_deadline_store().await;
        let reg = make_reg_store(&store);
        let t1 = TenantId::new();
        let t2 = TenantId::new();
        let id = make_identity();

        reg.register_correlated(t1, "shared-tag", id.process_id, id.clone())
            .await
            .unwrap();

        assert_eq!(
            reg.lookup_correlated(t1, "shared-tag").await.unwrap().len(),
            1
        );
        assert_eq!(
            reg.lookup_correlated(t2, "shared-tag").await.unwrap().len(),
            0
        );
    }

    // ── SlateDbInboxStore tests ───────────────────────────────────────

    fn make_inbox_store(base: &SlateDbStore) -> SlateDbInboxStore {
        base.as_inbox_store()
    }

    #[tokio::test]
    async fn inbox_new_message_accepted() {
        let (store, _) = make_deadline_store().await;
        let inbox = make_inbox_store(&store);
        assert!(inbox.accept("sender:ref-001").await.unwrap());
    }

    #[tokio::test]
    async fn inbox_duplicate_rejected() {
        let (store, _) = make_deadline_store().await;
        let inbox = make_inbox_store(&store);
        assert!(inbox.accept("sender:ref-001").await.unwrap());
        assert!(!inbox.accept("sender:ref-001").await.unwrap());
    }

    #[tokio::test]
    async fn inbox_different_senders_same_ref_are_independent() {
        use crate::inbox::inbox_key;
        let (store, _) = make_deadline_store().await;
        let inbox = make_inbox_store(&store);
        assert!(
            inbox
                .accept(&inbox_key("sender-A", "ref-001").unwrap())
                .await
                .unwrap()
        );
        assert!(
            inbox
                .accept(&inbox_key("sender-B", "ref-001").unwrap())
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn inbox_purge_expired_removes_old_entries() {
        let (store, _) = make_deadline_store().await;
        let inbox = make_inbox_store(&store);

        inbox.accept("sender:old-ref").await.unwrap();

        // Purge everything before well into the future.
        let far_future = OffsetDateTime::now_utc() + Duration::hours(200);
        let purged = inbox.purge_expired(far_future).await.unwrap();
        assert_eq!(purged, 1, "one entry should have been purged");

        // The key must be gone — it should be accepted as new now.
        assert!(
            inbox.accept("sender:old-ref").await.unwrap(),
            "key should be gone after purge"
        );
    }

    // ── ProjectionCheckpointStore tests ───────────────────────────────────────

    #[tokio::test]
    async fn projection_checkpoint_round_trip() {
        use crate::projection::{GlobalProjectionCheckpoint, ProjectionCheckpointStore};

        let store = make_store().await;

        // Loading a non-existent checkpoint returns an empty one (all zeros).
        let empty = store.load_projection_checkpoint("billing").await.unwrap();
        assert_eq!(empty.cursor_for(&StreamId::new("any")), 0);

        // Save a checkpoint with two cursor entries.
        let mut cp = GlobalProjectionCheckpoint::new();
        cp.advance(&StreamId::new("process/t1/p1"), 42);
        cp.advance(&StreamId::new("process/t1/p2"), 7);
        store
            .save_projection_checkpoint("billing", &cp)
            .await
            .unwrap();

        // Reload and verify cursors are preserved.
        let loaded = store.load_projection_checkpoint("billing").await.unwrap();
        assert_eq!(loaded.cursor_for(&StreamId::new("process/t1/p1")), 42);
        assert_eq!(loaded.cursor_for(&StreamId::new("process/t1/p2")), 7);
        assert_eq!(loaded.cursor_for(&StreamId::new("process/t1/p3")), 0);
    }

    #[tokio::test]
    async fn projection_checkpoints_are_independent_by_name() {
        use crate::projection::{GlobalProjectionCheckpoint, ProjectionCheckpointStore};

        let store = make_store().await;

        let mut cp_a = GlobalProjectionCheckpoint::new();
        cp_a.advance(&StreamId::new("s1"), 10);
        store
            .save_projection_checkpoint("proj-a", &cp_a)
            .await
            .unwrap();

        let mut cp_b = GlobalProjectionCheckpoint::new();
        cp_b.advance(&StreamId::new("s1"), 99);
        store
            .save_projection_checkpoint("proj-b", &cp_b)
            .await
            .unwrap();

        let loaded_a = store.load_projection_checkpoint("proj-a").await.unwrap();
        let loaded_b = store.load_projection_checkpoint("proj-b").await.unwrap();
        assert_eq!(loaded_a.cursor_for(&StreamId::new("s1")), 10);
        assert_eq!(loaded_b.cursor_for(&StreamId::new("s1")), 99);
    }

    #[tokio::test]
    async fn advance_projection_cursors_only_writes_changed_streams() {
        use crate::projection::{GlobalProjectionCheckpoint, ProjectionCheckpointStore};

        let store = make_store().await;

        // Save an initial checkpoint with two streams.
        let mut prev = GlobalProjectionCheckpoint::new();
        prev.advance(&StreamId::new("p/a"), 10);
        prev.advance(&StreamId::new("p/b"), 20);
        store
            .save_projection_checkpoint("adv-test", &prev)
            .await
            .unwrap();

        // Only stream "p/b" advances; "p/a" stays at 10.
        let mut curr = prev.clone();
        curr.advance(&StreamId::new("p/b"), 30);
        store
            .advance_projection_cursors("adv-test", &prev, &curr)
            .await
            .unwrap();

        // After advance, both cursors should be visible and correct.
        let loaded = store.load_projection_checkpoint("adv-test").await.unwrap();
        assert_eq!(loaded.cursor_for(&StreamId::new("p/a")), 10);
        assert_eq!(loaded.cursor_for(&StreamId::new("p/b")), 30);
    }
}
