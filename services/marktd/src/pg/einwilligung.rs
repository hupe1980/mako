//! PostgreSQL implementation of [`EinwilligungRepository`] — the ESA consent
//! registry (§49 Abs. 2 Nr. 9 MsbG).

use mako_markt::{
    error::MdmError,
    repository::{
        ConsentCode, ConsentDecision, ConsentPerspective, EinwilligungRecord,
        EinwilligungRepository, EsaFrameworkAgreement,
    },
};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

/// PostgreSQL-backed ESA consent + framework-agreement repository.
#[derive(Clone, Debug)]
pub struct PgEinwilligungRepository {
    pool: PgPool,
}

impl PgEinwilligungRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_consent(row: &PgRow) -> Result<EinwilligungRecord, sqlx::Error> {
    Ok(EinwilligungRecord {
        id: row.try_get("id")?,
        tenant: row.try_get("tenant")?,
        anschlussnutzer_ref: row.try_get("anschlussnutzer_ref")?,
        esa_mp_id: row.try_get("esa_mp_id")?,
        location_ids: row.try_get("location_ids")?,
        scope: row.try_get("scope")?,
        granted_at: row.try_get("granted_at")?,
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to").unwrap_or(None),
        revoked_at: row.try_get("revoked_at").unwrap_or(None),
        evidence_uri: row.try_get("evidence_uri").unwrap_or(None),
        evidence_hash: row.try_get("evidence_hash").unwrap_or(None),
    })
}

impl EinwilligungRepository for PgEinwilligungRepository {
    async fn grant(&self, rec: EinwilligungRecord) -> Result<Uuid, MdmError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        // A new grant supersedes any active consent for the same triple —
        // revoke the old one first so the partial UNIQUE index stays satisfied.
        sqlx::query(
            "UPDATE esa_einwilligungen SET revoked_at = now(), updated_at = now() \
             WHERE tenant = $1 AND esa_mp_id = $2 AND anschlussnutzer_ref = $3 \
               AND revoked_at IS NULL",
        )
        .bind(&rec.tenant)
        .bind(&rec.esa_mp_id)
        .bind(&rec.anschlussnutzer_ref)
        .execute(&mut *tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO esa_einwilligungen \
                 (tenant, anschlussnutzer_ref, esa_mp_id, location_ids, scope, \
                  valid_from, valid_to, evidence_uri, evidence_hash) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id",
        )
        .bind(&rec.tenant)
        .bind(&rec.anschlussnutzer_ref)
        .bind(&rec.esa_mp_id)
        .bind(&rec.location_ids)
        .bind(&rec.scope)
        .bind(rec.valid_from)
        .bind(rec.valid_to)
        .bind(&rec.evidence_uri)
        .bind(&rec.evidence_hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(id)
    }

