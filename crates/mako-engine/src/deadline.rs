//! Deadline tracking for regulatory process timers.
//!
//! Every MaKo process is subject to hard regulatory deadlines defined in the
//! BDEW Application Handbooks. Deadline semantics vary by process family:
//!
//! | Process family | Deadline unit | Helper |
//! |---|---|---|
//! | GPKE Lieferantenwechsel (BK6-22-024) | 24 wall-clock hours | [`fristen::add_hours`] |
//! | WiM / GeLi Gas / MABIS | Werktage | [`fristen::add_werktage`] |
//!
//! Use the helpers in [`crate::fristen`] to compute the correct `due_at`
//! timestamp before constructing a [`Deadline`].
//!
//! The `DeadlineStore` persists these timers per process stream. A background
//! scheduler polls [`DeadlineStore::due_now`] and dispatches a
//! `TimeoutDeadline` command to the owning process when a deadline lapses.
//! The process workflow then handles the command — e.g. by escalating the
//! case or switching to a failure path.
//!
//! # Usage
//!
//! ```rust,ignore
//! use mako_engine::fristen;
//!
//! // GPKE 24h Lieferantenwechsel (BK6-22-024):
//! let due = fristen::add_hours(OffsetDateTime::now_utc(), 24);
//! // WiM 5-Werktage confirmation window:
//! let due = fristen::add_werktage(OffsetDateTime::now_utc().date(), 5,
//!     fristen::HolidayCalendar::BdewMaKo).midnight().assume_utc();
//!
//! let deadline = Deadline::new(
//!     process.stream_id().clone(),
//!     process.process_id(),
//!     process.tenant_id(),
//!     process.workflow_id().clone(),
//!     "aperak-response-window",
//!     due,
//! );
//! deadline_store.register(&deadline).await?;
//!
//! // When the counterparty responds in time, cancel the deadline:
//! deadline_store.cancel(deadline.deadline_id()).await?;
//!
//! // Background scheduler (runs every N minutes):
//! let result = deadline_store.due_now(100).await?;
//! for d in result.deadlines {
//!     process_handle.execute(TimeoutDeadline { deadline_id: d.deadline_id() }).await?;
//!     deadline_store.cancel(d.deadline_id()).await?;
//! }
//! ```
//!
//! [`fristen::add_hours`]: crate::fristen::add_hours
//! [`fristen::add_werktage`]: crate::fristen::add_werktage

use std::sync::Arc;

#[cfg(any(test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;

use time::OffsetDateTime;

use crate::{
    error::EngineError,
    ids::{DeadlineId, ProcessId, StreamId, TenantId},
    version::WorkflowId,
};

// ── Deadline ──────────────────────────────────────────────────────────────────

/// A registered regulatory deadline for a single process stream.
///
/// Create with [`Deadline::new`], persist via [`DeadlineStore::register`], and
/// cancel via [`DeadlineStore::cancel`] when the process advances past the
/// deadline before it fires.
///
/// The `label` field identifies the deadline type (e.g.
/// `"aperak-response-window"`) and is used by the scheduler to dispatch the
/// correct timeout command.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Deadline {
    /// Unique identifier for this deadline entry.
    deadline_id: DeadlineId,

    /// The process stream this deadline belongs to.
    stream_id: StreamId,

    /// The process instance this deadline belongs to.
    process_id: ProcessId,

    /// The tenant that owns this process.
    tenant_id: TenantId,

    /// The workflow that owns this process (name + format version).
    ///
    /// Stored so the deadline scheduler can reconstruct a [`ProcessIdentity`]
    /// and route the `TimeoutExpired` command to the correct workflow type
    /// without a separate registry lookup.
    ///
    /// [`ProcessIdentity`]: crate::ids::ProcessIdentity
    workflow_id: WorkflowId,

    /// Human-readable label identifying the deadline type.
    label: Box<str>,

    /// When this deadline expires.
    due_at: OffsetDateTime,

    /// When this deadline was registered.
    created_at: OffsetDateTime,
}

