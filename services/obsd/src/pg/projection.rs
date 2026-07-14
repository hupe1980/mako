//! PostgreSQL implementation of [`ProcessProjectionRepository`].

use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use mako_obs::{
    domain::{DeadlineRisk, KpiReport, ObsQuery, ProcessProjection, ProcessState},
    error::ObsError,
    repository::ProcessProjectionRepository,
};

#[derive(Clone, Debug)]
pub struct PgProcessProjectionRepository {
    pool: PgPool,
}

impl PgProcessProjectionRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return a reference to the underlying pool (used by readiness probe).
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

impl ProcessProjectionRepository for PgProcessProjectionRepository {
    async fn upsert(&self, p: &ProcessProjection) -> Result<(), ObsError> {
        sqlx::query(
            r"INSERT INTO process_projections
                  (process_id, pid, family, workflow_name, state, malo_id, partner_mp_id,
                   mdm_role, deadline_at, deadline_risk, started_at, last_event_at,
                   erc_code, initiator_is_affiliate, tenant, completed_at, updated_at)
              VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,
                      CASE WHEN $5 IN ('completed','rejected','cancelled') THEN now() ELSE NULL END,
                      now())
              ON CONFLICT (process_id) DO UPDATE SET
                  state                  = EXCLUDED.state,
                  deadline_risk          = EXCLUDED.deadline_risk,
                  last_event_at          = EXCLUDED.last_event_at,
                  erc_code               = COALESCE(EXCLUDED.erc_code, process_projections.erc_code),
                  malo_id                = COALESCE(EXCLUDED.malo_id, process_projections.malo_id),
                  partner_mp_id          = COALESCE(EXCLUDED.partner_mp_id, process_projections.partner_mp_id),
                  mdm_role               = COALESCE(EXCLUDED.mdm_role, process_projections.mdm_role),
                  initiator_is_affiliate = EXCLUDED.initiator_is_affiliate OR process_projections.initiator_is_affiliate,
                  workflow_name  = CASE WHEN EXCLUDED.workflow_name <> ''
                                        THEN EXCLUDED.workflow_name
                                        ELSE process_projections.workflow_name END,
                  -- Set completed_at once when state first becomes terminal; never overwrite.
                  completed_at   = CASE
                                       WHEN EXCLUDED.state IN ('completed','rejected','cancelled')
                                            AND process_projections.completed_at IS NULL
                                       THEN now()
                                       ELSE process_projections.completed_at
                                   END,
                  updated_at     = now()",
        )
        .bind(p.process_id)
        .bind(p.pid as i32)
        .bind(&p.family)
        .bind(&p.workflow_name)
        .bind(state_to_str(p.state))
        .bind(&p.malo_id)
        .bind(&p.partner_mp_id)
        .bind(&p.mdm_role)
        .bind(p.deadline_at)
        .bind(risk_to_str(p.deadline_risk))
        .bind(p.started_at)
        .bind(p.last_event_at)
        .bind(&p.erc_code)
        .bind(p.initiator_is_affiliate)
        .bind(&p.tenant)
        .execute(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;
        Ok(())
    }

    async fn query(&self, q: &ObsQuery) -> Result<Vec<ProcessProjection>, ObsError> {
        let rows = sqlx::query(
            r"SELECT process_id, pid, family, workflow_name, state, malo_id, partner_mp_id,
                     mdm_role, deadline_at, deadline_risk, started_at, last_event_at,
                     erc_code, initiator_is_affiliate, tenant
              FROM process_projections
              WHERE ($1::text IS NULL OR state = $1)
                AND ($2::int  IS NULL OR pid   = $2)
                AND ($3::text IS NULL OR partner_mp_id = $3)
                AND ($4::text IS NULL OR mdm_role = $4)
                AND ($5::timestamptz IS NULL OR started_at >= $5)
                AND ($6::text IS NULL OR tenant = $6)
              ORDER BY last_event_at DESC
              LIMIT $7",
        )
        .bind(q.state.map(state_to_str))
        .bind(q.pid.map(|p| p as i32))
        .bind(&q.partner_mp_id)
        .bind(&q.mdm_role)
        .bind(q.since)
        .bind(&q.tenant)
        .bind(q.limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| row_to_projection(&row))
            .collect::<Result<Vec<_>, _>>()
    }

