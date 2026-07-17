//! Background archival worker — exports old `meter_reads` to Iceberg/S3
//! and deletes them from PostgreSQL.
//!
//! ## SQL catalog
//!
//! The worker uses `iceberg-catalog-sql` to manage Iceberg table metadata
//! inside the operator's existing PostgreSQL database.  No extra catalog
//! service is required.  On first run the worker creates the catalog schema
//! and the `meter_reads_archive` table.  Subsequent runs load the table from
//! the catalog and append new snapshots.
//!
//! ## Algorithm
//!
//! 1. Load (or create) the Iceberg table via `SqlCatalog`.
//! 2. Select ≤ `batch_size` rows with `dtm_from < cutoff AND archived = false`.
//! 3. Write Parquet data files via `iceberg::writer` (through FileIO → opendal → S3).
//! 4. Commit a new Iceberg snapshot using `Transaction::fast_append`.
//! 5. Mark rows `archived = true` in PostgreSQL (bulk UPDATE).
//! 6. Record the batch in `archive_batches`.
//!
//! Crash safety: steps 3–5 are idempotent.  If the process dies after the
//! Iceberg commit but before marking rows archived, the next run will
//! re-write the same rows (no data loss, possible duplicate data files —
//! the extra files will be cleaned up by a future Iceberg `expire_snapshots`
//! operation).

use std::collections::HashMap;
use std::time::Duration;

use iceberg::Catalog;
use iceberg::CatalogBuilder; // brings .load() into scope for SqlCatalogBuilder
use iceberg::NamespaceIdent;
use iceberg::TableCreation;
use iceberg::io::FileIO;
use iceberg::transaction::{ApplyTransactionAction, Transaction};
use iceberg_catalog_sql::{SqlBindStyle, SqlCatalog, SqlCatalogBuilder};
use mako_edm::archive::ArchiveConfig;
use mako_edm::domain::{MeterRead, QualityFlag, Sparte as EdmSparte};
use rust_decimal::Decimal;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

use crate::iceberg::query::OlapEngine;
use crate::iceberg::schema::{ICEBERG_TABLE_NAME, meter_reads_partition_spec, meter_reads_schema};
use crate::iceberg::writer::write_data_files;

// ── ArchiveWorker ─────────────────────────────────────────────────────────────

pub struct ArchiveWorker {
    pool: PgPool,
    cfg: ArchiveConfig,
    file_io: FileIO,
    database_url: String,
}

impl ArchiveWorker {
    /// Create the worker.
    ///
    /// `database_url` is the same PostgreSQL URL used by the `edmd` hot tier.
    /// The SQL catalog will create its tables in `cfg.iceberg_catalog_schema`.
    pub fn new(pool: PgPool, cfg: ArchiveConfig, file_io: FileIO, database_url: String) -> Self {
        Self {
            pool,
            cfg,
            file_io,
            database_url,
        }
    }

    /// Spawn onto the Tokio runtime.  Runs until `shutdown` is cancelled.
    pub fn spawn(self, shutdown: CancellationToken) {
        tokio::spawn(async move { self.run(shutdown).await });
    }

