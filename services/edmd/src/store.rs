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
    BillingPeriodQuery, CorrectionRecord, ImbalanceReport, Messtyp, MeterBillingPeriod,
    MeterDataReceipt, MeterRead, QualityFlag, Sparte, TimeSeriesQuery, Typ2DeliveryPath, Typ2Read,
    error::EdmError,
    messtyp_as_str, messtyp_from_str,
    repository::{TimeSeriesRepository, Typ2Repository},
};

fn store_err(e: impl std::fmt::Display) -> EdmError {
    EdmError::Database(e.to_string())
}

/// The identity column every store is keyed on. A reading is unique only *within*
/// a tenant, so every series read is scoped by it — an unscoped read would fold
/// two tenants' readings for one MaLo into a single series.
const TENANT_COL: &str = "tenant";

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
    /// Cold-tier partition granularity.
    pub partition_step: time::Duration,
    /// How far each archival sweep advances the tiering watermark.
    pub archival_step: time::Duration,
    /// Target Parquet file size in the cold tier, in bytes.
    pub cold_file_target_bytes: usize,
}

impl Default for TieringConfig {
    fn default() -> Self {
        Self {
            settlement_lag: time::Duration::weeks(1),
            partition_step: time::Duration::DAY,
            archival_step: time::Duration::DAY,
            cold_file_target_bytes: 512 * 1024 * 1024,
        }
    }
}

/// The `SqlCatalog` metadata namespace key and the Iceberg namespace the cold
/// tables live in — edmd invariants, so they are fixed here rather than in config.
const CATALOG_NAME: &str = "meterstore";
const CATALOG_NAMESPACE: &str = "metering";
/// The `SqlCatalog`'s own metadata pool, bound small: this single catalog (shared
/// by both tables) plus edmd's main pool and meterstore's hot tier must not
/// exhaust PostgreSQL's connection slots (the `SqlCatalog` default is 10).
const CATALOG_POOL_MAX: u32 = 4;

/// Build edmd's two meter-data tables over **one** shared Iceberg catalog and
/// **one** DataFusion session.
///
/// edmd's storage is exactly the shape [`meterstore::MeterCatalog`] exists for: an
/// authoritative `meter_reads` store beside a non-authoritative `esa_typ2_reads`
/// stream that must never reach a billing query. Building them as a catalog rather
/// than two independent handles means one `SqlCatalog` (one metadata pool) and one
/// `SessionContext` instead of two of each — the tables still keep their own
/// watermark, archiver and tiering. Each returned [`MeterStore`] is a cheap clone
/// sharing that session; the [`meterstore::ColdTier`] carries the
/// [`catalog_facade`](meterstore::ColdTier::catalog_facade) for the read-only
/// Iceberg REST endpoint.
///
/// The cold tier (the `SqlCatalog` + object-store backend chosen from
/// `warehouse_uri`'s scheme) is built by `meterstore` itself — edmd wires none of
/// the Iceberg catalog stack directly. Returns `(reads, typ2, cold)`.
pub async fn build_stores(
    pool: PgPool,
    database_url: &str,
    warehouse_uri: &str,
    tiering: TieringConfig,
    auth: &WarehouseAuth,
) -> anyhow::Result<(MeterStore, MeterStore, meterstore::ColdTier)> {
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
    let hot = Arc::new(PostgresHot::new(pool.clone()));

    // `meter_reads` is authoritative and carries the GDPR subject registry;
    // `esa_typ2_reads` is the non-authoritative ESA stream (never billed).
    let reads = table_builder(&cold, &hot, &pool, READS_TABLE, true, tiering).await?;
    let typ2 = table_builder(&cold, &hot, &pool, TYP2_TABLE, false, tiering).await?;

    let catalog = MeterCatalog::builder()
        .table(reads)
        .table(typ2)
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
    Ok((reads_store, typ2_store, cold_tier))
}

