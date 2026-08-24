//! PostgreSQL implementation of [`ProcessProjectionRepository`].

use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use mako_obs::{
    domain::{DeadlineRisk, KpiReport, ObsQuery, ProcessProjection, ProcessState},
    error::ObsError,
    repository::ProcessProjectionRepository,
};

/// Terminal `state` literals as they appear in SQL. Pinned to
/// [`ProcessState::is_terminal`] by `terminal_state_sql_matches_domain`.
pub const TERMINAL_STATE_SQL: &str = "'completed','rejected','failed'";

/// The columns every projection read selects, in the order `row_to_projection`
/// expects them. One constant so a column added to the table cannot reach one
/// query and miss three.
const PROJECTION_COLUMNS: &str = "process_id, pid, family, workflow_name, state, malo_id, \
     partner_mp_id, mdm_role, deadline_at, deadline_source, deadline_risk, started_at, \
     last_event_at, erc_code, initiator_is_affiliate, tenant";

/// Upsert with a terminal-state guard.
///
/// The projection is fed by an at-least-once fan-out, so events arrive
/// redelivered and out of order. Three rules keep the row honest:
///
/// - once `state` is terminal it never moves back: a redelivered
///   `aperak.accepted` after `process.completed` would otherwise flip the row to
///   `running` and put a finished process back into the overdue sweep;
/// - `pid` / `family` / `started_at` / `deadline_at` are repaired when a later
///   event carries real values and the stored row still holds the defaults a
///   non-`initiated` first event left behind (pid 0, no deadline) — otherwise
///   that process stays outside every deadline sweep and per-PID KPI forever;
/// - `deadline_source` travels with `deadline_at`, always, so a stored instant
///   can never lose the Festlegung it came from.
const UPSERT_SQL: &str = r"INSERT INTO process_projections
      (process_id, pid, family, workflow_name, state, malo_id, partner_mp_id,
       mdm_role, deadline_at, deadline_source, deadline_risk, started_at, last_event_at,
       erc_code, initiator_is_affiliate, tenant, completed_at, updated_at)
  VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,
          CASE WHEN $5 IN ('completed','rejected','failed') THEN now() ELSE NULL END,
          now())
  ON CONFLICT (process_id) DO UPDATE SET
      state                  = CASE
                                   WHEN process_projections.state IN ('completed','rejected','failed')
                                   THEN process_projections.state
                                   ELSE EXCLUDED.state
                               END,
      deadline_risk          = EXCLUDED.deadline_risk,
      last_event_at          = GREATEST(EXCLUDED.last_event_at, process_projections.last_event_at),
      erc_code               = COALESCE(EXCLUDED.erc_code, process_projections.erc_code),
      malo_id                = COALESCE(EXCLUDED.malo_id, process_projections.malo_id),
      partner_mp_id          = COALESCE(EXCLUDED.partner_mp_id, process_projections.partner_mp_id),
      mdm_role               = COALESCE(EXCLUDED.mdm_role, process_projections.mdm_role),
      initiator_is_affiliate = EXCLUDED.initiator_is_affiliate OR process_projections.initiator_is_affiliate,
      workflow_name  = CASE WHEN EXCLUDED.workflow_name <> ''
                            THEN EXCLUDED.workflow_name
                            ELSE process_projections.workflow_name END,
      -- Repair the columns a non-initiated first event could not fill.
      pid            = CASE WHEN process_projections.pid = 0 THEN EXCLUDED.pid
                            ELSE process_projections.pid END,
      family         = CASE WHEN process_projections.family IN ('', 'unknown')
                            THEN EXCLUDED.family
                            ELSE process_projections.family END,
      started_at     = LEAST(EXCLUDED.started_at, process_projections.started_at),
      deadline_at    = COALESCE(process_projections.deadline_at, EXCLUDED.deadline_at),
      -- The citation follows whichever instant survived, never the other one.
      deadline_source = CASE WHEN process_projections.deadline_at IS NULL
                             THEN EXCLUDED.deadline_source
                             ELSE process_projections.deadline_source END,
      -- Set completed_at once when state first becomes terminal; never overwrite.
      completed_at   = CASE
                           WHEN EXCLUDED.state IN ('completed','rejected','failed')
                                AND process_projections.completed_at IS NULL
                           THEN now()
                           ELSE process_projections.completed_at
                       END,
      updated_at     = now()";

