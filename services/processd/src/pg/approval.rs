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
    /// `makod` command to dispatch when an operator approves. `None` means the
    /// approval carries no market message.
    pub approve_command: Option<String>,
    /// `makod` command to dispatch when an operator rejects. `None` means the
    /// rejection is recorded without a market message.
    pub reject_command: Option<String>,
    /// Marktrolle forwarded on the dispatched command.
    pub marktrolle: Option<String>,
    pub expires_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
    pub decided_at: Option<OffsetDateTime>,
    /// `sub` of the principal who decided this entry (§ 20 EnWG / GoBD).
    pub decided_by: Option<String>,
    pub tenant: String,
}

impl ApprovalQueueEntry {
    /// A pending entry. Pair it with [`Self::with_commands`] unless the decision
    /// genuinely has no market message — the REST handler dispatches what is
    /// stored here and nothing else.
    pub fn pending(
        process_id: Uuid,
        pid: i32,
        malo_id: Option<String>,
        reason: String,
        expires_at: OffsetDateTime,
        tenant: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            process_id,
            pid,
            malo_id,
            reason,
            status: QueueStatus::Pending,
            approve_command: None,
            reject_command: None,
            marktrolle: None,
            expires_at,
            created_at: OffsetDateTime::now_utc(),
            decided_at: None,
            decided_by: None,
            tenant,
        }
    }

    #[must_use]
    pub fn with_commands(mut self, approve: &str, reject: &str, marktrolle: Option<&str>) -> Self {
        self.approve_command = Some(approve.to_owned());
        self.reject_command = Some(reject.to_owned());
        self.marktrolle = marktrolle.map(ToOwned::to_owned);
        self
    }

    /// A decision where only *approving* sends a market message.
    ///
    /// The REQOTE Preisanfrage is the case: the AHB answer to it is a QUOTES,
    /// and there is no „Preisanfrage ablehnen". Rejecting such an entry records
    /// the operator's decision not to quote automatically and sends nothing —
    /// which is why `reject_command` stays `None` rather than repeating the
    /// approve command.
    #[must_use]
    pub fn with_approve_command(mut self, approve: &str, marktrolle: Option<&str>) -> Self {
        self.approve_command = Some(approve.to_owned());
        self.reject_command = None;
        self.marktrolle = marktrolle.map(ToOwned::to_owned);
        self
    }
}

/// Every column `map_entry` reads.
const ENTRY_COLUMNS: &str = "id, process_id, pid, malo_id, reason, status, approve_command, \
                             reject_command, marktrolle, expires_at, created_at, decided_at, \
                             decided_by, tenant";

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
        approve_command: row.try_get("approve_command")?,
        reject_command: row.try_get("reject_command")?,
        marktrolle: row.try_get("marktrolle")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        decided_at: row.try_get("decided_at")?,
        decided_by: row.try_get("decided_by")?,
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
            "INSERT INTO approval_queue (id, process_id, pid, malo_id, reason, status, approve_command, reject_command, marktrolle, expires_at, created_at, tenant) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT (process_id, tenant) DO NOTHING",
        )
        .bind(entry.id).bind(entry.process_id).bind(entry.pid).bind(&entry.malo_id)
        .bind(&entry.reason).bind(entry.status.to_string())
        .bind(&entry.approve_command).bind(&entry.reject_command).bind(&entry.marktrolle)
        .bind(entry.expires_at).bind(entry.created_at).bind(&entry.tenant)
        .execute(&self.pool).await?;
        Ok(())
    }

    /// Atomically move a `Pending` entry to `status` and return it.
    ///
    /// The decision must be claimed **before** the market command is dispatched:
    /// dispatch-then-record let two operators deciding at once send both an
    /// einwilligung and an ablehnen (different idempotency keys) while the DB
    /// recorded only one, and let a terminal entry re-trigger a market message.
    /// `Ok(None)` means the entry is gone or no longer Pending.
    pub async fn claim(
        &self,
        id: Uuid,
        tenant: &str,
        status: QueueStatus,
        decided_by: &str,
    ) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
        let opt = sqlx::query(&format!(
            "UPDATE approval_queue SET status = $3, decided_at = now(), decided_by = $4 \
             WHERE id = $1 AND tenant = $2 AND status = 'Pending' RETURNING {ENTRY_COLUMNS}"
        ))
        .bind(id)
        .bind(tenant)
        .bind(status.to_string())
        .bind(decided_by)
        .fetch_optional(&self.pool)
        .await?;
        opt.map(|r| map_entry(&r)).transpose()
    }

    /// Release a claim taken by [`Self::claim`] so the operator can retry.
    pub async fn unclaim(&self, id: Uuid, tenant: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE approval_queue SET status = 'Pending', decided_at = NULL, decided_by = NULL \
             WHERE id = $1 AND tenant = $2",
        )
        .bind(id)
        .bind(tenant)
        .execute(&self.pool)
        .await?;
        Ok(())
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
        let rows = sqlx::query(&format!(
            "SELECT {ENTRY_COLUMNS} FROM approval_queue WHERE tenant = $1 AND ($2::text IS NULL OR status = $2) ORDER BY created_at DESC LIMIT $3"
        ))
        .bind(tenant).bind(status_str).bind(limit as i64)
        .fetch_all(&self.pool).await?;
        rows.iter().map(map_entry).collect()
    }

    pub async fn find_by_id(
        &self,
        id: Uuid,
        tenant: &str,
    ) -> Result<Option<ApprovalQueueEntry>, sqlx::Error> {
        let opt = sqlx::query(&format!(
            "SELECT {ENTRY_COLUMNS} FROM approval_queue WHERE id = $1 AND tenant = $2"
        ))
        .bind(id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await?;
        opt.map(|r| map_entry(&r)).transpose()
    }
}