/// The per-table [`MeterStoreBuilder`], ready to join a [`MeterCatalog`].
///
/// `tenant` is the non-nullable **identity** column (it joins the merge key, so two
/// tenants reporting the same measuring point stay distinct). `source`,
/// `sender_mp_id` and `allocation_version` are nullable **attribute** columns:
/// provenance that travels with the values but stays out of the merge key, folded
/// from the newest contributing delivery on read (§4.2) — declaring them here is
/// what stops the store round-trip dropping them. When `with_subject` is set, a
/// nullable `subject_ref` **subject** column and a [`SubjectRegistry`] are attached
/// for GDPR Art. 17 erasure.
async fn table_builder(
    cold: &Arc<meterstore::IcebergCold>,
    hot: &Arc<PostgresHot>,
    pool: &PgPool,
    table_name: &str,
    with_subject: bool,
    tiering: TieringConfig,
) -> anyhow::Result<MeterStoreBuilder> {
    use meterstore::arrow::datatypes::{DataType, Field};

    // `source` is a fixed `IngestionSource` vocabulary, so it is declared as a
    // coded column — enforced by a DB `CHECK` like sparte/unit/quality, not just
    // by the application that writes it. `sender_mp_id` is a free MP-ID and
    // `allocation_version` a free MaBiS label (INITIAL/CORRECTION/ESA-…), so both
    // stay unconstrained.
    let source_codes: Vec<&str> = crate::domain::IngestionSource::ALL
        .iter()
        .map(|s| s.as_str())
        .collect();
    let mut config = TableConfig::new(table_name)
        .partition_step(tiering.partition_step)
        .archival_step(tiering.archival_step)
        .settlement_lag(tiering.settlement_lag)
        .identity_column(Field::new("tenant", DataType::Utf8, false))
        .attribute_column(meterstore::coded_column("source", &source_codes, true))
        .attribute_column(Field::new("sender_mp_id", DataType::Utf8, true))
        .attribute_column(Field::new("allocation_version", DataType::Utf8, true));
    if with_subject {
        config = config.subject_column("subject_ref");
    }
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
    let mut builder = MeterStore::builder()
        .hot(hot.clone())
        .cold(cold.clone(), provider)
        .table(config);
    if with_subject {
        builder = builder.subject_registry(SubjectRegistry::new(pool.clone()));
    }
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
    pool: PgPool,
}