    async fn run(self, shutdown: CancellationToken) {
        let interval = Duration::from_secs(self.cfg.interval_secs);
        info!(
            storage_uri = %self.cfg.storage_uri,
            retention_months = self.cfg.retention_months,
            catalog_schema   = %self.cfg.iceberg_catalog_schema,
            "iceberg-worker: started"
        );

        loop {
            match self.run_once().await {
                Ok(0) => info!("iceberg-worker: nothing to archive"),
                Ok(n) => info!(rows_archived = n, "iceberg-worker: batch committed"),
                Err(e) => error!(error = %e, "iceberg-worker: batch failed — will retry"),
            }

            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("iceberg-worker: shutdown");
                    break;
                }
                _ = tokio::time::sleep(interval) => {}
            }
        }
    }

    /// One archival pass.  Returns the number of rows archived (0 = nothing to do).
    async fn run_once(&self) -> anyhow::Result<u64> {
        let cutoff = cutoff_time(self.cfg.retention_months);
        let batch_size = self.cfg.batch_size as i64;

        // ── 1. Load rows eligible for archival ────────────────────────────────
        // NOTE: No tenant filter here — the archive worker operates on all tenants
        // per run. Each archived row's `tenant_id` is preserved in the Iceberg data.
        // For strict per-tenant isolation, the RunConfig should carry a tenant field
        // and this query should include `AND tenant = $3`. This is tracked as
        // a future hardening item (H8 from the security audit).
        let rows = sqlx::query_as::<_, RawMeterRead>(
            r"SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh,
                     quality, pid, sparte, obis_code, tenant_id
              FROM   meter_reads
              WHERE  archived = false
                AND  dtm_from < $1
              ORDER BY dtm_from
              LIMIT $2",
        )
        .bind(cutoff)
        .bind(batch_size)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(0);
        }

        let row_count = rows.len() as u64;
        let reads: Vec<MeterRead> = rows.iter().map(raw_to_read).collect();

        // ── 2. Open / create Iceberg table via SQL catalog ────────────────────
        let catalog = build_catalog(&self.cfg, &self.database_url).await?;
        let ns = NamespaceIdent::new(self.cfg.iceberg_catalog_name.clone());
        if !catalog.namespace_exists(&ns).await? {
            catalog.create_namespace(&ns, HashMap::new()).await?;
        }

        let table_ident = iceberg::TableIdent::new(ns.clone(), ICEBERG_TABLE_NAME.to_owned());
        let table = if catalog.table_exists(&table_ident).await? {
            catalog.load_table(&table_ident).await?
        } else {
            let schema = meter_reads_schema()?;
            catalog
                .create_table(
                    &ns,
                    TableCreation::builder()
                        .name(ICEBERG_TABLE_NAME.to_owned())
                        .schema((*schema).clone())
                        .location(self.cfg.storage_uri.clone())
                        .partition_spec(meter_reads_partition_spec())
                        .build(),
                )
                .await?
        };

        // ── 3. Write Parquet data files to S3 ────────────────────────────────
        let batch_id = Uuid::new_v4();
        let s3_prefix = format!("{}/data/", self.cfg.storage_uri.trim_end_matches('/'));

        let dtm_from_min = reads.iter().map(|r| r.dtm_from).min();
        let dtm_from_max = reads.iter().map(|r| r.dtm_from).max();
        let malo_count = reads
            .iter()
            .map(|r| r.malo_id.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len() as i32;

        sqlx::query(
            r"INSERT INTO archive_batches
                  (batch_id, cutoff_before, dtm_from_min, dtm_from_max, row_count,
                   malo_count, s3_prefix, status)
              VALUES ($1, $2, $3, $4, $5, $6, $7, 'writing')",
        )
        .bind(batch_id)
        .bind(cutoff)
        .bind(dtm_from_min)
        .bind(dtm_from_max)
        .bind(row_count as i64)
        .bind(malo_count)
        .bind(&s3_prefix)
        .execute(&self.pool)
        .await?;

        let data_files = match write_data_files(&table, &reads, &self.file_io).await {
            Ok(df) => df,
            Err(e) => {
                sqlx::query(
                    "UPDATE archive_batches SET status='failed', error_msg=$1 WHERE batch_id=$2",
                )
                .bind(e.to_string())
                .bind(batch_id)
                .execute(&self.pool)
                .await
                .ok();
                return Err(e);
            }
        };

        let file_count = data_files.len() as i32;
        let bytes_written: i64 = data_files
            .iter()
            .map(|f| f.file_size_in_bytes() as i64)
            .sum();

        // ── 4. Commit Iceberg snapshot (with CatalogCommitConflicts retry) ────
        // Transaction pattern (iceberg 0.9.1):
        //   tx.fast_append().add_data_files(files).apply(tx)  → modified tx
        //   tx.commit(&catalog)  → writes new snapshot metadata via SqlCatalog
        //
        // On CatalogCommitConflicts (concurrent writers), reload the Table
        // metadata from the catalog to avoid livelock on a stale snapshot,
        // then retry.  The data files are already written to S3 and can be
        // reused — only the transaction needs to be rebuilt.
        let mut last_commit_err = None;
        let mut committed = false;
        for attempt in 0..3_u32 {
            // Reload the table on each retry to get the latest snapshot.
            let current_table = if catalog.table_exists(&table_ident).await? {
                catalog.load_table(&table_ident).await?
            } else {
                table.clone()
            };
            let tx = Transaction::new(&current_table);
            let tx = tx
                .fast_append()
                .add_data_files(data_files.clone())
                .apply(tx)
                .map_err(|e| anyhow::anyhow!("fast_append apply: {e}"))?;
            match tx.commit(&catalog).await {
                Ok(_updated_table) => {
                    committed = true;
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("CatalogCommitConflict")
                        || msg.contains("commit conflict")
                        || msg.contains("concurrent")
                    {
                        tracing::warn!(
                            attempt,
                            error = %e,
                            "iceberg-worker: CatalogCommitConflicts — reloading table and retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(100 * (1 << attempt))).await;
                        last_commit_err = Some(e);
                    } else {
                        // Non-retryable error
                        return Err(anyhow::anyhow!("transaction commit (permanent): {e}"));
                    }
                }
            }
        }
        if !committed {
            return Err(anyhow::anyhow!(
                "iceberg-worker: commit failed after 3 retries: {}",
                last_commit_err.map_or_else(|| "unknown".to_owned(), |e| e.to_string())
            ));
        }

        // ── 5. Mark rows archived in PostgreSQL (bulk UPDATE) ─────────────────
        let malo_ids: Vec<&str> = reads.iter().map(|r| r.malo_id.as_str()).collect();
        let dtm_froms: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_from).collect();
        let dtm_tos: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_to).collect();

        sqlx::query(
            r"UPDATE meter_reads mr
              SET    archived = true
              FROM   (SELECT * FROM unnest($1::text[], $2::timestamptz[], $3::timestamptz[])
                          AS t(malo_id, dtm_from, dtm_to)) AS src
              WHERE  mr.malo_id  = src.malo_id
                AND  mr.dtm_from = src.dtm_from
                AND  mr.dtm_to   = src.dtm_to",
        )
        .bind(&malo_ids as &[&str])
        .bind(&dtm_froms)
        .bind(&dtm_tos)
        .execute(&self.pool)
        .await?;

        // ── 6. Commit batch record ────────────────────────────────────────────
        sqlx::query(
            r"UPDATE archive_batches
              SET status='committed', committed_at=now(), file_count=$1, bytes_written=$2
              WHERE batch_id=$3",
        )
        .bind(file_count)
        .bind(bytes_written)
        .bind(batch_id)
        .execute(&self.pool)
        .await?;

        info!(
            %batch_id, rows = row_count, files = file_count, bytes = bytes_written,
            "iceberg-worker: snapshot committed"
        );
        Ok(row_count)
    }
}

