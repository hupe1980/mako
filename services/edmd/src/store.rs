//! Meterstore-backed storage layer for `edmd`: `TimeSeriesRepository` +
//! `Typ2Repository`.
//!
//! Replaces the former hand-rolled `pg/` + `iceberg/` tiers with a single
//! [`MeterStore`].
//!
//! `edmd`'s authoritative meter-data store. The hot (recent) window lives in
//! PostgreSQL and the settled history in Apache Iceberg — both tiers owned by a
//! [`MeterStore`], which routes each interval to the tier that owns it and
//! version-resolves reads across the watermark. edmd keeps its own `PgPool` for
//! the *business* tables that are not readings (`meter_data_receipts`,
//! `meter_read_corrections`, `estimated_read_confirmations`,
//! `meter_billing_periods`, reading orders, SMGW, …), so each repository holds
//! **both** handles.
//!
//! Storage split:
//! - **`meter_reads`** (resolved) / `meter_reads_versions` (raw audit trail) →
//!   owned by `MeterStore`, both tiers created by `store.create_tables()`.
//! - **`esa_typ2_reads`** → a second, independent `MeterStore` table for the
//!   non-authoritative ESA "Werte nach Typ 2" stream (never billed).
//! - **edmd business tables** → stay in edmd's own `PgPool`.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::scalar::ScalarValue;
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};

use metering::interval::{MeasurementUnit, MeterInterval};
use metering::measurement_series::{MeasurementSeries, MeasurementSource};
use metering::obis::ObisCode;
use meterstore::encode::StoredSeries;
use meterstore::{
    MeterCatalog, MeterStore, MeterStoreBuilder, PostgresHot, ScopedVersion, SubjectRegistry,
    TableConfig, Version, VersionScope,
};

/// The cold-tier object-store credentials type, owned by `meterstore`. Re-exported
/// so `edmd.toml` config maps onto it and callers need not depend on `meterstore`
/// by name for this one struct.
pub use meterstore::WarehouseAuth;

use crate::domain::validation::ValidatedReads;
use crate::domain::{
    BillingPeriodQuery, CorrectionRecord, EnergyDirection, ImbalanceReport, Messtyp,
    MeterBillingPeriod, MeterDataReceipt, MeterRead, MeterReading, QualityFlag, Sparte,
    TimeSeriesQuery, Typ2DeliveryPath, Typ2Read,
    error::EdmError,
    messtyp_as_str, messtyp_from_str,
    repository::{TimeSeriesRepository, Typ2Repository},
};

/// Map a `meterstore` failure onto edmd's error type, **keeping whether a retry
/// could ever work**.
///
/// `meterstore` separates the two: `IntegrityViolation` is a refused delivery —
/// an overlapping span, a restated value, a second network operator on one
/// reading — and no retry changes it, because the message has to change.
/// Everything transient (`Storage`, `LockTimeout`, an object store that could
/// not be reached) is retryable. Flattening both into one variant would make an
/// ingest door answer `5xx` to a delivery that can never be stored, and the
/// fan-out would redeliver it for as long as its retry budget allows.
fn store_err(e: meterstore::Error) -> EdmError {
    if e.is_retryable() {
        return EdmError::Database(e.to_string());
    }
    let constraint = match &e {
        meterstore::Error::IntegrityViolation { constraint, .. } => constraint.clone(),
        _ => None,
    };
    EdmError::Rejected {
        detail: e.to_string(),
        constraint,
    }
}

/// Map a **`sqlx`** failure — edmd's own PostgreSQL pool, not meterstore's.
///
/// Split the same way and for the same reason: a lost connection or a deadlock
/// is worth another attempt, while a column that is not there or will not decode
/// is a schema/code disagreement that no retry resolves. Redelivering the second
/// is a loop, and the loop hides the mismatch that caused it.
fn pg_err(e: sqlx::Error) -> EdmError {
    match &e {
        sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::ColumnNotFound(_)
        | sqlx::Error::ColumnIndexOutOfBounds { .. }
        | sqlx::Error::TypeNotFound { .. } => EdmError::Internal(e.to_string()),
        _ => EdmError::Database(e.to_string()),
    }
}

/// The same mapping for the failures that are **not** meterstore's.
///
/// A MeLo that is not a Zählpunktbezeichnung, an OBIS code that will not parse:
/// statements about the *input*, none of them fixed by a retry. They were
/// `Internal` before, which reads as "edmd is broken" for what is in fact a
/// malformed delivery from a counterparty — and, once the two are told apart by
/// retryability, would have been the wrong answer as well as the wrong word.
fn input_err(e: impl std::fmt::Display) -> EdmError {
    EdmError::Rejected {
        detail: e.to_string(),
        constraint: None,
    }
}

/// The Marktpartner that filed a reading, as a typed BDEW-Codenummer.
///
/// `MeasurementSource::Mscons` and `VersionScope` both take a `BdewCode`, so the
/// MP-ID is parsed once at the boundary rather than carried as a string that
/// only fails deeper in. The check digit is verified but not enforced: the
/// Bildungsvorschrift exempts GS1-issued GLNs, so refusing on it would reject
/// codes the market has issued.
fn operator_code(id: &str) -> Result<metering::ids::BdewCode, EdmError> {
    id.parse()
        .map_err(|e: metering::ParseError| EdmError::Rejected {
            detail: format!("sender MP-ID {id:?} is not a BDEW-Codenummer: {e}"),
            constraint: None,
        })
}

/// Parse a MaLo-ID at the store boundary.
///
/// `metering::MaloId` enforces the BDEW Bildungsvorschrift — eleven digits, a
/// Vergabestelle in 1–9, and the Anwendungshilfe check digit — so this is the
/// point where a string stops being a string. edmd's own query types keep
/// `String` deliberately: they are built from HTTP parameters and a
/// counterparty-supplied value has to be *reportable*, not un-representable.
/// Validating here rather than at construction means a malformed ID is one
/// named refusal at the boundary that cares, instead of an opaque store error.
fn malo(id: &str) -> Result<metering::MaloId, EdmError> {
    id.parse()
        .map_err(|e: metering::ParseError| EdmError::InvalidMaloId {
            malo_id: id.to_owned(),
            reason: e.to_string(),
        })
}

/// The UTC window a settlement period covers, for one commodity.
///
/// The balancing day is **not** the calendar day for gas: a Gastag runs 06:00 to
/// 06:00 Berlin (GaBi Gas, following Art. 3 Nr. 6 VO (EU) 312/2014), so a gas
/// period aggregated over calendar days carries six hours of the wrong day —
/// every day, not only across a DST transition. Electricity balances on the
/// calendar day.
///
/// Both boundaries come from `metering::calendar`, which resolves them through
/// the Berlin zone rather than a fixed offset, so a period containing a DST
/// transition is 23 or 25 hours long as it should be.
fn period_window(sparte: Sparte, from: Date, to: Date) -> (OffsetDateTime, OffsetDateTime) {
    match sparte {
        Sparte::Gas => (
            metering::calendar::gas_day_start_utc(from),
            metering::calendar::gas_day_end_utc(to),
        ),
        _ => (
            metering::calendar::day_start_utc(from),
            metering::calendar::day_end_utc(to),
        ),
    }
}

/// The identity column every store is keyed on. A reading is unique only *within*
/// a tenant, so every series read is scoped by it — an unscoped read would fold
/// two tenants' readings for one MaLo into a single series.
///
/// Typed reads scope with `column_eq`; the caller-supplied SQL endpoint cannot,
/// and uses [`MeterStore::scoped`](meterstore::MeterStore::scoped) instead.
pub(crate) const TENANT_COL: &str = "tenant";

/// The tenant identity as the store binds it, for [`SeriesQuery::column_eq`].
fn tenant_scope(tenant: &str) -> ScalarValue {
    ScalarValue::Utf8(Some(tenant.to_owned()))
}

/// The erasure-subject natural id for one MaLo **within a tenant**.
///
/// A MaLo-ID is not unique across tenants, so the subject that Article 17 erasure
/// unlinks must be qualified by tenant — otherwise two tenants' readings for the
/// same MaLo would share one mapping and erasing either would unlink both.
#[must_use]
pub fn subject_natural_id(tenant: &str, malo_id: &str) -> String {
    format!("{tenant}:{malo_id}")
}

/// Physical (raw, versioned) table name for the authoritative meter reads.
pub const READS_TABLE: &str = "meter_reads_versions";
/// Physical (raw, versioned) table name for the ESA Typ-2 store.
pub const TYP2_TABLE: &str = "esa_typ2_reads_versions";

/// The Zählerstandsgang table — register values at instants, not energy over
/// spans.
///
/// A separate table because `value` means two different things: summing
/// Zählerstände gives a number with no meaning that looks exactly like a
/// consumption total, so `meterstore` keeps the two time models apart and
/// refuses `append` here and `append_readings` there.
pub const ZSG_TABLE: &str = "meter_readings_versions";

// ─────────────────────────────────────────────────────────────────────────────
// Store construction
// ─────────────────────────────────────────────────────────────────────────────

/// meterstore tiering knobs, supplied by edmd's `[archive]` config.
///
/// meterstore is a library edmd links in-process and reads no config of its own, so
/// these come from `edmd.toml` rather than being hardcoded here. Only operator-tunable
/// knobs live in config; the identity/subject columns, catalog name and namespace
/// are edmd invariants and stay fixed in [`build_stores`].
#[derive(Debug, Clone, Copy)]
pub struct TieringConfig {
    /// How long an interval stays mutable in the hot tier before it settles.
    pub settlement_lag: time::Duration,
    /// How far each archival sweep advances the tiering watermark — and, with
    /// it, the hot table's partition granularity. One step, because two could
    /// only ever express the mistake of disagreeing.
    pub archival_step: time::Duration,
    /// Target Parquet file size in the cold tier, in bytes.
    pub cold_file_target_bytes: usize,
    /// How long a DDL statement waits for its lock before giving up.
    ///
    /// Partition creation runs on the **write path** and the archival detach on
    /// its own schedule; PostgreSQL queues locks in arrival order, so either can
    /// stall every reader and writer behind it. Timed out, the statement gives
    /// up having changed nothing.
    pub ddl_lock_timeout: time::Duration,
}

impl Default for TieringConfig {
    fn default() -> Self {
        Self {
            settlement_lag: time::Duration::weeks(1),
            archival_step: time::Duration::DAY,
            cold_file_target_bytes: 512 * 1024 * 1024,
            ddl_lock_timeout: time::Duration::seconds(3),
        }
    }
}

/// The `SqlCatalog` metadata namespace key and the Iceberg namespace the cold
/// tables live in — edmd invariants, so they are fixed here rather than in config.
const CATALOG_NAME: &str = "meterstore";
const CATALOG_NAMESPACE: &str = "metering";
/// The `SqlCatalog`'s own metadata pool, bound small: this single catalog (shared
/// by every table) plus edmd's main pool and meterstore's hot tier must not
/// exhaust PostgreSQL's connection slots (the `SqlCatalog` default is 10).
const CATALOG_POOL_MAX: u32 = 4;