impl MeterStoreTimeSeriesRepository {
    /// Wrap an already-constructed [`MeterStore`] and edmd's business-table pool.
    #[must_use]
    pub fn new(store: MeterStore, pool: PgPool) -> Self {
        Self { store, pool }
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

    /// As [`TimeSeriesRepository::query`], but reading the data **as it was known
    /// at** `as_of` — the § 60 Abs. 6 MsbG point-in-time (bitemporal)
    /// reconstruction.
    ///
    /// Backed by meterstore's `as_known_at`, which pins the row-level `recorded_at`
    /// transaction-time axis across **both** tiers and re-runs version resolution
    /// under that ceiling. So a correction delivered after `as_of`, **and an
    /// interval first stored after it**, are both invisible — the value returned is
    /// the one that was in force at that instant. This replaces the former overlay
    /// that reverted corrected values from an audit table but could not hide a
    /// later-inserted interval, so it now reconstructs set membership too.
    pub async fn query_as_of(
        &self,
        q: &TimeSeriesQuery,
        as_of: OffsetDateTime,
    ) -> Result<Vec<MeterRead>, EdmError> {
        let store = self.store.as_known_at(as_of).await.map_err(store_err)?;
        let resolved = store
            .series(q.malo_id.clone())
            .column_eq(TENANT_COL, tenant_scope(&q.tenant))
            .range(q.from, q.to)
            .collect_resolved()
            .await
            .map_err(store_err)?;
        Ok(resolved
            .map(|r| series_to_reads(&r, &q.tenant))
            .unwrap_or_default())
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
/// A delivery that states none falls back to **transaction time in
/// milliseconds**, which is a deliberate choice of magnitude, not just of
/// precision:
///
/// - Unix milliseconds are 13 digits, one short of the ≥ 14 MSCONS mandates,
///   so a timestamp fallback always sorts *below* any stated version in the
///   same scope. A delivery that says which version it is therefore always
///   beats one that does not, whatever order they arrived in. Anything finer
///   (microseconds, 16 digits) crosses into the MSCONS band and the two
///   schemes would start interleaving by accident.
/// - It is still sub-second, so two writes for one interval in the same second
///   are two versions rather than one `(merge_key, version)` conflict on the
///   hot tier — which whole-second timestamps produced.
///
/// This is not a substitute for the wire version. Between two unversioned
/// deliveries, arrival order still decides, and a replayed original still wins.
/// Only a version off the wire fixes that, and the `process.completed` payload
/// does not carry one yet.
fn version_of(mscons: Option<u128>, recorded_at: OffsetDateTime) -> Result<Version, EdmError> {
    match mscons {
        Some(v) => Version::mscons(v).map_err(store_err),
        None => {
            let millis = (recorded_at.unix_timestamp_nanos() / 1_000_000).max(1);
            Version::new(millis as u128).map_err(store_err)
        }
    }
}

fn read_to_stored(r: &MeterRead) -> Result<StoredSeries, EdmError> {
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
    let operator = r.sender_mp_id.clone().unwrap_or_else(|| r.tenant.clone());
    let scope = VersionScope::for_interval(&operator, r.dtm_from).map_err(store_err)?;
    let recorded_at = r.valid_from_tx.unwrap_or_else(OffsetDateTime::now_utc);
    let version = version_of(r.mscons_version, recorded_at)?;

    let source = MeasurementSource::Mscons {
        pid: r.pid,
        message_ref: None,
        sender_mp_id: operator.clone(),
    };
    let mut series =
        MeasurementSeries::new(r.malo_id.clone(), obis, vec![interval], source, recorded_at);
    series.melo_id = r.melo_id.clone();

    let unit = match r.sparte {
        Sparte::Gas | Sparte::Wasser => MeasurementUnit::CubicMetre,
        _ => MeasurementUnit::KiloWattHour,
    };
    Ok(StoredSeries::of(
        r.sparte,
        series,
        ScopedVersion::new(scope, version),
        recorded_at,
    )
    .in_unit(unit)
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
        .map_err(store_err)?;
        Ok(())
    }

    async fn store_reads(&self, reads: ValidatedReads) -> Result<(), EdmError> {
        let reads = reads.as_slice();
        if reads.is_empty() {
            return Ok(());
        }
        // Enrol each distinct MaLo as an erasure subject and stamp the pseudonymous
        // reference on every row it owns. meterstore refuses a write whose
        // subject_ref does not resolve to a live mapping, and Article 17 erasure
        // works by destroying that mapping — so registration has to precede the
        // append. `register` is idempotent, so a re-ingest is one lookup per MaLo.
        let mut stored: Vec<StoredSeries> = Vec::with_capacity(reads.len());
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
                read_to_stored(r)?.with_extra("subject_ref", ScalarValue::Utf8(Some(subject))),
            );
        }

        // Route the whole batch through `append`: it splits current vs late
        // (below-watermark) intervals across the two tiers, and returns the
        // displacements that DRIVE the § 60 audit — no separate re-read of prior
        // state (which would race exactly when two corrections arrive together).
        let outcome = self.store.append(&stored).await.map_err(store_err)?;

        // ── § 60 Abs. 6 audit + § 60 Abs. 2 confirmation, from displacements ──
        for d in &outcome.displacements {
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
                // A prior value stopped being current → immutable audit row
                // (§ 60 Abs. 6 MsbG). Reconstructed from the write's own report,
                // never from a second read.
                sqlx::query(
                    r"INSERT INTO meter_read_corrections
                          (malo_id, dtm_from, dtm_to, obis_code_norm,
                           original_kwh, original_quality, corrected_kwh, corrected_quality,
                           reason, source, corrected_by, tenant)
                      VALUES ($1,$2,$9,$3,$4,$5,$6,$7,
                              'Neulieferung überschreibt gespeichertes Intervall (§ 60 Abs. 6 MsbG)',
                              'MSCONS_UPDATE','edmd-ingest',$8)",
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
                .execute(&self.pool)
                .await
                .map_err(store_err)?;
            }

            // § 60 Abs. 2: a measured/corrected value discharges an open
            // confirmation; an estimated/substituted one opens the obligation.
            match d.written.quality {
                QualityFlag::Measured | QualityFlag::Corrected => {
                    sqlx::query(
                        r"UPDATE estimated_read_confirmations
                          SET status='BESTAETIGT', resolved_at=now(), resolved_by='meterstore-ingest'
                          WHERE tenant=$4 AND malo_id=$1 AND dtm_from=$2 AND obis_code_norm=$3
                            AND status IN ('OFFEN','UEBERFAELLIG')",
                    )
                    .bind(&d.malo_id)
                    .bind(d.from)
                    .bind(&d.obis_code)
                    .bind(&tenant)
                    .execute(&self.pool)
                    .await
                    .map_err(store_err)?;
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
                    .map_err(store_err)?;
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
        // Single-meter typed read, version-resolved & tier-split by meterstore.
        let resolved = self
            .store
            .series(q.malo_id.clone())
            .column_eq(TENANT_COL, tenant_scope(&q.tenant))
            .range(q.from, q.to)
            .collect_resolved()
            .await
            .map_err(store_err)?;
        Ok(resolved
            .map(|r| series_to_reads(&r, &q.tenant))
            .unwrap_or_default())
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
        .map_err(store_err)?;
        rows.into_iter().map(|row| row_to_receipt(&row)).collect()
    }

    async fn imbalance(
        &self,
        malo_id: &str,
        from: Date,
        to: Date,
        tenant: &str,
    ) -> Result<ImbalanceReport, EdmError> {
        // Aggregate over the tier-split resolved series (§60 MMM); a Date range
        // is a Berlin calendar period, so its UTC window comes from the calendar
        // (23:00/22:00 UTC boundaries, DST-correct length).
        let from_ts = metering::calendar::day_start_utc(from);
        let to_ts = metering::calendar::day_end_utc(to);
        // Only billable qualities enter the MMM saldo — the FAULTY/UNKNOWN filter
        // is pushed into the scan (§60 Abs. 2 billable set) rather than applied
        // after materialising the series.
        let resolved = self
            .store
            .series(malo_id.to_string())
            .column_eq(TENANT_COL, tenant_scope(tenant))
            .range(from_ts, to_ts)
            .quality_in(&billable_qualities())
            .collect_resolved()
            .await
            .map_err(store_err)?;
        let reads = resolved
            .map(|r| series_to_reads(&r, tenant))
            .unwrap_or_default();
        if reads.is_empty() {
            return Err(EdmError::NoData {
                malo_id: malo_id.to_owned(),
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        let total: Decimal = reads.iter().map(|r| r.quantity_kwh).sum();
        Ok(ImbalanceReport {
            malo_id: malo_id.to_owned(),
            period_from: from,
            period_to: to,
            lf_quantity_kwh: total,
            nb_quantity_kwh: total,
            delta_kwh: Decimal::ZERO,
            delta_pct: Decimal::ZERO,
            quality: QualityFlag::Unknown,
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
            .series(malo_id.to_string())
            .column_eq(TENANT_COL, tenant_scope(tenant))
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
        .map_err(store_err)?;

        if let Some(row) = pre {
            let dec = |c: &str| row.try_get::<Option<Decimal>, _>(c).ok().flatten();
            let sparte_str: String = row.try_get("sparte").unwrap_or_else(|_| "STROM".into());
            let messtyp_str: String = row.try_get("messtyp").unwrap_or_else(|_| "SLP".into());
            let quality_str: String = row.try_get("quality").unwrap_or_else(|_| "UNKNOWN".into());
            return Ok(Some(MeterBillingPeriod {
                malo_id: q.malo_id.clone(),
                period_from: q.period_from,
                period_to: q.period_to,
                messtyp: messtyp_from_str(&messtyp_str),
                sparte: str_to_sparte(&sparte_str),
                arbeitsmenge_kwh: row.try_get("arbeitsmenge_kwh").unwrap_or(Decimal::ZERO),
                arbeitsmenge_ht_kwh: dec("arbeitsmenge_ht_kwh"),
                arbeitsmenge_nt_kwh: dec("arbeitsmenge_nt_kwh"),
                spitzenleistung_kw: dec("spitzenleistung_kw"),
                brennwert_kwh_per_m3: dec("brennwert_kwh_per_m3"),
                zustandszahl: dec("zustandszahl"),
                zaehlerstand_anfang: dec("zaehlerstand_anfang"),
                zaehlerstand_ende: dec("zaehlerstand_ende"),
                quality: str_to_quality(&quality_str),
                lastprofil: None,
                profil_typ: None,
            }));
        }

        // 2. Fall back to on-the-fly aggregation from the resolved series. The
        //    billing period is a Berlin calendar period (Liefermonat), so the UTC
        //    window comes from the calendar.
        let from_ts = metering::calendar::day_start_utc(q.period_from);
        let to_ts = metering::calendar::day_end_utc(q.period_to);
        // Billable qualities only (§60 Abs. 2), pushed into the scan.
        let resolved = self
            .store
            .series(q.malo_id.clone())
            .column_eq(TENANT_COL, tenant_scope(&q.tenant))
            .range(from_ts, to_ts)
            .quality_in(&billable_qualities())
            .collect_resolved()
            .await
            .map_err(store_err)?;
        let reads: Vec<MeterRead> = resolved
            .map(|r| series_to_reads(&r, &q.tenant))
            .unwrap_or_default();
        if reads.is_empty() {
            return Ok(None);
        }

        let total_kwh: Decimal = reads.iter().map(|r| r.quantity_kwh).sum();
        let sparte = reads.first().map_or(Sparte::Strom, |r| r.sparte);
        let first_interval_min = reads
            .first()
            .map(|r| (r.dtm_to - r.dtm_from).whole_minutes().unsigned_abs());
        let messtyp = match first_interval_min {
            Some(m) if m <= 60 => Messtyp::Rlm,
            _ => Messtyp::Slp,
        };
        let spitzenleistung_kw = if messtyp == Messtyp::Rlm && sparte == Sparte::Strom {
            reads
                .iter()
                .filter(|r| (r.dtm_to - r.dtm_from).whole_minutes().unsigned_abs() == 15)
                .map(|r| r.quantity_kwh * Decimal::from(4))
                .max()
        } else {
            None
        };
        let worst_quality = reads
            .iter()
            .map(|r| r.quality)
            .max_by_key(|q| q.severity_rank())
            .unwrap_or_default();

        let result = MeterBillingPeriod {
            malo_id: q.malo_id.clone(),
            period_from: q.period_from,
            period_to: q.period_to,
            messtyp,
            sparte,
            arbeitsmenge_kwh: total_kwh,
            arbeitsmenge_ht_kwh: None,
            arbeitsmenge_nt_kwh: None,
            spitzenleistung_kw,
            brennwert_kwh_per_m3: None,
            zustandszahl: None,
            zaehlerstand_anfang: None,
            zaehlerstand_ende: None,
            quality: worst_quality,
            lastprofil: None,
            profil_typ: None,
        };

        // Cache the computed aggregate.
        let _ = sqlx::query(
            r"INSERT INTO meter_billing_periods
                  (malo_id, period_from, period_to, messtyp, sparte,
                   arbeitsmenge_kwh, spitzenleistung_kw, quality, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
              ON CONFLICT ON CONSTRAINT mbp_tenant_period_unique
              DO UPDATE SET arbeitsmenge_kwh = EXCLUDED.arbeitsmenge_kwh,
                            spitzenleistung_kw = EXCLUDED.spitzenleistung_kw,
                            quality = EXCLUDED.quality,
                            computed_at = now()",
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
        .execute(&self.pool)
        .await;

        Ok(Some(result))
    }

    async fn update_gas_quality(
        &self,
        tenant: &str,
        malo_id: &str,
        brennwert_kwh_per_m3: Option<Decimal>,
        zustandszahl: Option<Decimal>,
    ) -> Result<u64, EdmError> {
        // Business table (meter_billing_periods) — unchanged from pg/timeseries.
        let result = sqlx::query(
            r"UPDATE meter_billing_periods
              SET brennwert_kwh_per_m3 = COALESCE($2, brennwert_kwh_per_m3),
                  zustandszahl        = COALESCE($3, zustandszahl)
              WHERE malo_id = $1 AND tenant = $4
                AND (brennwert_kwh_per_m3 IS NULL OR zustandszahl IS NULL)",
        )
        .bind(malo_id)
        .bind(brennwert_kwh_per_m3)
        .bind(zustandszahl)
        .bind(tenant)
        .execute(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(result.rows_affected())
    }

    async fn store_corrections(
        &self,
        records: &[CorrectionRecord],
    ) -> Result<Vec<uuid::Uuid>, EdmError> {
        use crate::domain::CorrectionSource;

        let mut ids = Vec::with_capacity(records.len());
        for rec in records {
            // 1. Immutable audit row (§ 60 Abs. 6 MsbG) — the original value is
            //    carried by the correction request itself.
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
            .fetch_one(&self.pool)
            .await
            .map_err(store_err)?;
            let correction_id: uuid::Uuid = row.try_get("correction_id").map_err(store_err)?;
            ids.push(correction_id);

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
                .series(rec.malo_id.clone())
                .column_eq(TENANT_COL, tenant_scope(&rec.tenant))
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
            let stored = read_to_stored(&corrected)?.with_extra(
                "subject_ref",
                ScalarValue::Utf8(Some(subject.as_str().to_owned())),
            );
            self.store.append(&[stored]).await.map_err(store_err)?;

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
                .execute(&self.pool)
                .await
                .map_err(store_err)?;
            }
        }
        Ok(ids)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ESA "Werte nach Typ 2" store — a SECOND meterstore table (never billed).
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
    let operator = r.sender_mp_id.clone().unwrap_or_else(|| r.tenant.clone());
    let scope = VersionScope::for_interval(&operator, r.dtm_from).map_err(store_err)?;
    let recorded_at = r.received_at.unwrap_or_else(OffsetDateTime::now_utc);
    // Typ-2 values are stored as delivered and never corrected, so arrival order
    // is the only order there is — but it still has to be an order, hence the
    // nanosecond resolution (see `version_of`).
    let version = version_of(None, recorded_at)?;
    let source = MeasurementSource::Mscons {
        pid: r.pid,
        message_ref: None,
        sender_mp_id: operator,
    };
    let mut series =
        MeasurementSeries::new(r.malo_id.clone(), obis, vec![interval], source, recorded_at);
    series.melo_id = r.melo_id.clone();
    let unit = match r.sparte {
        Sparte::Gas | Sparte::Wasser => MeasurementUnit::CubicMetre,
        _ => MeasurementUnit::KiloWattHour,
    };
    Ok(StoredSeries::of(
        r.sparte,
        series,
        ScopedVersion::new(scope, version),
        recorded_at,
    )
    .in_unit(unit)
    .with_extra("tenant", ScalarValue::Utf8(Some(r.tenant.clone())))
    // The reporting operator's MP-ID, recovered on read-back. `source` and
    // `allocation_version` do not apply to the non-authoritative Typ-2 stream,
    // so they stay NULL.
    .with_extra("sender_mp_id", ScalarValue::Utf8(r.sender_mp_id.clone())))
}

impl Typ2Repository for MeterStoreTyp2Repository {
    async fn store_typ2_reads(&self, reads: &[Typ2Read]) -> Result<(), EdmError> {
        if reads.is_empty() {
            return Ok(());
        }
        let stored: Vec<StoredSeries> =
            reads.iter().map(typ2_to_stored).collect::<Result<_, _>>()?;
        self.store.append(&stored).await.map_err(store_err)?;
        Ok(())
    }

    async fn query_typ2(&self, q: &TimeSeriesQuery) -> Result<Vec<Typ2Read>, EdmError> {
        let resolved = self
            .store
            .series(q.malo_id.clone())
            .column_eq(TENANT_COL, tenant_scope(&q.tenant))
            .range(q.from, q.to)
            .collect_resolved()
            .await
            .map_err(store_err)?;
        Ok(resolved
            .map(|r| series_to_typ2(&r, &q.tenant))
            .unwrap_or_default())
    }
}

// ── mapping helpers ─────────────────────────────────────────────────────────

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
            malo_id: series.malo_id.clone(),
            melo_id: series.melo_id.clone(),
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
    series
        .intervals
        .iter()
        .map(|iv| Typ2Read {
            malo_id: series.malo_id.clone(),
            melo_id: series.melo_id.clone(),
            dtm_from: iv.from,
            dtm_to: iv.to,
            quantity_kwh: iv.value,
            quality: iv.quality,
            pid,
            sparte: resolved.sparte,
            obis_code: iv.obis_code.map(|o| o.to_string()),
            tenant: tenant.to_string(),
            delivery_path: Typ2DeliveryPath::MsconsBackend,
            sender_mp_id: sender_mp_id.clone(),
            received_at: None,
        })
        .collect()
}

fn row_to_receipt(row: &sqlx::postgres::PgRow) -> Result<MeterDataReceipt, EdmError> {
    Ok(MeterDataReceipt {
        process_id: row.try_get("process_id").map_err(store_err)?,
        pid: row.try_get::<i32, _>("pid").map_err(store_err)? as u32,
        malo_id: row.try_get("malo_id").map_err(store_err)?,
        sender_mp_id: row.try_get("sender_mp_id").map_err(store_err)?,
        message_ref: row.try_get("message_ref").map_err(store_err)?,
        received_at: row.try_get("received_at").map_err(store_err)?,
        tenant: row.try_get("tenant").map_err(store_err)?,
    })
}

/// Canonical form of an OBIS code as it enters the audit-row key.
fn normalise_obis(obis_code: Option<&str>) -> String {
    obis_code.map_or_else(String::new, |s| {
        s.parse::<ObisCode>()
            .map_or_else(|_| s.to_owned(), |c| c.to_string())
    })
}

/// The canonical wire spelling of a quality flag, shared with the API surface so
/// a response cannot invent a different vocabulary than the one stored.
pub(crate) fn quality_to_str(q: QualityFlag) -> &'static str {
    match q {
        QualityFlag::Measured => "MEASURED",
        QualityFlag::Estimated => "ESTIMATED",
        QualityFlag::Substituted => "SUBSTITUTED",
        QualityFlag::Calculated => "CALCULATED",
        QualityFlag::Corrected => "CORRECTED",
        QualityFlag::Preliminary => "PRELIMINARY",
        QualityFlag::Faulty => "FAULTY",
        QualityFlag::Unknown => "UNKNOWN",
    }
}

fn str_to_quality(s: &str) -> QualityFlag {
    match s {
        "MEASURED" => QualityFlag::Measured,
        "ESTIMATED" => QualityFlag::Estimated,
        "SUBSTITUTED" => QualityFlag::Substituted,
        "CALCULATED" => QualityFlag::Calculated,
        "CORRECTED" => QualityFlag::Corrected,
        "PRELIMINARY" => QualityFlag::Preliminary,
        "FAULTY" => QualityFlag::Faulty,
        _ => QualityFlag::Unknown,
    }
}

fn str_to_sparte(s: &str) -> Sparte {
    match s {
        "GAS" => Sparte::Gas,
        "WAERME" => Sparte::Waerme,
        "WASSER" => Sparte::Wasser,
        _ => Sparte::Strom,
    }
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
}