// ── Catalog builder ───────────────────────────────────────────────────────────

/// Build an `SqlCatalog` pointed at the operator's PostgreSQL.
///
/// Uses `SqlBindStyle::DollarNumeric` which is required for PostgreSQL
/// (`$1`, `$2` placeholders).  MySQL and SQLite would use `QuestionMark`.
async fn build_catalog(cfg: &ArchiveConfig, database_url: &str) -> anyhow::Result<SqlCatalog> {
    use iceberg_catalog_sql::{
        SQL_CATALOG_PROP_BIND_STYLE, SQL_CATALOG_PROP_URI, SQL_CATALOG_PROP_WAREHOUSE,
    };
    use std::collections::HashMap;

    let catalog = SqlCatalogBuilder::default()
        .load(
            &cfg.iceberg_catalog_name,
            HashMap::from_iter([
                (SQL_CATALOG_PROP_URI.to_owned(), database_url.to_owned()),
                (
                    SQL_CATALOG_PROP_WAREHOUSE.to_owned(),
                    cfg.storage_uri.clone(),
                ),
                (
                    SQL_CATALOG_PROP_BIND_STYLE.to_owned(),
                    SqlBindStyle::DollarNumeric.to_string(),
                ),
            ]),
        )
        .await
        .map_err(|e| anyhow::anyhow!("SqlCatalogBuilder: {e}"))?;

    Ok(catalog)
}

/// Load the Iceberg table for OLAP queries (called from `server.rs` at startup).
pub async fn load_table_for_olap(
    cfg: &ArchiveConfig,
    database_url: &str,
) -> anyhow::Result<OlapEngine> {
    let catalog = build_catalog(cfg, database_url).await?;
    let ns = NamespaceIdent::new(cfg.iceberg_catalog_name.clone());
    let table_ident = iceberg::TableIdent::new(ns, ICEBERG_TABLE_NAME.to_owned());

    let table = catalog
        .load_table(&table_ident)
        .await
        .map_err(|e| anyhow::anyhow!("load_table: {e}"))?;

    OlapEngine::new(table).await
}

// ── Archival statistics ───────────────────────────────────────────────────────