    async fn get(&self, process_id: Uuid) -> Result<Option<ProcessProjection>, ObsError> {
        let row = sqlx::query(
            r"SELECT process_id, pid, family, workflow_name, state, malo_id, partner_mp_id,
                     mdm_role, deadline_at, deadline_risk, started_at, last_event_at,
                     erc_code, initiator_is_affiliate, tenant
              FROM process_projections
              WHERE process_id = $1",
        )
        .bind(process_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;

        row.map(|r| row_to_projection(&r)).transpose()
    }

    async fn kpi_report(
        &self,
        pid: u32,
        from: Date,
        to: Date,
        tenant: &str,
    ) -> Result<KpiReport, ObsError> {
        let row = sqlx::query(
            r"SELECT
                  COUNT(*)                                             AS total_initiated,
                  COUNT(*) FILTER (WHERE state = 'completed')        AS total_completed,
                  COUNT(*) FILTER (WHERE state = 'rejected')         AS total_rejected,
                  COUNT(*) FILTER (WHERE state = 'aperak_timeout')   AS total_timeout,
                  COUNT(*) FILTER (WHERE state = 'cancelled')        AS total_cancelled,
                  AVG(EXTRACT(EPOCH FROM (completed_at - started_at)) / 3600.0)
                      FILTER (WHERE completed_at IS NOT NULL)        AS avg_cycle_time_hours,
                  PERCENTILE_CONT(0.95) WITHIN GROUP (
                      ORDER BY EXTRACT(EPOCH FROM (completed_at - started_at)) / 3600.0
                  ) FILTER (WHERE completed_at IS NOT NULL)          AS p95_cycle_time_hours
              FROM process_projections
              WHERE pid = $1
                AND started_at::date >= $2
                AND started_at::date <= $3
                AND ($4::text IS NULL OR tenant = $4)",
        )
        .bind(pid as i32)
        .bind(from)
        .bind(to)
        .bind(if tenant.is_empty() {
            None
        } else {
            Some(tenant)
        })
        .fetch_one(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;

        use sqlx::Row;
        let total: i64 = row.try_get("total_initiated").unwrap_or(0);
        if total == 0 {
            return Err(ObsError::NoKpiData {
                pid,
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        let completed: i64 = row.try_get("total_completed").unwrap_or(0);
        let rejected: i64 = row.try_get("total_rejected").unwrap_or(0);
        let timeout: i64 = row.try_get("total_timeout").unwrap_or(0);
        let cancelled: i64 = row.try_get("total_cancelled").unwrap_or(0);
        let avg_cycle_time_hours: f64 = row.try_get("avg_cycle_time_hours").unwrap_or(0.0);
        let p95_cycle_time_hours: f64 = row.try_get("p95_cycle_time_hours").unwrap_or(0.0);

        let compliance = (total - timeout) as f64 / total as f64;

        Ok(KpiReport {
            pid,
            period_from: from,
            period_to: to,
            total_initiated: total as u64,
            total_completed: completed as u64,
            total_rejected: rejected as u64,
            total_aperak_timeout: timeout as u64,
            total_cancelled: cancelled as u64,
            aperak_compliance_rate: compliance,
            avg_cycle_time_hours,
            p95_cycle_time_hours,
        })
    }

    async fn overdue_processes(
        &self,
        now: OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<ProcessProjection>, ObsError> {
        let rows = sqlx::query(
            r"SELECT process_id, pid, family, workflow_name, state, malo_id, partner_mp_id,
                     mdm_role, deadline_at, deadline_risk, started_at, last_event_at,
                     erc_code, initiator_is_affiliate, tenant
              FROM process_projections
              WHERE state NOT IN ('completed','rejected','cancelled')
                AND deadline_at IS NOT NULL
                AND deadline_at < $1
                AND ($2::text IS NULL OR tenant = $2)
              ORDER BY deadline_at ASC",
        )
        .bind(now)
        .bind(if tenant.is_empty() {
            None
        } else {
            Some(tenant)
        })
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| row_to_projection(&row))
            .collect::<Result<Vec<_>, _>>()
    }
}

// ── Row mapping helpers ───────────────────────────────────────────────────────

fn row_to_projection(row: &sqlx::postgres::PgRow) -> Result<ProcessProjection, ObsError> {
    use sqlx::Row;
    Ok(ProcessProjection {
        process_id: row
            .try_get("process_id")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        pid: row
            .try_get::<i32, _>("pid")
            .map_err(|e| ObsError::Database(e.to_string()))? as u32,
        family: row
            .try_get("family")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        workflow_name: row
            .try_get("workflow_name")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        state: str_to_state(
            row.try_get::<&str, _>("state")
                .map_err(|e| ObsError::Database(e.to_string()))?,
        ),
        malo_id: row
            .try_get("malo_id")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        partner_mp_id: row
            .try_get("partner_mp_id")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        mdm_role: row
            .try_get("mdm_role")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        deadline_at: row
            .try_get("deadline_at")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        deadline_risk: str_to_risk(
            row.try_get::<&str, _>("deadline_risk")
                .map_err(|e| ObsError::Database(e.to_string()))?,
        ),
        started_at: row
            .try_get("started_at")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        last_event_at: row
            .try_get("last_event_at")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        erc_code: row
            .try_get("erc_code")
            .map_err(|e| ObsError::Database(e.to_string()))?,
        initiator_is_affiliate: row.try_get("initiator_is_affiliate").unwrap_or(false),
        tenant: row
            .try_get("tenant")
            .map_err(|e| ObsError::Database(e.to_string()))?,
    })
}

fn state_to_str(s: ProcessState) -> &'static str {
    match s {
        ProcessState::Initiated => "initiated",
        ProcessState::Running => "running",
        ProcessState::AperakTimeout => "aperak_timeout",
        ProcessState::Completed => "completed",
        ProcessState::Rejected => "rejected",
        ProcessState::Cancelled => "cancelled",
    }
}

fn str_to_state(s: &str) -> ProcessState {
    match s {
        "running" => ProcessState::Running,
        "aperak_timeout" => ProcessState::AperakTimeout,
        "completed" => ProcessState::Completed,
        "rejected" => ProcessState::Rejected,
        "cancelled" => ProcessState::Cancelled,
        _ => ProcessState::Initiated,
    }
}

fn risk_to_str(r: DeadlineRisk) -> &'static str {
    match r {
        DeadlineRisk::Green => "green",
        DeadlineRisk::Amber => "amber",
        DeadlineRisk::Red => "red",
    }
}

fn str_to_risk(s: &str) -> DeadlineRisk {
    match s {
        "amber" => DeadlineRisk::Amber,
        "red" => DeadlineRisk::Red,
        _ => DeadlineRisk::Green,
    }
}
