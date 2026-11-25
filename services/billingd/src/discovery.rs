//! Durable evidence of which supply components the billing run could enumerate.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::Serialize;
use sqlx::{Connection as _, PgConnection, PgPool};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, JsonSchema, utoipa::ToSchema, sqlx::FromRow)]
pub struct Discovery {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub run_id: Uuid,
    #[schemars(with = "String", extend("format" = "date"))]
    pub business_date: Date,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String", extend("format" = "date-time"))]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub next_attempt_at: Option<OffsetDateTime>,
    pub source: String,
    pub origin: String,
    pub status: String,
    pub error_code: Option<String>,
    pub candidate_count: Option<i64>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub last_failure_at: Option<OffsetDateTime>,
    #[schemars(with = "String", extend("format" = "date"))]
    pub ended_from: Date,
    #[schemars(with = "String", extend("format" = "date"))]
    pub ended_through: Date,
}

pub async fn latest(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
) -> anyhow::Result<Option<Discovery>> {
    Ok(sqlx::query_as("SELECT run_id, business_date, started_at, finished_at, next_attempt_at,
        'vertragd'::text AS source, origin, status, error_code, candidate_count, ended_from, ended_through,
        (SELECT max(finished_at) FROM billing_discovery WHERE tenant=$1 AND status='failed') AS last_failure_at
        FROM billing_discovery WHERE tenant=$1 ORDER BY started_at DESC, run_id DESC LIMIT 1")
        .bind(tenant).fetch_optional(executor).await?)
}

/// The session lock releases on disconnect, including an interrupted worker.
pub(crate) struct Attempt {
    pub id: Uuid,
    pub from: Date,
    pub through: Date,
    pub conn: PgConnection,
}

pub(crate) enum Start {
    Busy,
    Replay(Uuid),
    New(Attempt),
}

impl Attempt {
    pub async fn start(
        pool: &PgPool,
        tenant: &str,
        today: Date,
        next: Option<OffsetDateTime>,
        requested: Option<Uuid>,
    ) -> anyhow::Result<Start> {
        // A detached worker connection leaves the request pool available even when its limit is one.
        let mut conn = pool.acquire().await?.detach();
        let acquired: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_lock(hashtextextended(
            jsonb_build_array(current_schema(), 'billing-discovery', $1::text)::text, 0))",
        )
        .bind(tenant)
        .fetch_one(&mut conn)
        .await?;
        if !acquired {
            return Ok(Start::Busy);
        }
        let mut tx = conn.begin().await?;
        sqlx::query("UPDATE billing_discovery SET status='failed', error_code='sweep_interrupted', finished_at=clock_timestamp()
            WHERE tenant=$1 AND status='running'").bind(tenant).execute(&mut *tx).await?;
        let previous: Option<(Uuid,String)> = sqlx::query_as("SELECT run_id, status FROM billing_discovery WHERE tenant=$1 AND
            ((run_id=$2 AND origin='manual') OR ($2 IS NULL AND origin='scheduled' AND business_date=$3 AND status='completed'))
            ORDER BY started_at DESC LIMIT 1").bind(tenant).bind(requested).bind(today).fetch_optional(&mut *tx).await?;
        if let Some((id, status)) = previous {
            tx.commit().await?;
            anyhow::ensure!(status == "completed", "the recorded sweep failed");
            return Ok(Start::Replay(id));
        }
        let (covered, first): (Option<Date>,Option<Date>) = sqlx::query_as("SELECT
            max(ended_through) FILTER (WHERE status='completed'), min(ended_from) FROM billing_discovery WHERE tenant=$1")
            .bind(tenant).fetch_one(&mut *tx).await?;
        let ordinary_from = today - time::Duration::days(84);
        let from = covered
            .and_then(Date::next_day)
            .or(first)
            .unwrap_or(ordinary_from)
            .min(ordinary_from);
        let through = (from + time::Duration::days(84)).min(today);
        let id = requested.unwrap_or_else(Uuid::new_v4);
        let origin = if requested.is_some() {
            "manual"
        } else {
            "scheduled"
        };
        sqlx::query("INSERT INTO billing_discovery (tenant, run_id, business_date, next_attempt_at, ended_from, ended_through, origin)
            VALUES ($1,$2,$3,$4,$5,$6,$7)").bind(tenant).bind(id).bind(today).bind(next).bind(from).bind(through).bind(origin)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Start::New(Self {
            id,
            from,
            through,
            conn,
        }))
    }

    pub async fn fail(&mut self, tenant: &str, code: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE billing_discovery SET status='failed', error_code=$3, finished_at=clock_timestamp()
            WHERE tenant=$1 AND run_id=$2 AND status='running'").bind(tenant).bind(self.id).bind(code)
            .execute(&mut self.conn).await.context("record discovery failure")?;
        Ok(())
    }
}

/// `list_unbilled_periods`' answer.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema, utoipa::ToSchema)]
pub struct UnbilledPeriods {
    /// The earliest `due_on` first.
    pub periods: Vec<crate::pg::UnbilledPeriodRow>,
    /// How many periods `periods` holds.
    pub count: usize,
    /// At least `limit` periods matched: more may be waiting than were listed.
    pub saturated: bool,
    pub discovery: Option<Discovery>,
}

/// `summarize_billing_deadlines`' answer.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema, utoipa::ToSchema)]
pub struct DeadlineSummary {
    /// One row per Lieferant (MP-ID).
    pub lieferanten: Vec<crate::pg::DeadlineSummaryRow>,
    pub discovery: Option<Discovery>,
}

async fn snapshot(pool: &PgPool) -> anyhow::Result<sqlx::Transaction<'_, sqlx::Postgres>> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
impl UnbilledPeriods {
    pub async fn read(
        pool: &PgPool,
        tenant: &str,
        open: bool,
        limit: Option<i64>,
    ) -> anyhow::Result<Self> {
        let limit = limit.unwrap_or(100).clamp(1, 500);
        let mut tx = snapshot(pool).await?;
        let mut periods =
            crate::pg::list_unbilled_periods(&mut *tx, tenant, open, limit + 1).await?;
        let saturated = periods.len() > usize::try_from(limit)?;
        if saturated {
            periods.pop();
        }
        let discovery = latest(&mut *tx, tenant).await?;
        tx.commit().await?;
        Ok(Self {
            count: periods.len(),
            periods,
            saturated,
            discovery,
        })
    }
}
impl DeadlineSummary {
    pub async fn read(pool: &PgPool, tenant: &str) -> anyhow::Result<Self> {
        let mut tx = snapshot(pool).await?;
        let lieferanten = crate::pg::deadline_summary(&mut *tx, tenant).await?;
        let discovery = latest(&mut *tx, tenant).await?;
        tx.commit().await?;
        Ok(Self {
            lieferanten,
            discovery,
        })
    }
}
