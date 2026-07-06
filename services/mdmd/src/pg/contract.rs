//! PostgreSQL implementation of [`ContractRepository`].

use mako_mdm::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{ContractRecord, ContractRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

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
    async fn upsert(
        &self,
        contract_id: &str,
        malo_id: Option<&MaloId>,
        sparte: Sparte,
        vertragsart: &str,
        data: serde_json::Value,
        if_match: Option<i64>,
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
            r#"INSERT INTO contracts (contract_id, malo_id, sparte, vertragsart, version, data, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, now())
               ON CONFLICT (contract_id) DO UPDATE
               SET malo_id = EXCLUDED.malo_id,
                   sparte = EXCLUDED.sparte,
                   vertragsart = EXCLUDED.vertragsart,
                   version = EXCLUDED.version,
                   data = EXCLUDED.data,
                   updated_at = now()"#,
        )
        .bind(contract_id)
        .bind(malo_id)
        .bind(sparte.to_string())
        .bind(vertragsart)
        .bind(new_version)
        .bind(&data)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, contract_id: &str) -> Result<Option<ContractRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            "SELECT contract_id, malo_id, sparte, vertragsart, version, data, created_at, updated_at FROM contracts WHERE contract_id = $1"
        )
        .bind(contract_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| {
            let sparte_str: String = r.get("sparte");
            ContractRecord {
                contract_id: r.get("contract_id"),
                malo_id: r.get("malo_id"),
                sparte: sparte_str
                    .parse::<Sparte>()
                    .expect("DB has CHECK constraint on sparte"),
                vertragsart: r.get("vertragsart"),
                version: r.get("version"),
                data: r.get("data"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            }
        }))
    }
}
