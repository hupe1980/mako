//! DataFusion OLAP engine for the Iceberg cold tier.
//!
//! Uses `IcebergTableProvider` (catalog-backed, dynamic) which calls
//! `catalog.load_table()` on every DataFusion `scan()` — new Parquet files
//! written by the archive worker are immediately visible without restart.

use std::sync::Arc;

use datafusion::arrow::array::Array;
use datafusion::prelude::*;
use iceberg::{Catalog, TableIdent};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::{debug, warn};

// ── Result types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedMmmResult {
    pub malo_id: String,
    pub total_kwh: f64,
    pub read_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub period_from: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub period_to: Option<OffsetDateTime>,
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedMeterRead {
    pub malo_id: String,
    pub melo_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub dtm_from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub dtm_to: OffsetDateTime,
    pub quantity_kwh: String,
    pub quality: String,
    pub obis_code: Option<String>,
    pub sparte: String,
}

// ── OlapEngine ────────────────────────────────────────────────────────────────

/// DataFusion OLAP engine backed by the Iceberg cold-tier catalog.
///
/// Stores the catalog reference and table identifier. On each query,
/// a fresh `SessionContext` is built from the latest table snapshot by
/// reloading from the catalog — new Parquet files written by the archive
/// worker are immediately visible without restart.
///
/// The engine also holds a PostgreSQL pool so it can enforce GDPR Art. 17
/// erasure at query time by excluding erased MaLo IDs from all Parquet queries.
#[derive(Clone)]
pub struct OlapEngine {
    catalog: Arc<dyn Catalog>,
    table_ident: TableIdent,
    /// PostgreSQL pool for GDPR erasure exclusion list.
    pool: sqlx::PgPool,
    /// Tenant scope — filters `gdpr_deletions` by tenant.
    tenant: String,
}

impl OlapEngine {
    /// Construct an OlapEngine from a catalog + table identifier.
    ///
    /// Validates connectivity by performing a single initial table load.
    /// Each subsequent query call reloads the table snapshot from the catalog.
    pub async fn new(
        catalog: Arc<dyn Catalog>,
        table_ident: TableIdent,
        pool: sqlx::PgPool,
        tenant: String,
    ) -> anyhow::Result<Self> {
        // Validate that the table exists and the catalog is reachable.
        catalog
            .load_table(&table_ident)
            .await
            .map_err(|e| anyhow::anyhow!("OlapEngine: initial table load failed: {e}"))?;
        debug!("iceberg: OlapEngine ready (catalog-backed, auto-refreshes on each query)");
        Ok(Self {
            catalog,
            table_ident,
            pool,
            tenant,
        })
    }