/// Build edmd's three meter-data tables over **one** shared Iceberg catalog and
/// **one** DataFusion session.
///
/// edmd's storage is exactly the shape [`meterstore::MeterCatalog`] exists for:
/// the authoritative `meter_reads` intervals, the non-authoritative
/// `esa_typ2_reads` stream that must never reach a billing query, and the
/// `meter_readings` Zählerstandsgang — a **point** table, register values at
/// instants rather than energy over spans. One catalog means one `SqlCatalog`
/// (one metadata pool) and one `SessionContext`; each table still keeps its own
/// watermark, archiver and tiering. Each returned [`MeterStore`] is a cheap clone
/// sharing that session; the [`meterstore::ColdTier`] carries the
/// [`catalog_facade`](meterstore::ColdTier::catalog_facade) for the read-only
/// Iceberg REST endpoint.
///
/// The cold tier (the `SqlCatalog` + object-store backend chosen from
/// `warehouse_uri`'s scheme) is built by `meterstore` itself — edmd wires none of
/// the Iceberg catalog stack directly. Returns `(reads, typ2, zsg, cold)`.
pub async fn build_stores(
    pool: PgPool,
    database_url: &str,
    warehouse_uri: &str,
    tiering: TieringConfig,
    auth: &WarehouseAuth,
) -> anyhow::Result<(MeterStore, MeterStore, MeterStore, meterstore::ColdTier)> {
    let cold_tier = meterstore::IcebergSqlCatalog {
        database_url,
        warehouse_uri,
        catalog_name: CATALOG_NAME,
        namespace: CATALOG_NAMESPACE,
        file_target_bytes: tiering.cold_file_target_bytes,
        metadata_pool_max_connections: CATALOG_POOL_MAX,
        auth,
    }
    .build()
    .await?;
    let cold = cold_tier.cold();
    // One hot tier serves every table: `PostgresHot` takes the table as a method
    // argument, so a single handle over one pool covers both.
    let hot = Arc::new(PostgresHot::new(pool.clone()).ddl_lock_timeout(tiering.ddl_lock_timeout));

    // `meter_reads` is authoritative and carries the GDPR subject registry;
    // `esa_typ2_reads` is the non-authoritative ESA stream (never billed).
    use meterstore::config::TimeModel;
    let reads = table_builder(
        &cold,
        &hot,
        &pool,
        READS_TABLE,
        true,
        tiering,
        TimeModel::Interval,
    )
    .await?;
    let typ2 = table_builder(
        &cold,
        &hot,
        &pool,
        TYP2_TABLE,
        false,
        tiering,
        TimeModel::Interval,
    )
    .await?;
    // The Zählerstandsgang. BK6-24-174 makes it the *primary* record — the MSB
    // transmits register values and derives the Lastgang from them — so it is
    // tiered like any other measurement rather than growing without bound in the
    // hot tier beside the intervals it produced.
    let zsg = table_builder(
        &cold,
        &hot,
        &pool,
        ZSG_TABLE,
        true,
        tiering,
        TimeModel::Point,
    )
    .await?;

    let catalog = MeterCatalog::builder()
        .table(reads)
        .table(typ2)
        .table(zsg)
        .build()
        .await?;
    catalog.create_tables().await?; // hot + registry (cold already provisioned)

    // Clone the per-table handles out for the two repositories. `MeterStore` is a
    // cheap Arc-backed clone that shares the session the catalog built, so the
    // catalog wrapper itself is not kept.
    let reads_store = catalog
        .table(READS_TABLE)
        .expect("reads table registered")
        .clone();
    let typ2_store = catalog
        .table(TYP2_TABLE)
        .expect("typ2 table registered")
        .clone();
    let zsg_store = catalog
        .table(ZSG_TABLE)
        .expect("zsg table registered")
        .clone();
    Ok((reads_store, typ2_store, zsg_store, cold_tier))
}

/// The per-table [`MeterStoreBuilder`], ready to join a [`MeterCatalog`].
///
/// `tenant` is the non-nullable **identity** column (it joins the merge key, so two
/// tenants reporting the same measuring point stay distinct). `source`,
/// `sender_mp_id` and `allocation_version` are nullable **attribute** columns:
/// provenance that travels with the values but stays out of the merge key, folded
/// from the newest contributing delivery on read (§4.2) — declaring them here is
/// what stops the store round-trip dropping them.
///
/// Every table carries a nullable `subject_ref` **subject** column and a
/// [`SubjectRegistry`] for GDPR Art. 17 erasure: a Typ-2 value is
/// non-authoritative for settlement, which says nothing about whether it is
/// personal data, and the Zählerstandsgang is the primary measurement. One
/// registry spans them, so one erasure unlinks all three. `with_subject` selects
/// only the ingestion-provenance columns, which the Typ-2 stream does not carry.
async fn table_builder(
    cold: &Arc<meterstore::IcebergCold>,
    hot: &Arc<PostgresHot>,
    pool: &PgPool,
    table_name: &str,
    with_subject: bool,
    tiering: TieringConfig,
    time_model: meterstore::config::TimeModel,
) -> anyhow::Result<MeterStoreBuilder> {
    use meterstore::arrow::datatypes::{DataType, Field};

    // `source` is a fixed `IngestionSource` vocabulary, so it is declared as a
    // coded column — enforced by a DB `CHECK` like sparte/unit/quality, not just
    // by the application that writes it. `sender_mp_id` is a Marktpartner-ID and
    // is declared `ValueCheck::Bdew`: thirteen digits, checked in the write path
    // and by the hot table's own `CHECK`. Deliberately no check digit — §2.3 of
    // the Bildungsvorschrift exempts GS1-issued GLNs, which are legitimate
    // Marktpartner-IDs under a different procedure. `allocation_version` is a
    // free MaBiS label (INITIAL/CORRECTION/ESA-…) with no format to enforce, so
    // it stays unconstrained.
    let source_codes: Vec<&str> = crate::domain::IngestionSource::ALL
        .iter()
        .map(|s| s.as_str())
        .collect();
    let delivery_paths: Vec<&str> = crate::domain::Typ2DeliveryPath::ALL
        .iter()
        .map(|p| p.as_str())
        .collect();
    let mut config = TableConfig::new(table_name)
        // A Zählerstandsgang is instants, a Lastgang spans. `identify_by_melo`
        // follows the model: a register belongs to the *meter*, so two meters
        // under one Marktlokation carry the same OBIS code at the same instants
        // and the Messlokation is what tells their readings apart.
        .time_model(time_model)
        .archival_step(tiering.archival_step)
        .settlement_lag(tiering.settlement_lag)
        .identity_column(Field::new("tenant", DataType::Utf8, false))
        .attribute_column(meterstore::checked_column(
            "sender_mp_id",
            meterstore::ValueCheck::Bdew,
            true,
        ))
        // Every store carries the GDPR subject reference. A Typ-2 value is
        // non-authoritative for settlement, which says nothing about whether it
        // is personal data: it is a quarter-hourly consumption series against a
        // MaLo-ID, exactly like the authoritative one, and § 60 Abs. 6 MsbG's
        // deletion duty makes no exception for it — nor for the register
        // readings the intervals are derived from. `SubjectRegistry` is a single
        // `meterstore_subject_map` over one pool, so a subject registered by any
        // of them is the same subject and one erasure unlinks all three.
        .subject_column("subject_ref");
    config = if with_subject {
        // The authoritative store: ingestion provenance and the MaBiS delivery
        // label, plus the GDPR subject reference.
        config
            .attribute_column(meterstore::coded_column("source", &source_codes, true))
            .attribute_column(Field::new("allocation_version", DataType::Utf8, true))
    } else {
        // The ESA Typ-2 stream carries neither — but it does carry the transport
        // it arrived on (Codeliste 1.4 Kap. 4.6: MSCONS backend vs. direct from
        // the SMGW over SM-PKI). Without the column the field was write-only:
        // every Typ-2 value read back as `MSCONS_BACKEND` whatever it was
        // delivered by, and the model documented a column that did not exist.
        config
            .attribute_column(meterstore::coded_column(
                "delivery_path",
                &delivery_paths,
                true,
            ))
            // `SG1 RFF+AGI` on the delivering MSCONS 13027 — the Belegnummer of
            // the ORDERS that ordered these values (MSCONS AHB 3.2 §11.2 hint
            // [574]), and the first hop of the PID overview's `EZ-03` routing.
            //
            // It is the **only** thing on a value delivery that names the
            // subscription it belongs to, and a Meldepunkt may carry several: a
            // subscription is the (Meldepunkt, Messprodukt) pair, and the
            // catalogue offers `9991 00000 305 6` and `9991 00000 314 7` for
            // the same Marktlokation. Without it the delivery-surveillance
            // sweep had to infer the subscription from whichever OBIS registers
            // had stopped, and could not name the one that is silent.
            .attribute_column(Field::new("bestellung_ref", DataType::Utf8, true))
    };
    let config = config.build()?;

    // `table_provider` eagerly loads the cold table's schema, so the table must
    // already exist when we ask for it. On a fresh catalog it does not: the
    // catalog's `create_tables()` provisions it, but that runs *after* the store is
    // built — and the provider is needed to build the store. Break the cycle by
    // provisioning the cold tier here first. It is idempotent (an existing table is
    // loaded and its partition spec checked, never recreated).
    cold.create_table_with(
        table_name,
        &config.extra_columns(),
        &config.identity_column_names(),
    )
    .await?;
    let provider = cold.table_provider(table_name).await?;
    let builder = MeterStore::builder()
        .hot(hot.clone())
        .cold(cold.clone(), provider)
        .table(config)
        .subject_registry(SubjectRegistry::new(pool.clone()));
    Ok(builder)
}

// ─────────────────────────────────────────────────────────────────────────────
// Authoritative time-series repository
// ─────────────────────────────────────────────────────────────────────────────

/// Meterstore-backed authoritative time-series store.
///
/// `store` owns `meter_reads` across both tiers; `pool` owns edmd's business
/// tables (audit, confirmations, billing-period cache).
#[derive(Clone)]
pub struct MeterStoreTimeSeriesRepository {
    store: MeterStore,
    /// The Zählerstandsgang table — a **point** store, keyed by Messlokation.
    zsg: MeterStore,
    pool: PgPool,
}

impl MeterStoreTimeSeriesRepository {
    /// Wrap the interval store, the Zählerstandsgang store and edmd's
    /// business-table pool.
    #[must_use]
    pub fn new(store: MeterStore, zsg: MeterStore, pool: PgPool) -> Self {
        Self { store, zsg, pool }
    }

    /// The Zählerstandsgang store (register values at instants).
    #[must_use]
    pub fn zsg_store(&self) -> &MeterStore {
        &self.zsg
    }

    /// The edmd business-table pool (readiness probe, metrics, audit queries).
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The underlying store (OLAP `sql`/`query`, `as_of`, `subject_registry`).
    #[must_use]
    pub fn store(&self) -> &MeterStore {
        &self.store
    }

