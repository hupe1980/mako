//! Archive domain types for Iceberg/S3 long-term storage of meter reads.
//!
//! `ArchiveConfig` is intentionally free of S3 credentials — those live in
//! the service layer so the domain crate stays free of I/O dependencies.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Iceberg/S3 archival configuration.
///
/// Set via the `[archive]` section of `edmd.toml`:
///
/// ```toml
/// [archive]
/// enabled                = true
/// storage_uri            = "s3://my-bucket/edmd/meter_reads"
/// retention_months       = 12      # rows older than this are archived
/// batch_size             = 100000  # rows per batch
/// interval_secs          = 3600    # run every hour
/// # Iceberg catalog lives in the same PostgreSQL — no extra service needed.
/// iceberg_catalog_schema = "iceberg_catalog"  # default
/// iceberg_catalog_name   = "edmd"             # default
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveConfig {
    /// Enable archival worker.  Default: `false`.
    #[serde(default)]
    pub enabled: bool,

    /// Root URI for Parquet/Iceberg data files.
    /// Supported schemes: `s3://`, `gs://`, `abfss://`, `file://`.
    /// Example: `s3://my-bucket/edmd/meter_reads`
    pub storage_uri: String,

    /// Months of hot-tier retention.  Reads older than this are archived.
    /// Default: 12.
    #[serde(default = "default_retention_months")]
    pub retention_months: u32,

    /// Maximum rows per archival batch.
    /// Default: 100 000.
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,

    /// Archival worker interval in seconds.
    /// Default: 3 600 (1 hour).
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,

    /// PostgreSQL schema that holds the Iceberg SQL-catalog tables
    /// (`iceberg_tables`, `iceberg_namespace_properties`, etc.).
    /// Default: `"iceberg_catalog"`.
    #[serde(default = "default_catalog_schema")]
    pub iceberg_catalog_schema: String,

    /// Logical catalog name registered in the SQL catalog.
    /// Default: `"edmd"`.
    #[serde(default = "default_catalog_name")]
    pub iceberg_catalog_name: String,

    /// S3/object-store AWS access key ID.
    /// Use `"env:AWS_ACCESS_KEY_ID"` to read from the environment.
    pub access_key_id: Option<String>,

    /// S3/object-store AWS secret access key.
    /// Use `"env:AWS_SECRET_ACCESS_KEY"` to read from the environment.
    pub secret_access_key: Option<String>,

    /// AWS region.  Default: `"eu-central-1"`.
    #[serde(default = "default_aws_region")]
    pub region: String,

    /// Optional S3-compatible endpoint override (MinIO, LocalStack, etc.).
    pub endpoint_url: Option<String>,
}

fn default_retention_months() -> u32 {
    12
}
fn default_batch_size() -> u32 {
    100_000
}
fn default_interval_secs() -> u64 {
    3_600
}
fn default_catalog_schema() -> String {
    "iceberg_catalog".to_owned()
}
fn default_catalog_name() -> String {
    "edmd".to_owned()
}
fn default_aws_region() -> String {
    "eu-central-1".to_owned()
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            storage_uri: String::new(),
            retention_months: default_retention_months(),
            batch_size: default_batch_size(),
            interval_secs: default_interval_secs(),
            iceberg_catalog_schema: default_catalog_schema(),
            iceberg_catalog_name: default_catalog_name(),
            access_key_id: None,
            secret_access_key: None,
            region: default_aws_region(),
            endpoint_url: None,
        }
    }
}

// ── Batch metadata ─────────────────────────────────────────────────────────────

/// Lifecycle status of an archival batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveBatchStatus {
    Pending,
    Writing,
    Committed,
    Failed,
}

impl std::fmt::Display for ArchiveBatchStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Writing => write!(f, "writing"),
            Self::Committed => write!(f, "committed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Metadata for a completed (or in-progress) archival batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedBatch {
    pub batch_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub cutoff_before: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub dtm_from_min: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub dtm_from_max: Option<OffsetDateTime>,
    pub row_count: i64,
    pub malo_count: i32,
    pub s3_prefix: String,
    pub file_count: i32,
    pub bytes_written: i64,
    pub status: ArchiveBatchStatus,
    pub error_msg: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub committed_at: Option<OffsetDateTime>,
    pub tenant_id: Option<Uuid>,
}

/// Summary of the Iceberg/S3 archive tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveStats {
    pub total_batches: i64,
    pub committed_batches: i64,
    pub total_rows_archived: i64,
    pub total_bytes_written: i64,
    pub oldest_cutoff: Option<OffsetDateTime>,
    pub newest_cutoff: Option<OffsetDateTime>,
}
