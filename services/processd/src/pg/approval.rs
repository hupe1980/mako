//! PostgreSQL implementation for `approval_queue`.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum QueueStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl std::fmt::Display for QueueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Approved => write!(f, "Approved"),
            Self::Rejected => write!(f, "Rejected"),
            Self::Expired => write!(f, "Expired"),
        }
    }
}

impl std::str::FromStr for QueueStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(Self::Pending),
            "Approved" => Ok(Self::Approved),
            "Rejected" => Ok(Self::Rejected),
            "Expired" => Ok(Self::Expired),
            other => Err(format!("unknown QueueStatus: {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalQueueEntry {
    pub id: Uuid,
    pub process_id: Uuid,
    pub pid: i32,
    pub malo_id: Option<String>,
    pub reason: String,
    pub status: QueueStatus,
    pub expires_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
    pub decided_at: Option<OffsetDateTime>,
    pub tenant: String,
}

fn map_entry(row: &PgRow) -> Result<ApprovalQueueEntry, sqlx::Error> {
    let status_str: String = row.try_get("status")?;
    let status = status_str
        .parse::<QueueStatus>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "status".into(),
            source: Box::new(std::io::Error::other(e)),
        })?;
    Ok(ApprovalQueueEntry {
        id: row.try_get("id")?,
        process_id: row.try_get("process_id")?,
        pid: row.try_get("pid")?,
        malo_id: row.try_get("malo_id")?,
        reason: row.try_get("reason")?,
        status,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        decided_at: row.try_get("decided_at")?,
        tenant: row.try_get("tenant")?,
    })
}

#[derive(Clone, Debug)]
pub struct PgApprovalQueue {
    pool: PgPool,
}

impl PgApprovalQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn enqueue(&self, entry: &ApprovalQueueEntry) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO approval_queue (id, process_id, pid, malo_id, reason, status, expires_at, created_at, tenant) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT (process_id, tenant) DO NOTHING",
        )
        .bind(entry.id).bind(entry.process_id).bind(entry.pid).bind(&entry.malo_id)
        .bind(&entry.reason).bind(entry.status.to_string()).bind(entry.expires_at)
        .bind(entry.created_at).bind(&entry.tenant)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn approve(&self, id: Uuid, tenant: &str) -> Result<bool, sqlx::Error> {
        let r = sqlx::query("UPDATE approval_queue SET status = 'Approved', decided_at = now() WHERE id = $1 AND tenant = $2 AND status = 'Pending'")
            .bind(id).bind(tenant).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn reject(&self, id: Uuid, tenant: &str) -> Result<bool, sqlx::Error> {
        let r = sqlx::query("UPDATE approval_queue SET status = 'Rejected', decided_at = now() WHERE id = $1 AND tenant = $2 AND status = 'Pending'")
            .bind(id).bind(tenant).execute(&self.pool).await?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn expire_stale(&self) -> Result<u64, sqlx::Error> {
        let r = sqlx::query("UPDATE approval_queue SET status = 'Expired', decided_at = now() WHERE status = 'Pending' AND expires_at < now()")
            .execute(&self.pool).await?;
        Ok(r.rows_affected())
    }

    pub async fn list(
        &self,
        tenant: &str,
        status: Option<QueueStatus>,
        limit: u32,
    ) -> Result<Vec<ApprovalQueueEntry>, sqlx::Error> {
        let status_str = status.map(|s| s.to_string());
        let rows = sqlx::query(
            "SELECT id, process_id, pid, malo_id, reason, status, expires_at, created_at, decided_at, tenant FROM approval_queue WHERE tenant = $1 AND ($2::text IS NULL OR status = $2) ORDER BY created_at DESC LIMIT $3",
        )
        .bind(tenant).bind(status_str).bind(limit as i64)
        .fetch_all(&self.pool).await?;
        rows.iter().map(map_entry).collect()
    }

    pub async fn find_by_id(
        &self,
        id: Uuid,
        tenant: &str,
    ) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
        let opt = sqlx::query(
            "SELECT id, process_id, pid, malo_id, reason, status, expires_at, created_at, decided_at, tenant FROM approval_queue WHERE id = $1 AND tenant = $2",
        )
        .bind(id).bind(tenant).fetch_optional(&self.pool).await?;
        opt.map(|r| map_entry(&r)).transpose()
    }
}