    /// The meter and register an invoice's opening/closing Zählerstand is about.
    ///
    /// "Dominant" is the register carrying the most readings, which for a ZSG is
    /// the one the Lastgang is derived from. A prosumer reports a feed-in
    /// register too, and an invoice's Zählerstand is about the consumption meter
    /// the customer can walk up to and read.
    async fn dominant_register(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Option<(String, String)>, EdmError> {
        let deliveries = self
            .zsg
            .readings(malo(malo_id)?)
            .map_err(store_err)?
            .column_eq(TENANT_COL, tenant_scope(tenant))
            .map_err(store_err)?
            .deliveries()
            .await
            .map_err(store_err)?;

        let mut counts: std::collections::BTreeMap<(String, String), usize> =
            std::collections::BTreeMap::new();
        for d in deliveries {
            let Some(melo) = d.melo_id.as_ref() else {
                continue;
            };
            *counts
                .entry((melo.as_str().to_owned(), d.obis_code.to_string()))
                .or_default() += d.readings.len();
        }
        // Most readings wins; the key breaks a tie deterministically.
        Ok(counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .map(|(k, _)| k))
    }

    /// Append a value **edmd itself authored**, ensuring it becomes the current one.
    ///
    /// Ordinary ingest must not do this: a delivery that arrives late is
    /// legitimately shadowed by a newer one. An operator correction
    /// (`POST /api/v1/corrections/{malo_id}`) and a § 60 Abs. 2 Ersatzwert are
    /// not deliveries — they are edmd asserting a value about a slot whose
    /// current content it has just judged unusable — and neither carries an
    /// MSCONS version, so through a plain `append` both resolve to `Shadowed`
    /// while the audit row, the § 60 confirmation and the cache invalidation all
    /// record a correction that did not take.
    ///
    /// [`MeterStore::append_authoritative`] treats the version as a *floor* and
    /// re-appends at the next version **of the scope already in force**. That
    /// last part is why edmd cannot write the loop itself: a version is
    /// comparable only within its own scope, and the hot tier refuses a second
    /// network operator for one reading.
    async fn append_superseding(
        &self,
        read: &MeterRead,
        subject: &str,
    ) -> Result<Option<meterstore::session::Displacement>, EdmError> {
        let stored = read_to_stored(read)?
            .with_extra("subject_ref", ScalarValue::Utf8(Some(subject.to_owned())));
        let outcome = self
            .store
            .append_authoritative(&[stored])
            .await
            .map_err(store_err)?;
        // Every displacement it returns took effect; `written` carries the
        // version the row actually landed at, which is what the audit records.
        Ok(outcome.displacements.into_iter().next())
    }

    /// Aggregate a billing period from the resolved series and re-cache it.
    ///
    /// The read-through half of [`TimeSeriesRepository::billing_period`], split
    /// out so the cache-hit path can fall back to it — a cached row whose Sparte
    /// or quality does not decode is a row edmd did not write, and answering
    /// from it would put a guessed commodity or a non-billable flag on an
    /// invoice.
    async fn recompute_billing_period(
        &self,
        q: &BillingPeriodQuery,
    ) -> Result<Option<MeterBillingPeriod>, EdmError> {
        // On-the-fly aggregation from the resolved series, over the commodity's
        // own balancing period (Gastag for Gas).
        let (from_ts, to_ts) = period_window(q.sparte, q.period_from, q.period_to);
        // Billable qualities only (§60 Abs. 2), pushed into the scan.
        let reads: Vec<MeterRead> =
            read_all_channels(&self.store, &q.malo_id, &q.tenant, from_ts, to_ts, true).await?;
        if reads.is_empty() {
            return Ok(None);
        }

        // The Arbeitsmenge is the **Bezug**, projected onto one canonical set of
        // registers: the resolved read spans every channel, and a prosumer's
        // Einspeisung or a dual-tariff meter's HT/NT split beside its total would
        // otherwise be summed into the invoiced figure (`domain::register`).
        let intervals = crate::domain::energy_intervals(&reads, EnergyDirection::Bezug);
        let total_kwh: Decimal = intervals.iter().map(|iv| iv.value).sum();
        // The tariff split, reported when the meter actually delivers one. It is
        // read off the HT/NT registers rather than the total, so it stays `None`
        // for a single-tariff meter instead of claiming a zero NT quantity.
        let tariff_sum = |stage: fn(&metering::obis::ObisCode) -> bool| -> Option<Decimal> {
            let sum: Decimal = reads
                .iter()
                .filter(|r| r.quality.is_billable())
                .filter_map(|r| {
                    let c: metering::obis::ObisCode = r.obis_code.as_deref()?.parse().ok()?;
                    (c.is_import() && crate::domain::register::is_energy_register(c) && stage(&c))
                        .then_some(r.quantity_kwh)
                })
                .sum();
            (sum != Decimal::ZERO).then_some(sum)
        };
        let arbeitsmenge_ht_kwh = tariff_sum(metering::obis::ObisCode::is_ht);
        let arbeitsmenge_nt_kwh = tariff_sum(metering::obis::ObisCode::is_nt);
        let sparte = reads.first().map_or(Sparte::Strom, |r| r.sparte);
        let first_interval_min = intervals
            .first()
            .map(|iv| (iv.to - iv.from).whole_minutes().unsigned_abs());
        let messtyp = match first_interval_min {
            Some(m) if m <= 60 => Messtyp::Rlm,
            _ => Messtyp::Slp,
        };
        // Peak demand is a **power**: the interval's energy divided by its own
        // length in hours. Hard-coding the ÷¼ h of a quarter-hour grid meant an
        // hourly RLM series — legitimate, and the norm for Gas — reported no
        // Leistungsmaximum at all, because no interval matched the filter.
        let spitzenleistung_kw = if messtyp == Messtyp::Rlm && sparte == Sparte::Strom {
            intervals
                .iter()
                .filter_map(|iv| {
                    let minutes = Decimal::from((iv.to - iv.from).whole_minutes().unsigned_abs());
                    (minutes > Decimal::ZERO).then(|| iv.value * Decimal::from(60) / minutes)
                })
                .max()
        } else {
            None
        };
        let worst_quality = crate::domain::worst_quality(&intervals);

        // § 40 Abs. 2 Nr. 6 EnWG: an energy invoice must show the opening and
        // closing register reading. They come from the Zählerstandsgang, which
        // is why edmd stores it (BK6-24-174) — an aggregate that cannot fill
        // them builds an invoice missing a statutory line item.
        let (zaehlerstand_anfang, zaehlerstand_ende) = self
            .period_zaehlerstaende(&q.malo_id, from_ts, to_ts, &q.tenant)
            .await
            .unwrap_or_else(|e| {
                // A missing pair is a gap on the invoice, not a wrong
                // Arbeitsmenge — so the aggregate still answers, and says so.
                tracing::warn!(
                    malo_id = %q.malo_id, error = %e,
                    "edmd: could not read the period's Zählerstände (§ 40 Abs. 2 Nr. 6 EnWG)"
                );
                (None, None)
            });

        let result = MeterBillingPeriod {
            malo_id: q.malo_id.clone(),
            period_from: q.period_from,
            period_to: q.period_to,
            messtyp,
            sparte,
            arbeitsmenge_kwh: total_kwh,
            arbeitsmenge_ht_kwh,
            arbeitsmenge_nt_kwh,
            spitzenleistung_kw,
            brennwert_kwh_per_m3: None,
            zustandszahl: None,
            zaehlerstand_anfang,
            zaehlerstand_ende,
            quality: worst_quality,
            lastprofil: None,
            profil_typ: None,
        };

        // Cache the computed aggregate.
        //
        // The conflict target is the index's **column list**, not
        // `ON CONSTRAINT mbp_tenant_period_unique`. That name belongs to a
        // `CREATE UNIQUE INDEX`, and `ON CONFLICT ON CONSTRAINT` only accepts a
        // *table constraint*, so `ON CONFLICT ON CONSTRAINT` here makes
        // PostgreSQL reject every one of these statements with
        // "constraint … does not exist" — and a rejection discarded by
        // `let _ =` leaves the cache permanently empty while every billing read
        // recomputes from the resolved series.
        let cached = sqlx::query(
            // The tariff split is cached with the total. It is read back by the
            // cache-hit branch above, so leaving it out of the write made the
            // *same* period answer with an HT/NT split on the computing request
            // and without one on every request after it.
            r"INSERT INTO meter_billing_periods
                  (malo_id, period_from, period_to, messtyp, sparte,
                   arbeitsmenge_kwh, spitzenleistung_kw, quality, tenant,
                   arbeitsmenge_ht_kwh, arbeitsmenge_nt_kwh,
                   zaehlerstand_anfang, zaehlerstand_ende)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
              ON CONFLICT (malo_id, period_from, period_to, tenant)
              DO UPDATE SET messtyp             = EXCLUDED.messtyp,
                            sparte              = EXCLUDED.sparte,
                            arbeitsmenge_kwh    = EXCLUDED.arbeitsmenge_kwh,
                            spitzenleistung_kw  = EXCLUDED.spitzenleistung_kw,
                            quality             = EXCLUDED.quality,
                            arbeitsmenge_ht_kwh = EXCLUDED.arbeitsmenge_ht_kwh,
                            arbeitsmenge_nt_kwh = EXCLUDED.arbeitsmenge_nt_kwh,
                            zaehlerstand_anfang = EXCLUDED.zaehlerstand_anfang,
                            zaehlerstand_ende   = EXCLUDED.zaehlerstand_ende,
                            computed_at         = now()",
        )
        .bind(&result.malo_id)
        .bind(result.period_from)
        .bind(result.period_to)
        .bind(messtyp_as_str(result.messtyp))
        .bind(result.sparte.as_str())
        .bind(result.arbeitsmenge_kwh)
        .bind(result.spitzenleistung_kw)
        .bind(quality_to_str(result.quality))
        .bind(&q.tenant)
        .bind(result.arbeitsmenge_ht_kwh)
        .bind(result.arbeitsmenge_nt_kwh)
        .bind(result.zaehlerstand_anfang)
        .bind(result.zaehlerstand_ende)
        .execute(&self.pool)
        .await;
        // A failed cache write is not a failed read — the aggregate is correct
        // either way — but it must be visible, not swallowed.
        if let Err(e) = cached {
            tracing::warn!(
                malo_id = %q.malo_id, error = %e,
                "edmd: could not cache the computed billing period"
            );
        }

        Ok(Some(result))
    }

    /// As [`TimeSeriesRepository::query`], but reading the data **as it was known
    /// at** `as_of` — the § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) point-in-time (bitemporal)
    /// reconstruction.
    ///
    /// Backed by meterstore's `as_known_at`, which pins the row-level `recorded_at`
    /// transaction-time axis across **both** tiers and re-runs version resolution
    /// under that ceiling. So a correction delivered after `as_of`, **and an
    /// interval first stored after it**, are both invisible — the value returned is
    /// the one that was in force at that instant. Set membership is
    /// reconstructed, not overlaid: reverting corrected values from an audit
    /// table alone cannot hide a later-inserted interval.
    pub async fn query_as_of(
        &self,
        q: &TimeSeriesQuery,
        as_of: OffsetDateTime,
    ) -> Result<Vec<MeterRead>, EdmError> {
        let store = self.store.as_known_at(as_of).await.map_err(store_err)?;
        // Multi-register, exactly like `query()`: this answers the same
        // question at an earlier transaction time, so it describes the same
        // measuring point. meterstore declines a range spanning two channels,
        // and a prosumer or an HT/NT point has two.
        let mut out: Vec<MeterRead> = store
            .series(malo(&q.malo_id)?)
            .map_err(store_err)?
            .column_eq(TENANT_COL, tenant_scope(&q.tenant))
            .map_err(store_err)?
            .range(q.from, q.to)
            .collect_by_channel()
            .await
            .map_err(store_err)?
            .values()
            .flat_map(|r| series_to_reads(r, &q.tenant))
            .collect();
        out.sort_by(|a, b| a.dtm_from.cmp(&b.dtm_from));
        Ok(out)
    }
}

/// Map a stored [`MeterRead`] to a meterstore [`StoredSeries`] (one interval).
///
/// **Version scope carries the network operator (BDEW Codenummer), never the
/// tenant / forwarding MP-ID** — `sender_mp_id`, falling back to the tenant only
/// when the reading carries no operator. The scope is derived from the
/// *interval* (`VersionScope::for_interval`), so a July reading corrected in
/// August still lands in July's scope.
/// The meterstore version a delivery resolves under.
///
/// Resolution has to follow the version the network operator assigned, not the
/// order deliveries arrived in — otherwise replaying an original after its
/// correction landed gives the stale value the higher version and supersedes
/// the correction. So a stated version is used as stated, through
/// `Version::mscons`: the strict constructor, so a malformed label fails at the
/// boundary instead of entering the resolved view, where a short version still
/// orders — wrongly but silently — within its scope.
///
/// A delivery that states none falls back to [`Version::arrival`], which derives
/// one from transaction time and reports `is_well_formed() == false` — the fact
/// that separates "the operator said so" from "we assigned one".
///
/// This is not a substitute for the wire version. Between two unversioned
/// deliveries, arrival order still decides, and a replayed original still wins.
/// Only a version off the wire fixes that, and the `process.completed` payload
/// does not carry one yet.
fn version_of(mscons: Option<u128>, recorded_at: OffsetDateTime) -> Result<Version, EdmError> {
    match mscons {
        Some(v) => Version::mscons(v).map_err(store_err),
        None => Version::arrival(recorded_at).map_err(store_err),
    }
}

/// The next representable instant after `at`, for an inclusive upper bound.
///
/// meterstore's ranges are half-open `[from, to)`, and two of edmd's reading
/// questions are inclusive: "every Zählerstand in this window" and "the last one
/// **at or before** this bound" — a reading dated exactly at a period end did
/// hold at the period end, and § 40 Abs. 2 Nr. 6 EnWG puts that number on an
/// invoice. Instants are stored at microsecond precision, so one microsecond is
/// the smallest step that cannot skip a stored value.
fn inclusive_end(at: OffsetDateTime) -> OffsetDateTime {
    at + time::Duration::microseconds(1)
}

/// Every register a measuring point reported in a window, as `MeterRead`s.
///
/// **A MaLo is a set of registers, and this returns all of them.** meterstore
/// refuses to collect a range spanning two channels — a prosumer reports import
/// beside export at the same instants, and folded into one series `aggregate`
/// sums both and doubles the month. edmd's callers project the registers
/// themselves through [`crate::domain::register`], so the answer is the whole
/// point rather than one channel of it.
///
/// `collect_by_channel` is **one scan**, split per channel inside the store, so
/// every channel is resolved against the *same* tier boundary. A `SELECT
/// DISTINCT obis_code` plus one typed read each would be `1 + N` round trips
/// with N boundaries read at N different moments.
async fn read_all_channels(
    store: &MeterStore,
    malo_id: &str,
    tenant: &str,
    from: OffsetDateTime,
    to: OffsetDateTime,
    billable_only: bool,
) -> Result<Vec<MeterRead>, EdmError> {
    let mut q = store
        .series(malo(malo_id)?)
        .map_err(store_err)?
        .column_eq(TENANT_COL, tenant_scope(tenant))
        .map_err(store_err)?
        .range(from, to);
    if billable_only {
        q = q.quality_in(&billable_qualities());
    }
    let mut out: Vec<MeterRead> = q
        .collect_by_channel()
        .await
        .map_err(store_err)?
        .values()
        .flat_map(|r| series_to_reads(r, tenant))
        .collect();
    // Callers assume chronological order; the map is register-major.
    out.sort_by(|a, b| a.dtm_from.cmp(&b.dtm_from));
    Ok(out)
}

/// The unit a stored quantity is expressed in — always the Sparte's **billing**
/// unit, never its measured one.
///
/// Every ingest door converts gas from the m³ its register counts into kWh_Hs
/// before the value reaches the store (§ 25 Nr. 4 MessEV / DVGW G 685), so the
/// column holds kWh and must say kWh. Labelling it `CubicMetre` — the *measured*
/// unit — described a gas reading as roughly a tenth of itself to every consumer
/// that trusts the unit: the BO4E `Mengeneinheit` on `Zeitreihe`/`Energiemenge`,
/// and anything reading the cold tier through the Iceberg facade. Water is the
/// one Sparte whose measured and billed unit coincide (m³), so it is unaffected;
/// heat registers kWh_th on-device and needs no conversion either.
fn stored_unit(sparte: Sparte) -> MeasurementUnit {
    sparte.billing_unit()
}

fn read_to_stored(r: &MeterRead) -> Result<StoredSeries, EdmError> {
    read_to_stored_at(r, None)
}

/// [`read_to_stored`], optionally overriding the version the delivery resolves
/// under.
///
/// The override exists for values **edmd itself authors** — an operator
/// correction and a § 60 Abs. 2 Ersatzwert — which must supersede whatever
/// currently holds. See [`MeterStoreTimeSeriesRepository::append_superseding`].
fn read_to_stored_at(r: &MeterRead, version: Option<Version>) -> Result<StoredSeries, EdmError> {
    let obis: Option<ObisCode> = r.obis_code.as_deref().and_then(|s| ObisCode::parse(s).ok());
    let interval = MeterInterval {
        from: r.dtm_from,
        to: r.dtm_to,
        value: r.quantity_kwh,
        quality: r.quality,
        obis_code: obis,
    };
    // The operator that assigned the MSCONS version — the BDEW Codenummer of the
    // reporting NB/MSB, not the tenant.
    let operator = operator_code(r.sender_mp_id.as_deref().unwrap_or(&r.tenant))?;
    // The Bilanzierungsmonat the version scope names is Sparte-dependent: gas
    // balances on the Gastag, so 01.02. 03:00 still belongs to January.
    let scope = VersionScope::for_interval(operator, r.dtm_from, r.sparte).map_err(store_err)?;
    let recorded_at = r.valid_from_tx.unwrap_or_else(OffsetDateTime::now_utc);
    let version = match version {
        Some(v) => v,
        None => version_of(r.mscons_version, recorded_at)?,
    };

    let source = MeasurementSource::Mscons {
        pid: r.pid,
        message_ref: None,
        sender_mp_id: operator,
    };
    let mut series =
        MeasurementSeries::new(malo(&r.malo_id)?, obis, vec![interval], source, recorded_at);
    // A MeLo is optional on a reading; when present it must be a real
    // Zählpunktbezeichnung, so a malformed one is refused here rather than
    // stored as a string nobody can resolve.
    series.melo_id = r
        .melo_id
        .as_deref()
        .map(|m| {
            m.parse::<metering::MeloId>()
                .map_err(|e: metering::ParseError| input_err(format!("not a MeLo-ID: {m} ({e})")))
        })
        .transpose()?;

    Ok(StoredSeries::of(
        r.sparte,
        series,
        ScopedVersion::new(scope, version),
        recorded_at,
    )
    .in_unit(stored_unit(r.sparte))
    .with_extra("tenant", ScalarValue::Utf8(Some(r.tenant.clone())))
    // Provenance the MeasurementSeries cannot carry — recovered verbatim on
    // read-back rather than defaulted (see `series_to_reads`).
    .with_extra(
        "source",
        ScalarValue::Utf8(Some(r.source.as_str().to_owned())),
    )
    .with_extra("sender_mp_id", ScalarValue::Utf8(r.sender_mp_id.clone()))
    .with_extra(
        "allocation_version",
        ScalarValue::Utf8(Some(r.allocation_version.clone())),
    ))
}

impl TimeSeriesRepository for MeterStoreTimeSeriesRepository {
    async fn store_receipt(&self, receipt: &MeterDataReceipt) -> Result<(), EdmError> {
        // Receipts remain an edmd business table — unchanged from pg/timeseries.
        sqlx::query(
            r"INSERT INTO meter_data_receipts
                  (process_id, pid, malo_id, sender_mp_id, message_ref, received_at, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, $7)
              ON CONFLICT (process_id) DO NOTHING",
        )
        .bind(receipt.process_id)
        .bind(receipt.pid as i32)
        .bind(&receipt.malo_id)
        .bind(&receipt.sender_mp_id)
        .bind(&receipt.message_ref)
        .bind(receipt.received_at)
        .bind(&receipt.tenant)
        .execute(&self.pool)
        .await
        .map_err(pg_err)?;
        Ok(())
    }

