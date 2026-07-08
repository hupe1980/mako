//! PostgreSQL implementation of [`ContractRepository`].

use mako_markt::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{ContractRecord, ContractRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::Date;

/// PostgreSQL-backed contract repository.
#[derive(Clone, Debug)]
pub struct PgContractRepository {
    pool: PgPool,
}

impl PgContractRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl ContractRepository for PgContractRepository {
    #[allow(clippy::too_many_arguments)]
    async fn upsert(
        &self,
        contract_id: &str,
        malo_id: Option<&MaloId>,
        sparte: Sparte,
        vertragsart: &str,
        data: serde_json::Value,
        valid_from: Option<Date>,
        valid_to: Option<Date>,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError> {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT version FROM contracts WHERE contract_id = $1")
                .bind(contract_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = match (current, if_match) {
            (Some(v), Some(expected)) if v != expected => {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: v.to_string(),
                });
            }
            (Some(v), _) => v + 1,
            (None, _) => 1,
        };

        sqlx::query(
            r#"INSERT INTO contracts
                   (contract_id, malo_id, sparte, vertragsart, version,
                    data, valid_from, valid_to, bo4e_version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
               ON CONFLICT (contract_id) DO UPDATE
               SET malo_id      = EXCLUDED.malo_id,
                   sparte       = EXCLUDED.sparte,
                   vertragsart  = EXCLUDED.vertragsart,
                   version      = EXCLUDED.version,
                   data         = EXCLUDED.data,
                   valid_from   = EXCLUDED.valid_from,
                   valid_to     = EXCLUDED.valid_to,
                   bo4e_version = EXCLUDED.bo4e_version,
                   updated_at   = now()"#,
        )
        .bind(contract_id)
        .bind(malo_id.map(|id| id.to_string()))
        .bind(sparte.to_string())
        .bind(vertragsart)
        .bind(new_version)
        .bind(&data)
        .bind(valid_from)
        .bind(valid_to)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, contract_id: &str) -> Result<Option<ContractRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            r#"SELECT contract_id, malo_id, sparte, vertragsart, version,
                      data, valid_from, valid_to, bo4e_version, created_at, updated_at
               FROM contracts
               WHERE contract_id = $1"#,
        )
        .bind(contract_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(map_row))
    }

    async fn find_active_by_malo(
        &self,
        malo_id: &MaloId,
        at: Date,
    ) -> Result<Vec<ContractRecord>, MdmError> {
        let rows: Vec<PgRow> = sqlx::query(
            r#"SELECT contract_id, malo_id, sparte, vertragsart, version,
                      data, valid_from, valid_to, bo4e_version, created_at, updated_at
               FROM contracts
               WHERE malo_id = $1
                 AND (valid_from IS NULL OR valid_from <= $2)
                 AND (valid_to   IS NULL OR valid_to   >= $2)
               ORDER BY valid_from DESC NULLS LAST"#,
        )
        .bind(malo_id.to_string())
        .bind(at)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(map_row).collect())
    }
}

fn map_row(r: PgRow) -> ContractRecord {
    let sparte_str: String = r.get("sparte");
    ContractRecord {
        contract_id: r.get("contract_id"),
        malo_id: r
            .get::<Option<String>, _>("malo_id")
            .and_then(|s| s.parse().ok()),
        sparte: sparte_str
            .parse::<Sparte>()
            .expect("DB has CHECK constraint on sparte"),
        vertragsart: r.get("vertragsart"),
        version: r.get("version"),
        data: r.get("data"),
        valid_from: r.get("valid_from"),
        valid_to: r.get("valid_to"),
        bo4e_version: r
            .try_get("bo4e_version")
            .unwrap_or_else(|_| "v202501.0.0".to_owned()),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}
