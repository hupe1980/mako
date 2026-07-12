//! PostgreSQL implementation of [`PriCatRepository`].
//!
//! Backed by the `pricat_versions` and `pricat_dispatch_log` tables
//! (see `migrations/0001_initial.sql`).

use mako_markt::{
    error::MdmError,
    repository::{
        PreisblattSource, PriCatDispatchEntry, PriCatDispatchState, PriCatRepository, PriCatVersion,
    },
};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::Date;

/// PostgreSQL-backed PRICAT version history and dispatch repository.
#[derive(Clone, Debug)]
pub struct PgPriCatRepository {
    pool: PgPool,
}

impl PgPriCatRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ── row helpers ──────────────────────────────────────────────────────────────

fn row_to_version(r: &PgRow) -> PriCatVersion {
    let source_str: String = r.try_get("source").unwrap_or_else(|_| "api".to_owned());
    let source = source_str
        .parse::<PreisblattSource>()
        .unwrap_or(PreisblattSource::Api);

    let state_str: String = r
        .try_get("dispatch_state")
        .unwrap_or_else(|_| "pending".to_owned());
    let dispatch_state = match state_str.as_str() {
        "queued" => PriCatDispatchState::Queued,
        "done" => PriCatDispatchState::Done,
        "error" => PriCatDispatchState::Error,
        _ => PriCatDispatchState::Pending,
    };

    PriCatVersion {
        id: r.get("id"),
        nb_mp_id: r.get("nb_mp_id"),
        tenant: r.get("tenant"),
        valid_from: r.get("valid_from"),
        valid_to: r.try_get("valid_to").ok().flatten(),
        data: r.get("data"),
        bo4e_version: r
            .try_get("bo4e_version")
            .unwrap_or_else(|_| "v202607.0.0".to_owned()),
        source,
        dispatch_state,
        dispatch_error: r.try_get("dispatch_error").ok().flatten(),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

fn row_to_dispatch_entry(r: &PgRow) -> PriCatDispatchEntry {
    PriCatDispatchEntry {
        id: r.get("id"),
        pricat_version_id: r.get("pricat_version_id"),
        nb_mp_id: r.get("nb_mp_id"),
        lf_mp_id: r.get("lf_mp_id"),
        tenant: r.get("tenant"),
        process_id: r.try_get("process_id").ok().flatten(),
        dispatched_at: r.get("dispatched_at"),
        outcome: r.try_get("outcome").unwrap_or_else(|_| "ok".to_owned()),
        error_detail: r.try_get("error_detail").ok().flatten(),
    }
}

// ── dispatch_state helper ─────────────────────────────────────────────────────

fn state_str(state: &PriCatDispatchState) -> &'static str {
    match state {
        PriCatDispatchState::Pending => "pending",
        PriCatDispatchState::Queued => "queued",
        PriCatDispatchState::Done => "done",
        PriCatDispatchState::Error => "error",
    }
}

// ── PriCatRepository impl ────────────────────────────────────────────────────

impl PriCatRepository for PgPriCatRepository {
    #[allow(clippy::too_many_arguments)]
    async fn upsert_version(
        &self,
        nb_mp_id: &str,
        tenant: &str,
        valid_from: Date,
        valid_to: Option<Date>,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<uuid::Uuid, MdmError> {
        let row: (uuid::Uuid,) = sqlx::query_as(
            r#"INSERT INTO pricat_versions
                   (nb_mp_id, tenant, valid_from, valid_to, data, bo4e_version, source,
                    dispatch_queued_at, dispatch_done_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, NULL)
               ON CONFLICT (nb_mp_id, tenant, valid_from) DO UPDATE
               SET valid_to           = EXCLUDED.valid_to,
                   data               = EXCLUDED.data,
                   bo4e_version       = EXCLUDED.bo4e_version,
                   source             = EXCLUDED.source,
                   dispatch_queued_at = NULL,
                   dispatch_done_at   = NULL,
                   dispatch_error     = NULL,
                   updated_at         = now()
               RETURNING id"#,
        )
        .bind(nb_mp_id)
        .bind(tenant)
        .bind(valid_from)
        .bind(valid_to)
        .bind(&data)
        .bind(bo4e_version)
        .bind(source.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.0)
    }

    async fn list_versions(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<PriCatVersion>, MdmError> {
        let rows = sqlx::query(
            r#"SELECT id, nb_mp_id, tenant, valid_from, valid_to, data, bo4e_version,
                      source,
                      CASE
                          WHEN dispatch_done_at IS NOT NULL THEN 'done'
                          WHEN dispatch_queued_at IS NOT NULL THEN 'queued'
                          WHEN dispatch_error IS NOT NULL THEN 'error'
                          ELSE 'pending'
                      END AS dispatch_state,
                      dispatch_error,
                      created_at, updated_at
               FROM pricat_versions
               WHERE nb_mp_id = $1 AND tenant = $2
               ORDER BY valid_from DESC"#,
        )
        .bind(nb_mp_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.iter().map(row_to_version).collect())
    }

    async fn find_latest(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Option<PriCatVersion>, MdmError> {
        let row = sqlx::query(
            r#"SELECT id, nb_mp_id, tenant, valid_from, valid_to, data, bo4e_version,
                      source,
                      CASE
                          WHEN dispatch_done_at IS NOT NULL THEN 'done'
                          WHEN dispatch_queued_at IS NOT NULL THEN 'queued'
                          WHEN dispatch_error IS NOT NULL THEN 'error'
                          ELSE 'pending'
                      END AS dispatch_state,
                      dispatch_error,
                      created_at, updated_at
               FROM pricat_versions
               WHERE nb_mp_id = $1 AND tenant = $2
               ORDER BY valid_from DESC
               LIMIT 1"#,
        )
        .bind(nb_mp_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.as_ref().map(row_to_version))
    }

    async fn list_pending(&self, tenant: &str) -> Result<Vec<PriCatVersion>, MdmError> {
        let rows = sqlx::query(
            r#"SELECT id, nb_mp_id, tenant, valid_from, valid_to, data, bo4e_version,
                      source,
                      CASE
                          WHEN dispatch_done_at IS NOT NULL THEN 'done'
                          WHEN dispatch_queued_at IS NOT NULL THEN 'queued'
                          WHEN dispatch_error IS NOT NULL THEN 'error'
                          ELSE 'pending'
                      END AS dispatch_state,
                      dispatch_error,
                      created_at, updated_at
               FROM pricat_versions
               WHERE tenant = $1 AND dispatch_done_at IS NULL
               ORDER BY nb_mp_id, valid_from DESC"#,
        )
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.iter().map(row_to_version).collect())
    }

    async fn mark_queued(&self, id: uuid::Uuid) -> Result<(), MdmError> {
        sqlx::query(
            "UPDATE pricat_versions SET dispatch_queued_at = now(), updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn mark_done(&self, id: uuid::Uuid) -> Result<(), MdmError> {
        sqlx::query(
            "UPDATE pricat_versions
             SET dispatch_done_at = now(), dispatch_error = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn mark_error(&self, id: uuid::Uuid, error: &str) -> Result<(), MdmError> {
        sqlx::query(
            "UPDATE pricat_versions
             SET dispatch_error = $2, dispatch_queued_at = NULL, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn log_dispatch(&self, entry: PriCatDispatchEntry) -> Result<(), MdmError> {
        sqlx::query(
            r#"INSERT INTO pricat_dispatch_log
                   (id, pricat_version_id, nb_mp_id, lf_mp_id, tenant, process_id,
                    dispatched_at, outcome, error_detail)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(entry.id)
        .bind(entry.pricat_version_id)
        .bind(&entry.nb_mp_id)
        .bind(&entry.lf_mp_id)
        .bind(&entry.tenant)
        .bind(entry.process_id)
        .bind(entry.dispatched_at)
        .bind(&entry.outcome)
        .bind(&entry.error_detail)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn dispatch_log(
        &self,
        pricat_version_id: uuid::Uuid,
    ) -> Result<Vec<PriCatDispatchEntry>, MdmError> {
        let rows = sqlx::query(
            r#"SELECT id, pricat_version_id, nb_mp_id, lf_mp_id, tenant,
                      process_id, dispatched_at, outcome, error_detail
               FROM pricat_dispatch_log
               WHERE pricat_version_id = $1
               ORDER BY dispatched_at DESC"#,
        )
        .bind(pricat_version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.iter().map(row_to_dispatch_entry).collect())
    }
}

// ── unused helper suppression ─────────────────────────────────────────────────
// state_str is only used indirectly via the trait; keep a reference to silence
// dead_code lint without adding cfg(test).
const _: fn() = || {
    let _ = state_str(&PriCatDispatchState::Pending);
};