/// KPIs for one PID over a calendar period, bucketed by `started_at`.
///
/// **The two clocks are two columns.** `total_aperak_timeout` counts rows in the
/// technical-acknowledgement state; `total_frist_breached` counts rows that
/// passed their *business* Antwortfrist. One number for both names the wrong
/// obligation.
const KPI_SQL: &str = r"SELECT
      COUNT(*)                                            AS total_initiated,
      COUNT(*) FILTER (WHERE state = 'completed')         AS total_completed,
      COUNT(*) FILTER (WHERE state = 'rejected')          AS total_rejected,
      COUNT(*) FILTER (WHERE state = 'failed')            AS total_failed,
      COUNT(*) FILTER (WHERE state = 'aperak_timeout')    AS total_aperak_timeout,
      COUNT(*) FILTER (WHERE deadline_at IS NOT NULL)     AS total_with_frist,
      COUNT(*) FILTER (
          WHERE deadline_at IS NOT NULL
            AND deadline_at < COALESCE(completed_at, now())
      )                                                   AS total_frist_breached,
      AVG(EXTRACT(EPOCH FROM (completed_at - started_at)) / 3600.0)
          FILTER (WHERE completed_at IS NOT NULL)         AS avg_cycle_time_hours,
      PERCENTILE_CONT(0.95) WITHIN GROUP (
          ORDER BY EXTRACT(EPOCH FROM (completed_at - started_at)) / 3600.0
      ) FILTER (WHERE completed_at IS NOT NULL)           AS p95_cycle_time_hours
  FROM process_projections
  WHERE pid = $1
    AND started_at::date >= $2
    AND started_at::date <= $3
    AND ($4::text IS NULL OR tenant = $4)";

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
        sqlx::query(UPSERT_SQL)
            .bind(p.process_id)
            .bind(i32::try_from(p.pid).unwrap_or(0))
            .bind(&p.family)
            .bind(&p.workflow_name)
            .bind(p.state.as_str())
            .bind(&p.malo_id)
            .bind(&p.partner_mp_id)
            .bind(&p.mdm_role)
            .bind(p.deadline_at)
            .bind(&p.deadline_source)
            .bind(p.deadline_risk.as_str())
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
        let rows = sqlx::query(&format!(
            "SELECT {PROJECTION_COLUMNS}
              FROM process_projections
              WHERE ($1::text IS NULL OR state = $1)
                AND ($2::int  IS NULL OR pid   = $2)
                AND ($3::text IS NULL OR partner_mp_id = $3)
                AND ($4::text IS NULL OR mdm_role = $4)
                AND ($5::timestamptz IS NULL OR started_at >= $5)
                AND ($6::text IS NULL OR tenant = $6)
                AND ($7::text IS NULL OR family = $7)
              ORDER BY last_event_at DESC
              LIMIT $8"
        ))
        .bind(q.state.map(ProcessState::as_str))
        .bind(q.pid.and_then(|p| i32::try_from(p).ok()))
        .bind(&q.partner_mp_id)
        .bind(&q.mdm_role)
        .bind(q.since)
        .bind(&q.tenant)
        .bind(&q.family)
        .bind(i64::from(q.limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| row_to_projection(&row))
            .collect::<Result<Vec<_>, _>>()
    }

    async fn get(&self, process_id: Uuid) -> Result<Option<ProcessProjection>, ObsError> {
        let row = sqlx::query(&format!(
            "SELECT {PROJECTION_COLUMNS} FROM process_projections WHERE process_id = $1"
        ))
        .bind(process_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;

        row.map(|r| row_to_projection(&r)).transpose()
    }

    /// KPIs for one PID over a calendar period, bucketed by `started_at`.
    ///
    /// **The two clocks are two columns.** `total_aperak_timeout` counts rows in
    /// the technical-acknowledgement state; `total_frist_breached` counts rows
    /// that passed their *business* Antwortfrist — closed after it, or still
    /// open past it.
    ///
    /// `total_with_frist` is reported beside the rate because it is the
    /// interesting figure: a bucket where few PIDs carry a published window is
    /// mostly *unmeasured*, which a compliance rate near 1.0 would hide.
    async fn kpi_report(
        &self,
        pid: u32,
        from: Date,
        to: Date,
        tenant: &str,
    ) -> Result<KpiReport, ObsError> {
        let row = sqlx::query(KPI_SQL)
            .bind(i32::try_from(pid).unwrap_or(0))
            .bind(from)
            .bind(to)
            .bind(tenant_filter(tenant))
            .fetch_one(&self.pool)
            .await
            .map_err(|e| ObsError::Database(e.to_string()))?;

        use sqlx::Row;
        let count = |name: &str| -> i64 { row.try_get(name).unwrap_or(0) };
        let total = count("total_initiated");
        if total == 0 {
            return Err(ObsError::NoKpiData {
                pid,
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        let with_frist = count("total_with_frist");
        let breached = count("total_frist_breached");

        Ok(KpiReport {
            pid,
            period_from: from,
            period_to: to,
            total_initiated: total.unsigned_abs(),
            total_completed: count("total_completed").unsigned_abs(),
            total_rejected: count("total_rejected").unsigned_abs(),
            total_failed: count("total_failed").unsigned_abs(),
            total_aperak_timeout: count("total_aperak_timeout").unsigned_abs(),
            total_frist_breached: breached.unsigned_abs(),
            total_with_frist: with_frist.unsigned_abs(),
            // No published window in the bucket means no rate — not 100 %.
            frist_compliance_rate: (with_frist > 0)
                .then(|| 1.0 - (breached as f64 / with_frist as f64)),
            // NULL until something in the bucket closes. Carried as `None`, so
            // no surface has to patch a placeholder 0.0 back to null — and none
            // can forget to.
            avg_cycle_time_hours: row.try_get("avg_cycle_time_hours").unwrap_or(None),
            p95_cycle_time_hours: row.try_get("p95_cycle_time_hours").unwrap_or(None),
        })
    }

    async fn overdue_processes(
        &self,
        now: OffsetDateTime,
        tenant: &str,
    ) -> Result<Vec<ProcessProjection>, ObsError> {
        let rows = sqlx::query(&format!(
            "SELECT {PROJECTION_COLUMNS}
              FROM process_projections
              WHERE state NOT IN ({TERMINAL_STATE_SQL})
                AND deadline_at IS NOT NULL
                AND deadline_at < $1
                AND ($2::text IS NULL OR tenant = $2)
              ORDER BY deadline_at ASC
              LIMIT $3"
        ))
        .bind(now)
        .bind(tenant_filter(tenant))
        .bind(OVERDUE_LIMIT)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ObsError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| row_to_projection(&row))
            .collect::<Result<Vec<_>, _>>()
    }
}

/// Cap on rows one overdue query returns.
///
/// Bounded so a backlog cannot return a million rows into an agent's context or
/// an operator's browser. Callers that need to know whether the cap bit compare
/// the length against this.
pub const OVERDUE_LIMIT: i64 = 500;

fn tenant_filter(tenant: &str) -> Option<&str> {
    (!tenant.is_empty()).then_some(tenant)
}

// ── Row mapping ───────────────────────────────────────────────────────────────

fn row_to_projection(row: &sqlx::postgres::PgRow) -> Result<ProcessProjection, ObsError> {
    use sqlx::Row;
    let db = |e: sqlx::Error| ObsError::Database(e.to_string());
    Ok(ProcessProjection {
        process_id: row.try_get("process_id").map_err(db)?,
        pid: row.try_get::<i32, _>("pid").map_err(db)?.unsigned_abs(),
        family: row.try_get("family").map_err(db)?,
        workflow_name: row.try_get("workflow_name").map_err(db)?,
        // An unrecognised literal is not silently `initiated`: that would report
        // a finished process as running. It is a read failure.
        state: ProcessState::from_str_exact(row.try_get::<&str, _>("state").map_err(db)?)
            .ok_or_else(|| {
                ObsError::Database("stored process state is not a known literal".to_owned())
            })?,
        malo_id: row.try_get("malo_id").map_err(db)?,
        partner_mp_id: row.try_get("partner_mp_id").map_err(db)?,
        mdm_role: row.try_get("mdm_role").map_err(db)?,
        deadline_at: row.try_get("deadline_at").map_err(db)?,
        deadline_source: row.try_get("deadline_source").map_err(db)?,
        deadline_risk: DeadlineRisk::from_str_or_unknown(
            row.try_get::<&str, _>("deadline_risk").map_err(db)?,
        ),
        started_at: row.try_get("started_at").map_err(db)?,
        last_event_at: row.try_get("last_event_at").map_err(db)?,
        erc_code: row.try_get("erc_code").map_err(db)?,
        initiator_is_affiliate: row.try_get("initiator_is_affiliate").unwrap_or(false),
        tenant: row.try_get("tenant").map_err(db)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SQL literals and `ProcessState::is_terminal` must name the same set,
    /// or the upsert guard lets a redelivered event reopen a finished process.
    #[test]
    fn terminal_state_sql_matches_domain() {
        for s in ProcessState::ALL {
            let literal = format!("'{}'", s.as_str());
            assert_eq!(
                TERMINAL_STATE_SQL.contains(&literal),
                s.is_terminal(),
                "{literal} membership in TERMINAL_STATE_SQL disagrees with is_terminal()"
            );
            assert_eq!(
                UPSERT_SQL.contains(&literal),
                s.is_terminal(),
                "{literal} membership in the upsert guard disagrees with is_terminal()"
            );
        }
    }

    /// The KPI query must count the literals the projection actually writes.
    ///
    /// Casing or spelling drift here is silent: a filter on a literal nothing
    /// stores reports zero, which reads as "this never happens".
    #[test]
    fn the_kpi_query_counts_stored_literals() {
        for s in [
            ProcessState::Completed,
            ProcessState::Rejected,
            ProcessState::Failed,
            ProcessState::AperakTimeout,
        ] {
            let filter = format!("state = '{}'", s.as_str());
            assert!(
                KPI_SQL.contains(&filter),
                "the KPI report does not count {s:?} under its stored literal"
            );
        }
        assert!(
            !KPI_SQL.contains("'cancelled'"),
            "the KPI report still names the retired `cancelled` literal"
        );
    }

    /// The KPI bucket is keyed on `started_at`, never `updated_at`.
    ///
    /// A report grouped by `updated_at` moves rows between periods as later
    /// events touch them, so re-running a closed period yields different
    /// numbers — which is the one property a regulatory filing must not have.
    #[test]
    fn the_kpi_bucket_is_keyed_on_started_at() {
        assert!(KPI_SQL.contains("started_at::date >="));
        assert!(
            !KPI_SQL.contains("updated_at"),
            "the KPI bucket must not depend on when a row was last touched"
        );
    }

    /// The two clocks stay two columns.
    #[test]
    fn the_kpi_report_separates_the_aperak_clock_from_the_antwortfrist() {
        assert!(
            KPI_SQL.contains("AS total_aperak_timeout"),
            "the technical acknowledgement clock"
        );
        assert!(
            KPI_SQL.contains("AS total_frist_breached") && KPI_SQL.contains("deadline_at <"),
            "the business Antwortfrist, measured against deadline_at"
        );
    }

    /// Every projection read must select every column `row_to_projection`
    /// looks up, or a query added later fails at runtime on a missing column.
    #[test]
    fn the_projection_column_list_covers_every_mapped_field() {
        for column in [
            "process_id",
            "pid",
            "family",
            "workflow_name",
            "state",
            "malo_id",
            "partner_mp_id",
            "mdm_role",
            "deadline_at",
            "deadline_source",
            "deadline_risk",
            "started_at",
            "last_event_at",
            "erc_code",
            "initiator_is_affiliate",
            "tenant",
        ] {
            assert!(
                PROJECTION_COLUMNS.contains(column),
                "`{column}` is mapped but not selected"
            );
        }
    }

    /// A stored instant may never lose its Fundstelle.
    #[test]
    fn the_upsert_carries_the_deadline_source_with_the_deadline() {
        assert!(UPSERT_SQL.contains("deadline_source"));
    }
}
