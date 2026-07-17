//! Archive domain types for Iceberg/S3 long-term storage of meter reads.
//!
//! This module contains the domain-facing batch metadata and statistics types.
//! `ArchiveConfig` (infrastructure config with S3 credentials) lives in
//! `services/edmd/src/config.rs` where it belongs alongside other service config.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

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
    /// Tenant data-isolation key. Matches `archive_batches.tenant`.
    pub tenant: String,
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