    async fn store_reads(&self, reads: ValidatedReads) -> Result<(), EdmError> {
        let reads = reads.as_slice();
        if reads.is_empty() {
            return Ok(());
        }
        // Which door this batch came in by. Every caller builds a batch from one
        // ingest path, so the first row names it for the whole write; it is what
        // the § 147 AO audit rows below are attributed to.
        let batch_source = reads[0].source;
        // Enrol each distinct MaLo as an erasure subject and stamp the pseudonymous
        // reference on every row it owns. meterstore refuses a write whose
        // subject_ref does not resolve to a live mapping, and Article 17 erasure
        // works by destroying that mapping — so registration has to precede the
        // append. `register` is idempotent, so a re-ingest is one lookup per MaLo.
        let mut stored: Vec<StoredSeries> = Vec::with_capacity(reads.len());
        let mut refs: Vec<String> = Vec::with_capacity(reads.len());
        let mut subjects: HashMap<String, String> = HashMap::new();
        for r in reads {
            let natural = subject_natural_id(&r.tenant, &r.malo_id);
            let subject = match subjects.get(&natural) {
                Some(s) => s.clone(),
                None => {
                    let s = self
                        .store
                        .register_subject(&natural)
                        .await
                        .map_err(store_err)?
                        .as_str()
                        .to_owned();
                    subjects.insert(natural, s.clone());
                    s
                }
            };
            stored.push(
                read_to_stored(r)?
                    .with_extra("subject_ref", ScalarValue::Utf8(Some(subject.clone()))),
            );
            refs.push(subject);
        }

        // Route the whole batch through `append`: it splits current vs late
        // (below-watermark) intervals across the two tiers, and returns the
        // displacements that DRIVE the § 60 audit — no separate re-read of prior
        // state (which would race exactly when two corrections arrive together).
        //
        // A batch edmd **authored** takes the other route. A § 60 Abs. 2
        // Ersatzwert stands in for a reading the substitution logic has already
        // judged unusable, so it has to become the current value. Through the
        // plain batch append it would be silently outranked by any `FAULTY`
        // reading carrying a stated MSCONS version — which is exactly the case
        // Ersatzwertbildung exists for. See [`Self::append_superseding`].
        let mut displacements = self
            .store
            .append(&stored)
            .await
            .map_err(store_err)?
            .displacements;

        // Repair the writes that did not take.
        //
        // The batch append stays the fast path — one round trip for the whole
        // batch, which a month of quarter-hourly Ersatzwerte very much needs — and
        // only the intervals reported as `Shadowed`/`Duplicate` are re-asserted
        // individually. In ordinary traffic that is none of them.
        if batch_source.is_edmd_authored() {
            // Match a displacement back to the read that produced it by identity,
            // not by position: `append` documents one entry per interval but not
            // the order, and re-asserting the wrong interval would write one
            // value over another's slot.
            //
            // The register is keyed on its **canonical** spelling, because that is
            // what the store reports back: a read carrying `1-0:1.8.0*255` comes
            // back as `1-0:1.8.0`, and matching the raw string would miss it and
            // skip the repair without a word.
            let by_key: HashMap<(&str, String, OffsetDateTime), (&MeterRead, &String)> = reads
                .iter()
                .zip(&refs)
                .map(|(r, subject)| {
                    (
                        (
                            r.malo_id.as_str(),
                            normalise_obis(r.obis_code.as_deref()),
                            r.dtm_from,
                        ),
                        (r, subject),
                    )
                })
                .collect();
            for slot in &mut displacements {
                if slot.effect.changed_current_value() {
                    continue;
                }
                let key = (slot.malo_id.as_str(), slot.obis_code.clone(), slot.from);
                let Some((read, subject)) = by_key.get(&key).copied() else {
                    continue;
                };
                if let Some(d) = self.append_superseding(read, subject).await? {
                    *slot = d;
                }
            }
        }

        // ── § 60 Abs. 6 audit + § 60 Abs. 2 confirmation, from displacements ──
        for d in &displacements {
            if !d.effect.changed_current_value() {
                continue; // Shadowed / Duplicate: nothing became current.
            }
            let tenant = d
                .identity
                .iter()
                .find(|(k, _)| k == "tenant")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();

            if let Some(prior) = d.superseded.as_ref() {
                // A prior value stopped being current → immutable audit row.
                // § 146 Abs. 4 AO requires the superseded figure to stay
                // recoverable; the row is reconstructed from the write's own
                // report, never from a second read (which would race exactly
                // when two corrections arrive together).
                sqlx::query(
                    r"INSERT INTO meter_read_corrections
                          (malo_id, dtm_from, dtm_to, obis_code_norm,
                           original_kwh, original_quality, corrected_kwh, corrected_quality,
                           reason, source, corrected_by, tenant)
                      VALUES ($1,$2,$9,$3,$4,$5,$6,$7,$10,$11,$12,$8)",
                )
                .bind(&d.malo_id)
                .bind(d.from)
                .bind(&d.obis_code)
                .bind(prior.value)
                .bind(quality_to_str(prior.quality))
                .bind(d.written.value)
                .bind(quality_to_str(d.written.quality))
                .bind(&tenant)
                .bind(d.to)
                .bind(format!(
                    "Neulieferung über {door} überschreibt gespeichertes Intervall \
                     (§ 146 Abs. 4 AO)",
                    door = batch_source.as_str()
                ))
                .bind(correction_source_of(batch_source))
                .bind(format!("edmd-ingest:{}", batch_source.as_str()))
                .execute(&self.pool)
                .await
                .map_err(pg_err)?;
            }

            // § 60 Abs. 2: a measured/corrected value discharges an open
            // confirmation; an estimated/substituted one opens the obligation.
            match d.written.quality {
                QualityFlag::Measured | QualityFlag::Corrected => {
                    sqlx::query(
                        r"UPDATE estimated_read_confirmations
                          SET status='BESTAETIGT', resolved_at=now(), resolved_by=$5
                          WHERE tenant=$4 AND malo_id=$1 AND dtm_from=$2 AND obis_code_norm=$3
                            AND status IN ('OFFEN','UEBERFAELLIG')",
                    )
                    .bind(&d.malo_id)
                    .bind(d.from)
                    .bind(&d.obis_code)
                    .bind(&tenant)
                    .bind(batch_source.as_str())
                    .execute(&self.pool)
                    .await
                    .map_err(pg_err)?;
                }
                QualityFlag::Estimated | QualityFlag::Substituted => {
                    sqlx::query(
                        r"INSERT INTO estimated_read_confirmations
                              (tenant, malo_id, dtm_from, dtm_to, obis_code_norm, quality)
                          VALUES ($1,$2,$3,$6,$4,$5)
                          ON CONFLICT (tenant, malo_id, dtm_from, obis_code_norm) DO NOTHING",
                    )
                    .bind(&tenant)
                    .bind(&d.malo_id)
                    .bind(d.from)
                    .bind(&d.obis_code)
                    .bind(quality_to_str(d.written.quality))
                    .bind(d.to)
                    .execute(&self.pool)
                    .await
                    .map_err(pg_err)?;
                }
                _ => {}
            }
        }

        // Invalidate any cached billing-period aggregate the readings fall inside
        // — unchanged obligation from pg/timeseries.
        let periods: Vec<(String, String, OffsetDateTime, OffsetDateTime)> = {
            let mut acc: std::collections::HashMap<(&str, &str), (OffsetDateTime, OffsetDateTime)> =
                std::collections::HashMap::new();
            for r in reads {
                let e = acc
                    .entry((r.tenant.as_str(), r.malo_id.as_str()))
                    .or_insert((r.dtm_from, r.dtm_to));
                e.0 = e.0.min(r.dtm_from);
                e.1 = e.1.max(r.dtm_to);
            }
            acc.into_iter()
                .map(|((t, m), (from, to))| (t.to_owned(), m.to_owned(), from, to))
                .collect()
        };
        for (tenant, malo_id, from, to) in periods {
            if let Err(e) = sqlx::query(
                r"DELETE FROM meter_billing_periods
                  WHERE tenant = $1 AND malo_id = $2
                    AND period_from <= $4::date AND period_to >= $3::date",
            )
            .bind(&tenant)
            .bind(&malo_id)
            // Period bounds are Berlin calendar dates, so the reads' overlap
            // window must be expressed in Berlin days too.
            .bind(metering::calendar::local_day(from))
            .bind(metering::calendar::local_day(to))
            .execute(&self.pool)
            .await
            {
                tracing::warn!(%malo_id, error = %e,
                    "edmd: could not invalidate cached billing period after ingest");
            }
        }

        Ok(())
    }

