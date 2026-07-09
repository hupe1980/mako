//! PostgreSQL implementation of [`NbContractRepository`].

use mako_markt::{
    error::MdmError,
    repository::{NbContractRecord, NbContractRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed NB network contract repository.
#[derive(Clone, Debug)]
pub struct PgNbContractRepository {
    pool: PgPool,
}

impl PgNbContractRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const SELECT_COLS: &str = "contract_id, malo_id, nb_mp_id, sparte, netzebene, \
    bilanzierungsmethode, billing_schedule, valid_from, valid_to, version, tenant";

impl NbContractRepository for PgNbContractRepository {
    async fn upsert(&self, rec: NbContractRecord) -> Result<i64, MdmError> {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT version FROM nb_contracts WHERE contract_id = $1")
                .bind(&rec.contract_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = current.map_or(1, |v| v + 1);

        sqlx::query(
            r#"INSERT INTO nb_contracts
               (contract_id, malo_id, nb_mp_id, sparte, netzebene,
                bilanzierungsmethode, billing_schedule,
                valid_from, valid_to, version, tenant, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now())
               ON CONFLICT (contract_id) DO UPDATE
               SET malo_id               = EXCLUDED.malo_id,
                   nb_mp_id                = EXCLUDED.nb_mp_id,
                   sparte                = EXCLUDED.sparte,
                   netzebene             = EXCLUDED.netzebene,
                   bilanzierungsmethode  = EXCLUDED.bilanzierungsmethode,
                   billing_schedule      = EXCLUDED.billing_schedule,
                   valid_from            = EXCLUDED.valid_from,
                   valid_to              = EXCLUDED.valid_to,
                   version               = EXCLUDED.version,
                   updated_at            = now()"#,
        )
        .bind(&rec.contract_id)
        .bind(&rec.malo_id)
        .bind(&rec.nb_mp_id)
        .bind(rec.sparte.to_string())
        .bind(&rec.netzebene)
        .bind(&rec.bilanzierungsmethode)
        .bind(rec.billing_schedule.to_string())
        .bind(rec.valid_from)
        .bind(rec.valid_to)
        .bind(new_version)
        .bind(&rec.tenant)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, contract_id: &str) -> Result<Option<NbContractRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM nb_contracts WHERE contract_id = $1"
        ))
        .bind(contract_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_rec))
    }

    async fn find_active(
        &self,
        malo_id: &str,
        date: time::Date,
        tenant: &str,
    ) -> Result<Option<NbContractRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(&format!(
            r#"SELECT {SELECT_COLS} FROM nb_contracts
               WHERE malo_id = $1
                 AND valid_from <= $2
                 AND (valid_to IS NULL OR valid_to >= $2)
                 AND tenant = $3
               ORDER BY valid_from DESC
               LIMIT 1"#
        ))
        .bind(malo_id)
        .bind(date)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_rec))
    }

    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<NbContractRecord>, MdmError> {
        let rows: Vec<PgRow> = sqlx::query(&format!(
            r#"SELECT {SELECT_COLS} FROM nb_contracts
               WHERE nb_mp_id = $1 AND tenant = $2
               ORDER BY malo_id, valid_from"#
        ))
        .bind(nb_mp_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_rec).collect())
    }
}

fn row_to_rec(r: PgRow) -> NbContractRecord {
    use mako_markt::{domain::MaloId, repository::BillingSchedule};

    let sparte_str: String = r.get("sparte");
    let sparte = sparte_str
        .parse()
        .unwrap_or(mako_markt::domain::Sparte::Strom);

    let billing_schedule_str: String = r.get("billing_schedule");
    let billing_schedule = BillingSchedule::from_str_or_default(&billing_schedule_str);

    NbContractRecord {
        contract_id: r.get("contract_id"),
        malo_id: r
            .get::<String, _>("malo_id")
            .parse::<MaloId>()
            .expect("DB has REFERENCES malo(malo_id) — checksum already validated"),
        nb_mp_id: r.get("nb_mp_id"),
        sparte,
        netzebene: r.get("netzebene"),
        bilanzierungsmethode: r.get("bilanzierungsmethode"),
        billing_schedule,
        valid_from: r.get("valid_from"),
        valid_to: r.get("valid_to"),
        tenant: r.get("tenant"),
        version: r.get("version"),
    }
}