    async fn get(&self, tenant: &str, id: Uuid) -> Result<Option<EinwilligungRecord>, MdmError> {
        sqlx::query("SELECT * FROM esa_einwilligungen WHERE tenant = $1 AND id = $2")
            .bind(tenant)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
            .as_ref()
            .map(map_consent)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn list_for_esa(
        &self,
        tenant: &str,
        esa_mp_id: &str,
    ) -> Result<Vec<EinwilligungRecord>, MdmError> {
        let rows = sqlx::query(
            "SELECT * FROM esa_einwilligungen \
             WHERE tenant = $1 AND esa_mp_id = $2 AND revoked_at IS NULL \
             ORDER BY granted_at DESC",
        )
        .bind(tenant)
        .bind(esa_mp_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        rows.iter()
            .map(map_consent)
            .collect::<Result<_, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn revoke(&self, tenant: &str, id: Uuid) -> Result<Option<EinwilligungRecord>, MdmError> {
        // Atomic: flip revoked_at only when still active, and return the row so
        // the caller can fire the 17008 Abbestellung exactly once.
        sqlx::query(
            "UPDATE esa_einwilligungen SET revoked_at = now(), updated_at = now() \
             WHERE tenant = $1 AND id = $2 AND revoked_at IS NULL \
             RETURNING *",
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .as_ref()
        .map(map_consent)
        .transpose()
        .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn upsert_framework(&self, rec: EsaFrameworkAgreement) -> Result<(), MdmError> {
        sqlx::query(
            "INSERT INTO esa_framework_agreements \
                 (tenant, msb_mp_id, esa_mp_id, signed_at, edi_agreement, cert_state) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             ON CONFLICT (tenant, msb_mp_id, esa_mp_id) DO UPDATE \
             SET signed_at = EXCLUDED.signed_at, edi_agreement = EXCLUDED.edi_agreement, \
                 cert_state = EXCLUDED.cert_state, updated_at = now()",
        )
        .bind(&rec.tenant)
        .bind(&rec.msb_mp_id)
        .bind(&rec.esa_mp_id)
        .bind(rec.signed_at)
        .bind(rec.edi_agreement)
        .bind(&rec.cert_state)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn get_framework(
        &self,
        tenant: &str,
        msb_mp_id: &str,
        esa_mp_id: &str,
    ) -> Result<Option<EsaFrameworkAgreement>, MdmError> {
        sqlx::query(
            "SELECT * FROM esa_framework_agreements \
             WHERE tenant = $1 AND msb_mp_id = $2 AND esa_mp_id = $3",
        )
        .bind(tenant)
        .bind(msb_mp_id)
        .bind(esa_mp_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .map(|row| {
            Ok::<_, sqlx::Error>(EsaFrameworkAgreement {
                tenant: row.try_get("tenant")?,
                msb_mp_id: row.try_get("msb_mp_id")?,
                esa_mp_id: row.try_get("esa_mp_id")?,
                signed_at: row.try_get("signed_at").unwrap_or(None),
                edi_agreement: row.try_get("edi_agreement")?,
                cert_state: row.try_get("cert_state")?,
            })
        })
        .transpose()
        .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn consent_check(
        &self,
        tenant: &str,
        esa_mp_id: &str,
        msb_mp_id: &str,
        location_id: &str,
        perspective: ConsentPerspective,
    ) -> Result<ConsentDecision, MdmError> {
        // 1. Framework agreement: only an *explicit* negative signal blocks. An
        //    absent record does not — the operator may simply not have populated
        //    the framework registry, and the ESA's self-assertion still stands.
        if let Some(row) = sqlx::query(
            "SELECT edi_agreement, cert_state FROM esa_framework_agreements \
             WHERE tenant = $1 AND msb_mp_id = $2 AND esa_mp_id = $3",
        )
        .bind(tenant)
        .bind(msb_mp_id)
        .bind(esa_mp_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        {
            let edi_agreement: bool = row.try_get("edi_agreement").unwrap_or(false);
            let cert_state: String = row.try_get("cert_state").unwrap_or_default();
            let cert_negative = matches!(cert_state.as_str(), "rejected" | "revoked" | "suspended");
            if !edi_agreement || cert_negative {
                return Ok(ConsentDecision::from_code(ConsentCode::FrameworkRejected));
            }
        }

        // 2. Location consents. An active consent delivers; a record that exists
        //    but is entirely revoked is the Widerruf clearing case; no record at
        //    all is self-assertion (never a block).
        let row = sqlx::query(
            "SELECT bool_or(revoked_at IS NULL) AS has_active, count(*) AS n \
             FROM esa_einwilligungen \
             WHERE tenant = $1 AND esa_mp_id = $2 AND $3 = ANY(location_ids)",
        )
        .bind(tenant)
        .bind(esa_mp_id)
        .bind(location_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        let has_active: Option<bool> = row.try_get("has_active").unwrap_or(None);
        let n: i64 = row.try_get("n").unwrap_or(0);

        let code = if has_active == Some(true) {
            ConsentCode::Active
        } else if n > 0 {
            ConsentCode::Revoked
        } else {
            // Missing record: self-assertion for the MSB, no lawful basis for
            // the ESA that must originate the request.
            match perspective {
                ConsentPerspective::MsbInbound => ConsentCode::SelfAssertion,
                ConsentPerspective::EsaOutbound => ConsentCode::NoConsent,
            }
        };
        Ok(ConsentDecision::from_code(code))
    }
}