impl Deadline {
    /// Construct a new deadline.
    ///
    /// `deadline_id` and `created_at` are generated automatically.
    ///
    /// `workflow_id` must match the [`WorkflowId`] under which the owning
    /// process was started (i.e. `process.workflow_id().clone()`). The
    /// deadline scheduler uses it to reconstruct a [`ProcessIdentity`] and
    /// route the `TimeoutExpired` command to the correct workflow type.
    ///
    /// [`ProcessIdentity`]: crate::ids::ProcessIdentity
    #[must_use]
    pub fn new(
        stream_id: StreamId,
        process_id: ProcessId,
        tenant_id: TenantId,
        workflow_id: WorkflowId,
        label: impl Into<Box<str>>,
        due_at: OffsetDateTime,
    ) -> Self {
        Self {
            deadline_id: DeadlineId::new(),
            stream_id,
            process_id,
            tenant_id,
            workflow_id,
            label: label.into(),
            due_at,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// Return `true` when this deadline has passed relative to `now`.
    ///
    /// ```rust
    /// use mako_engine::deadline::Deadline;
    /// use mako_engine::ids::{ProcessId, StreamId, TenantId};
    /// use mako_engine::version::WorkflowId;
    /// use time::{Duration, OffsetDateTime};
    ///
    /// let past = Deadline::new(
    ///     StreamId::new("process/x"),
    ///     ProcessId::new(),
    ///     TenantId::new(),
    ///     WorkflowId::new("gpke-supplier-change", "FV2025-10-01"),
    ///     "aperak-response-window",
    ///     OffsetDateTime::now_utc() - Duration::seconds(1),
    /// );
    /// assert!(past.is_due(OffsetDateTime::now_utc()));
    /// ```
    #[must_use]
    pub fn is_due(&self, now: OffsetDateTime) -> bool {
        self.due_at <= now
    }

    /// The unique identifier of this deadline.
    #[must_use]
    pub fn deadline_id(&self) -> DeadlineId {
        self.deadline_id
    }

    /// The stream this deadline belongs to.
    #[must_use]
    pub fn stream_id(&self) -> &StreamId {
        &self.stream_id
    }

    /// The process instance this deadline belongs to.
    #[must_use]
    pub fn process_id(&self) -> ProcessId {
        self.process_id
    }

    /// The tenant that owns this process.
    #[must_use]
    pub fn tenant_id(&self) -> TenantId {
        self.tenant_id
    }

    /// The workflow that owns this process.
    ///
    /// Used by the deadline scheduler to reconstruct a [`ProcessIdentity`]
    /// and route the `TimeoutExpired` command to the correct workflow type.
    ///
    /// [`ProcessIdentity`]: crate::ids::ProcessIdentity
    #[must_use]
    pub fn workflow_id(&self) -> &WorkflowId {
        &self.workflow_id
    }

    /// The human-readable label identifying the deadline type (e.g.
    /// `"aperak-response-window"`).
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// When this deadline expires.
    #[must_use]
    pub fn due_at(&self) -> OffsetDateTime {
        self.due_at
    }

    /// When this deadline was registered.
    #[must_use]
    pub fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }
}

// ── DueNowResult ──────────────────────────────────────────────────────────────

/// Result of a [`DeadlineStore::due_now`] poll.
///
/// When `has_more` is `true`, the store has additional expired deadlines beyond
/// the returned `deadlines`. The scheduler should drain in a loop until
/// `has_more` is `false` to avoid leaving unfired deadlines in the store.
///
/// ```rust
/// # tokio_test::block_on(async {
/// # use mako_engine::deadline::{InMemoryDeadlineStore, DeadlineStore, Deadline};
/// # use mako_engine::ids::{ProcessId, StreamId, TenantId};
/// # use time::OffsetDateTime;
/// let store = InMemoryDeadlineStore::new();
/// loop {
///     let result = store.due_now(50).await.unwrap();
///     for deadline in result.deadlines {
///         // dispatch TimeoutDeadline command …
///         store.cancel(deadline.deadline_id()).await.unwrap();
///     }
///     if !result.has_more { break; }
/// }
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct DueNowResult {
    /// Expired deadlines, ordered soonest-first.
    pub deadlines: Vec<Deadline>,
    /// `true` when the store contains more expired deadlines beyond `deadlines`.
    pub has_more: bool,
}

// ── DeadlineStore ─────────────────────────────────────────────────────────────

/// Storage contract for process deadlines.
///
/// ## Scheduler contract
///
/// A background timer task should poll this store periodically:
///
/// 1. Call [`DeadlineStore::due_now`] to retrieve expired deadlines.
/// 2. Dispatch a `TimeoutDeadline` command to each owning process.
/// 3. Call [`DeadlineStore::cancel`] to remove the fired deadline.
///
/// Cancelling a deadline before the scheduler fires it prevents a spurious
/// `TimeoutDeadline` command from being dispatched to the process. Always
/// cancel deadlines when the process advances past them naturally (e.g. when
/// the expected counterparty response arrives in time).
///
/// ## Blanket `Arc` implementation
///
/// `Arc<S>` implements `DeadlineStore` whenever `S: DeadlineStore`, enabling
/// shared access from both the scheduler and command handlers.
#[allow(async_fn_in_trait)]
pub trait DeadlineStore: Send + Sync {
    /// Register a new deadline.
    ///
    /// Upserts by `deadline_id`: if a deadline with the same ID already
    /// exists it is replaced.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Deadline`] on storage failure.
    async fn register(&self, deadline: &Deadline) -> Result<(), EngineError>;