    async fn query(&self, q: &TimeSeriesQuery) -> Result<Vec<MeterRead>, EdmError> {
        read_all_channels(&self.store, &q.malo_id, &q.tenant, q.from, q.to, false).await
    }

    async fn receipts(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<MeterDataReceipt>, EdmError> {
        // Business table — unchanged from pg/timeseries.
        let rows = sqlx::query(
            r"SELECT process_id, pid, malo_id, sender_mp_id, message_ref, received_at, tenant
              FROM meter_data_receipts
              WHERE malo_id = $1 AND received_at >= $2 AND received_at <= $3 AND tenant = $4
              ORDER BY received_at DESC",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(pg_err)?;
        rows.into_iter().map(|row| row_to_receipt(&row)).collect()
    }

    async fn imbalance(
        &self,
        malo_id: &str,
        from: Date,
        to: Date,
        tenant: &str,
        sparte: Sparte,
        bilanziert_kwh: Decimal,
    ) -> Result<ImbalanceReport, EdmError> {
        // Aggregate over the tier-split resolved series. The window is the
        // commodity's balancing period: calendar days for Strom, the
        // 06:00–06:00 Gastag for Gas.
        let (from_ts, to_ts) = period_window(sparte, from, to);
        // Only billable qualities enter the saldo — the FAULTY/UNKNOWN filter is
        // pushed into the scan rather than applied after materialising the
        // series.
        let reads = read_all_channels(&self.store, malo_id, tenant, from_ts, to_ts, true).await?;
        if reads.is_empty() {
            return Err(EdmError::NoData {
                malo_id: malo_id.to_owned(),
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        // A Mehr-/Mindermengensaldo settles the **grid draw**, so the series is
        // projected onto the Bezug registers before it is summed. The resolved
        // read spans every channel the measuring point reported, and folding
        // those together would settle a prosumer's Bezug plus its Einspeisung,
        // or a dual-tariff meter's consumption twice over (`domain::register`).
        let intervals = crate::domain::energy_intervals(&reads, EnergyDirection::Bezug);
        let gemessen: Decimal = intervals.iter().map(|iv| iv.value).sum();
        // The GPKE Kap. 8.4 arithmetic and its sign convention come from
        // `metering`, not from a second implementation here.
        let saldo = metering::compute_imbalance(gemessen, bilanziert_kwh);
        // The worst quality that actually contributed, not a fixed `UNKNOWN` —
        // a saldo built partly from Ersatzwerte is a different fact from one
        // built entirely from measurements, and the settlement side must see it.
        let quality = crate::domain::worst_quality(&intervals);
        Ok(ImbalanceReport {
            malo_id: malo_id.to_owned(),
            period_from: from,
            period_to: to,
            sparte,
            gemessen_kwh: saldo.actual_kwh,
            bilanziert_kwh: saldo.contracted_kwh,
            mehrmenge_kwh: saldo.mehr_kwh,
            mindermenge_kwh: saldo.minder_kwh,
            delta_kwh: saldo.delta_kwh,
            delta_pct: saldo.delta_pct(),
            quality,
            interval_count: intervals.len(),
        })
    }

    async fn latest_read(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Option<MeterRead>, EdmError> {
        // The newest interval is resolved with `ORDER BY from DESC LIMIT 1` at the
        // storage layer rather than by loading the whole history and taking the
        // maximum in memory.
        let resolved = self
            .store
            .series(malo(malo_id)?)
            .map_err(store_err)?
            .column_eq(TENANT_COL, tenant_scope(tenant))
            .map_err(store_err)?
            .latest_resolved()
            .await
            .map_err(store_err)?;
        Ok(resolved.and_then(|r| series_to_reads(&r, tenant).into_iter().next()))
    }

    async fn billing_period(
        &self,
        q: &BillingPeriodQuery,
    ) -> Result<Option<MeterBillingPeriod>, EdmError> {
        // 1. Cached aggregate (business table).
        let pre = sqlx::query(
            r"SELECT messtyp, sparte, arbeitsmenge_kwh, spitzenleistung_kw,
                     arbeitsmenge_ht_kwh, arbeitsmenge_nt_kwh,
                     brennwert_kwh_per_m3, zustandszahl,
                     zaehlerstand_anfang, zaehlerstand_ende, quality
              FROM meter_billing_periods
              WHERE malo_id = $1 AND period_from = $2 AND period_to = $3 AND tenant = $4",
        )
        .bind(&q.malo_id)
        .bind(q.period_from)
        .bind(q.period_to)
        .bind(&q.tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_err)?;

        if let Some(row) = pre {
            let dec = |c: &str| row.try_get::<Option<Decimal>, _>(c).ok().flatten();
            // A cache row that cannot be decoded is not a cache miss and not a
            // zero — it is a row edmd did not write, and answering from it would
            // put a guessed Sparte or a non-billable quality on an invoice. The
            // read falls through to the on-the-fly aggregation instead, which
            // recomputes from the resolved series and rewrites the row.
            let decoded = (|| {
                let sparte = str_to_sparte(&row.try_get::<String, _>("sparte").ok()?)?;
                let quality = str_to_quality(&row.try_get::<String, _>("quality").ok()?)?;
                Some((sparte, quality))
            })();
            let Some((sparte, quality)) = decoded else {
                tracing::warn!(
                    malo_id = %q.malo_id,
                    "edmd: cached billing period holds an unreadable sparte/quality — recomputing"
                );
                return self.recompute_billing_period(q).await;
            };
            let messtyp_str: String = row.try_get("messtyp").unwrap_or_else(|_| "SLP".into());
            return Ok(Some(MeterBillingPeriod {
                malo_id: q.malo_id.clone(),
                period_from: q.period_from,
                period_to: q.period_to,
                messtyp: messtyp_from_str(&messtyp_str),
                sparte,
                arbeitsmenge_kwh: row.try_get("arbeitsmenge_kwh").unwrap_or(Decimal::ZERO),
                arbeitsmenge_ht_kwh: dec("arbeitsmenge_ht_kwh"),
                arbeitsmenge_nt_kwh: dec("arbeitsmenge_nt_kwh"),
                spitzenleistung_kw: dec("spitzenleistung_kw"),
                brennwert_kwh_per_m3: dec("brennwert_kwh_per_m3"),
                zustandszahl: dec("zustandszahl"),
                zaehlerstand_anfang: dec("zaehlerstand_anfang"),
                zaehlerstand_ende: dec("zaehlerstand_ende"),
                quality,
                lastprofil: None,
                profil_typ: None,
            }));
        }

        self.recompute_billing_period(q).await
    }

    async fn record_gas_quality(
        &self,
        q: &crate::domain::GasQualityRecord,
    ) -> Result<u64, EdmError> {
        let mut tx = self.pool.begin().await.map_err(pg_err)?;

        // 1. The delivery itself. A re-delivery for the same period supersedes
        //    the previous values rather than appending a second row — the grid
        //    operator's latest published Brennwert for a month is the one that
        //    settles it.
        sqlx::query(
            r"INSERT INTO gas_quality_data
                  (malo_id, period_from, period_to, brennwert_kwh_per_m3,
                   zustandszahl, source_pid, tenant)
              VALUES ($1,$2,$3,$4,$5,$6,$7)
              ON CONFLICT (malo_id, period_from, period_to, tenant) DO UPDATE
                  SET brennwert_kwh_per_m3 =
                          COALESCE(EXCLUDED.brennwert_kwh_per_m3,
                                   gas_quality_data.brennwert_kwh_per_m3),
                      zustandszahl =
                          COALESCE(EXCLUDED.zustandszahl, gas_quality_data.zustandszahl),
                      source_pid  = COALESCE(EXCLUDED.source_pid, gas_quality_data.source_pid),
                      received_at = now()",
        )
        .bind(&q.malo_id)
        .bind(q.period_from)
        .bind(q.period_to)
        .bind(q.brennwert_kwh_per_m3)
        .bind(q.zustandszahl)
        .bind(q.source_pid.map(|p| p as i32))
        .bind(&q.tenant)
        .execute(&mut *tx)
        .await
        .map_err(pg_err)?;

        // 2. Backfill cached billing-period aggregates that overlap the period
        //    and are still missing their gas factors. Bounded by the delivery's
        //    own period: an unqualified update patched every period the MaLo
        //    ever had with whatever month happened to arrive last.
        let result = sqlx::query(
            r"UPDATE meter_billing_periods
              SET brennwert_kwh_per_m3 = COALESCE($2, brennwert_kwh_per_m3),
                  zustandszahl        = COALESCE($3, zustandszahl)
              WHERE malo_id = $1 AND tenant = $4
                AND period_from <= $6 AND period_to >= $5
                AND (brennwert_kwh_per_m3 IS NULL OR zustandszahl IS NULL)",
        )
        .bind(&q.malo_id)
        .bind(q.brennwert_kwh_per_m3)
        .bind(q.zustandszahl)
        .bind(&q.tenant)
        .bind(q.period_from)
        .bind(q.period_to)
        .execute(&mut *tx)
        .await
        .map_err(pg_err)?;

        tx.commit().await.map_err(pg_err)?;
        Ok(result.rows_affected())
    }

    async fn store_corrections(
        &self,
        records: &[CorrectionRecord],
    ) -> Result<Vec<uuid::Uuid>, EdmError> {
        use crate::domain::CorrectionSource;

        // The audit rows commit together or not at all. One autocommit
        // statement at a time leaves the earlier rows standing when a
        // multi-interval correction fails half-way — against the documented
        // "all or none" contract, and with no way for the caller to tell which
        // half landed. The store appends run outside this transaction
        // by necessity (meterstore owns its own tiers), so the ordering is:
        // audit rows staged → intervals appended → audit committed. A crash
        // between the append and the commit loses audit rows for values that did
        // change, which is the recoverable direction: the version history in
        // `meter_reads_versions` still holds both values.
        let mut tx = self.pool.begin().await.map_err(pg_err)?;
        let mut ids = Vec::with_capacity(records.len());
        let mut touched: Vec<(String, String, OffsetDateTime, OffsetDateTime)> =
            Vec::with_capacity(records.len());
        for rec in records {
            // 1. Immutable audit row — the original value is carried by the
            //    correction request itself. § 147 Abs. 1 AO / GoBD: the record
            //    of the superseded figure is a Buchungsbeleg and must stay
            //    unveränderbar (§ 146 Abs. 4 AO).
            let source_str = match rec.source {
                CorrectionSource::MsconsUpdate => "MSCONS_UPDATE",
                CorrectionSource::Operator => "OPERATOR",
                CorrectionSource::AutoSubstitute => "AUTO_SUBSTITUTE",
                CorrectionSource::ImsysDirectPush => "IMSYS_DIRECT_PUSH",
                CorrectionSource::Other => "OTHER",
            };
            let obis_norm = normalise_obis(rec.obis_code.as_deref());
            let row = sqlx::query(
                r"INSERT INTO meter_read_corrections
                      (malo_id, dtm_from, dtm_to, obis_code_norm,
                       original_kwh, original_quality, corrected_kwh, corrected_quality,
                       reason, source, corrected_by, process_id, pid, tenant)
                  VALUES ($1,$2,$3,$14,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
                  RETURNING correction_id",
            )
            .bind(&rec.malo_id)
            .bind(rec.dtm_from)
            .bind(rec.dtm_to)
            .bind(rec.original_kwh)
            .bind(quality_to_str(rec.original_quality))
            .bind(rec.corrected_kwh)
            .bind(quality_to_str(rec.corrected_quality))
            .bind(&rec.reason)
            .bind(source_str)
            .bind(&rec.corrected_by)
            .bind(rec.process_id)
            .bind(rec.pid.map(|p| p as i32))
            .bind(&rec.tenant)
            .bind(&obis_norm)
            .fetch_one(&mut *tx)
            .await
            .map_err(pg_err)?;
            let correction_id: uuid::Uuid = row.try_get("correction_id").map_err(pg_err)?;
            ids.push(correction_id);
            touched.push((
                rec.tenant.clone(),
                rec.malo_id.clone(),
                rec.dtm_from,
                rec.dtm_to,
            ));

            // 2. Append the corrected interval at a HIGHER version — the store
            //    routes it to the tier that owns the (possibly archived) interval
            //    and applies latest-version-wins on resolution.
            //
            //    The corrected interval already lives in the store; both its Sparte
            //    and the reporting operator are authoritative there. A
            //    `CorrectionRecord` carries neither, so they are recovered from the
            //    newest delivery rather than assumed. Defaulting the Sparte to Strom
            //    would relabel a gas or water correction as electricity on the very
            //    next read; dropping the operator would derive the version scope from
            //    the tenant instead — a *different* scope from the value being
            //    corrected — so the store's one-operator exclusion would reject the
            //    correction as a conflicting claim on the interval rather than accept
            //    it as the supersede it is.
            let (sparte, operator) = self
                .store
                .series(malo(&rec.malo_id)?)
                .map_err(store_err)?
                .column_eq(TENANT_COL, tenant_scope(&rec.tenant))
                .map_err(store_err)?
                // The correction is about one register, so the read is too: a
                // measuring point reporting import beside export has two rows
                // at these instants and a series can describe only one.
                .obis(rec.obis_code.as_deref().unwrap_or_default())
                .map_err(store_err)?
                .range(rec.dtm_from, rec.dtm_to)
                .collect_resolved()
                .await
                .map_err(store_err)?
                .map_or((Sparte::Strom, None), |r| {
                    (r.sparte, extra_str(&r.extra, "sender_mp_id"))
                });
            let corrected = MeterRead {
                malo_id: rec.malo_id.clone(),
                melo_id: None,
                dtm_from: rec.dtm_from,
                dtm_to: rec.dtm_to,
                quantity_kwh: rec.corrected_kwh,
                quality: rec.corrected_quality,
                pid: rec.pid.unwrap_or(0),
                sparte,
                obis_code: rec.obis_code.clone(),
                tenant: rec.tenant.clone(),
                source: crate::domain::IngestionSource::Correction,
                push_session: None,
                quality_warnings: None,
                // Preserve the original operator so the correction lands in the same
                // version scope as the value it supersedes (see the read above).
                sender_mp_id: operator,
                allocation_version: "CORRECTION".to_owned(),
                valid_from_tx: Some(OffsetDateTime::now_utc()),
                mscons_version: None,
            };
            // A correction is another write to the subject store, so it carries
            // the same pseudonymous reference; `register` returns the existing one.
            let subject = self
                .store
                .register_subject(&subject_natural_id(&rec.tenant, &rec.malo_id))
                .await
                .map_err(store_err)?;
            // A correction is edmd asserting a value, not a delivery arriving:
            // it must become current, whatever version the reading it supersedes
            // was delivered under. Failing here aborts the transaction, so the
            // audit row and the discharged confirmation roll back with it rather
            // than describing a correction that never took effect.
            self.append_superseding(&corrected, subject.as_str())
                .await?;

            // 3. § 60 Abs. 2: a corrected real value discharges the open
            //    confirmation for the slot.
            if matches!(
                rec.corrected_quality,
                QualityFlag::Measured | QualityFlag::Corrected
            ) {
                sqlx::query(
                    r"UPDATE estimated_read_confirmations
                      SET status='BESTAETIGT', resolved_at=now(), resolved_by=$5
                      WHERE tenant=$4 AND malo_id=$1 AND dtm_from=$2 AND obis_code_norm=$3
                        AND status IN ('OFFEN','UEBERFAELLIG')",
                )
                .bind(&rec.malo_id)
                .bind(rec.dtm_from)
                .bind(&obis_norm)
                .bind(&rec.tenant)
                .bind(source_str)
                .execute(&mut *tx)
                .await
                .map_err(pg_err)?;
            }
        }

        // 4. Drop the cached billing-period aggregates the corrected intervals
        //    fall inside. `billing_period` is read-through, so a stale cache row
        //    is not merely old — it is what `invoicd` and `netzbilanzd` invoice
        //    from, and it would keep serving the pre-correction total forever.
        //    The ingest path has always done this; the correction path did not,
        //    which made an explicit § 60 correction the one write that could not
        //    reach a bill.
        for (tenant, malo_id, from, to) in &touched {
            sqlx::query(
                r"DELETE FROM meter_billing_periods
                  WHERE tenant = $1 AND malo_id = $2
                    AND period_from <= $4::date AND period_to >= $3::date",
            )
            .bind(tenant)
            .bind(malo_id)
            .bind(metering::calendar::local_day(*from))
            .bind(metering::calendar::local_day(*to))
            .execute(&mut *tx)
            .await
            .map_err(pg_err)?;
        }

        tx.commit().await.map_err(pg_err)?;
        Ok(ids)
    }

    async fn store_readings(&self, readings: &[MeterReading]) -> Result<u64, EdmError> {
        if readings.is_empty() {
            return Ok(0);
        }
        // A Zählerstandsgang is one register of one meter, so the batch is
        // grouped by `(MeLo, OBIS)` and each group appended as its own delivery.
        // Both are part of a reading's identity: a Marktlokation may be measured
        // by several Messlokationen, and two meters carry the same register at
        // the same instants.
        let mut groups: std::collections::BTreeMap<(String, String), Vec<&MeterReading>> =
            std::collections::BTreeMap::new();
        for r in readings {
            let melo = r.melo_id.clone().ok_or_else(|| {
                EdmError::Internal(format!(
                    "a Zählerstand for MaLo {} names no Messlokation. The register belongs to \
                     the meter, and two meters under one Marktlokation carry the same OBIS code \
                     at the same instants — stored without it, the second silently overwrites \
                     the first",
                    r.malo_id
                ))
            })?;
            groups
                .entry((melo, normalise_obis(r.obis_code.as_deref())))
                .or_default()
                .push(r);
        }

        let mut written = 0_u64;
        for ((melo, obis), rows) in groups {
            let first = rows[0];
            let obis: ObisCode = obis.parse().map_err(|_| {
                EdmError::Internal(format!(
                    "a Zählerstandsgang must name the register it comes from; {:?} is not an \
                     OBIS code",
                    first.obis_code
                ))
            })?;
            let values: Vec<metering::reading::MeterReading> = rows
                .iter()
                .map(|r| metering::reading::MeterReading {
                    at: r.read_at,
                    value: r.zaehlerstand,
                    quality: r.quality,
                    obis_code: Some(obis),
                })
                .collect();

            let natural = subject_natural_id(&first.tenant, &first.malo_id);
            let subject = self
                .zsg
                .register_subject(&natural)
                .await
                .map_err(store_err)?
                .as_str()
                .to_owned();

            let operator = first
                .sender_mp_id
                .clone()
                .unwrap_or_else(|| first.tenant.clone());
            let scope = VersionScope::for_interval(operator.as_str(), first.read_at, first.sparte)
                .map_err(store_err)?;
            let recorded_at = OffsetDateTime::now_utc();

            let melo: metering::MeloId = melo.parse().map_err(|e: metering::ParseError| {
                input_err(format!("not a MeLo-ID: {melo} ({e})"))
            })?;
            let mut stored = meterstore::encode::StoredReadings::new(
                malo(&first.malo_id)?,
                obis,
                first.sparte,
                values.clone(),
                // The door the delivery came in by, as the reading records it.
                match first.source {
                    crate::domain::IngestionSource::Manual => MeasurementSource::ManualEntry {
                        operator_id: first
                            .sender_mp_id
                            .clone()
                            .unwrap_or_else(|| first.tenant.clone()),
                        reason: "Ablesung".to_owned(),
                    },
                    _ => MeasurementSource::Mscons {
                        pid: 0,
                        message_ref: first.push_session.clone(),
                        sender_mp_id: operator_code(
                            first.sender_mp_id.as_deref().unwrap_or(&first.tenant),
                        )?,
                    },
                },
                ScopedVersion::new(scope, version_of(None, recorded_at)?),
                recorded_at,
            )
            // A Zählerstand is stored in the unit the **register** counts, never
            // the one the Sparte settles in: the conversion applies to the
            // difference (§ 25 Nr. 4 MessEV), and a converted register value is
            // no longer the number on the meter.
            .in_unit(first.sparte.measured_unit())
            .with_melo_id(melo)
            .with_extra("subject_ref", ScalarValue::Utf8(Some(subject)))
            .with_extra(TENANT_COL, ScalarValue::Utf8(Some(first.tenant.clone())))
            .with_extra(
                "source",
                ScalarValue::Utf8(Some(first.source.as_str().to_owned())),
            );
            // How often the register is read, derived from the timestamps rather
            // than assumed — § 2 Satz 1 Nr. 27 MsbG names a quarter-hourly Strom
            // and an hourly Gas cadence, and completeness asks "how many values a
            // day should there be".
            if let Some(cadence) = metering::reading::detect_reading_cadence(&values) {
                stored = stored.at_cadence(cadence);
            }
            if let Some(mp) = &first.sender_mp_id {
                stored = stored.with_extra("sender_mp_id", ScalarValue::Utf8(Some(mp.clone())));
            }

            let outcome = self
                .zsg
                .append_readings(&[stored])
                .await
                .map_err(store_err)?;
            written += outcome.hot_rows + outcome.cold_rows;
        }
        Ok(written)
    }

    async fn readings(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<MeterReading>, EdmError> {
        // Every register of every meter under the Marktlokation. `collect` reads
        // one channel of one meter; `deliveries` is the multi-channel shape.
        let deliveries = self
            .zsg
            .readings(malo(malo_id)?)
            .map_err(store_err)?
            .column_eq(TENANT_COL, tenant_scope(tenant))
            .map_err(store_err)?
            // The trait's window is inclusive on both ends.
            .range(from, inclusive_end(to))
            .deliveries()
            .await
            .map_err(store_err)?;

        let mut out: Vec<MeterReading> = Vec::new();
        for d in deliveries {
            for r in &d.readings {
                out.push(MeterReading {
                    malo_id: d.malo_id.as_str().to_owned(),
                    read_at: r.at,
                    zaehlerstand: r.value,
                    quality: r.quality,
                    sparte: d.sparte,
                    obis_code: Some(d.obis_code.to_string()),
                    melo_id: d.melo_id.as_ref().map(|m| m.as_str().to_owned()),
                    tenant: tenant.to_owned(),
                    source: extra_str(&d.extra, "source")
                        .map_or_else(crate::domain::IngestionSource::default, |s| {
                            crate::domain::IngestionSource::from_db_str(&s)
                        }),
                    sender_mp_id: extra_str(&d.extra, "sender_mp_id"),
                    push_session: None,
                });
            }
        }
        out.sort_by_key(|r| r.read_at);
        Ok(out)
    }

    async fn log_zsg_conversion(
        &self,
        entries: &[crate::domain::ZsgConversionEntry],
    ) -> Result<(), EdmError> {
        for e in entries {
            sqlx::query(
                r"INSERT INTO zsg_conversion_log
                      (tenant, malo_id, obis_code_norm, span_from, span_to, outcome,
                       previous_value, current_value, delta, register_capacity, session_id)
                  VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
            )
            .bind(&e.tenant)
            .bind(&e.malo_id)
            .bind(&e.obis_code_norm)
            .bind(e.span_from)
            .bind(e.span_to)
            .bind(e.outcome)
            .bind(e.previous_value)
            .bind(e.current_value)
            .bind(e.delta)
            .bind(e.register_capacity)
            .bind(&e.session_id)
            .execute(&self.pool)
            .await
            .map_err(pg_err)?;
        }
        Ok(())
    }

    async fn period_zaehlerstaende(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        tenant: &str,
    ) -> Result<(Option<Decimal>, Option<Decimal>), EdmError> {
        let Some((melo, register)) = self.dominant_register(malo_id, tenant).await? else {
            return Ok((None, None));
        };
        // "At or before", on both ends. A Zählerstand dated after the period end
        // did not hold at the period end, and reporting it as the closing
        // reading overstates the period on an invoice a customer checks against
        // the meter. `until(bound).latest()` is exactly that, resolved at the
        // storage layer rather than by folding the history in memory.
        let at_or_before = async |bound: OffsetDateTime| -> Result<Option<Decimal>, EdmError> {
            Ok(self
                .zsg
                .readings(malo(malo_id)?)
                .map_err(store_err)?
                .column_eq(TENANT_COL, tenant_scope(tenant))
                .map_err(store_err)?
                .melo(melo.as_str())
                .map_err(store_err)?
                .obis(&register)
                .map_err(store_err)?
                // "At or before": a Zählerstand dated exactly at the bound held
                // at the bound.
                .until(inclusive_end(bound))
                .latest()
                .await
                .map_err(store_err)?
                .map(|r| r.value))
        };
        let anfang = at_or_before(from).await?;
        let ende = at_or_before(to).await?;
        Ok((anfang, ende))
    }
}

// ── Zählerstandsgang (BK6-24-174) ────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// ESA "Werte nach Typ 2" store — its own meterstore table (never billed).
// ─────────────────────────────────────────────────────────────────────────────

/// Meterstore-backed ESA "Werte nach Typ 2" store.
#[derive(Clone)]
pub struct MeterStoreTyp2Repository {
    store: MeterStore,
}

impl MeterStoreTyp2Repository {
    /// Wrap the Typ-2 meterstore table.
    #[must_use]
    pub fn new(store: MeterStore) -> Self {
        Self { store }
    }

    /// The underlying store.
    #[must_use]
    pub fn store(&self) -> &MeterStore {
        &self.store
    }
}

fn typ2_to_stored(r: &Typ2Read) -> Result<StoredSeries, EdmError> {
    let obis: Option<ObisCode> = r.obis_code.as_deref().and_then(|s| ObisCode::parse(s).ok());
    let interval = MeterInterval {
        from: r.dtm_from,
        to: r.dtm_to,
        value: r.quantity_kwh,
        quality: r.quality,
        obis_code: obis,
    };
    let operator = operator_code(r.sender_mp_id.as_deref().unwrap_or(&r.tenant))?;
    let scope = VersionScope::for_interval(operator, r.dtm_from, r.sparte).map_err(store_err)?;
    let recorded_at = r.received_at.unwrap_or_else(OffsetDateTime::now_utc);
    // Typ-2 values are stored as delivered and never corrected, so arrival order
    // is the only order there is — but it still has to be an order, hence the
    // millisecond resolution (see `version_of`): two deliveries in the same
    // second are two versions rather than one merge-key conflict.
    let version = version_of(None, recorded_at)?;
    let source = MeasurementSource::Mscons {
        pid: r.pid,
        message_ref: None,
        sender_mp_id: operator,
    };
    let mut series =
        MeasurementSeries::new(malo(&r.malo_id)?, obis, vec![interval], source, recorded_at);
    // A MeLo is optional on a reading; when present it must be a real
    // Zählpunktbezeichnung, so a malformed one is refused here rather than
    // stored as a string nobody can resolve.
    series.melo_id = r
        .melo_id
        .as_deref()
        .map(|m| {
            m.parse::<metering::MeloId>()
                .map_err(|e: metering::ParseError| input_err(format!("not a MeLo-ID: {m} ({e})")))
        })
        .transpose()?;
    Ok(StoredSeries::of(
        r.sparte,
        series,
        ScopedVersion::new(scope, version),
        recorded_at,
    )
    .in_unit(stored_unit(r.sparte))
    .with_extra("tenant", ScalarValue::Utf8(Some(r.tenant.clone())))
    // The reporting operator's MP-ID and the transport the value arrived on,
    // both recovered on read-back. `source` and `allocation_version` do not
    // apply to the non-authoritative Typ-2 stream and are not declared for it.
    .with_extra("sender_mp_id", ScalarValue::Utf8(r.sender_mp_id.clone()))
    .with_extra(
        "delivery_path",
        ScalarValue::Utf8(Some(r.delivery_path.as_str().to_owned())),
    )
    .with_extra(
        "bestellung_ref",
        ScalarValue::Utf8(r.bestellung_ref.clone()),
    ))
}

impl Typ2Repository for MeterStoreTyp2Repository {
    async fn store_typ2_reads(&self, reads: &[Typ2Read]) -> Result<(), EdmError> {
        if reads.is_empty() {
            return Ok(());
        }
        // Same erasure enrolment as the authoritative store, and deliberately the
        // same natural id: one `(tenant, MaLo)` subject, one mapping, so a single
        // Article 17 erasure unlinks the Typ-2 readings along with the billed
        // ones. Without it the ESA stream was the one place an erased MaLo stayed
        // named. `register_subject` is idempotent, so this is one lookup per MaLo.
        let mut subjects: HashMap<String, String> = HashMap::new();
        let mut stored: Vec<StoredSeries> = Vec::with_capacity(reads.len());
        for r in reads {
            let natural = subject_natural_id(&r.tenant, &r.malo_id);
            let subject = match subjects.get(&natural) {
                Some(s) => s.clone(),
                None => {
                    let s = self
                        .store
                        .register_subject(&natural)
                        .await
                        .map_err(store_err)?
                        .as_str()
                        .to_owned();
                    subjects.insert(natural, s.clone());
                    s
                }
            };
            stored.push(
                typ2_to_stored(r)?.with_extra("subject_ref", ScalarValue::Utf8(Some(subject))),
            );
        }
        self.store.append(&stored).await.map_err(store_err)?;
        Ok(())
    }

    /// Every Typ-2 register a Meldepunkt was delivered in a window.
    ///
    /// **Multi-register, and it has to be.** The QUOTES 15003 Angebot names one
    /// to 23 `PIA+5 … :SRW` OBIS-Kennzahlen per subscription (QUOTES AHB 1.1a
    /// §4.3, condition `[2073]`), and one Meldepunkt may carry several
    /// subscriptions — a subscription is the (Meldepunkt, Messprodukt) pair.
    ///
    /// meterstore declines a range spanning two channels, correctly: a
    /// `MeasurementSeries` holds one `obis_code`, and folding import beside
    /// export puts two values at every instant. So this is one scan split per
    /// channel inside the store, with every register resolved against the same
    /// tier boundary.
    async fn query_typ2(&self, q: &TimeSeriesQuery) -> Result<Vec<Typ2Read>, EdmError> {
        let by_channel = self
            .store
            .series(malo(&q.malo_id)?)
            .map_err(store_err)?
            .column_eq(TENANT_COL, tenant_scope(&q.tenant))
            .map_err(store_err)?
            .range(q.from, q.to)
            .collect_by_channel()
            .await
            .map_err(store_err)?;
        let mut out: Vec<Typ2Read> = by_channel
            .values()
            .flat_map(|r| series_to_typ2(r, &q.tenant))
            .collect();
        // Callers read a delivery, which is chronological; the map is
        // register-major.
        out.sort_by(|a, b| a.dtm_from.cmp(&b.dtm_from));
        Ok(out)
    }
}

// ── mapping helpers ─────────────────────────────────────────────────────────

/// The `meter_read_corrections.source` category an ingest door falls into.
///
/// The audit row records *who* superseded a stored value, so it is derived from
/// the ingestion source rather than filed as a blanket `MSCONS_UPDATE`. Filed
/// that way, an Ersatzwert overwriting a faulty slot and an SMGW re-push
/// correcting one both read, in the § 147 AO trail, as an EDIFACT redelivery
/// that never happened.
fn correction_source_of(source: crate::domain::IngestionSource) -> &'static str {
    use crate::domain::IngestionSource as S;
    match source {
        S::Mscons => "MSCONS_UPDATE",
        S::DirectPush | S::DirectGas => "IMSYS_DIRECT_PUSH",
        S::AutoSubstitute => "AUTO_SUBSTITUTE",
        S::Manual | S::Estimated => "OPERATOR",
        // An API import, an IoT uplink, or a value written by the correction
        // endpoint (which files its own row) — none of them is one of the four
        // named categories, and guessing would put a false one in the trail.
        S::ApiImport | S::IotPush | S::Correction => "OTHER",
    }
}

/// The § 60 Abs. 2 MsbG billable quality set — every flag except `FAULTY` and
/// `UNKNOWN`. Derived from [`metering::QualityFlag::is_billable`] so it cannot
/// drift from the domain rule, and passed to
/// [`SeriesQuery::quality_in`](meterstore::SeriesQuery::quality_in) to push the
/// filter into the scan instead of re-deriving `NOT IN ('FAULTY','UNKNOWN')` at
/// each call site.
fn billable_qualities() -> Vec<QualityFlag> {
    QualityFlag::ALL
        .iter()
        .copied()
        .filter(|q| q.is_billable())
        .collect()
}

/// A stored string attribute column, or `None` when absent / NULL.
fn extra_str(extra: &std::collections::BTreeMap<String, ScalarValue>, key: &str) -> Option<String> {
    match extra.get(key) {
        Some(ScalarValue::Utf8(v) | ScalarValue::LargeUtf8(v)) => v.clone(),
        _ => None,
    }
}

fn series_to_reads(resolved: &meterstore::ResolvedSeries, tenant: &str) -> Vec<MeterRead> {
    let series = &resolved.series;
    let pid = match &series.source {
        MeasurementSource::Mscons { pid, .. } => *pid,
        _ => 0,
    };
    // Provenance recovered from the newest contributing delivery's attribute
    // columns rather than hard-coded — a read-back MeterRead names its true
    // ingestion source, reporting operator and allocation version.
    let source = extra_str(&resolved.extra, "source")
        .map_or_else(crate::domain::IngestionSource::default, |s| {
            crate::domain::IngestionSource::from_db_str(&s)
        });
    let sender_mp_id = extra_str(&resolved.extra, "sender_mp_id");
    let allocation_version =
        extra_str(&resolved.extra, "allocation_version").unwrap_or_else(|| "INITIAL".to_owned());
    series
        .intervals
        .iter()
        .map(|iv| MeterRead {
            malo_id: series.malo_id.to_string(),
            melo_id: series.melo_id.as_ref().map(ToString::to_string),
            dtm_from: iv.from,
            dtm_to: iv.to,
            quantity_kwh: iv.value,
            quality: iv.quality,
            pid,
            sparte: resolved.sparte,
            obis_code: iv.obis_code.map(|o| o.to_string()),
            tenant: tenant.to_string(),
            source,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: sender_mp_id.clone(),
            allocation_version: allocation_version.clone(),
            valid_from_tx: None,
            mscons_version: None,
        })
        .collect()
}

fn series_to_typ2(resolved: &meterstore::ResolvedSeries, tenant: &str) -> Vec<Typ2Read> {
    let series = &resolved.series;
    let pid = match &series.source {
        MeasurementSource::Mscons { pid, .. } => *pid,
        _ => 13027,
    };
    let sender_mp_id = extra_str(&resolved.extra, "sender_mp_id");
    let bestellung_ref = extra_str(&resolved.extra, "bestellung_ref");
    let delivery_path = extra_str(&resolved.extra, "delivery_path")
        .map_or(Typ2DeliveryPath::MsconsBackend, |p| {
            Typ2DeliveryPath::from_db_str(&p)
        });
    series
        .intervals
        .iter()
        .map(|iv| Typ2Read {
            malo_id: series.malo_id.to_string(),
            melo_id: series.melo_id.as_ref().map(ToString::to_string),
            dtm_from: iv.from,
            dtm_to: iv.to,
            quantity_kwh: iv.value,
            quality: iv.quality,
            pid,
            sparte: resolved.sparte,
            obis_code: iv.obis_code.map(|o| o.to_string()),
            tenant: tenant.to_string(),
            delivery_path,
            sender_mp_id: sender_mp_id.clone(),
            bestellung_ref: bestellung_ref.clone(),
            received_at: None,
        })
        .collect()
}

fn row_to_receipt(row: &sqlx::postgres::PgRow) -> Result<MeterDataReceipt, EdmError> {
    Ok(MeterDataReceipt {
        process_id: row.try_get("process_id").map_err(pg_err)?,
        pid: row.try_get::<i32, _>("pid").map_err(pg_err)? as u32,
        malo_id: row.try_get("malo_id").map_err(pg_err)?,
        sender_mp_id: row.try_get("sender_mp_id").map_err(pg_err)?,
        message_ref: row.try_get("message_ref").map_err(pg_err)?,
        received_at: row.try_get("received_at").map_err(pg_err)?,
        tenant: row.try_get("tenant").map_err(pg_err)?,
    })
}

/// Canonical form of an OBIS code as it enters the audit-row key.
///
/// [`crate::domain::normalise_obis_code`] and nothing else — the store, the
/// validator and the BO4E export each had their own copy, and a merge key that
/// normalises differently from the group it is validated in is two registers
/// wearing one name.
use crate::domain::normalise_obis_code as normalise_obis;

/// The canonical wire spelling of a quality flag, shared with the API surface so
/// a response cannot invent a different vocabulary than the one stored.
///
/// [`QualityFlag::as_str`] and nothing else: a hand-written `match` here would be
/// a second copy of a vocabulary `metering` publishes, free to drift from the
/// `FromStr` that reads it back and the DB `CHECK` that constrains it.
pub(crate) fn quality_to_str(q: QualityFlag) -> &'static str {
    q.as_str()
}

/// Read a `quality` column back, or `None` when the value is not one edmd wrote.
///
/// `metering`'s `FromStr` refuses unknown input deliberately: an unrecognised
/// status is not `Unknown` — which is a statement about the *measurement* — but
/// a parse failure, which is a statement about the *storage*. The lenient
/// version this replaced mapped a corrupted or newly-added code onto `Unknown`,
/// and `Unknown` is non-billable, so a cache row that failed to decode silently
/// removed a period's Arbeitsmenge from every bill.
fn str_to_quality(s: &str) -> Option<QualityFlag> {
    s.parse().ok()
}

/// Read a `sparte` column back, or `None` when the value is not one edmd wrote.
///
/// Same reasoning: defaulting to `Strom` relabelled a cached gas period as
/// electricity, and the Sparte decides the unit and the balancing day.
fn str_to_sparte(s: &str) -> Option<Sparte> {
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    /// The wire version, when there is one, is what resolution orders by — so a
    /// correction stays superseding no matter when a replay of the original
    /// arrives.
    #[test]
    fn a_wire_version_outranks_arrival_order() {
        let original = version_of(Some(20_260_701_100_000), datetime!(2026-07-01 10:00 UTC))
            .expect("well-formed MSCONS version");
        // The correction was issued earlier in wall-clock terms than the replay
        // of the original, which lands a month later.
        let correction = version_of(Some(20_260_702_090_000), datetime!(2026-07-02 09:00 UTC))
            .expect("well-formed MSCONS version");
        let replay = version_of(Some(20_260_701_100_000), datetime!(2026-08-15 12:00 UTC))
            .expect("well-formed MSCONS version");

        assert!(correction.get() > original.get());
        assert_eq!(
            replay.get(),
            original.get(),
            "a replay carries the version it was issued under, not the one it \
             arrived at, so it cannot supersede the correction"
        );
        assert!(replay.get() < correction.get());
    }

    /// A version MSCONS would not have issued fails at the boundary rather than
    /// entering the resolved view, where a short version still orders — wrongly
    /// but silently — within its scope.
    #[test]
    fn a_malformed_wire_version_is_refused() {
        assert!(version_of(Some(7), datetime!(2026-07-01 10:00 UTC)).is_err());
    }

    /// Without a wire version, arrival time stands in — in milliseconds, so two
    /// writes in the same second are two versions rather than one
    /// `(merge_key, version)` conflict on the hot tier.
    #[test]
    fn the_arrival_time_fallback_separates_writes_inside_one_second() {
        let first = version_of(None, datetime!(2026-07-01 10:00:00.100 UTC)).expect("version");
        let second = version_of(None, datetime!(2026-07-01 10:00:00.200 UTC)).expect("version");
        assert!(first.get() < second.get());
    }

    /// The fallback stays *below* the MSCONS band on purpose: a delivery that
    /// states its version must beat one that does not, in either arrival order.
    /// Finer than milliseconds would cross into 14+ digits and the two schemes
    /// would interleave by accident.
    #[test]
    fn a_stated_version_always_outranks_the_arrival_time_fallback() {
        let fallback = version_of(None, datetime!(2026-07-01 10:00:00.100 UTC)).expect("version");
        assert!(
            !fallback.is_well_formed(),
            "13 digits — deliberately not a well-formed MSCONS version"
        );

        // The smallest version MSCONS could assign, arriving before the
        // fallback was written, still wins.
        let smallest_stated =
            version_of(Some(10_000_000_000_000), datetime!(2020-01-01 0:00 UTC)).expect("version");
        assert!(smallest_stated.get() > fallback.get());
    }

    /// A stored quantity carries the unit it is stored *in*, which for gas is
    /// kWh — not the m³ its register counts.
    ///
    /// Every ingest door applies the Brennwert conversion before the value
    /// reaches the store, so tagging gas `CubicMetre` described the reading as
    /// roughly a tenth of itself to everything that reads the unit: the BO4E
    /// `Mengeneinheit` on an exported `Zeitreihe`, and any external engine
    /// reading the cold tier through the Iceberg facade.
    #[test]
    fn a_stored_quantity_is_labelled_in_the_unit_it_is_stored_in() {
        assert_eq!(
            stored_unit(Sparte::Gas),
            MeasurementUnit::KiloWattHour,
            "gas is converted at ingest (§ 25 Nr. 4 MessEV) and stored as energy"
        );
        assert_eq!(stored_unit(Sparte::Strom), MeasurementUnit::KiloWattHour);
        assert_eq!(
            stored_unit(Sparte::Waerme),
            MeasurementUnit::KiloWattHour,
            "a heat meter integrates on-device and registers kWh_th"
        );
        assert_eq!(
            stored_unit(Sparte::Wasser),
            MeasurementUnit::CubicMetre,
            "water is the one Sparte measured and billed in the same unit"
        );
        for sparte in Sparte::ALL {
            assert_eq!(
                stored_unit(sparte),
                sparte.billing_unit(),
                "{} must be stored in its settlement unit",
                sparte.as_str()
            );
        }
    }

    /// Gas balances on the Gastag, electricity on the calendar day.
    ///
    /// The failure this pins is silent and daily: aggregating a gas Lastgang
    /// over calendar days books the 00:00–06:00 draw into the neighbouring
    /// Bilanzierungstag. Not a DST edge case — every day of the year, six hours
    /// of it, on a settlement figure the BIKO reconciles.
    #[test]
    fn a_gas_period_runs_on_the_gastag_and_strom_on_the_calendar_day() {
        use time::macros::{date, datetime};

        let day = date!(2026 - 01 - 15);

        let (from, to) = period_window(Sparte::Strom, day, day);
        assert_eq!(
            from,
            datetime!(2026-01-14 23:00 UTC),
            "Strom starts 00:00 Berlin"
        );
        assert_eq!(to, datetime!(2026-01-15 23:00 UTC));

        let (from, to) = period_window(Sparte::Gas, day, day);
        assert_eq!(
            from,
            datetime!(2026-01-15 5:00 UTC),
            "Gas starts 06:00 Berlin"
        );
        assert_eq!(to, datetime!(2026-01-16 5:00 UTC));

        // Six hours apart, which is the whole point.
        assert_eq!(
            (metering::calendar::gas_day_start_utc(day) - metering::calendar::day_start_utc(day))
                .whole_hours(),
            6
        );
    }

    /// The long Gastag is the one named after the **Saturday**.
    ///
    /// The clocks change at 03:00 local on the Sunday, which lies *inside* the
    /// gas day that began 06:00 on Saturday — so Saturday's Gastag is 25 hours
    /// and Sunday's is a normal 24. A period keyed on the Sunday would be the
    /// intuitive guess and the wrong one.
    #[test]
    fn the_long_gastag_belongs_to_the_saturday() {
        use time::macros::date;

        let saturday = date!(2026 - 10 - 24);
        let sunday = date!(2026 - 10 - 25);

        let (from, to) = period_window(Sparte::Gas, saturday, saturday);
        assert_eq!(
            (to - from).whole_hours(),
            25,
            "Saturday's Gastag is 25 hours"
        );

        let (from, to) = period_window(Sparte::Gas, sunday, sunday);
        assert_eq!((to - from).whole_hours(), 24, "Sunday's is an ordinary day");

        // Electricity's long day *is* the Sunday — the two calendars differ.
        let (from, to) = period_window(Sparte::Strom, sunday, sunday);
        assert_eq!((to - from).whole_hours(), 25);
    }
}
