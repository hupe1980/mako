#![expect(
    async_fn_in_trait,
    reason = "the trait is implemented and awaited within this workspace, where the future's Send bound is known"
)]

use time::Date;
use uuid::Uuid;

use crate::obs::{
    domain::{KpiReport, ObsQuery, ProcessProjection},
    error::ObsError,
};

/// Persistent store for [`ProcessProjection`] read-model entries.
///
/// **Production backend**: PostgreSQL.
/// **Test backend**: the in-memory store of the module's unit tests.
pub trait ProcessProjectionRepository: Send + Sync + 'static {
    /// Upsert a process projection.
    ///
    /// - On insert: sets all fields.
    /// - On update: advances `state`, `last_event_at`, `deadline_risk`,
    ///   and `erc_code` if the incoming event carries a later timestamp.
    ///
    /// Idempotent: re-applying the same event is safe.
    async fn upsert(&self, p: &ProcessProjection) -> Result<(), ObsError>;

    /// Query process projections matching the given filters.
    async fn query(&self, q: &ObsQuery) -> Result<Vec<ProcessProjection>, ObsError>;

    /// Retrieve a single projection by process ID, scoped to its owning tenant.
    ///
    /// `tenant` is a predicate, not a label: a process ID is a UUID a caller
    /// can hold without being entitled to the row behind it, so a read that
    /// does not carry the tenant is a cross-tenant read of the whole projection
    /// store.
    async fn get(
        &self,
        process_id: Uuid,
        tenant: &str,
    ) -> Result<Option<ProcessProjection>, ObsError>;

    /// Compute a KPI report for one PID over a calendar period.
    ///
    /// `tenant` is the operator MP-ID (GLN). Filters results to that tenant.
    async fn kpi_report(
        &self,
        pid: u32,
        from: Date,
        to: Date,
        tenant: &str,
    ) -> Result<KpiReport, ObsError>;

    /// Return all non-terminal processes whose `deadline_at` is in the past.
    ///
    /// `tenant` is the operator MP-ID (GLN). Filters results to that tenant.
    async fn overdue_processes(
        &self,
        now: time::OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<ProcessProjection>, ObsError>;

    /// One page of the tenant's open processes with one of `pids` (any when
    /// empty) on one of `malos` (any when empty), the
    /// earliest Frist first and those without one last, strictly after
    /// `after` — a keyset page, so a page costs the same at any depth and a
    /// process closing between two pages shifts none.
    async fn open_processes(
        &self,
        tenant: &str,
        pids: &[u32],
        malos: &[String],
        after: Option<&OpenCursor>,
        limit: i64,
    ) -> Result<Vec<ProcessProjection>, ObsError>;

    /// What [`Self::open_processes`]'s filter covers in total, and how many of
    /// those Fristen fall in `[von, bis)` — counted by the store, so the
    /// figure holds however many processes are open.
    async fn count_open_processes(
        &self,
        tenant: &str,
        pids: &[u32],
        malos: &[String],
        von: time::OffsetDateTime,
        bis: time::OffsetDateTime,
    ) -> Result<OpenCount, ObsError>;
}

/// The answer of [`ProcessProjectionRepository::count_open_processes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCount {
    /// Open processes the filter keeps.
    pub offen: i64,
    /// Of those, the ones whose Frist falls in the window.
    pub faellig: i64,
    /// The earliest such Frist.
    pub frueheste: Option<time::OffsetDateTime>,
}

/// Where a page of [`ProcessProjectionRepository::open_processes`] ends: the
/// last row's Frist (`None` sorts last) and its id as the tiebreak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCursor {
    pub frist: Option<time::OffsetDateTime>,
    pub process_id: uuid::Uuid,
}