    /// Cancel a registered deadline by ID.
    ///
    /// No-op when the deadline does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Deadline`] on storage failure.
    async fn cancel(&self, id: DeadlineId) -> Result<(), EngineError>;

    /// Return up to `limit` deadlines whose `due_at <= now_utc()`, ordered
    /// soonest-first.
    ///
    /// When the store contains more expired deadlines than `limit`, the
    /// returned [`DueNowResult::has_more`] is `true`. Callers should drain
    /// in a loop until `has_more` is `false`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Deadline`] on storage failure.
    async fn due_now(&self, limit: usize) -> Result<DueNowResult, EngineError>;

    /// Return all active deadlines for `stream_id`, in registration order.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Deadline`] on storage failure.
    async fn for_stream(&self, stream_id: &StreamId) -> Result<Vec<Deadline>, EngineError>;

    /// Total number of registered deadlines.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Deadline`] on storage failure.
    async fn len(&self) -> Result<usize, EngineError>;

    /// Return `true` when no deadlines are registered.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Deadline`] on storage failure.
    async fn is_empty(&self) -> Result<bool, EngineError> {
        Ok(self.len().await? == 0)
    }

    /// Count deadlines whose `due_at ≤ now` that have not yet been cancelled.
    ///
    /// Indicates scheduler lag: a non-zero value means `TimeoutExpired` commands
    /// are not being dispatched in time, which is a compliance violation.
    ///
    /// The default implementation delegates to [`due_now`] with a limit of
    /// 10 000; if there are more overdue deadlines, returns 10 000 (capped).
    /// Implementations can override for a more efficient point-count query.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Deadline`] on storage failure.
    ///
    /// [`due_now`]: DeadlineStore::due_now
    async fn overdue_count(&self) -> Result<usize, EngineError> {
        const LIMIT: usize = 10_000;
        let result = self.due_now(LIMIT).await?;
        Ok(if result.has_more {
            LIMIT
        } else {
            result.deadlines.len()
        })
    }
}

// ── Arc<S> blanket impl ───────────────────────────────────────────────────────

impl<S: DeadlineStore> DeadlineStore for Arc<S> {
    async fn register(&self, deadline: &Deadline) -> Result<(), EngineError> {
        self.as_ref().register(deadline).await
    }

    async fn cancel(&self, id: DeadlineId) -> Result<(), EngineError> {
        self.as_ref().cancel(id).await
    }

    async fn due_now(&self, limit: usize) -> Result<DueNowResult, EngineError> {
        self.as_ref().due_now(limit).await
    }

    async fn for_stream(&self, stream_id: &StreamId) -> Result<Vec<Deadline>, EngineError> {
        self.as_ref().for_stream(stream_id).await
    }

    async fn len(&self) -> Result<usize, EngineError> {
        self.as_ref().len().await
    }

