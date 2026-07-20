#![allow(async_fn_in_trait)]

use time::Date;

use crate::{
    domain::{
        BillingPeriodQuery, ImbalanceReport, MeterBillingPeriod, MeterDataReceipt, MeterRead,
        TimeSeriesQuery,
    },
    error::EdmError,
};

/// Persistent store for MSCONS meter data receipts and typed reads.
///
/// **Production backend**: TimescaleDB (PostgreSQL hypertable on `dtm_from`).
/// **Test backend**: [`crate::testing::InMemoryTimeSeriesRepository`].
pub trait TimeSeriesRepository: Send + Sync + 'static {
    /// Record that MSCONS data was received for a MaLo.
    ///
    /// Idempotent on `process_id`: re-inserting the same process is a no-op.
    async fn store_receipt(&self, receipt: &MeterDataReceipt) -> Result<(), EdmError>;

    /// Upsert a batch of typed meter reads.
    ///
    /// Idempotent: duplicate `(malo_id, dtm_from, dtm_to)` rows are silently
    /// overwritten with the latest `quality` and `quantity_kwh`.
    async fn store_reads(&self, reads: &[MeterRead]) -> Result<(), EdmError>;

    /// Query typed meter reads for a MaLo over a time window.
    async fn query(&self, q: &TimeSeriesQuery) -> Result<Vec<MeterRead>, EdmError>;

    /// Query raw delivery receipts for a MaLo (all MSCONS PIDs).
    ///
    /// All results are scoped to `tenant` — cross-tenant queries are not possible.
    async fn receipts(
        &self,
        malo_id: &str,
        from: time::OffsetDateTime,
        to: time::OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<MeterDataReceipt>, EdmError>;

    /// Compute Mehr-/Mindermengen imbalance for one MaLo in one billing period.
    ///
    /// `tenant` is mandatory — passing an empty string is rejected at the SQL layer
    /// by the `AND tenant = $N` guard.
    async fn imbalance(
        &self,
        malo_id: &str,
        from: Date,
        to: Date,
        tenant: &str,
    ) -> Result<ImbalanceReport, EdmError>;

    /// Return the most recent typed read for a MaLo.
    ///
    /// `tenant` is mandatory.
    async fn latest_read(&self, malo_id: &str, tenant: &str)
    -> Result<Option<MeterRead>, EdmError>;

    /// Return the aggregated billing-period summary for a MaLo.
    ///
    /// Aggregates all `meter_reads` rows in `[period_from, period_to]` into a
    /// single [`MeterBillingPeriod`]:
    /// - `arbeitsmenge_kwh` = SUM(quantity_kwh)
    /// - `spitzenleistung_kw` = MAX over 15-min intervals × 4 (RLM Strom only)
    /// - `brennwert_kwh_per_m3` and `zustandszahl` from latest Gas-specific receipt
    ///
    /// Returns `None` when no reads exist for the period.
    ///
    /// Consumed by `invoicd` for RLM plausibility checks (M16) and by
    /// `netzbilanzd` for INVOIC generation (N4).
    async fn billing_period(
        &self,
        q: &BillingPeriodQuery,
    ) -> Result<Option<MeterBillingPeriod>, EdmError>;

    /// Update Gas quality fields (`brennwert_kwh_per_m3`, `zustandszahl`) in
    /// `meter_billing_periods` for a MaLo.
    ///
    /// Called by `edmd` when a `de.mako.process.completed` event arrives for
    /// PID 13007 (Gasbeschaffenheitsdaten). Updates the billing-period rows for
    /// the MaLo **within `tenant`** that currently have `NULL` gas quality
    /// fields. A MaLo-ID is not unique across tenants, and the calorific value
    /// directly scales invoiced kWh, so the tenant scope is mandatory.
    ///
    /// Returns the number of updated rows.
    async fn update_gas_quality(
        &self,
        tenant: &str,
        malo_id: &str,
        brennwert_kwh_per_m3: Option<&str>,
        zustandszahl: Option<&str>,
    ) -> Result<u64, EdmError>;

    /// Record a retroactive correction to one or more meter read intervals.
    ///
    /// ## Semantics
    ///
    /// 1. The original interval in `meter_reads` is **overwritten** with the
    ///    corrected `quantity_kwh` and `quality`.
    /// 2. An immutable `meter_read_corrections` row is inserted, preserving the
    ///    original value, correction reason, and operator identity.
    /// 3. `meter_reads.correction_count` is incremented.
    ///
    /// This gives the **query layer** the latest (corrected) value, while the
    /// **audit layer** retains the full correction history per §22 MessZV.
    ///
    /// ## Atomicity
    ///
    /// All corrections in `records` are applied in a single database transaction.
    /// If any correction fails, none are committed.
    ///
    /// Returns the UUIDs of the newly created `meter_read_corrections` rows.
    async fn store_corrections(
        &self,
        records: &[crate::domain::CorrectionRecord],
    ) -> Result<Vec<uuid::Uuid>, EdmError>;
}
