//! PostgreSQL implementation of [`PartnerRepository`].

use mako_markt::{
    domain::{MarktpartnerId, Sparte},
    error::MdmError,
    repository::{PartnerRecord, PartnerRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed partner repository.
#[derive(Clone, Debug)]
pub struct PgPartnerRepository {
    pool: PgPool,
}

impl PgPartnerRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const SELECT_COLS: &str = "mp_id, display_name, marktrolle, sparte, rollencodetyp, makoadresse, channels, version, updated_at";

impl PartnerRepository for PgPartnerRepository {
    async fn upsert(&self, partner: PartnerRecord) -> Result<i64, MdmError> {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT version FROM partners WHERE mp_id = $1")
                .bind(&partner.mp_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = current.map_or(1, |v| v + 1);
        let sparte_str = partner.sparte.map(|s| s.to_string());

        sqlx::query(
            r#"INSERT INTO partners (mp_id, display_name, marktrolle, sparte, rollencodetyp, makoadresse, channels, version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
               ON CONFLICT (mp_id) DO UPDATE
               SET display_name  = EXCLUDED.display_name,
                   marktrolle    = EXCLUDED.marktrolle,
                   sparte        = EXCLUDED.sparte,
                   rollencodetyp = EXCLUDED.rollencodetyp,
                   makoadresse   = EXCLUDED.makoadresse,
                   channels      = EXCLUDED.channels,
                   version       = EXCLUDED.version,
                   updated_at    = now()"#,
        )
        .bind(&partner.mp_id)
        .bind(&partner.display_name)
        .bind(&partner.marktrolle)
        .bind(sparte_str)
        .bind(&partner.rollencodetyp)
        .bind(&partner.makoadresse)
        .bind(&partner.channels)
        .bind(new_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, id: &MarktpartnerId) -> Result<Option<PartnerRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM partners WHERE mp_id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_partner))
    }

    async fn list(&self) -> Result<Vec<PartnerRecord>, MdmError> {
        let rows: Vec<PgRow> = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM partners ORDER BY mp_id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_partner).collect())
    }
}

fn row_to_partner(r: PgRow) -> PartnerRecord {
    let sparte_str: Option<String> = r.get("sparte");
    let makoadresse: Option<Vec<String>> = r.try_get("makoadresse").unwrap_or(None);
    PartnerRecord {
        mp_id: r.get("mp_id"),
        display_name: r.get("display_name"),
        marktrolle: r.get("marktrolle"),
        sparte: sparte_str.as_deref().map(parse_sparte),
        rollencodetyp: r.try_get("rollencodetyp").unwrap_or(None),
        makoadresse: makoadresse.unwrap_or_default(),
        channels: r.get("channels"),
        version: r.get("version"),
        updated_at: r.get("updated_at"),
    }
}

fn parse_sparte(s: &str) -> Sparte {
    match s {
        "GAS" => Sparte::Gas,
        _ => Sparte::Strom,
    }
}