    async fn overdue_count(&self) -> Result<usize, EngineError> {
        self.as_ref().overdue_count().await
    }
}

// ── NoopDeadlineStore ─────────────────────────────────────────────────────────

/// A [`DeadlineStore`] that never persists anything.
///
/// `register` succeeds silently; `due_now` always returns an empty list.
/// Use this as the default when deadline tracking is not needed.
///
/// # ⚠️ Silent deadline loss
///
/// `NoopDeadlineStore` **discards every deadline registration silently**. No
/// scheduler timeout will ever fire. Missed deadlines are a compliance
/// violation under BNetzA monitoring. Do not use in production.
///
/// This type is available in all build configurations so it can serve as a
/// default type parameter in [`EngineBuilder`]. However, [`EngineBuilder::new`]
/// (which wires this as the default) is only available with the `testing`
/// feature or in `cfg(test)`. Production binaries must call
/// [`EngineBuilder::with_stores`] instead.
///
/// [`EngineBuilder`]: crate::builder::EngineBuilder
/// [`EngineBuilder::new`]: crate::builder::EngineBuilder::new
/// [`EngineBuilder::with_stores`]: crate::builder::EngineBuilder::with_stores
#[derive(Debug, Clone, Copy, Default)]
#[must_use = "NoopDeadlineStore discards all deadlines silently — use a persistent DeadlineStore in production"]
pub struct NoopDeadlineStore;

#[cfg(any(test, feature = "testing"))]
impl DeadlineStore for NoopDeadlineStore {
    async fn register(&self, _deadline: &Deadline) -> Result<(), EngineError> {
        Ok(())
    }

    async fn cancel(&self, _id: DeadlineId) -> Result<(), EngineError> {
        Ok(())
    }

    async fn due_now(&self, _limit: usize) -> Result<DueNowResult, EngineError> {
        Ok(DueNowResult {
            deadlines: Vec::new(),
            has_more: false,
        })
    }

    async fn for_stream(&self, _stream_id: &StreamId) -> Result<Vec<Deadline>, EngineError> {
        Ok(Vec::new())
    }

