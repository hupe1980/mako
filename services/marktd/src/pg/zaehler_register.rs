//! PostgreSQL implementation of [`ZaehlzeitRepository`] for `marktd`.

use mako_markt::{
    error::MdmError,
    repository::{ZaehlzeitRegisterRecord, ZaehlzeitRepository, ZaehlzeitSaisonRecord},
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Clone)]
pub struct PgZaehlzeitRepository {
    pool: PgPool,
}

impl PgZaehlzeitRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl ZaehlzeitRepository for PgZaehlzeitRepository {
    async fn upsert_register(&self, rec: &ZaehlzeitRegisterRecord) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO zaehler_register
                  (id, zaehler_id, tenant, bezeichnung, zaehlerauspraegung,
                   obis_kennzahl, einheit, valid_from, valid_to, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
              ON CONFLICT (zaehler_id, tenant, bezeichnung, valid_from) DO UPDATE
              SET zaehlerauspraegung = EXCLUDED.zaehlerauspraegung,
                  obis_kennzahl      = EXCLUDED.obis_kennzahl,
                  einheit            = EXCLUDED.einheit,
                  valid_to           = EXCLUDED.valid_to,
                  updated_at         = now()",
        )
        .bind(rec.id)
        .bind(&rec.zaehler_id)
        .bind(&rec.tenant)
        .bind(&rec.bezeichnung)
        .bind(&rec.zaehlerauspraegung)
        .bind(&rec.obis_kennzahl)
        .bind(&rec.einheit)
        .bind(rec.valid_from)
        .bind(rec.valid_to)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn list_registers_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<ZaehlzeitRegisterRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT id, zaehler_id, tenant, bezeichnung, zaehlerauspraegung,
                     obis_kennzahl, einheit, valid_from, valid_to, updated_at
              FROM zaehler_register
              WHERE zaehler_id = $1 AND tenant = $2
              ORDER BY valid_from DESC",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter().map(row_to_register).collect()
    }

    async fn upsert_saison(&self, rec: &ZaehlzeitSaisonRecord) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO zaehler_saisons
                  (id, register_id, saison, wochentage, zeit_von, zeit_bis, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, now())
              ON CONFLICT (id) DO UPDATE
              SET saison     = EXCLUDED.saison,
                  wochentage = EXCLUDED.wochentage,
                  zeit_von   = EXCLUDED.zeit_von,
                  zeit_bis   = EXCLUDED.zeit_bis,
                  updated_at = now()",
        )
        .bind(rec.id)
        .bind(rec.register_id)
        .bind(&rec.saison)
        .bind(&rec.wochentage)
        .bind(&rec.zeit_von)
        .bind(&rec.zeit_bis)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn list_saisons_by_register(
        &self,
        register_id: Uuid,
        _tenant: &str,
    ) -> Result<Vec<ZaehlzeitSaisonRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT id, register_id, saison, wochentage, zeit_von, zeit_bis, updated_at
              FROM zaehler_saisons
              WHERE register_id = $1
              ORDER BY saison, zeit_von",
        )
        .bind(register_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter().map(row_to_saison).collect()
    }

    async fn resolve_tariff_zone(
        &self,
        zaehler_id: &str,
        tenant: &str,
        local_datetime: time::PrimitiveDateTime,
    ) -> Result<Option<String>, MdmError> {
        use time::Weekday;
        let time_str = format!(
            "{:02}:{:02}",
            local_datetime.hour(),
            local_datetime.minute()
        );
        let weekday_iso: i32 = match local_datetime.weekday() {
            Weekday::Monday => 1,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
            Weekday::Sunday => 7,
        };
        // Single query: JOIN registers + saisons, filter by time + weekday.
        // Returns the zaehlerauspraegung (HT/NT/EINZEL) of the first matching window.
        let row = sqlx::query(
            r"SELECT r.zaehlerauspraegung
              FROM zaehler_register r
              JOIN zaehler_saisons  s ON s.register_id = r.id
              WHERE r.zaehler_id = $1 AND r.tenant = $2
                AND r.valid_to IS NULL
                AND s.wochentage @> $3::jsonb
                AND s.zeit_von <= $4
                AND s.zeit_bis  > $4
              LIMIT 1",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .bind(serde_json::json!([weekday_iso]))
        .bind(&time_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.and_then(|r| r.try_get::<String, _>("zaehlerauspraegung").ok()))
    }
}

fn row_to_register(row: &sqlx::postgres::PgRow) -> Result<ZaehlzeitRegisterRecord, MdmError> {
    Ok(ZaehlzeitRegisterRecord {
        id: row
            .try_get("id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zaehler_id: row
            .try_get("zaehler_id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        tenant: row
            .try_get("tenant")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        bezeichnung: row
            .try_get("bezeichnung")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zaehlerauspraegung: row
            .try_get("zaehlerauspraegung")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        obis_kennzahl: row
            .try_get("obis_kennzahl")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        einheit: row.try_get("einheit").unwrap_or_else(|_| "KWH".to_owned()),
        valid_from: row
            .try_get("valid_from")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        valid_to: row
            .try_get("valid_to")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
    })
}

fn row_to_saison(row: &sqlx::postgres::PgRow) -> Result<ZaehlzeitSaisonRecord, MdmError> {
    Ok(ZaehlzeitSaisonRecord {
        id: row
            .try_get("id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        register_id: row
            .try_get("register_id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        saison: row
            .try_get("saison")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        wochentage: row
            .try_get("wochentage")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zeit_von: row
            .try_get("zeit_von")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zeit_bis: row
            .try_get("zeit_bis")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
    })
}