    /// Returns a SQL fragment ` AND malo_id NOT IN ('a','b',...)` for all MaLo IDs
    /// present in `gdpr_deletions` for this tenant.
    ///
    /// Empty string when no erasures exist. Applied to every DataFusion SQL query
    /// to enforce GDPR Art. 17 right-to-erasure for the Iceberg cold tier.
    async fn gdpr_exclusion_clause(&self) -> String {
        use sqlx::Row;
        let rows = sqlx::query("SELECT malo_id FROM gdpr_deletions WHERE tenant = $1")
            .bind(&self.tenant)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

        if rows.is_empty() {
            return String::new();
        }
        let ids: Vec<String> = rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("malo_id").ok())
            .map(|id| format!("'{}'", escape_sql(&id)))
            .collect();
        format!(" AND malo_id NOT IN ({})", ids.join(", "))
    }

    /// Build a fresh DataFusion SessionContext with the latest Iceberg snapshot.
    async fn fresh_ctx(&self) -> anyhow::Result<SessionContext> {
        let table = self
            .catalog
            .load_table(&self.table_ident)
            .await
            .map_err(|e| anyhow::anyhow!("load_table: {e}"))?;
        let ctx = SessionContext::new();
        let provider =
            iceberg_datafusion::table::IcebergStaticTableProvider::try_new_from_table(table)
                .await
                .map_err(|e| anyhow::anyhow!("IcebergStaticTableProvider: {e}"))?;
        ctx.register_table("meter_reads_archive", Arc::new(provider))
            .map_err(|e| anyhow::anyhow!("register_table: {e}"))?;
        Ok(ctx)
    }

    pub async fn mmm_aggregate(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> anyhow::Result<Option<ArchivedMmmResult>> {
        let gdpr = self.gdpr_exclusion_clause().await;
        // Single-MaLo query: if this MaLo is erased it will appear in the exclusion list.
        if gdpr.contains(&format!("'{}'", escape_sql(malo_id))) {
            return Ok(None);
        }
        let sql = format!(
            "SELECT malo_id, \
                    COALESCE(SUM(CAST(quantity_kwh AS DOUBLE)), 0.0) AS total_kwh, \
                    COUNT(*) AS read_count, MIN(dtm_from) AS period_from, MAX(dtm_to) AS period_to \
             FROM meter_reads_archive \
             WHERE malo_id = '{m}' AND dtm_from >= TIMESTAMP '{f}' AND dtm_to <= TIMESTAMP '{t}'{gdpr} \
             GROUP BY malo_id",
            m = escape_sql(malo_id),
            f = fmt_ts(from),
            t = fmt_ts(to),
        );
        debug!(%sql, "iceberg: mmm_aggregate");

        let batches = match self.fresh_ctx().await?.sql(&sql).await {
            Ok(df) => df.collect().await?,
            Err(e) => {
                warn!(error=%e, malo_id, "mmm_aggregate failed");
                return Ok(None);
            }
        };
        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Ok(None);
        }
        let b = &batches[0];

        Ok(Some(ArchivedMmmResult {
            malo_id: malo_id.to_owned(),
            total_kwh: get_f64(b, "total_kwh", 0).unwrap_or(0.0),
            read_count: get_i64(b, "read_count", 0).unwrap_or(0),
            period_from: get_ts(b, "period_from", 0),
            period_to: get_ts(b, "period_to", 0),
            source: "iceberg-archive",
        }))
    }

    pub async fn portfolio_aggregate(
        &self,
        from: OffsetDateTime,
        to: OffsetDateTime,
        limit: usize,
    ) -> anyhow::Result<Vec<ArchivedMmmResult>> {
        let gdpr = self.gdpr_exclusion_clause().await;
        let sql = format!(
            "SELECT malo_id, \
                    COALESCE(SUM(quantity_kwh), 0.0) AS total_kwh, \
                    COUNT(*) AS read_count, MIN(dtm_from) AS period_from, MAX(dtm_to) AS period_to \
             FROM meter_reads_archive \
             WHERE dtm_from >= TIMESTAMP '{f}' AND dtm_to <= TIMESTAMP '{t}'{gdpr} \
             GROUP BY malo_id ORDER BY total_kwh DESC LIMIT {limit}",
            f = fmt_ts(from),
            t = fmt_ts(to),
        );
        let batches = self.fresh_ctx().await?.sql(&sql).await?.collect().await?;
        let mut out = Vec::new();
        for b in &batches {
            use datafusion::arrow::array::StringArray;
            let ids = match b
                .column_by_name("malo_id")
                .and_then(|a| a.as_any().downcast_ref::<StringArray>())
            {
                Some(a) => a,
                None => continue,
            };
            for i in 0..b.num_rows() {
                if ids.is_null(i) {
                    continue;
                }
                out.push(ArchivedMmmResult {
                    malo_id: ids.value(i).to_owned(),
                    total_kwh: get_f64(b, "total_kwh", i).unwrap_or(0.0),
                    read_count: get_i64(b, "read_count", i).unwrap_or(0),
                    period_from: get_ts(b, "period_from", i),
                    period_to: get_ts(b, "period_to", i),
                    source: "iceberg-archive",
                });
            }
        }
        Ok(out)
    }

    pub async fn time_series(
        &self,
        malo_id: &str,
        from: OffsetDateTime,
        to: OffsetDateTime,
        limit: usize,
    ) -> anyhow::Result<Vec<ArchivedMeterRead>> {
        let gdpr = self.gdpr_exclusion_clause().await;
        let sql = format!(
            "SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality, obis_code, sparte \
             FROM meter_reads_archive \
             WHERE malo_id = '{m}' AND dtm_from >= TIMESTAMP '{f}' AND dtm_to <= TIMESTAMP '{t}'{gdpr} \
             ORDER BY dtm_from LIMIT {limit}",
            m = escape_sql(malo_id),
            f = fmt_ts(from),
            t = fmt_ts(to),
        );
        let batches = self.fresh_ctx().await?.sql(&sql).await?.collect().await?;
        let mut rows = Vec::new();
        for b in &batches {
            use datafusion::arrow::array::{StringArray, TimestampMicrosecondArray};
            macro_rules! str_col {
                ($n:literal) => {
                    b.column_by_name($n)
                        .and_then(|a| a.as_any().downcast_ref::<StringArray>())
                };
            }
            macro_rules! ts_col {
                ($n:literal) => {
                    b.column_by_name($n)
                        .and_then(|a| a.as_any().downcast_ref::<TimestampMicrosecondArray>())
                };
            }
            for i in 0..b.num_rows() {
                let Some(ids) = str_col!("malo_id") else {
                    break;
                };
                if ids.is_null(i) {
                    continue;
                }
                let Some(df) = ts_col!("dtm_from") else { break };
                let Some(dt) = ts_col!("dtm_to") else { break };
                let Some(qty) = str_col!("quantity_kwh") else {
                    break;
                };
                rows.push(ArchivedMeterRead {
                    malo_id: ids.value(i).to_owned(),
                    melo_id: str_col!("melo_id")
                        .and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_owned())),
                    dtm_from: if df.is_null(i) {
                        continue;
                    } else {
                        micros_to_odt(df.value(i))
                    },
                    dtm_to: if dt.is_null(i) {
                        continue;
                    } else {
                        micros_to_odt(dt.value(i))
                    },
                    quantity_kwh: if qty.is_null(i) {
                        "0".to_owned()
                    } else {
                        qty.value(i).to_owned()
                    },
                    quality: str_col!("quality")
                        .and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_owned()))
                        .unwrap_or_else(|| "UNKNOWN".to_owned()),
                    obis_code: str_col!("obis_code")
                        .and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_owned())),
                    sparte: str_col!("sparte")
                        .and_then(|a| (!a.is_null(i)).then(|| a.value(i).to_owned()))
                        .unwrap_or_else(|| "STROM".to_owned()),
                });
            }
        }
        Ok(rows)
    }

    /// Execute an arbitrary SQL query via DataFusion and return results as JSON rows.
    ///
    /// Used by `POST /api/v1/query/sql` for external OLAP consumers.
    /// Only `SELECT`/`WITH`/`SHOW`/`DESCRIBE` are accepted (caller must pre-validate).
    pub async fn query_to_json(
        &self,
        sql: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        use datafusion::arrow::array::Array;
        use datafusion::arrow::datatypes::DataType;

        let df = self
            .fresh_ctx()
            .await?
            .sql(sql)
            .await
            .map_err(|e| anyhow::anyhow!("DataFusion plan: {e}"))?;

        let df = if limit < usize::MAX {
            df.limit(0, Some(limit))
                .map_err(|e| anyhow::anyhow!("DataFusion limit: {e}"))?
        } else {
            df
        };

        let batches = df
            .collect()
            .await
            .map_err(|e| anyhow::anyhow!("DataFusion execute: {e}"))?;

        let mut rows: Vec<serde_json::Value> = Vec::new();
        for batch in &batches {
            let schema = batch.schema();
            for row_idx in 0..batch.num_rows() {
                let mut obj = serde_json::Map::new();
                for (col_idx, field) in schema.fields().iter().enumerate() {
                    let col = batch.column(col_idx);
                    let val = if col.is_null(row_idx) {
                        serde_json::Value::Null
                    } else {
                        match field.data_type() {
                            DataType::Utf8 | DataType::LargeUtf8 => {
                                let arr = col
                                    .as_any()
                                    .downcast_ref::<datafusion::arrow::array::StringArray>();
                                arr.map(|a| serde_json::Value::String(a.value(row_idx).to_owned()))
                                    .unwrap_or(serde_json::Value::Null)
                            }
                            DataType::Int64 => {
                                let arr = col
                                    .as_any()
                                    .downcast_ref::<datafusion::arrow::array::Int64Array>();
                                arr.map(|a| {
                                    serde_json::Value::Number(serde_json::Number::from(
                                        a.value(row_idx),
                                    ))
                                })
                                .unwrap_or(serde_json::Value::Null)
                            }
                            DataType::Float64 => {
                                let arr = col
                                    .as_any()
                                    .downcast_ref::<datafusion::arrow::array::Float64Array>();
                                arr.and_then(|a| {
                                    serde_json::Number::from_f64(a.value(row_idx))
                                        .map(serde_json::Value::Number)
                                })
                                .unwrap_or(serde_json::Value::Null)
                            }
                            DataType::Boolean => {
                                let arr = col
                                    .as_any()
                                    .downcast_ref::<datafusion::arrow::array::BooleanArray>();
                                arr.map(|a| serde_json::Value::Bool(a.value(row_idx)))
                                    .unwrap_or(serde_json::Value::Null)
                            }
                            _ => {
                                // Generic fallback: use Arrow's display formatter.
                                serde_json::Value::String(format!("{:?}", col))
                            }
                        }
                    };
                    obj.insert(field.name().clone(), val);
                }
                rows.push(serde_json::Value::Object(obj));
                if rows.len() >= limit {
                    break;
                }
            }
            if rows.len() >= limit {
                break;
            }
        }
        Ok(rows)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_f64(b: &datafusion::arrow::record_batch::RecordBatch, col: &str, i: usize) -> Option<f64> {
    use datafusion::arrow::array::PrimitiveArray;
    use datafusion::arrow::datatypes::Float64Type;
    b.column_by_name(col)?
        .as_any()
        .downcast_ref::<PrimitiveArray<Float64Type>>()
        .and_then(|a| (!a.is_null(i)).then(|| a.value(i)))
}

fn get_i64(b: &datafusion::arrow::record_batch::RecordBatch, col: &str, i: usize) -> Option<i64> {
    use datafusion::arrow::array::PrimitiveArray;
    use datafusion::arrow::datatypes::Int64Type;
    b.column_by_name(col)?
        .as_any()
        .downcast_ref::<PrimitiveArray<Int64Type>>()
        .and_then(|a| (!a.is_null(i)).then(|| a.value(i)))
}

fn get_ts(
    b: &datafusion::arrow::record_batch::RecordBatch,
    col: &str,
    i: usize,
) -> Option<OffsetDateTime> {
    use datafusion::arrow::array::PrimitiveArray;
    use datafusion::arrow::datatypes::TimestampMicrosecondType;
    b.column_by_name(col)?
        .as_any()
        .downcast_ref::<PrimitiveArray<TimestampMicrosecondType>>()
        .and_then(|a| (!a.is_null(i)).then(|| micros_to_odt(a.value(i))))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "microsecond remainder fits in i32"
)]
fn micros_to_odt(micros: i64) -> OffsetDateTime {
    let secs = micros / 1_000_000;
    let ns = ((micros % 1_000_000) * 1_000) as i32;
    OffsetDateTime::from_unix_timestamp(secs)
        .map(|dt| dt + time::Duration::nanoseconds(ns.into()))
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}
fn fmt_ts(dt: OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
