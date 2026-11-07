//! PostgreSQL storage: [`PgCorrelationIndex`].

use mako_markt::{
    domain::ProcessStatus,
    error::MdmError,
    repository::{CorrelationEntry, CorrelationFilter},
};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::OffsetDateTime;
use uuid::Uuid;

/// PostgreSQL-backed process-correlation index.
#[derive(Clone, Debug)]
pub struct PgCorrelationIndex {
    pool: PgPool,
}

impl PgCorrelationIndex {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const SELECT_COLS: &str = r#"process_id, workflow_name, pid, malo_id, melo_id, contract_id,
       erp_contract_id, erp_order_id, edifact_conv_id, marktrolle,
       format_version, status, initiated_at, completed_at"#;

impl PgCorrelationIndex {
    /// Insert a new correlation entry (idempotent — duplicate `process_id` is a no-op).
    pub async fn insert(&self, entry: CorrelationEntry) -> Result<(), MdmError> {
        let status = entry.status.to_string();
        sqlx::query(
            r#"INSERT INTO process_correlation
                   (process_id, workflow_name, pid, malo_id, melo_id, contract_id,
                    erp_contract_id, erp_order_id, edifact_conv_id, marktrolle,
                    format_version, status, initiated_at, completed_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
               ON CONFLICT (process_id) DO NOTHING"#,
        )
        .bind(entry.process_id)
        .bind(entry.workflow_name)
        .bind(entry.pid)
        .bind(entry.malo_id.as_deref())
        .bind(entry.melo_id.as_deref())
        .bind(entry.contract_id)
        .bind(entry.erp_contract_id)
        .bind(entry.erp_order_id)
        .bind(entry.edifact_conv_id)
        .bind(entry.marktrolle)
        .bind(entry.format_version)
        .bind(status)
        .bind(entry.initiated_at)
        .bind(entry.completed_at)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Update status and `completed_at` for a process.
    pub async fn update_status(
        &self,
        process_id: Uuid,
        status: ProcessStatus,
        completed_at: Option<OffsetDateTime>,
    ) -> Result<(), MdmError> {
        sqlx::query(
            "UPDATE process_correlation SET status = $2, completed_at = $3 WHERE process_id = $1",
        )
        .bind(process_id)
        .bind(status.to_string())
        .bind(completed_at)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Update `edifact_conv_id` when the first `de.mako.*` event is received.
    pub async fn update_edifact_conv_id(
        &self,
        process_id: Uuid,
        conv_id: Uuid,
    ) -> Result<(), MdmError> {
        sqlx::query("UPDATE process_correlation SET edifact_conv_id = $2 WHERE process_id = $1")
            .bind(process_id)
            .bind(conv_id)
            .execute(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Look up by ERP order ID (`Idempotency-Key` from command submission).
    pub async fn find_by_erp_order_id(
        &self,
        erp_order_id: &str,
    ) -> Result<Option<CorrelationEntry>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            mako_service::sql::dynamic(format!("SELECT {SELECT_COLS} FROM process_correlation WHERE erp_order_id = $1 ORDER BY initiated_at DESC LIMIT 1")))
        .bind(erp_order_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_entry))
    }

    /// Look up by `process_id`.
    pub async fn find_by_process_id(
        &self,
        process_id: Uuid,
    ) -> Result<Option<CorrelationEntry>, MdmError> {
        let row: Option<PgRow> = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM process_correlation WHERE process_id = $1"
        )))
        .bind(process_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_entry))
    }

    /// Return correlations matching the filter.
    pub async fn list(&self, filter: CorrelationFilter) -> Result<Vec<CorrelationEntry>, MdmError> {
        let status_str = filter.status.map(|s| s.to_string());
        let rows: Vec<PgRow> = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM process_correlation
                 WHERE ($1::text IS NULL OR erp_order_id = $1)
                   AND ($2::text IS NULL OR malo_id = $2)
                   AND ($3::text IS NULL OR status = $3)
                 ORDER BY initiated_at DESC LIMIT 200"
        )))
        .bind(filter.erp_order_id)
        .bind(filter.malo_id.as_deref())
        .bind(status_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_entry).collect())
    }
}

fn row_to_entry(r: PgRow) -> CorrelationEntry {
    let status_str: String = r.get("status");
    CorrelationEntry {
        process_id: r.get("process_id"),
        workflow_name: r.get("workflow_name"),
        pid: r.get("pid"),
        malo_id: super::opt_id(&r, "malo_id"),
        melo_id: super::opt_id(&r, "melo_id"),
        contract_id: r.get("contract_id"),
        erp_contract_id: r.get("erp_contract_id"),
        erp_order_id: r.get("erp_order_id"),
        edifact_conv_id: r.get("edifact_conv_id"),
        marktrolle: r.get("marktrolle"),
        format_version: r.get("format_version"),
        status: status_str.parse().unwrap_or(ProcessStatus::Running),
        initiated_at: r.get("initiated_at"),
        completed_at: r.get("completed_at"),
    }
}