    async fn len(&self) -> Result<usize, EngineError> {
        Ok(0)
    }
}

// ── InMemoryDeadlineStore ─────────────────────────────────────────────────────

/// An in-memory [`DeadlineStore`] for tests and development.
///
/// Backed by a `HashMap` protected by a `Mutex`. Cloning shares the
/// underlying data via `Arc` — all clones see the same deadlines.
///
/// **Not production-safe.** Use this for:
/// - Unit and integration tests
/// - Examples and local development
/// - Verifying the scheduler loop without an external timer service
///
/// Only available in `#[cfg(test)]` or with the `testing` feature enabled.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default, Clone)]
pub struct InMemoryDeadlineStore {
    inner: Arc<RwLock<HashMap<DeadlineId, Deadline>>>,
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryDeadlineStore {
    /// Create an empty deadline store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return `true` when no deadlines are registered.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

#[cfg(any(test, feature = "testing"))]
impl DeadlineStore for InMemoryDeadlineStore {
    async fn register(&self, deadline: &Deadline) -> Result<(), EngineError> {
        self.inner
            .write()
            .await
            .insert(deadline.deadline_id, deadline.clone());
        Ok(())
    }

    async fn cancel(&self, id: DeadlineId) -> Result<(), EngineError> {
        self.inner.write().await.remove(&id);
        Ok(())
    }

    async fn due_now(&self, limit: usize) -> Result<DueNowResult, EngineError> {
        let now = OffsetDateTime::now_utc();
        let map = self.inner.read().await;
        let mut due: Vec<_> = map.values().filter(|d| d.is_due(now)).cloned().collect();
        // Soonest-first: the scheduler processes the most urgent deadlines first.
        due.sort_by_key(|d| d.due_at);
        // Probe one extra to detect whether more remain.
        let has_more = due.len() > limit;
        due.truncate(limit);
        Ok(DueNowResult {
            deadlines: due,
            has_more,
        })
    }

    async fn for_stream(&self, stream_id: &StreamId) -> Result<Vec<Deadline>, EngineError> {
        let map = self.inner.read().await;
        Ok(map
            .values()
            .filter(|d| &d.stream_id == stream_id)
            .cloned()
            .collect())
    }

    async fn len(&self) -> Result<usize, EngineError> {
        Ok(self.inner.read().await.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn make_deadline(due_at: OffsetDateTime) -> Deadline {
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
    async fn register_and_cancel() {
        let store = InMemoryDeadlineStore::new();
        let d = make_deadline(OffsetDateTime::now_utc() + Duration::days(5));
        let id = d.deadline_id;

        store.register(&d).await.unwrap();
        assert_eq!(store.len().await.unwrap(), 1);

        store.cancel(id).await.unwrap();
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn due_now_only_returns_overdue() {
        let store = InMemoryDeadlineStore::new();
        let past = make_deadline(OffsetDateTime::now_utc() - Duration::seconds(1));
        let future = make_deadline(OffsetDateTime::now_utc() + Duration::days(5));

        store.register(&past).await.unwrap();
        store.register(&future).await.unwrap();

        let due = store.due_now(100).await.unwrap();
        assert_eq!(due.deadlines.len(), 1);
        assert_eq!(due.deadlines[0].label.as_ref(), "aperak-response-window");
        assert!(!due.has_more);
    }

    #[tokio::test]
    async fn due_now_ordered_soonest_first() {
        let store = InMemoryDeadlineStore::new();
        let t1 = OffsetDateTime::now_utc() - Duration::seconds(60);
        let t2 = OffsetDateTime::now_utc() - Duration::seconds(10);
        let t3 = OffsetDateTime::now_utc() - Duration::seconds(1);

        // Register out of order to verify sorting.
        store.register(&make_deadline(t3)).await.unwrap();
        store.register(&make_deadline(t1)).await.unwrap();
        store.register(&make_deadline(t2)).await.unwrap();

        let due = store.due_now(10).await.unwrap();
        assert_eq!(due.deadlines.len(), 3);
        assert!(due.deadlines[0].due_at <= due.deadlines[1].due_at);
        assert!(due.deadlines[1].due_at <= due.deadlines[2].due_at);
        assert!(!due.has_more);
    }

    #[tokio::test]
    async fn for_stream_filters_by_stream() {
        let store = InMemoryDeadlineStore::new();
        let stream1 = StreamId::new("process/aaa");
        let stream2 = StreamId::new("process/bbb");
        let d1 = Deadline::new(
            stream1.clone(),
            ProcessId::new(),
            TenantId::new(),
            WorkflowId::new("test-workflow", "FV2025-10-01"),
            "label",
            OffsetDateTime::now_utc() + Duration::days(1),
        );
        let d2 = Deadline::new(
            stream2.clone(),
            ProcessId::new(),
            TenantId::new(),
            WorkflowId::new("test-workflow", "FV2025-10-01"),
            "label",
            OffsetDateTime::now_utc() + Duration::days(1),
        );

        store.register(&d1).await.unwrap();
        store.register(&d2).await.unwrap();

        let for1 = store.for_stream(&stream1).await.unwrap();
        assert_eq!(for1.len(), 1);
        assert_eq!(for1[0].stream_id, stream1);
    }

    #[tokio::test]
    async fn register_upserts_on_same_id() {
        let store = InMemoryDeadlineStore::new();
        let mut d = make_deadline(OffsetDateTime::now_utc() + Duration::days(5));
        store.register(&d).await.unwrap();

        let new_due = OffsetDateTime::now_utc() + Duration::days(10);
        d.due_at = new_due;
        store.register(&d).await.unwrap();

        assert_eq!(
            store.len().await.unwrap(),
            1,
            "upsert must not create a duplicate"
        );
        let found = store.for_stream(&d.stream_id).await.unwrap();
        assert_eq!(found[0].due_at, new_due);
    }

    #[tokio::test]
    async fn noop_store_succeeds_silently() {
        let store = NoopDeadlineStore;
        let d = make_deadline(OffsetDateTime::now_utc() - Duration::seconds(1));
        store.register(&d).await.unwrap();
        assert!(store.due_now(10).await.unwrap().deadlines.is_empty());
        assert!(store.is_empty().await.unwrap());
    }

    #[tokio::test]
    async fn clone_shares_state() {
        let store1 = InMemoryDeadlineStore::new();
        let store2 = store1.clone();
        let d = make_deadline(OffsetDateTime::now_utc() + Duration::days(1));
        store1.register(&d).await.unwrap();
        assert_eq!(store2.len().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn due_now_has_more_signals_truncation() {
        let store = InMemoryDeadlineStore::new();
        let past = OffsetDateTime::now_utc() - Duration::seconds(1);
        for _ in 0..5 {
            store.register(&make_deadline(past)).await.unwrap();
        }

        // Request fewer than available — has_more must be true.
        let r = store.due_now(3).await.unwrap();
        assert_eq!(r.deadlines.len(), 3);
        assert!(r.has_more);

        // Request all — has_more must be false.
        let r2 = store.due_now(10).await.unwrap();
        assert_eq!(r2.deadlines.len(), 5);
        assert!(!r2.has_more);
    }
}
