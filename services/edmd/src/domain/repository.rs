#![allow(async_fn_in_trait)]

use crate::domain::validation::ValidatedReads;
use rust_decimal::Decimal;
use time::Date;

use crate::domain::{
    BillingPeriodQuery, ImbalanceReport, MeterBillingPeriod, MeterDataReceipt, MeterRead, Sparte,
    TimeSeriesQuery, Typ2Read, error::EdmError,
};

/// Persistent store for MSCONS meter data receipts and typed reads.
///
/// **Backend**: [`MeterStoreTimeSeriesRepository`], a `meterstore` hot/cold tier
/// (PostgreSQL for the recent window, Apache Iceberg for the settled history) for
/// the readings, plus edmd's own `PgPool` for the business tables. Tests exercise
/// the same backend against real PostgreSQL and a filesystem Iceberg warehouse via
/// testcontainers (see `services/edmd/tests/meterstore_integration.rs`).
///
/// [`MeterStoreTimeSeriesRepository`]: crate::store::MeterStoreTimeSeriesRepository
pub trait TimeSeriesRepository: Send + Sync + 'static {
    /// Record that MSCONS data was received for a MaLo.
    ///
    /// Idempotent on `process_id`: re-inserting the same process is a no-op.
    async fn store_receipt(&self, receipt: &MeterDataReceipt) -> Result<(), EdmError>;

    /// Upsert a batch of typed meter reads.
    ///
    /// Idempotent: duplicate `(malo_id, dtm_from, dtm_to)` rows are silently
    /// overwritten with the latest `quality` and `quantity_kwh`.
    async fn store_reads(&self, reads: ValidatedReads) -> Result<(), EdmError>;

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

    /// Compute the Mehr-/Mindermengensaldo for one MaLo in one billing period.
    ///
    /// `bilanziert_kwh` is the profile-allocated quantity the balancing side
    /// booked, and it is a **parameter** because edmd cannot know it: it is a
    /// commercial figure in the supplier's system, not a measurement. edmd
    /// supplies the measured half. Deriving both from the same measured total —
    /// as this used to — makes the delta structurally zero.
    ///
    /// `tenant` is mandatory; the read is scoped by it in the store query.
    ///
    /// Returns [`EdmError::NoData`] when the period holds no billable reading:
    /// a saldo against nothing measured is not "zero imbalance", it is unknown.
    async fn imbalance(
        &self,
        malo_id: &str,
        from: Date,
        to: Date,
        tenant: &str,
        sparte: Sparte,
        bilanziert_kwh: Decimal,
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

    /// Record a Gasbeschaffenheit delivery (MSCONS PID 13007) for one MaLo.
    ///
    /// Two writes, one transaction:
    ///
    /// 1. The delivery itself is appended to `gas_quality_data`, keyed by
    ///    `(malo_id, period_from, period_to, tenant)`. This is the *record* of
    ///    what the gas grid operator published — Brennwert varies by supply area
    ///    and month, so a value is only meaningful together with the period it
    ///    applies to, and `GET /api/v1/gas-quality/{malo_id}` reads it back.
    /// 2. Cached `meter_billing_periods` aggregates overlapping that period and
    ///    still missing their gas factors are backfilled, so a billing read does
    ///    not have to join.
    ///
    /// Both are tenant-scoped: a MaLo-ID is not unique across tenants and the
    /// calorific value directly scales invoiced kWh.
    ///
    /// Returns the number of backfilled billing-period rows.
    async fn record_gas_quality(
        &self,
        q: &crate::domain::GasQualityRecord,
    ) -> Result<u64, EdmError>;

    /// Record a retroactive correction to one or more meter read intervals.
    ///
    /// ## Semantics
    ///
    /// 1. An immutable `meter_read_corrections` row is inserted, preserving the
    ///    original value, the correction reason, and the operator identity.
    /// 2. The corrected interval is **appended to the store at a higher
    ///    version** — nothing is overwritten. meterstore routes it to the tier
    ///    that owns the (possibly already archived) interval and applies
    ///    latest-version-wins on resolution, so the read path returns the
    ///    corrected value while `meter_reads_versions` keeps both.
    /// 3. Any open § 60 Abs. 2 MsbG confirmation for the slot is discharged when
    ///    the corrected value is a real one (`MEASURED` / `CORRECTED`).
    /// 4. Cached `meter_billing_periods` aggregates covering the corrected
    ///    intervals are invalidated, so the next billing read recomputes.
    ///
    /// The audit trail exists because a billed figure is a Buchungsbeleg: § 147
    /// Abs. 1 AO requires it to be retained and § 146 Abs. 4 AO requires the
    /// original to stay recoverable after a change. (§ 60 Abs. 6 MsbG is the
    /// opposite duty — a *deletion* ceiling on personal Messwerte — and is
    /// discharged by the GDPR erasure path, not by this one.)
    ///
    /// ## Atomicity
    ///
    /// The audit rows, the confirmation updates and the cache invalidation all
    /// commit in one transaction. The interval appends go to meterstore, which
    /// owns its own tiers, so they are not enrolled in it: a crash between the
    /// append and the commit loses audit rows for values that did change, which
    /// is recoverable — both versions are still in `meter_reads_versions`.
    ///
    /// Returns the UUIDs of the newly created `meter_read_corrections` rows.
    async fn store_corrections(
        &self,
        records: &[crate::domain::CorrectionRecord],
    ) -> Result<Vec<uuid::Uuid>, EdmError>;

    /// Persist a **Zählerstandsgang** — the register readings themselves.
    ///
    /// The primary record behind every derived interval. BK6-24-174 puts the
    /// Zählerstandsgang → Lastgang differencing at the Messstellenbetreiber, so
    /// edmd holds both halves: these rows and the intervals they produced.
    /// Keeping only the difference would satisfy billing and fail § 146 Abs. 4
    /// AO, which requires the original to stay recoverable.
    ///
    /// Idempotent on `(tenant, malo_id, obis_code_norm, read_at)`: a redelivered
    /// Zählerstandsgang overwrites, exactly as a redelivered Lastgang does.
    async fn store_readings(
        &self,
        readings: &[crate::domain::MeterReading],
    ) -> Result<u64, EdmError>;

    /// Read a MaLo's Zählerstände over a window, ascending by instant.
    async fn readings(
        &self,
        malo_id: &str,
        from: time::OffsetDateTime,
        to: time::OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<crate::domain::MeterReading>, EdmError>;

    /// Record what the ZSG conversion did across each contested span.
    ///
    /// Reconstructed wraps and refused differences alike — see
    /// [`ZsgConversionEntry`](crate::domain::ZsgConversionEntry).
    async fn log_zsg_conversion(
        &self,
        entries: &[crate::domain::ZsgConversionEntry],
    ) -> Result<(), EdmError>;

    /// The register readings bracketing a billing period, for § 40 Abs. 2 Nr. 6
    /// EnWG.
    ///
    /// An energy invoice must show the opening and closing Zählerstand. Returns
    /// the last reading at or before `from` and the last at or before `to`, on
    /// the point's dominant register — the pair a customer can check against the
    /// meter — or `None` for either end that has no reading.
    ///
    /// Deliberately "at or before" rather than "nearest": a Zählerstand dated
    /// after the period end did not hold at the period end, and putting it on
    /// the invoice as the closing reading overstates the period.
    async fn period_zaehlerstaende(
        &self,
        malo_id: &str,
        from: time::OffsetDateTime,
        to: time::OffsetDateTime,
        tenant: &str,
    ) -> Result<(Option<rust_decimal::Decimal>, Option<rust_decimal::Decimal>), EdmError>;
}

/// Persistent store for **ESA "Werte nach Typ 2"** intervals (MSCONS PID 13027).
///
/// A deliberately separate trait and table (`esa_typ2_reads`) from
/// [`TimeSeriesRepository`]. Typ-2 data is non-authoritative (Codeliste 1.4
/// Kap. 4.6; WiM Strom Teil 2 §4) and must never reach a billing path — so it
/// shares *no* read method with the billing store. There is no `imbalance`,
/// `billing_period`, `latest_read`, `store_corrections` or substitute-value
/// method here **by design**: a Typ-2 value can only be stored and read back
/// verbatim, never aggregated for invoicing.
pub trait Typ2Repository: Send + Sync + 'static {
    /// Upsert a batch of ESA Typ-2 intervals.
    ///
    /// Idempotent on `(tenant, malo_id, dtm_from, obis_code_norm)`: a
    /// re-delivery overwrites the prior value. There is no correction audit
    /// trail — a Typ-2 value carries no legal reconciliation obligation.
    async fn store_typ2_reads(&self, reads: &[Typ2Read]) -> Result<(), EdmError>;

    /// Read ESA Typ-2 intervals for a MaLo over a time window.
    ///
    /// The *only* read path. It is not — and must never become — reachable from
    /// any billing aggregation.
    async fn query_typ2(&self, q: &TimeSeriesQuery) -> Result<Vec<Typ2Read>, EdmError>;
}
