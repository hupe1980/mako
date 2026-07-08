#![allow(async_fn_in_trait)]

use time::Date;
use uuid::Uuid;

use crate::{
    domain::{KpiReport, ObsQuery, ProcessProjection},
    error::ObsError,
};

/// Persistent store for [`ProcessProjection`] read-model entries.
///
/// **Production backend**: PostgreSQL.
/// **Test backend**: [`crate::testing::InMemoryProcessProjectionRepository`].
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

    /// Retrieve a single projection by process ID.
    async fn get(&self, process_id: Uuid) -> Result<Option<ProcessProjection>, ObsError>;

    /// Compute a KPI report for one PID over a calendar period.
    async fn kpi_report(
        &self,
        pid: u32,
        from: Date,
        to: Date,
        tenant_id: Option<Uuid>,
    ) -> Result<KpiReport, ObsError>;

    /// Return all non-terminal processes whose `deadline_at` is in the past.
    async fn overdue_processes(
        &self,
        now: time::OffsetDateTime,
        tenant_id: Option<Uuid>,
    ) -> Result<Vec<ProcessProjection>, ObsError>;
}
