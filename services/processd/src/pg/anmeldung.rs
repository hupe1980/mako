//! PostgreSQL implementation for `anmeldung_decisions`.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::OffsetDateTime;
use uuid::Uuid;

/// Outcome of an Anmeldung STP decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum AnmeldungDecision {
    Accept,
    Reject,
    Escalate,
}

impl std::fmt::Display for AnmeldungDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Accept => write!(f, "Accept"),
            Self::Reject => write!(f, "Reject"),
            Self::Escalate => write!(f, "Escalate"),
        }
    }
}

impl std::str::FromStr for AnmeldungDecision {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Accept" => Ok(Self::Accept),
            "Reject" => Ok(Self::Reject),
            "Escalate" => Ok(Self::Escalate),
            other => Err(format!("unknown AnmeldungDecision: {other:?}")),
        }
    }
}

/// One row in `anmeldung_decisions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnmeldungDecisionRecord {
    pub id: Uuid,
    pub process_id: Uuid,
    pub pid: i32,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub decision: AnmeldungDecision,
    pub erc_code: Option<String>,
    pub detail: Option<String>,
    pub initiator_is_affiliate: bool,
    pub decided_at: OffsetDateTime,
    pub tenant: String,
}

fn map_decision(row: &PgRow) -> Result<AnmeldungDecisionRecord, sqlx::Error> {
    let decision_str: String = row.try_get("decision")?;
    let decision =
        decision_str
            .parse::<AnmeldungDecision>()
            .map_err(|e| sqlx::Error::ColumnDecode {
                index: "decision".into(),
                source: Box::new(std::io::Error::other(e)),
            })?;
    Ok(AnmeldungDecisionRecord {
        id: row.try_get("id")?,
        process_id: row.try_get("process_id")?,
        pid: row.try_get("pid")?,
        malo_id: row.try_get("malo_id")?,
        lf_mp_id: row.try_get("lf_mp_id")?,
        decision,
        erc_code: row.try_get("erc_code")?,
        detail: row.try_get("detail")?,
        initiator_is_affiliate: row.try_get("initiator_is_affiliate")?,
        decided_at: row.try_get("decided_at")?,
        tenant: row.try_get("tenant")?,
    })
}

#[derive(Clone, Debug)]
pub struct PgAnmeldungRepository {
    pool: PgPool,
}

impl PgAnmeldungRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, rec: &AnmeldungDecisionRecord) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO anmeldung_decisions (id, process_id, pid, malo_id, lf_mp_id, decision, erc_code, detail, initiator_is_affiliate, decided_at, tenant) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (process_id, tenant) DO NOTHING",
        )
        .bind(rec.id)
        .bind(rec.process_id)
        .bind(rec.pid)
        .bind(&rec.malo_id)
        .bind(&rec.lf_mp_id)
        .bind(rec.decision.to_string())
        .bind(&rec.erc_code)
        .bind(&rec.detail)
        .bind(rec.initiator_is_affiliate)
        .bind(rec.decided_at)
        .bind(&rec.tenant)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list(
        &self,
        tenant: &str,
        limit: u32,
    ) -> Result<Vec<AnmeldungDecisionRecord>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, process_id, pid, malo_id, lf_mp_id, decision, erc_code, detail, initiator_is_affiliate, decided_at, tenant FROM anmeldung_decisions WHERE tenant = $1 ORDER BY decided_at DESC LIMIT $2",
        )
        .bind(tenant)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(map_decision).collect()
    }

    pub async fn stp_rate(&self, tenant: &str, days: u32) -> Result<Option<f64>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT COUNT(*) FILTER (WHERE decision = 'Accept') AS accept_count, COUNT(*) FILTER (WHERE decision IN ('Accept', 'Reject')) AS decidable_count FROM anmeldung_decisions WHERE tenant = $1 AND decided_at >= now() - ($2::int * INTERVAL '1 day')",
        )
        .bind(tenant)
        .bind(days as i32)
        .fetch_one(&self.pool)
        .await?;
        let accepts: i64 = row.try_get("accept_count")?;
        let decidable: i64 = row.try_get("decidable_count")?;
        if decidable == 0 {
            Ok(None)
        } else {
            Ok(Some(accepts as f64 / decidable as f64))
        }
    }

    /// Find a single decision by `process_id`.
    pub async fn find_by_process_id(
        &self,
        process_id: Uuid,
        tenant: &str,
    ) -> Result<Option<AnmeldungDecisionRecord>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, process_id, pid, malo_id, lf_mp_id, decision, erc_code, detail, \
             initiator_is_affiliate, decided_at, tenant \
             FROM anmeldung_decisions WHERE process_id = $1 AND tenant = $2 LIMIT 1",
        )
        .bind(process_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(map_decision).transpose()
    }

    /// List decisions where `initiator_is_affiliate = true` — §20 EnWG parity audit.
    pub async fn list_affiliate_decisions(
        &self,
        tenant: &str,
        days: u32,
        limit: u32,
    ) -> Result<Vec<AnmeldungDecisionRecord>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, process_id, pid, malo_id, lf_mp_id, decision, erc_code, detail, \
             initiator_is_affiliate, decided_at, tenant \
             FROM anmeldung_decisions \
             WHERE tenant = $1 AND initiator_is_affiliate = true \
               AND decided_at >= now() - ($2::int * INTERVAL '1 day') \
             ORDER BY decided_at DESC LIMIT $3",
        )
        .bind(tenant)
        .bind(days as i32)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(map_decision).collect()
    }

    /// STP breakdown by ERC code — for root-cause analysis when STP drops.
    ///
    /// Returns rows: `(erc_code, count)` ordered by count descending.
    pub async fn stp_breakdown_by_erc(
        &self,
        tenant: &str,
        days: u32,
    ) -> Result<Vec<(Option<String>, i64)>, sqlx::Error> {
        let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
            "SELECT erc_code, COUNT(*)::bigint AS cnt \
             FROM anmeldung_decisions \
             WHERE tenant = $1 AND decision = 'Reject' \
               AND decided_at >= now() - ($2::int * INTERVAL '1 day') \
             GROUP BY erc_code ORDER BY cnt DESC",
        )
        .bind(tenant)
        .bind(days as i32)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}