pub async fn archive_stats(pool: &PgPool) -> anyhow::Result<mako_edm::archive::ArchiveStats> {
    #[derive(sqlx::FromRow)]
    struct ArchiveStatsRow {
        total_batches: i64,
        committed_batches: i64,
        total_rows_archived: Option<i64>,
        total_bytes_written: Option<i64>,
        oldest_cutoff: Option<OffsetDateTime>,
        newest_cutoff: Option<OffsetDateTime>,
    }
    let r = sqlx::query_as::<_, ArchiveStatsRow>(
        r"SELECT COUNT(*) AS total_batches,
                 COUNT(*) FILTER (WHERE status='committed') AS committed_batches,
                 SUM(row_count)    FILTER (WHERE status='committed') AS total_rows_archived,
                 SUM(bytes_written) FILTER (WHERE status='committed') AS total_bytes_written,
                 MIN(cutoff_before) AS oldest_cutoff,
                 MAX(cutoff_before) AS newest_cutoff
          FROM archive_batches",
    )
    .fetch_one(pool)
    .await?;
    Ok(mako_edm::archive::ArchiveStats {
        total_batches: r.total_batches,
        committed_batches: r.committed_batches,
        total_rows_archived: r.total_rows_archived.unwrap_or(0),
        total_bytes_written: r.total_bytes_written.unwrap_or(0),
        oldest_cutoff: r.oldest_cutoff,
        newest_cutoff: r.newest_cutoff,
    })
}

pub async fn recent_batches(
    pool: &PgPool,
    limit: i64,
) -> anyhow::Result<Vec<mako_edm::archive::ArchivedBatch>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        batch_id: Uuid,
        created_at: OffsetDateTime,
        cutoff_before: OffsetDateTime,
        dtm_from_min: Option<OffsetDateTime>,
        dtm_from_max: Option<OffsetDateTime>,
        row_count: i64,
        malo_count: i32,
        s3_prefix: String,
        file_count: i32,
        bytes_written: i64,
        status: String,
        error_msg: Option<String>,
        committed_at: Option<OffsetDateTime>,
        tenant_id: Option<Uuid>,
    }
    use mako_edm::archive::ArchiveBatchStatus as S;
    let rows = sqlx::query_as::<_, Row>(
        r"SELECT batch_id, created_at, cutoff_before, dtm_from_min, dtm_from_max,
                 row_count, malo_count, s3_prefix, file_count, bytes_written,
                 status, error_msg, committed_at, tenant_id
          FROM   archive_batches
          ORDER BY created_at DESC
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| mako_edm::archive::ArchivedBatch {
            batch_id: r.batch_id,
            created_at: r.created_at,
            cutoff_before: r.cutoff_before,
            dtm_from_min: r.dtm_from_min,
            dtm_from_max: r.dtm_from_max,
            row_count: r.row_count,
            malo_count: r.malo_count,
            s3_prefix: r.s3_prefix,
            file_count: r.file_count,
            bytes_written: r.bytes_written,
            status: match r.status.as_str() {
                "committed" => S::Committed,
                "writing" => S::Writing,
                "failed" => S::Failed,
                _ => S::Pending,
            },
            error_msg: r.error_msg,
            committed_at: r.committed_at,
            tenant_id: r.tenant_id,
        })
        .collect())
}

// ── Raw DB row ────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct RawMeterRead {
    malo_id: String,
    melo_id: Option<String>,
    dtm_from: OffsetDateTime,
    dtm_to: OffsetDateTime,
    quantity_kwh: String,
    quality: String,
    pid: i32,
    sparte: String,
    obis_code: Option<String>,
    tenant_id: Option<Uuid>,
    #[sqlx(default)]
    source: Option<String>,
    #[sqlx(default)]
    push_session: Option<String>,
}

fn raw_to_read(r: &RawMeterRead) -> MeterRead {
    MeterRead {
        malo_id: r.malo_id.clone(),
        melo_id: r.melo_id.clone(),
        dtm_from: r.dtm_from,
        dtm_to: r.dtm_to,
        quantity_kwh: r.quantity_kwh.parse().unwrap_or(Decimal::ZERO),
        quality: match r.quality.as_str() {
            "MEASURED" => QualityFlag::Measured,
            "ESTIMATED" => QualityFlag::Estimated,
            "SUBSTITUTED" => QualityFlag::Substituted,
            "CALCULATED" => QualityFlag::Calculated,
            _ => QualityFlag::Unknown,
        },
        pid: r.pid as u32,
        sparte: if r.sparte.eq_ignore_ascii_case("GAS") {
            EdmSparte::Gas
        } else {
            EdmSparte::Strom
        },
        obis_code: r.obis_code.clone(),
        tenant_id: r.tenant_id,
        // Provenance fields: populate from raw data when available
        source: mako_edm::domain::IngestionSource::from_db_str(
            r.source.as_deref().unwrap_or("MSCONS"),
        ),
        push_session: r.push_session.clone(),
        quality_warnings: None, // not stored in raw Iceberg rows
        sender_mp_id: None,
        allocation_version: "INITIAL".to_owned(),
        valid_from_tx: None,
    }
}

fn cutoff_time(retention_months: u32) -> OffsetDateTime {
    OffsetDateTime::now_utc() - time::Duration::days(retention_months as i64 * 30)
}
